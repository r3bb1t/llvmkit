//! What an *owned* module buys, now that `Module<B, S>` has no lifetime.
//!
//! llvmkit-specific: LLVM's C++ `Module` is heap-allocated and moved by
//! `std::unique_ptr`, so "put it in a container, hand it to another thread,
//! finish it there" is unremarkable there. It was *not* expressible in llvmkit
//! until cycle C: before C1 a module was a stack local of `Module::with_new`'s
//! frame, and before C4 the token still carried a region parameter that tied it
//! to a borrow. These tests lock the properties that only became statable once
//! the token owned its storage and dropped that region:
//!
//! - the token is `Send`, *including* under a brand type that is itself
//!   `!Send`, because the brand rides as `PhantomData<fn(B) -> B>`;
//! - a half-authored module can cross a thread boundary and be finished and
//!   verified on the other side;
//! - a module lives in a struct field and in a `Vec`;
//! - a `'static` id minted by a dead module is refused *deterministically* by a
//!   same-brand successor, rather than silently resolving against whatever now
//!   occupies that arena slot.
//!
//! The registry is process-global and the harness runs tests in parallel, so
//! every test that needs a registered brand declares its **own** brand type.

use std::thread;

use llvmkit_ir::metadata::{
    DebugMetadataOperand, DebugRecord, DebugVariableRecord, DebugVariableRecordKind,
};
use llvmkit_ir::{
    BlockId, Dyn, DynBrand, FunctionId, InstructionView, IntValue, IrBuilder, IrError, Linkage,
    MetadataAttachmentKind, MetadataField, MetadataFieldValue, MetadataKind, Module, ModuleBrand,
    NamedMetadataName, NoFolder, SpecializedMetadataKind, SpecializedMetadataNode, Unverified,
    Verified, module_new,
};

/// Declare a brand type exactly as a user would.
macro_rules! brand {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        struct $name;
        impl ModuleBrand for $name {}
    };
}

// --------------------------------------------------------------------------
// `Send`, even under a `!Send` brand
// --------------------------------------------------------------------------

/// A brand type that is deliberately **not** `Send`: a raw pointer is `!Send`,
/// and a struct containing one inherits that.
///
/// It still satisfies [`ModuleBrand`] — `Copy + Debug + Eq + Hash + 'static` —
/// because `*const ()` is all of those. Nothing stops a user writing a brand
/// like this; the point is that it cannot infect the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NotSendBrand(*const ());
impl ModuleBrand for NotSendBrand {}

/// Compile-assert: a module is `Send` in **both** verification states and under
/// a `!Send` brand.
///
/// This is the load-bearing half of the `Send` guarantee. The module stores its
/// brand as `Invariant<B>` = `PhantomData<fn(B) -> B>`; a `fn` pointer type is
/// `Send + Sync` whatever its argument and return types are, so `B`'s own auto
/// traits never reach the module. If someone ever changed that phantom to a
/// plain `PhantomData<B>`, this block would stop compiling.
///
/// The premise — that `NotSendBrand` really is `!Send`, so this is not a
/// vacuous assertion — is pinned by the companion compile-fail fixture
/// `tests/compile_fail/not_send_brand_is_really_not_send.rs`.
const _: () = {
    const fn assert_send<T: Send>() {}
    const fn assertions() {
        assert_send::<Module<NotSendBrand, Unverified>>();
        assert_send::<Module<NotSendBrand, Verified>>();
        assert_send::<Module<DynBrand, Unverified>>();
        // Ids are `'static` and `Send` for the same reason, which is what lets
        // them be carried across the boundary alongside the module.
        assert_send::<FunctionId<Dyn, NotSendBrand>>();
        assert_send::<BlockId<Dyn, NotSendBrand>>();
    }
    assertions()
};

/// And the conclusion actually holds at run time: a module under the `!Send`
/// brand really does move to another thread.
///
/// `NotSendBrand` is registry-registered, so this test owns it exclusively.
#[test]
fn a_module_under_a_not_send_brand_still_crosses_a_thread() -> Result<(), IrError> {
    let module = Module::branded::<NotSendBrand, _>("not-send-brand")?;
    let name = thread::spawn(move || module.name().to_owned())
        .join()
        .expect("worker thread completed");
    assert_eq!(name, "not-send-brand");
    Ok(())
}

// --------------------------------------------------------------------------
// Authoring across a thread boundary
// --------------------------------------------------------------------------

/// The headline: a module is **half authored** on one thread, moved to another,
/// **finished** there, and verified there.
///
/// The handoff is exactly what cycles B and C were built for. The `IrBuilder`
/// borrows the module, so it is dropped before the move; what crosses the
/// boundary is the owned token plus two lifetime-free ids (`FunctionId`,
/// `BlockId`). On the far side the ids are re-resolved against the token — the
/// builder re-enters the block through `position_at_end_dyn`, which re-checks
/// the module tag — and the body is completed.
#[test]
fn a_half_authored_module_is_finished_on_another_thread() -> Result<(), IrError> {
    brand!(Handoff);

    let module = Module::branded::<Handoff, _>("handoff")?;

    // --- thread A: declare, open a block, emit part of the body ---
    let i32_ty = module.i32_type();
    let fn_ty = module.function_type(i32_ty, [i32_ty.as_type()]);
    let f: FunctionId<Dyn, Handoff> = module.add_function_dyn("half", fn_ty, Linkage::External)?;
    let entry: BlockId<Dyn, Handoff> = module.view(f).append_basic_block(&module, "entry").id();

    {
        let builder = IrBuilder::new_for::<Dyn>(&module).position_at_end_dyn(entry)?;
        let n: IntValue<'_, i32, _> = module.view(f).param(0)?.try_into()?;
        builder.int_add(n, 1_i32, "sum")?;
        // Deliberately no terminator yet: the module is unfinished, and an
        // unfinished module is exactly what could not previously be moved.
    }

    // --- the move ---
    let printed = thread::spawn(move || -> Result<String, IrError> {
        // thread B: reopen the same block through the id and finish the body.
        let builder = IrBuilder::new_for::<Dyn>(&module).position_at_end_dyn(entry)?;
        let sum: IntValue<'_, i32, _> = module
            .view(f)
            .basic_blocks()
            .next()
            .expect("entry block")
            .instructions()
            .next()
            .expect("the add emitted on the other thread")
            .to_erased()
            .try_into()?;
        builder.ret(sum)?;

        // Verification also happens on the far thread — `verify` consumes the
        // token, which is only possible because the token is owned here.
        let verified = module.verify()?;
        Ok(format!("{verified}"))
    })
    .join()
    .expect("worker thread completed")?;

    assert!(printed.contains("define i32 @half(i32 %0)"), "{printed}");
    assert!(printed.contains("%sum = add i32 %0, 1"), "{printed}");
    assert!(printed.contains("ret i32 %sum"), "{printed}");
    Ok(())
}

/// The IR is byte-identical whether the same construction happens on one thread
/// or is split across two. Moving a module is a move of storage, nothing more.
#[test]
fn splitting_authoring_across_threads_emits_identical_ir() -> Result<(), IrError> {
    /// The two lifetime-free ids that carry authoring state across the handoff.
    type Resume = (FunctionId<Dyn, DynBrand>, BlockId<Dyn, DynBrand>);

    fn author_first_half(module: &Module<DynBrand, Unverified>) -> Result<Resume, IrError> {
        let i32_ty = module.i32_type();
        let fn_ty = module.function_type(i32_ty, [i32_ty.as_type()]);
        let f = module.add_function_dyn("split", fn_ty, Linkage::External)?;
        let entry = module.view(f).append_basic_block(module, "entry").id();
        let builder = IrBuilder::new_for::<Dyn>(module).position_at_end_dyn(entry)?;
        let n: IntValue<'_, i32, _> = module.view(f).param(0)?.try_into()?;
        builder.int_add(n, 7_i32, "sum")?;
        Ok((f, entry))
    }

    fn author_second_half(
        module: &Module<DynBrand, Unverified>,
        f: FunctionId<Dyn, DynBrand>,
        entry: BlockId<Dyn, DynBrand>,
    ) -> Result<(), IrError> {
        let builder = IrBuilder::new_for::<Dyn>(module).position_at_end_dyn(entry)?;
        let sum: IntValue<'_, i32, _> = module
            .view(f)
            .basic_blocks()
            .next()
            .expect("entry block")
            .instructions()
            .next()
            .expect("the add")
            .to_erased()
            .try_into()?;
        builder.ret(sum)?;
        Ok(())
    }

    let single = Module::dynamic("split");
    let (f, entry) = author_first_half(&single)?;
    author_second_half(&single, f, entry)?;
    let on_one_thread = format!("{}", single.verify()?);

    let crossing = Module::dynamic("split");
    let (f, entry) = author_first_half(&crossing)?;
    let on_two_threads = thread::spawn(move || -> Result<String, IrError> {
        author_second_half(&crossing, f, entry)?;
        Ok(format!("{}", crossing.verify()?))
    })
    .join()
    .expect("worker thread completed")?;

    assert_eq!(on_one_thread, on_two_threads);
    assert!(on_one_thread.contains("%sum = add i32 %0, 7"));
    Ok(())
}

// --------------------------------------------------------------------------
// A module is a value: struct fields and containers
// --------------------------------------------------------------------------

/// A module lives in a struct field, and the struct can be returned from the
/// function that built it. Under the old lifetime-brand this was impossible:
/// the token was pinned to the `with_new` frame.
#[test]
fn a_module_lives_in_a_struct_field() -> Result<(), IrError> {
    struct TranslationUnit {
        source_name: String,
        module: Module<DynBrand, Unverified>,
    }

    fn compile(source_name: &str) -> Result<TranslationUnit, IrError> {
        let module = Module::dynamic(source_name);
        let i32_ty = module.i32_type();
        module.add_global("counter", i32_ty.const_int(0_i32))?;
        Ok(TranslationUnit {
            source_name: source_name.to_owned(),
            module,
        })
    }

    let unit = compile("tu.c")?;
    assert_eq!(unit.source_name, "tu.c");
    assert_eq!(unit.module.name(), "tu.c");

    // And the field can be moved out and consumed by the linear transition.
    let verified = unit.module.verify()?;
    assert!(format!("{verified}").contains("@counter = global i32 0"));
    Ok(())
}

/// A `Vec<Module<DynBrand>>` — the shape the registry-exempt brand exists for.
/// Every element is a separate module with its own runtime tag, and the vector
/// outlives every frame that built an element.
#[test]
fn modules_collect_into_a_vec_and_are_drained_later() -> Result<(), IrError> {
    let mut units: Vec<Module<DynBrand, Unverified>> = Vec::new();
    for i in 0..8 {
        let module = Module::dynamic(format!("tu{i}"));
        let i32_ty = module.i32_type();
        module.add_global("g", i32_ty.const_int(i))?;
        units.push(module);
    }
    assert_eq!(units.len(), 8);

    let verified: Vec<Module<DynBrand, Verified>> = units
        .into_iter()
        .map(|module| module.verify())
        .collect::<Result<_, _>>()?;
    assert_eq!(verified.len(), 8);
    assert!(format!("{}", verified[5]).contains("@g = global i32 5"));

    // Distinct runtime tags: `DynBrand` gives up the *compile-time* half of
    // identity, never the runtime half.
    let mut ids: Vec<_> = verified.iter().map(Module::id).collect();
    ids.sort_by_key(|id| id.as_u64());
    ids.dedup();
    assert_eq!(ids.len(), 8);
    Ok(())
}

/// A `module_new!` brand is unnameable, so the module cannot be spelled in a
/// struct field's type — but it can still be *moved* into a generic container.
#[test]
fn a_generated_brand_module_moves_into_a_generic_holder() -> Result<(), IrError> {
    struct Holder<M> {
        module: M,
    }

    let holder = Holder {
        module: module_new!("held")?,
    };
    assert_eq!(holder.module.name(), "held");
    let verified = holder.module.verify()?;
    assert_eq!(verified.name(), "held");
    Ok(())
}

// --------------------------------------------------------------------------
// Stale-generation replay
// --------------------------------------------------------------------------

/// The scenario the brand registry exists to make *safe*, now expressible
/// end-to-end for the first time.
///
/// A brand is released when its module drops, so a **successor** may claim the
/// same brand. An id minted by the dead predecessor is `'static` and carries
/// that same brand type, so it still type-checks against the successor — the
/// compile-time half of identity cannot separate two generations of one brand.
/// The runtime half must, and does: every id carries the predecessor's
/// process-unique [`llvmkit_ir::ModuleId`], the counter never reuses a tag, and
/// the tag is compared *before* the arena is touched. So the stale id is
/// refused deterministically instead of resolving against whichever function
/// now occupies that slot.
///
/// Before C4 this test could not even be written: the predecessor's id was
/// tied to a region that ended with the predecessor.
#[test]
fn a_stale_id_from_a_dead_generation_is_refused_by_its_successor() -> Result<(), IrError> {
    brand!(Generation);

    // --- generation 1: mint an id, then die ---
    let (stale, stale_block): (FunctionId<Dyn, Generation>, BlockId<Dyn, Generation>) = {
        let gen1 = Module::branded::<Generation, _>("gen1")?;
        let void_ty = gen1.void_type();
        let fn_ty = gen1.function_type_no_parameters(void_ty);
        let f = gen1.add_function_dyn("predecessor", fn_ty, Linkage::External)?;
        let bb = gen1.view(f).append_basic_block(&gen1, "entry").id();
        (f, bb)
        // `gen1` drops here, releasing the brand. Both ids survive it.
    };

    // --- generation 2: same brand, fresh storage ---
    let gen2 = Module::branded::<Generation, _>("gen2")?;
    // Occupy the same arena slot shape, so a tag-blind resolver would find
    // *something* plausible rather than an empty slot.
    let void_ty = gen2.void_type();
    let fn_ty = gen2.function_type_no_parameters(void_ty);
    let fresh = gen2.add_function_dyn("successor", fn_ty, Linkage::External)?;
    let _fresh_block = gen2.view(fresh).append_basic_block(&gen2, "entry");

    // The stale id still type-checks — same brand type — which is precisely why
    // the runtime tag has to carry the guarantee.
    assert!(
        gen2.try_view(stale).is_none(),
        "a stale id must not resolve against a same-brand successor"
    );
    // ...and the live one does resolve, so the check is discriminating rather
    // than blanket-refusing.
    assert_eq!(gen2.view(fresh).name(), "successor");

    // The fallible id-consuming surfaces report it as a foreign id rather than
    // reopening whatever block now sits at that index.
    assert!(matches!(
        IrBuilder::new_for::<Dyn>(&gen2).position_at_end_dyn(stale_block),
        Err(IrError::ForeignValueId)
    ));
    Ok(())
}

/// The panicking resolver is equally deterministic: `view` panics rather than
/// mis-resolving.
#[test]
#[should_panic(expected = "id does not resolve in this module")]
fn viewing_a_stale_id_panics_rather_than_mis_resolving() {
    brand!(GenerationPanic);

    let stale: FunctionId<Dyn, GenerationPanic> = {
        let gen1 = Module::branded::<GenerationPanic, _>("gen1").expect("fresh brand");
        let void_ty = gen1.void_type();
        let fn_ty = gen1.function_type_no_parameters(void_ty);
        gen1.add_function_dyn("predecessor", fn_ty, Linkage::External)
            .expect("declaration succeeds")
    };

    let gen2 = Module::branded::<GenerationPanic, _>("gen2").expect("brand released on drop");
    let _ = gen2.view(stale);
}

/// `branded_once` closes the loophole entirely: the brand is *retired* on drop,
/// so no successor can ever exist to replay a stale id against.
#[test]
fn branded_once_retires_the_brand_so_no_successor_can_exist() -> Result<(), IrError> {
    brand!(OnceOnly);

    let stale: FunctionId<Dyn, OnceOnly> = {
        let only = Module::branded_once::<OnceOnly, _>("only")?;
        let void_ty = only.void_type();
        let fn_ty = only.function_type_no_parameters(void_ty);
        only.add_function_dyn("gone", fn_ty, Linkage::External)?
    };

    // The id outlives its module, and there is provably nothing to replay it
    // against: every later claim on the brand is refused, forever.
    let _ = stale;
    assert!(matches!(
        Module::branded::<OnceOnly, _>("successor"),
        Err(IrError::BrandRetired { .. })
    ));
    assert!(matches!(
        Module::branded_once::<OnceOnly, _>("successor"),
        Err(IrError::BrandRetired { .. })
    ));
    Ok(())
}

// --------------------------------------------------------------------------
// The metadata currency carries the same tag
// --------------------------------------------------------------------------

/// A metadata node minted in one module and handed to another is refused, even
/// when its arena slot is perfectly **in range** over there.
///
/// This is the case a range check cannot catch and the one the crate's law is
/// really about: before `MetadataId` carried a `ModuleId` tag, a metadata
/// handle was a bare `usize`, so module A's slot 0 was module B's slot 0 and the
/// printer resolved it against B's arena — a different node, silently. The two
/// modules here are deliberately *shaped the same*, so a tag-blind resolver
/// would find a plausible node rather than an empty slot.
///
/// Both modules are [`DynBrand`], which is the interesting brand: two
/// `Module<DynBrand>` handles have the same *type*, so nothing but the runtime
/// tag can separate them. (Two distinct named brands are separated statically
/// instead — `tests/compile_fail/cross_module_metadata_attachment.rs`.)
#[test]
fn a_metadata_id_from_another_module_is_refused_everywhere() -> Result<(), IrError> {
    let a = Module::dynamic("a");
    let b = Module::dynamic("b");

    // Same shape in both, so the foreign slot is in range in the target.
    let a_node = a.metadata_tuple([a.metadata_string("from-a")])?;
    let b_node = b.metadata_tuple([b.metadata_string("from-b")])?;
    assert_eq!(
        a.metadata_count(),
        b.metadata_count(),
        "the two arenas must be the same size for this test to mean anything"
    );

    // ---- module-level constructors ----
    assert!(matches!(
        b.metadata_tuple([a_node]),
        Err(IrError::ForeignMetadataId)
    ));
    assert!(matches!(
        b.metadata_tuple_with_distinct(true, [a_node]),
        Err(IrError::ForeignMetadataId)
    ));
    assert!(matches!(
        b.metadata_node(MetadataKind::Ref(a_node)),
        Err(IrError::ForeignMetadataId)
    ));
    assert!(matches!(
        b.metadata_specialized(
            SpecializedMetadataNode::new(SpecializedMetadataKind::DiFile).field(
                MetadataField::new("filename", MetadataFieldValue::Metadata(a_node))
            )
        ),
        Err(IrError::ForeignMetadataId)
    ));
    assert!(matches!(
        b.metadata_as_value(a_node),
        Err(IrError::ForeignMetadataId)
    ));
    assert!(matches!(
        b.metadata_set(a_node, MetadataKind::Null),
        Err(IrError::ForeignMetadataId)
    ));
    let named = b.get_or_insert_named_metadata("b.named");
    assert!(matches!(
        b.named_metadata_add_operand(named, a_node),
        Err(IrError::ForeignMetadataId)
    ));

    // ---- lookup: `None`, never module B's node at the same index ----
    assert!(
        b.metadata_get(a_node).is_none(),
        "a foreign id must not resolve to whatever sits at that index here"
    );
    assert!(
        b.metadata_get(b_node).is_some(),
        "the check is discriminating, not a blanket refusal"
    );

    // ---- attachment setters on the global / function handles ----
    let i8_ty = b.i8_type();
    let g = b.add_global("g", i8_ty.const_zero())?;
    assert!(matches!(
        b.view(g)
            .set_metadata(&b, MetadataAttachmentKind::AbsoluteSymbol, a_node),
        Err(IrError::ForeignMetadataId)
    ));

    // ---- attachment + debug-record setters on an instruction ----
    let void_ty = b.void_type();
    let fn_ty = b.function_type_no_parameters(void_ty);
    let f = b.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = b.view(f).append_basic_block(&b, "entry");
    let builder = IrBuilder::with_folder(&b, NoFolder).position_at_end(entry);
    let sum =
        builder.int_add::<i8, _, _, _>(i8_ty.const_int(1_u8), i8_ty.const_int(2_u8), "sum")?;
    builder.ret_void()?;
    let inst = InstructionView::try_from(b.view(sum).as_erased())?;

    assert!(matches!(
        inst.set_metadata(&b, MetadataAttachmentKind::Prof, a_node),
        Err(IrError::ForeignMetadataId)
    ));
    assert!(matches!(
        inst.push_debug_record(
            &b,
            DebugRecord::Variable(DebugVariableRecord::new(
                DebugVariableRecordKind::Value,
                DebugMetadataOperand::Metadata(a_node),
                b_node,
                b_node,
                b_node,
            )),
        ),
        Err(IrError::ForeignMetadataId)
    ));
    // ...including when only the *value* operand is foreign.
    let a_void_ty = a.void_type();
    let a_fn_ty = a.function_type_no_parameters(a_void_ty);
    let a_fn = a.add_function_dyn("a_fn", a_fn_ty, Linkage::External)?;
    let a_entry = a.view(a_fn).append_basic_block(&a, "entry");
    let a_builder = IrBuilder::with_folder(&a, NoFolder).position_at_end(a_entry);
    let a_i8 = a.i8_type();
    let a_sum =
        a_builder.int_add::<i8, _, _, _>(a_i8.const_int(1_u8), a_i8.const_int(2_u8), "sum")?;
    a_builder.ret_void()?;
    assert!(matches!(
        inst.push_debug_record(
            &b,
            DebugRecord::Variable(DebugVariableRecord::new(
                DebugVariableRecordKind::Value,
                DebugMetadataOperand::Value(a.view(a_sum).as_erased().id()),
                b_node,
                b_node,
                b_node,
            )),
        ),
        Err(IrError::ForeignValueId)
    ));

    // Nothing foreign made it in: module B still prints only its own node, and
    // the instruction carries no attachment at all.
    assert!(inst.metadata().is_empty());
    assert!(inst.debug_records().next().is_none());
    let text = format!("{b}");
    assert!(text.contains("from-b"), "{text}");
    assert!(!text.contains("from-a"), "{text}");
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: upstream
/// `Module::getOrInsertNamedMetadata` (`lib/IR/Module.cpp`) returns a bare
/// `NamedMDNode *` with no notion of which module owns it — identity is the
/// pointer. The named-metadata sibling of
/// `a_metadata_id_from_another_module_is_refused_everywhere` above: both
/// modules are `DynBrand`, so only the runtime `ModuleId` tag separates a
/// `NamedMetadataId` minted by one from the other. (Two distinct named brands
/// are separated statically instead —
/// `tests/compile_fail/cross_module_named_metadata_id.rs`.)
#[test]
fn a_named_metadata_id_from_another_module_is_refused() -> Result<(), IrError> {
    let a = Module::dynamic("a");
    let b = Module::dynamic("b");

    // Same shape in both, so the foreign slot is in range in the target.
    let a_named = a.get_or_insert_named_metadata("shared.name");
    let b_named = b.get_or_insert_named_metadata("shared.name");
    let b_node = b.metadata_tuple([b.metadata_string("from-b")])?;

    // The appender refuses the foreign id even though its slot is in range —
    // and reports the *named-metadata* currency, not the operand's.
    assert!(matches!(
        b.named_metadata_add_operand(a_named, b_node),
        Err(IrError::ForeignNamedMetadataId)
    ));

    // Clone-out lookup: `None` for the foreign id, never module B's node at
    // the same slot...
    assert!(
        b.named_metadata_get(a_named).is_none(),
        "a foreign id must not resolve to whatever sits at that slot here"
    );
    // ...and the check is discriminating, not a blanket refusal.
    b.named_metadata_add_operand(b_named, b_node)?;
    let node = b.named_metadata_get(b_named).expect("native id resolves");
    assert_eq!(node.name(), &NamedMetadataName::from("shared.name"));
    assert_eq!(node.operand_count(), 1);

    // The bare-noun lookup agrees with what get-or-insert minted, and a name
    // nothing here holds is `None`.
    assert_eq!(
        b.named_metadata(&NamedMetadataName::from("shared.name")),
        Some(b_named)
    );
    assert!(b.named_metadata(&NamedMetadataName::ModuleFlags).is_none());

    // Nothing foreign made it in: module B prints only its own operand.
    let text = format!("{b}");
    assert!(text.contains("!shared.name = !{!0}"), "{text}");
    assert!(!text.contains("from-a"), "{text}");
    Ok(())
}

/// The native path still works end to end, so the tag check above is a real
/// discrimination rather than a wall: the same calls with the target module's
/// own ids succeed and print.
#[test]
fn a_native_metadata_id_still_resolves_and_prints() -> Result<(), IrError> {
    let m = Module::dynamic("native");
    let node = m.metadata_tuple([m.metadata_string("x")])?;
    let named = m.get_or_insert_named_metadata("my.named");
    m.named_metadata_add_operand(named, node)?;
    assert!(matches!(
        m.metadata_get(node),
        Some(MetadataKind::Tuple { .. })
    ));

    let text = format!("{m}");
    assert!(text.contains("!0 = !{!\"x\"}"), "{text}");
    assert!(text.contains("!my.named = !{!0}"), "{text}");
    Ok(())
}
