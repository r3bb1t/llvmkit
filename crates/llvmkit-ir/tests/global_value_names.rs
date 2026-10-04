//! Renaming a global value through its module's value symbol table.
//!
//! Upstream renames a function, global variable, alias or ifunc with
//! `Value::setName`: `Value::setNameImpl` asks the file-static `getSymTab` for
//! the table to update — the parent `Module`'s, for a `GlobalValue` —
//! `ValueSymbolTable::createValueName` takes the new name or uniques a clash
//! through `ValueSymbolTable::makeUniqueName`, the old entry is removed, and
//! `Value::setName` then runs `Function::updateAfterNameChange` on a function.
//! The last cases cover what `setNameImpl` asserts for every value, which
//! llvmkit refuses, and `getOrInsertIntrinsicDeclarationImpl`, whose
//! `.invalid` arm is a global rename.
//!
//! `unittests/IR/ValueTest.cpp::TEST(ValueTest, setNameShrink)` renames a
//! function; it needs the parser and is ported whole in
//! `llvmkit-asmparser/tests/value_test.rs`. The cases below are
//! llvmkit-specific. Searched with `rg -n "setName\(" llvm/unittests` at the
//! vendored tag `llvmorg-22.1.4` (the repo commit does not pin `orig_cpp/`):
//! within `unittests/IR` the other hits rename a `StructType`
//! (`TypesTest.cpp`) or an argument; outside it, the one file that renames a
//! function is `unittests/Frontend/OpenMPIRBuilderTest.cpp`, whose
//! `F->setName("func")` sets up fixtures for `OpenMPIRBuilder`, which llvmkit
//! does not have. Each expected answer below is therefore derived by reading
//! the routine its doc comment names.

use llvmkit_ir::{
    Dyn, HasName, InlineAsmOptions, IntValue, IntrinsicId, InvalidValueNameReason, IrBuilder,
    IrError, Linkage, Module, PointerValue, SetName, Type,
};

/// A rename onto a name another global holds is uniqued with a dot and the
/// table's counter, and the counter is the *module's*: one table holds every
/// kind of global value, so a function renamed onto the same name takes the
/// next number. The names the renames give up are free again. Derived from
/// `ValueSymbolTable::createValueName` (a failed `vmap.insert` falls to
/// `makeUniqueName`) and `ValueSymbolTable::makeUniqueName` (a `GlobalValue`
/// appends `"."`, then `++LastUnique`). Positive control: a free name is taken
/// verbatim.
#[test]
fn a_taken_name_is_uniqued_with_a_dot_and_the_module_counter() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let a = m.add_global("a", i32_ty.const_int(0_i32)).expect("@a");
    let b = m.add_global("b", i32_ty.const_int(1_i32)).expect("@b");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(m.void_type()),
            Linkage::External,
        )
        .expect("@f");

    m.view(b).set_name(&m, "fresh").expect("renames");
    assert_eq!(m.view(b).name().as_deref(), Some("fresh"));

    m.view(b).set_name(&m, "a").expect("renames");
    assert_eq!(m.view(b).name().as_deref(), Some("a.1"));
    m.view(f).set_name(&m, "a").expect("renames");
    assert_eq!(m.view(f).name().as_deref(), Some("a.2"));
    assert_eq!(m.view(a).name().as_deref(), Some("a"));

    assert_eq!(m.global("a"), Some(a));
    assert_eq!(m.global("a.1"), Some(b));
    assert_eq!(m.function_dyn("a.2"), Some(f));
    // One table, but each lookup is `dyn_cast_or_null` to its own kind:
    // `Module::getNamedGlobal` does not answer for a function, nor
    // `Module::getFunction` for a global variable.
    assert_eq!(m.global("a.2"), None);
    assert_eq!(m.function_dyn("a"), None);
    assert!(
        matches!(m.function::<()>("a"), Ok(None)),
        "a name a global variable holds is no function, before any return-marker check"
    );
    assert_eq!(m.global("b"), None);
    assert_eq!(m.global("fresh"), None);
    assert_eq!(m.function_dyn("f"), None);
    m.add_global("b", i32_ty.const_int(2_i32))
        .expect("a name a rename gave up is free");
}

/// On a module whose target is NVPTX a clash is uniqued with the bare number:
/// `ValueSymbolTable::makeUniqueName` appends the dot only when
/// `!(M && M->getTargetTriple().isNVPTX())`, and `Triple::isNVPTX` is the
/// architecture `nvptx` or `nvptx64` — the first `-`-separated component, as
/// `Triple::Triple` parses it. The triple is read at each rename, so the same
/// module uses the dot before its triple is set (the positive control).
#[test]
fn an_nvptx_module_uniques_a_clash_without_the_dot() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    m.add_global("a", i32_ty.const_int(0_i32)).expect("@a");
    let b = m.add_global("b", i32_ty.const_int(1_i32)).expect("@b");
    let c = m.add_global("c", i32_ty.const_int(2_i32)).expect("@c");

    m.view(b).set_name(&m, "a").expect("renames");
    assert_eq!(m.view(b).name().as_deref(), Some("a.1"));

    m.set_target_triple("nvptx64-nvidia-cuda");
    m.view(c).set_name(&m, "a").expect("renames");
    assert_eq!(m.view(c).name().as_deref(), Some("a2"));
}

/// Renaming a global to the name it already has changes nothing and does not
/// advance the counter: `Value::setNameImpl` returns at `getName() == NameRef`
/// before `createValueName` could see a clash with the value itself.
#[test]
fn renaming_a_global_to_its_own_name_is_a_no_op() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let a = m.add_global("a", i32_ty.const_int(0_i32)).expect("@a");
    let b = m.add_global("b", i32_ty.const_int(1_i32)).expect("@b");

    m.view(a).set_name(&m, "a").expect("renames");
    assert_eq!(m.view(a).name().as_deref(), Some("a"));
    m.view(b).set_name(&m, "a").expect("renames");
    assert_eq!(m.view(b).name().as_deref(), Some("a.1"));
}

/// Clearing a global's name removes it from the table: `Value::setNameImpl`
/// removes the old `ValueName` and returns for an empty name. The global is
/// then unnamed — the printer numbers it — and its old name is free.
#[test]
fn clearing_a_global_name_frees_the_name_and_prints_a_slot() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let g = m.add_global("g", i32_ty.const_int(7_i32)).expect("@g");

    m.view(g).clear_name(&m);
    assert_eq!(m.view(g).name(), None);
    assert_eq!(m.global("g"), None);
    let text = format!("{m}");
    assert!(text.contains("@0 = global i32 7\n"), "{text}");
    m.add_global("g", i32_ty.const_int(8_i32))
        .expect("the cleared name is free");
}

/// Every kind of global value renames through the one table: the old name no
/// longer looks anything up, the new one finds the same value, and the
/// printer writes the new name at the definition and at each use.
#[test]
fn every_global_kind_renames_and_prints_under_its_new_name() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let g = m.add_global("g", i32_ty.const_int(0_i32)).expect("@g");
    let resolver = m
        .add_function_dyn(
            "resolver",
            m.function_type_no_parameters(ptr_ty),
            Linkage::External,
        )
        .expect("@resolver");
    let alias = m
        .alias_builder("al", i32_ty.as_type(), m.view(g))
        .build()
        .expect("@al");
    let ifunc = m
        .ifunc_builder(
            "ifn",
            i32_ty.as_type(),
            m.view(resolver).as_global_constant_ptr(),
        )
        .build()
        .expect("@ifn");

    m.view(g).set_name(&m, "g2").expect("renames");
    m.view(resolver).set_name(&m, "resolver2").expect("renames");
    m.view(alias).set_name(&m, "al2").expect("renames");
    m.view(ifunc).set_name(&m, "ifn2").expect("renames");

    assert_eq!(m.global("g"), None);
    assert_eq!(m.global("g2"), Some(g));
    assert_eq!(m.function_dyn("resolver"), None);
    assert_eq!(m.function_dyn("resolver2"), Some(resolver));
    assert_eq!(m.alias("al"), None);
    assert_eq!(m.alias("al2"), Some(alias));
    assert_eq!(m.ifunc("ifn"), None);
    assert_eq!(m.ifunc("ifn2"), Some(ifunc));

    let text = format!("{m}");
    assert!(text.contains("@g2 = global i32 0\n"), "{text}");
    assert!(text.contains("@al2 = alias i32, ptr @g2\n"), "{text}");
    assert!(
        text.contains("@ifn2 = ifunc i32, ptr @resolver2\n"),
        "{text}"
    );
    assert!(text.contains("declare ptr @resolver2()\n"), "{text}");
    for old in ["@g ", "@g\n", "@resolver(", "@resolver\n", "@al ", "@ifn "] {
        assert!(!text.contains(old), "{old:?} survived the rename:\n{text}");
    }
}

/// The erased handle renames a global as the typed one does — `Value::setName`
/// is one routine — and so does the `ptr @g` constant that stands for the
/// global, because upstream's `GlobalValue` *is* that constant
/// (`docs/divergences.md` D3). Reading the name through the constant answers
/// the global's name, as `getName` on `@g` does.
#[test]
fn a_global_renames_through_its_erased_handle_and_its_constant() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let g = m.add_global("g", i32_ty.const_int(0_i32)).expect("@g");

    m.view(g)
        .as_erased()
        .set_name(&m, "via_value")
        .expect("renames");
    assert_eq!(m.view(g).name().as_deref(), Some("via_value"));

    let constant = m.view(g).as_global_constant_ptr();
    constant.set_name(&m, "via_constant").expect("renames");
    assert_eq!(m.view(g).name().as_deref(), Some("via_constant"));
    assert_eq!(constant.name().as_deref(), Some("via_constant"));
    assert_eq!(m.global("via_constant"), Some(g));
}

/// Renaming a function recomputes its intrinsic identity from the new name, as
/// `Value::setName` does through `Function::updateAfterNameChange`: a name
/// without the `llvm.` prefix is no intrinsic, and an intrinsic's name — on a
/// function with that intrinsic's signature — is that intrinsic, whether the
/// function began as a declaration of it or as a plain function. Upstream has
/// no unit test for it: `rg -n "updateAfterNameChange" llvm/unittests` is
/// empty.
#[test]
fn renaming_a_function_recomputes_its_intrinsic_identity() {
    let m = Module::dynamic("m");
    let trap = m
        .get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new())
        .expect("llvm.trap");
    assert_eq!(m.view(trap).intrinsic_id(), Some(IntrinsicId::TRAP));

    m.view(trap).set_name(&m, "not_trap").expect("renames");
    assert_eq!(m.view(trap).intrinsic_id(), None);
    m.view(trap).set_name(&m, "llvm.trap").expect("renames");
    assert_eq!(m.view(trap).intrinsic_id(), Some(IntrinsicId::TRAP));

    let plain = m
        .add_function_dyn(
            "plain",
            m.function_type_no_parameters(m.void_type()),
            Linkage::External,
        )
        .expect("@plain");
    assert_eq!(m.view(plain).intrinsic_id(), None);
    m.view(plain)
        .set_name(&m, "llvm.debugtrap")
        .expect("renames");
    assert_eq!(
        m.view(plain).intrinsic_id(),
        IntrinsicId::lookup("llvm.debugtrap")
    );
    assert!(m.view(plain).intrinsic_id().is_some());
}

/// A global variable that holds an intrinsic's name blocks the intrinsic's
/// declaration. Upstream's `Intrinsic::getOrInsertDeclaration` would
/// `cast<Function>` the variable `Module::getOrInsertFunction` returns for the
/// name — an assertion — and llvmkit refuses instead. While each kind of
/// global kept its own name map, llvmkit declared a second global under the
/// same name. Positive control: without the variable the declaration succeeds.
#[test]
fn a_global_variable_holding_an_intrinsic_name_blocks_its_declaration() {
    let free = Module::dynamic("free");
    free.get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new())
        .expect("a free name declares the intrinsic");

    let m = Module::dynamic("m");
    m.add_global("llvm.trap", m.i32_type().const_int(0_i32))
        .expect("a global variable may take an llvm. name");
    let before = format!("{m}");
    let refused =
        m.get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new());
    assert!(
        matches!(&refused, Err(IrError::DuplicateFunctionName { name }) if name == "llvm.trap"),
        "{refused:?}"
    );
    assert_eq!(
        format!("{m}"),
        before,
        "a refused declaration must not mutate"
    );
}

/// Declaring an intrinsic over a function of another type that holds its
/// name renames that function `<name>.invalid` and declares the intrinsic
/// afresh. `getOrInsertIntrinsicDeclarationImpl` finds
/// `F->getFunctionType() != FT`, runs `F->setName(F->getName() +
/// ".invalid")`, and calls `Module::getOrInsertFunction` again, which now
/// finds the name free. The renamed function keeps its type and gets no
/// intrinsic identity: `llvm.trap` is not overloaded, so `lookupIntrinsicID`
/// matches only the exact name. The `getOrInsertDeclaration` tests in
/// `unittests/IR/IntrinsicsTest.cpp` declare into a module that holds no such
/// name, and `rg -n "invalid" llvm/unittests/IR/IntrinsicsTest.cpp` is empty
/// at the vendored tag `llvmorg-22.1.4`; the expected names are derived from
/// that routine.
#[test]
fn a_mismatched_function_holding_an_intrinsic_name_is_renamed_invalid() {
    let m = Module::dynamic("m");
    let stale = m
        .add_function_dyn(
            "stale",
            m.function_type_no_parameters(m.i32_type()),
            Linkage::External,
        )
        .expect("@stale");
    m.view(stale)
        .set_name(&m, "llvm.trap")
        .expect("a rename may take an llvm. name");

    let trap = m
        .get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new())
        .expect("the stale holder is renamed and llvm.trap declared");

    assert_ne!(trap, stale);
    assert_eq!(m.view(stale).name().as_deref(), Some("llvm.trap.invalid"));
    assert_eq!(m.view(stale).intrinsic_id(), None);
    assert_eq!(m.view(trap).name().as_deref(), Some("llvm.trap"));
    assert_eq!(m.view(trap).intrinsic_id(), Some(IntrinsicId::TRAP));
    assert_eq!(m.function_dyn("llvm.trap"), Some(trap));
    assert_eq!(m.function_dyn("llvm.trap.invalid"), Some(stale));
    let text = format!("{m}");
    assert!(
        text.contains("declare i32 @llvm.trap.invalid()\n"),
        "{text}"
    );
    assert!(text.contains("declare void @llvm.trap()"), "{text}");
}

/// When the function holding the intrinsic's name has the intrinsic's type,
/// `getOrInsertIntrinsicDeclarationImpl` returns it — `if
/// (F->getFunctionType() == FT) return F;` — a definition included: upstream
/// compares the type and nothing else. The renamed definition carries the
/// intrinsic's identity, as `Function::updateAfterNameChange` gives it.
/// Derived from those routines; no upstream unit test drives the arm.
#[test]
fn a_same_typed_definition_holding_an_intrinsic_name_is_returned() {
    let m = Module::dynamic("m");
    let body = m
        .add_function_dyn(
            "body",
            m.function_type_no_parameters(m.void_type()),
            Linkage::External,
        )
        .expect("@body");
    let entry = m.view(body).append_basic_block(&m, "entry");
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .ret_void()
        .expect("ret void");
    m.view(body)
        .set_name(&m, "llvm.trap")
        .expect("a rename may take an llvm. name");
    assert_eq!(m.view(body).intrinsic_id(), Some(IntrinsicId::TRAP));
    let before = format!("{m}");

    let trap = m
        .get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new())
        .expect("the definition is returned");

    assert_eq!(trap, body);
    assert_eq!(format!("{m}"), before, "nothing is renamed or declared");
}

/// A function renamed to an intrinsic's name gets no intrinsic identity when
/// its signature is not that intrinsic's. This pins llvmkit's recorded
/// divergence (`docs/divergences.md`, the `updateAfterNameChange` entry), not
/// upstream's answer: `Function::updateAfterNameChange` stores
/// `Intrinsic::lookupIntrinsicID(Name)`, from the name alone, so upstream
/// answers `Intrinsic::memcpy` here, and the `None` assertion flips when the
/// entry closes. The function first takes an intrinsic's name whose signature
/// it has — `llvm.debugtrap`, `void ()` — and gets that identity (the positive
/// control), so the mismatched rename after it must also drop an identity the
/// function already carried, not merely fail to add one.
#[test]
fn a_rename_onto_an_intrinsic_name_with_another_signature_stores_no_identity() {
    let m = Module::dynamic("m");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(m.void_type()),
            Linkage::External,
        )
        .expect("@f");

    m.view(f).set_name(&m, "llvm.debugtrap").expect("renames");
    assert_eq!(
        m.view(f).intrinsic_id(),
        IntrinsicId::lookup("llvm.debugtrap")
    );
    assert!(m.view(f).intrinsic_id().is_some());

    m.view(f)
        .set_name(&m, "llvm.memcpy.p0.p0.i64")
        .expect("renames");
    assert_eq!(m.view(f).intrinsic_id(), None);
}

/// `FunctionValue::is_intrinsic` mirrors `Function::isIntrinsic`, which
/// answers `HasLLVMReservedName`: whether the name starts with `llvm.`, set
/// from the name alone by `Function::updateAfterNameChange`. Upstream's header
/// warns it can be true while `getIntrinsicID()` is `not_intrinsic`; an
/// `llvm.` name no intrinsic has is that case on both sides, since
/// `lookupIntrinsicID` answers `not_intrinsic` for it. Positive control: a name
/// without the prefix answers false, and a declared intrinsic answers true
/// with its id. Derived from those routines; no upstream unit test calls
/// `isIntrinsic` on a renamed function.
#[test]
fn is_intrinsic_answers_the_llvm_prefix_alone() {
    let m = Module::dynamic("m");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(m.void_type()),
            Linkage::External,
        )
        .expect("@f");
    assert!(!m.view(f).is_intrinsic());

    m.view(f)
        .set_name(&m, "llvm.no_such_intrinsic")
        .expect("renames");
    assert!(m.view(f).is_intrinsic());
    assert_eq!(m.view(f).intrinsic_id(), None);

    let trap = m
        .get_or_insert_intrinsic_declaration_by_id(IntrinsicId::TRAP, Vec::<Type<'_, _>>::new())
        .expect("llvm.trap");
    assert!(m.view(trap).is_intrinsic());
    assert_eq!(m.view(trap).intrinsic_id(), Some(IntrinsicId::TRAP));
}

/// `Value::setNameImpl` asserts `!NameRef.contains(0)` ("Null bytes are not
/// allowed in names") before it asks `getSymTab` for a table, so the rule is
/// the same for a global and a local name. llvmkit refuses with
/// `InvalidValueNameReason::ContainsNul` and the value keeps its name: the
/// printed `@"a\00b"` would not read back, since `LLLexer` rejects a NUL in a
/// name. Derived from that routine. Positive control: the NUL-free name
/// renames.
#[test]
fn a_name_with_a_nul_byte_is_refused_and_changes_nothing() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let g = m.add_global("g", i32_ty.const_int(0_i32)).expect("@g");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type(m.void_type(), [i32_ty.as_type()]),
            Linkage::External,
        )
        .expect("@f");
    let before = format!("{m}");

    let refused = m.view(g).set_name(&m, "a\0b");
    assert!(
        matches!(
            &refused,
            Err(IrError::InvalidValueName {
                name,
                reason: InvalidValueNameReason::ContainsNul,
            }) if name == "a\0b"
        ),
        "{refused:?}"
    );
    let refused = m
        .view(f)
        .param(0)
        .expect("one parameter")
        .set_name(&m, "x\0");
    assert!(
        matches!(
            &refused,
            Err(IrError::InvalidValueName {
                reason: InvalidValueNameReason::ContainsNul,
                ..
            })
        ),
        "{refused:?}"
    );
    assert_eq!(format!("{m}"), before, "a refused name must not mutate");
    assert_eq!(m.global("g"), Some(g));

    m.view(g)
        .set_name(&m, "ab")
        .expect("a NUL-free name renames");
    assert_eq!(m.view(g).name().as_deref(), Some("ab"));
}

/// `Value::setNameImpl` asserts `!getType()->isVoidTy()` ("Cannot assign a
/// name to void values!") once its empty-name and unchanged-name returns have
/// passed. llvmkit refuses with `InvalidValueNameReason::VoidValue`, and the
/// `store` stays unnamed; clearing the name it does not have is the empty-name
/// return and changes nothing. Derived from that routine. Positive control: a
/// non-void instruction in the same block takes a name.
#[test]
fn a_void_value_refuses_a_name() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let f = m.add_function_dyn(
        "f",
        m.function_type(m.void_type(), [i32_ty.as_type(), ptr_ty.as_type()]),
        Linkage::External,
    )?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let v: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let p: PointerValue<'_, _> = m.view(f).param(1)?.try_into()?;
    let sum = b.int_add(v, 1_i32, "")?;
    let store = b.store(sum, p)?;

    let refused = store.to_erased().set_name(&m, "st");
    assert!(
        matches!(
            &refused,
            Err(IrError::InvalidValueName {
                name,
                reason: InvalidValueNameReason::VoidValue,
            }) if name == "st"
        ),
        "{refused:?}"
    );
    assert_eq!(store.to_erased().name(), None);
    store.to_erased().clear_name(&m);
    assert_eq!(store.to_erased().name(), None);

    b.view(sum).set_name(&m, "sum")?;
    assert_eq!(b.view(sum).name().as_deref(), Some("sum"));
    Ok(())
}

/// `getSymTab` ends in `assert(isa<Constant>(V) && "Unknown value type!")`
/// and `return true`. `InlineAsm` and `MetadataAsValue` are `Value`s but not
/// `Constant`s, so upstream asserts when either is named; llvmkit refuses with
/// the matching `InvalidValueNameReason`. Positive control: a constant — the
/// arm's own case — is accepted and stays unnamed, as `setNameImpl` returns
/// when `getSymTab` answers `true`. Derived from those routines.
#[test]
fn inline_asm_and_metadata_values_refuse_a_name() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let asm = m.inline_asm(
        m.function_type_no_parameters(m.void_type()),
        "nop",
        "",
        InlineAsmOptions::new(),
    );
    let refused = asm.as_erased().set_name(&m, "asm");
    assert!(
        matches!(
            &refused,
            Err(IrError::InvalidValueName {
                reason: InvalidValueNameReason::InlineAsm,
                ..
            })
        ),
        "{refused:?}"
    );

    let node = m.metadata_tuple([m.metadata_string("x")])?;
    let md = m.metadata_as_value(node)?;
    let refused = md.set_name(&m, "md");
    assert!(
        matches!(
            &refused,
            Err(IrError::InvalidValueName {
                reason: InvalidValueNameReason::MetadataAsValue,
                ..
            })
        ),
        "{refused:?}"
    );

    let five = m.i32_type().const_int(5_i32);
    five.set_name(&m, "five")?;
    assert_eq!(five.name(), None);
    Ok(())
}

/// Declaring an intrinsic over a same-typed definition that holds its name
/// returns the definition and leaves its arguments as they were.
/// `getOrInsertIntrinsicDeclarationImpl` returns `F` at `F->getFunctionType()
/// == FT` and names no argument; TableGen's `ArgName` is pretty-printer data.
/// The intrinsic here, `llvm.nvvm.tensormap.replace.fill.mode`, gives its
/// argument 1 the `ArgName` `fill_mode`, which llvmkit used to write onto the
/// returned function's argument. Derived from that routine; no upstream unit
/// test declares an intrinsic over a definition. Positive control: the
/// definition carries the intrinsic's identity, so this is the
/// `getFunctionType() == FT` arm.
#[test]
fn declaring_an_intrinsic_over_a_same_typed_definition_leaves_its_arguments_alone()
-> Result<(), IrError> {
    let m = Module::dynamic("m");
    let ptr_ty = m.ptr_type(0);
    let f = m.add_function_dyn(
        "f",
        m.function_type(m.void_type(), [ptr_ty.as_type(), m.i32_type().as_type()]),
        Linkage::External,
    )?;
    m.view(f).param(0)?.set_name(&m, "p")?;
    m.view(f).param(1)?.set_name(&m, "mode")?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .ret_void()?;
    m.view(f)
        .set_name(&m, "llvm.nvvm.tensormap.replace.fill.mode.p0")?;
    let id = m
        .view(f)
        .intrinsic_id()
        .expect("the definition is the intrinsic");
    let before = format!("{m}");

    let declared = m.get_or_insert_intrinsic_declaration_by_id(id, [ptr_ty.as_type()])?;

    assert_eq!(declared, f);
    assert_eq!(m.view(f).param(0)?.name().as_deref(), Some("p"));
    assert_eq!(m.view(f).param(1)?.name().as_deref(), Some("mode"));
    assert_eq!(format!("{m}"), before);
    Ok(())
}

/// A refused declaration leaves a mismatched holder of the intrinsic's name
/// exactly as it was. `getOrInsertIntrinsicDeclarationImpl` cannot fail, so
/// llvmkit runs every check that can refuse before the `.invalid` arm renames
/// anything; here the request's overload type belongs to another module, and
/// the refusal (`IrError::ForeignType`) comes before the lookup. Positive
/// control: the same request with this module's `i32` renames the holder
/// `llvm.ctpop.i32.invalid` and declares `llvm.ctpop.i32`. llvmkit-specific: the
/// refusal has no upstream counterpart, and the renaming is derived from that
/// routine.
#[test]
fn a_refused_intrinsic_declaration_leaves_a_mismatched_holder_alone() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let other = Module::dynamic("other");
    let holder = m.add_function_dyn(
        "holder",
        m.function_type_no_parameters(m.void_type()),
        Linkage::External,
    )?;
    m.view(holder).set_name(&m, "llvm.ctpop.i32")?;
    let before = format!("{m}");

    let refused = m.get_or_insert_intrinsic_declaration_by_id(
        IntrinsicId::CTPOP,
        [other.i32_type().as_type()],
    );
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    assert_eq!(
        format!("{m}"),
        before,
        "a refused declaration must not mutate"
    );
    assert_eq!(m.view(holder).name().as_deref(), Some("llvm.ctpop.i32"));

    let ctpop =
        m.get_or_insert_intrinsic_declaration_by_id(IntrinsicId::CTPOP, [m.i32_type().as_type()])?;
    assert_ne!(ctpop, holder);
    assert_eq!(
        m.view(holder).name().as_deref(),
        Some("llvm.ctpop.i32.invalid")
    );
    assert_eq!(m.view(ctpop).name().as_deref(), Some("llvm.ctpop.i32"));
    Ok(())
}
