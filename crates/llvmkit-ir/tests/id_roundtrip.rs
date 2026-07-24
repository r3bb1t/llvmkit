//! Public-surface coverage for the llvmkit 2.0 value-id family (cycle A):
//! `handle.to_id()` mints a storable, module-tagged id, and
//! [`Module::view`] / [`Module::try_view`] resolve it back into a borrowing
//! handle.
//!
//! llvmkit-specific: LLVM's C++ has no split between a borrowing handle and a
//! storable id — a `Value *` is both. These tests lock the round-trip
//! (`handle -> id -> handle`) for each id in the family, the `Copy + Send`
//! storability of the ids, and document the one branch that cannot be
//! exercised until cycle C.

use llvmkit_ir::{
    BasicBlockLabel, BlockId, Dyn, FloatValueId, FunctionId, GlobalId, GlobalVariable, IntValue,
    IntValueId, IrError, Linkage, Module, ModuleBrand, PointerValue, PointerValueId, Unverified,
    Value, ValueId,
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
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let a_id: IntValueId<i32, _> = a.to_id();
        assert_eq!(m.view(a_id), a, "IntValue did not survive to_id/view");

        // Pointer value (the second argument).
        let p: PointerValue = f.param(1)?.try_into()?;
        let p_id: PointerValueId<_> = p.to_id();
        assert_eq!(m.view(p_id), p, "PointerValue did not survive to_id/view");

        // Function value.
        let f_id: FunctionId<Dyn, _> = f.to_id();
        assert_eq!(m.view(f_id), f, "FunctionValue did not survive to_id/view");

        // Global variable.
        let g: GlobalVariable = m.add_global("g", i32_ty.const_int(0_u32))?;
        let g_id: GlobalId<_> = g.to_id();
        assert_eq!(m.view(g_id), g, "GlobalVariable did not survive to_id/view");

        // Block label (via both the copyable label and the linear block).
        let entry = f.append_basic_block(&m, "entry");
        let label: BasicBlockLabel<Dyn, _> = entry.label();
        let b_id: BlockId<Dyn, _> = label.to_id();
        assert_eq!(
            m.view(b_id),
            label,
            "BasicBlockLabel did not survive to_id/view"
        );
        assert_eq!(
            entry.to_id(),
            b_id,
            "linear BasicBlock::to_id disagreed with its label's id",
        );

        // Erased value id.
        let v: Value = a.into_erased();
        let v_id: ValueId<_> = v.to_id();
        assert_eq!(m.view(v_id), v, "erased Value did not survive to_id/view");

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
        let a: IntValue<i32> = f.param(0)?.try_into()?;

        assert_eq!(m.try_view(a.to_id()), Some(a));
        assert_eq!(m.try_view(f.to_id()), Some(f));

        // `view` and `try_view` agree on the resolvable case.
        assert_eq!(m.view(a.to_id()), m.try_view(a.to_id()).expect("owned id"));
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
        let g: GlobalVariable = m.add_global("g", i32_ty.const_int(7_u32))?;
        let g_id = g.to_id();

        let f = m.function_builder::<i32, _>("f", fn_ty).build()?;
        let f_id: FunctionId<i32, _> = f.to_id();

        let verified = m.verify()?;
        assert_eq!(verified.view(g_id), g);
        assert_eq!(verified.view(f_id), f, "typed FunctionId<i32> round-trip");
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

    fn id_bounds<'ctx, B: ModuleBrand + 'ctx>(_m: &Module<'ctx, B, Unverified>) {
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
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let rendered = format!("{:?}", a.to_id());
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
