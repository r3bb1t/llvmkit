//! Public-surface coverage for the llvmkit 2.0 value-id family (cycle A):
//! `handle.id()` mints a storable, module-tagged id, and
//! [`Module::view`] / [`Module::try_view`] resolve it back into a borrowing
//! handle.
//!
//! llvmkit-specific: LLVM's C++ has no split between a borrowing handle and a
//! storable id — a `Value *` is both. These tests lock the round-trip
//! (`handle -> id -> handle`) for each id in the family, the `Copy + Send`
//! storability of the ids, and document the one branch that cannot be
//! exercised until cycle C.

use llvmkit_ir::{
    BasicBlockLabel, BlockId, Dyn, FloatValue, FloatValueId, FunctionId, GlobalAliasId,
    GlobalIFuncId, GlobalId, GlobalVariable, IRBuilder, IntValue, IntValueId, IntoCallArg,
    IntoErasedValue, IntoFloatValue, IntoIntValue, IntoPointerValue, IrError, Linkage, Module,
    ModuleBrand, ModuleRef, PointerValue, PointerValueId, TypedFunctionId, TypedVarArgsFunctionId,
    Unverified, Value, ValueId,
};

/// Round-trip: every typed handle mints an id whose `view` reproduces the
/// original handle. Covers an int value, a pointer value, a function, a
/// global, and a block label — plus the erased `Value`.
#[test]
fn handles_round_trip_through_to_id_and_view() -> Result<(), IrError> {
    Module::with_new("id-round-trip", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), ptr_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;

        // Int value (a function argument narrowed to its static width).
        let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
        let a_id: IntValueId<i32, _> = a.id();
        assert_eq!(m.view(a_id), a, "IntValue did not survive id/view");

        // Pointer value (the second argument).
        let p: PointerValue<'_, _> = m.view(f).param(1)?.try_into()?;
        let p_id: PointerValueId<_> = p.id();
        assert_eq!(m.view(p_id), p, "PointerValue did not survive id/view");

        // Function value.
        let f_id: FunctionId<Dyn, _> = m.view(f).id();
        assert_eq!(
            m.view(f_id),
            m.view(f),
            "FunctionValue did not survive id/view"
        );

        // Global variable.
        let g: GlobalVariable<'_, _> = m.view(m.add_global("g", i32_ty.const_int(0_u32))?);
        let g_id: GlobalId<_> = g.id();
        assert_eq!(m.view(g_id), g, "GlobalVariable did not survive id/view");

        // Block label (via both the storable id and the linear block).
        let entry = m.view(f).append_basic_block(&m, "entry");
        let b_id: BlockId<Dyn, _> = entry.id();
        let label: BasicBlockLabel<Dyn, _> = m.view(b_id);
        assert_eq!(
            m.view(b_id),
            label,
            "BasicBlockLabel did not survive id/view"
        );
        assert_eq!(
            entry.id(),
            b_id,
            "linear BasicBlock::id disagreed with its label's id",
        );

        // Erased value id.
        let v: Value<'_, _> = a.into_erased();
        let v_id: ValueId<_> = v.id();
        assert_eq!(m.view(v_id), v, "erased Value did not survive id/view");

        Ok(())
    })
}

/// `try_view` returns `Some` for an id that genuinely belongs to the module
/// (the tag-check-passes branch). The foreign-tag `None` branch is exercised
/// in cycle C — see [`foreign_tag_rejection_is_deferred_to_cycle_c`].
#[test]
fn try_view_returns_some_for_owned_ids() -> Result<(), IrError> {
    Module::with_new("id-try-view", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

        assert_eq!(m.try_view(a.id()), Some(a));
        assert_eq!(m.try_view(m.view(f).id()), Some(m.view(f)));

        // `view` and `try_view` agree on the resolvable case.
        assert_eq!(m.view(a.id()), m.try_view(a.id()).expect("owned id"));
        Ok(())
    })
}

/// `view`/`try_view` also work through a [`Verified`](llvmkit_ir::Verified)
/// module token, not just the unverified one — the API lives on
/// `Module<_, _, S>` for every state `S`.
#[test]
fn view_works_on_verified_module() -> Result<(), IrError> {
    Module::with_new("id-verified-view", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let g: GlobalVariable<'_, _> = m.view(m.add_global("g", i32_ty.const_int(7_u32))?);
        let g_id = g.id();

        let f = m.function_builder::<i32, _>("f", fn_ty).build()?;
        let f_id: FunctionId<i32, _> = m.view(f).id();

        let verified = m.verify()?;
        // The id — not the handle — is what crosses the `verify` move: `g`
        // borrows the token `verify` consumed. Re-resolving `g_id` against the
        // verified token must land on the same global.
        assert_eq!(verified.view(g_id).id(), g_id);
        assert_eq!(
            verified.view(f_id),
            verified.view(f),
            "typed FunctionId<i32> round-trip"
        );
        Ok(())
    })
}

/// The ids are `Copy` (a stored id can be re-viewed repeatedly) and `Send`
/// (the invariant `fn(B) -> B` brand phantom is `Send` even though the cycle-A
/// brand `B = Brand<'brand>` is not `'static`). This is a *compile-time*
/// assertion instantiated with the closure's own brand.
#[test]
fn ids_are_copy_and_send() {
    fn assert_copy_send<T: Copy + Send>() {}

    fn id_bounds<'ctx, B: ModuleBrand + 'ctx>(_m: &'ctx Module<'ctx, B, Unverified>) {
        assert_copy_send::<ValueId<B>>();
        assert_copy_send::<IntValueId<i32, B>>();
        assert_copy_send::<FloatValueId<f64, B>>();
        assert_copy_send::<PointerValueId<B>>();
        assert_copy_send::<FunctionId<Dyn, B>>();
        assert_copy_send::<GlobalId<B>>();
        assert_copy_send::<BlockId<Dyn, B>>();
    }

    Module::with_new("id-copy-send", |m| id_bounds(&m));
}

/// `Debug` prints the tag and slot, never the phantom markers.
#[test]
fn id_debug_prints_tag_and_slot() -> Result<(), IrError> {
    Module::with_new("id-debug", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
        let rendered = format!("{:?}", a.id());
        assert!(rendered.contains("IntValueId"), "{rendered}");
        assert!(rendered.contains("tag"), "{rendered}");
        assert!(rendered.contains("slot"), "{rendered}");
        Ok(())
    })
}

/// The foreign-tag rejection path of `view`/`try_view` (an id minted in module
/// A, resolved against module B, whose different [`ModuleId`](llvmkit_ir::ModuleId)
/// makes the tag check fail) cannot be exercised in cycle A: two `Module::with_new`
/// closures have *distinct* lifetime brands `Brand<'a>` / `Brand<'b>`, so an id
/// from one is a different type than `ViewIn<'_, Brand<'other>>` expects and the
/// cross-module call is rejected at **compile** time (the desired safety), never
/// reaching the runtime tag comparison. A genuine runtime foreign-tag test needs
/// two modules that share a brand *type* — available in cycle C when brands stop
/// being lifetimes. Documented here rather than forced with an unsound cast.
#[test]
fn foreign_tag_rejection_is_deferred_to_cycle_c() {
    // Intentionally empty: see the doc comment. The tag-check-passes branch is
    // covered by `try_view_returns_some_for_owned_ids`; the tag-check-fails
    // branch lands in cycle C.
}

/// B1a: [`IRBuilder::view`] / [`IRBuilder::try_view`] are the builder-side
/// twins of the module pair — at a build site the `Module` token is often not
/// in scope but the builder always is, so `b.view(id)` is the canonical read.
/// Both must agree with `Module::view` / `Module::try_view` for the same id,
/// and both must work on an *unpositioned* builder (the methods live on the
/// state-generic impl block).
#[test]
fn builder_view_agrees_with_module_view() -> Result<(), IrError> {
    Module::with_new("builder-view", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), ptr_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;

        let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
        let p: PointerValue<'_, _> = m.view(f).param(1)?.try_into()?;
        let v: Value<'_, _> = a.into_erased();

        let b = IRBuilder::new(&m);

        assert_eq!(
            b.view(a.id()),
            m.view(a.id()),
            "builder view != module view"
        );
        assert_eq!(b.view(p.id()), p, "PointerValueId did not survive b.view");
        assert_eq!(b.view(v.id()), v, "erased ValueId did not survive b.view");
        assert_eq!(
            b.view(b.view(f).id()),
            b.view(f),
            "FunctionId did not survive b.view"
        );

        assert_eq!(
            b.try_view(a.id()),
            Some(a),
            "try_view rejected an id minted in this builder's module",
        );

        Ok(())
    })
}

/// A4: each *typed-value* id lifts back into its handle at a builder operand
/// position via the fallible `Into*Value` conversions, reproducing the handle
/// its `id` was minted from. This is the id analogue of the identity
/// operand lifts (`IntValue: IntoIntValue`), exercised directly here because
/// the builders do not accept ids until cycle B.
#[test]
fn typed_ids_lift_at_operand_positions() -> Result<(), IrError> {
    Module::with_new("id-operand", |m| {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(
            i32_ty,
            [i32_ty.as_type(), f32_ty.as_type(), ptr_ty.as_type()],
            false,
        );
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;

        let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
        let x: FloatValue<'_, f32, _> = m.view(f).param(1)?.try_into()?;
        let p: PointerValue<'_, _> = m.view(f).param(2)?.try_into()?;

        let mref = ModuleRef::from(m.as_view());

        // Owned id + owning module -> Ok(original handle).
        assert_eq!(
            a.id().into_int_value(mref)?,
            a,
            "IntValueId did not lift back to its IntValue operand",
        );
        assert_eq!(
            x.id().into_float_value(mref)?,
            x,
            "FloatValueId did not lift back to its FloatValue operand",
        );
        assert_eq!(
            p.id().into_pointer_value(mref)?,
            p,
            "PointerValueId did not lift back to its PointerValue operand",
        );

        Ok(())
    })
}

/// A4: the typed ids satisfy `IntoCallArg` for free through its blanket impls
/// over `IntoIntValue` / `IntoFloatValue` / `IntoPointerValue` — no dedicated
/// impl was written. A *compile-time* witness: if the bound did not hold this
/// test would not build.
#[test]
fn typed_ids_are_call_args() -> Result<(), IrError> {
    fn assert_int_call_arg<'ctx, B>(_: &IntValueId<i32, B>)
    where
        B: ModuleBrand + 'ctx,
        IntValueId<i32, B>: IntoCallArg<'ctx, i32, B>,
    {
    }
    fn assert_float_call_arg<'ctx, B>(_: &FloatValueId<f32, B>)
    where
        B: ModuleBrand + 'ctx,
        FloatValueId<f32, B>: IntoCallArg<'ctx, f32, B>,
    {
    }
    fn assert_ptr_call_arg<'ctx, B>(_: &PointerValueId<B>)
    where
        B: ModuleBrand + 'ctx,
        PointerValueId<B>: IntoCallArg<'ctx, llvmkit_ir::Ptr, B>,
    {
    }

    Module::with_new("id-call-arg", |m| -> Result<(), IrError> {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(
            i32_ty,
            [i32_ty.as_type(), f32_ty.as_type(), ptr_ty.as_type()],
            false,
        );
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
        let x: FloatValue<'_, f32, _> = m.view(f).param(1)?.try_into()?;
        let p: PointerValue<'_, _> = m.view(f).param(2)?.try_into()?;
        assert_int_call_arg(&a.id());
        assert_float_call_arg(&x.id());
        assert_ptr_call_arg(&p.id());
        Ok(())
    })
}

/// A4: the foreign-tag rejection branch of the operand lifts (an id minted in
/// module A, lifted against module B -> `Err(IrError::ForeignValueId)`) is
/// deferred to cycle C for the same reason as
/// [`foreign_tag_rejection_is_deferred_to_cycle_c`]: two `Module::with_new`
/// closures carry distinct lifetime brands, so a cross-module lift is rejected
/// at compile time and never reaches the runtime tag comparison. A genuine
/// runtime foreign-tag test needs two modules sharing a brand *type*, available
/// in cycle C.
#[test]
fn foreign_tag_operand_rejection_is_deferred_to_cycle_c() {
    // Intentionally empty: see the doc comment. The tag-check-passes branch is
    // covered by `typed_ids_lift_at_operand_positions`.
}

// --------------------------------------------------------------------------
// B1-ops: `IntoErasedValue` — every id at an erased-by-design operand slot
// --------------------------------------------------------------------------

/// B1-ops: every id in the family — including the *erased* [`ValueId`] —
/// satisfies [`IntoErasedValue`], the bound carried by operand slots whose
/// declared parameter type is the erased `Value`. A *compile-time* witness:
/// if any bound did not hold this test would not build.
///
/// The erased `ValueId` is admitted here and nowhere else. These slots are
/// erased *by design*, so erased-in / erased-out is not the silent
/// erased -> typed narrowing that `IntoIntValue` & co. forbid — a doctrine the
/// compile-fail fixture `erased_id_not_int_operand.rs` still locks.
#[test]
fn every_id_is_an_erased_operand() {
    fn assert_erased_operand<'ctx, B, I>(_: &I)
    where
        B: ModuleBrand + 'ctx,
        I: IntoErasedValue<'ctx, B>,
    {
    }

    Module::with_new("id-erased-operand", |m| {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(
            i32_ty,
            [i32_ty.as_type(), f32_ty.as_type(), ptr_ty.as_type()],
            false,
        );
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let g: GlobalVariable<'_, _> = m.view(m.add_global("g", i32_ty.const_int(0_u32)).unwrap());

        let a: IntValue<'_, i32, _> = m.view(f).param(0).unwrap().try_into().unwrap();
        let x: FloatValue<'_, f32, _> = m.view(f).param(1).unwrap().try_into().unwrap();
        let p: PointerValue<'_, _> = m.view(f).param(2).unwrap().try_into().unwrap();

        assert_erased_operand(&a.into_erased().id());
        assert_erased_operand(&a.id());
        assert_erased_operand(&x.id());
        assert_erased_operand(&p.id());
        assert_erased_operand(&m.view(f).id());
        assert_erased_operand(&g.id());

        // B1e: the alias / ifunc ids join the family — both handles are
        // `IsValue`s, so both widen at an erased-by-design operand slot.
        let alias = m
            .alias_builder("alias", i32_ty.as_type(), g)
            .build()
            .unwrap();
        let ifunc = m
            .ifunc_builder("ifunc", i32_ty.as_type(), g)
            .build()
            .unwrap();
        assert_erased_operand(&alias);
        assert_erased_operand(&ifunc);
    });
}

/// B1-ops: a stored id drives an erased operand slot end-to-end, with no
/// intervening `view`. The typed id goes in at `build_store`'s value operand
/// and the erased id at `build_freeze`'s — both slots that took only a
/// borrowing handle before this slice — and the emitted IR is exactly what the
/// handle spelling produces.
#[test]
fn ids_drive_erased_operand_slots_without_a_view() -> Result<(), IrError> {
    Module::with_new("id-erased-store", |m| {
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(m.void_type().as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function_dyn("inc", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let p: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;

        let v = b.build_int_load::<i32, _, _>(p, "v")?;
        // `build_int_add` already hands back a storable id (cycle B1a).
        let n: IntValueId<i32, _> = b.build_int_add(v, 1_i32, "n")?;

        // Typed id straight into the erased *stored value* operand.
        b.build_store(n, p)?;
        // Erased id straight into `freeze`'s erased operand.
        let erased: ValueId<_> = b.view(n).into_erased().id();
        b.build_freeze(erased, "fr")?;
        b.build_ret_void()?;

        let text = format!("{m}");
        assert!(
            text.contains("store i32 %n, ptr %0, align 4\n"),
            "typed id did not reach the store operand; got:\n{text}"
        );
        assert!(
            text.contains("%fr = freeze i32 %n\n"),
            "erased id did not reach the freeze operand; got:\n{text}"
        );
        Ok(())
    })
}

/// B1e: the module-level *declaration* family hands back its id directly, so
/// the round-trip witnessed here is the mirror of
/// [`handles_round_trip_through_to_id_and_view`] — `id -> view -> id` — across
/// the four ids this slice introduced plus the two it reuses. Also locks
/// [`TypedFunctionId::as_function`]'s pure retag against the facade's own
/// `as_function`: both must name the same function.
#[test]
fn declaration_ids_round_trip_through_view_and_id() -> Result<(), IrError> {
    Module::with_new("id-declarations", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);

        // Erased function declaration, and the chainable builder's tail.
        let raw: FunctionId<Dyn, _> = m.add_function_dyn("raw", fn_ty, Linkage::External)?;
        assert_eq!(m.view(raw).id(), raw, "FunctionId did not survive view/id");
        let built: FunctionId<i32, _> = m.function_builder::<i32, _>("built", fn_ty).build()?;
        assert_eq!(
            m.view(built).id(),
            built,
            "FunctionBuilder::build's id did not survive view/id"
        );

        // Typed facade: the full `(Ret, Params)` schema rides on the id.
        let add: TypedFunctionId<i32, (i32, i32), _> =
            m.add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)?;
        assert_eq!(
            m.view(add).id(),
            add,
            "TypedFunctionId did not survive view/id"
        );
        assert_eq!(
            m.view(add.as_function()),
            m.view(add).as_function(),
            "TypedFunctionId::as_function disagreed with the facade's as_function"
        );

        // Variadic twin.
        let va: TypedVarArgsFunctionId<i32, (i32,), _> =
            m.add_typed_varargs_function::<i32, (i32,), _>("va", Linkage::External)?;
        assert_eq!(
            m.view(va).id(),
            va,
            "TypedVarArgsFunctionId did not survive view/id"
        );

        // Global variable, alias and ifunc.
        let g: GlobalId<_> = m.add_global("g", i32_ty.const_int(0_u32))?;
        assert_eq!(m.view(g).id(), g, "GlobalId did not survive view/id");

        let alias: GlobalAliasId<_> = m
            .alias_builder("alias", i32_ty.as_type(), m.view(g))
            .build()?;
        assert_eq!(
            m.view(alias).id(),
            alias,
            "GlobalAliasId did not survive view/id"
        );

        let ifunc: GlobalIFuncId<_> = m
            .ifunc_builder("ifunc", i32_ty.as_type(), m.view(g))
            .build()?;
        assert_eq!(
            m.view(ifunc).id(),
            ifunc,
            "GlobalIFuncId did not survive view/id"
        );

        Ok(())
    })
}
