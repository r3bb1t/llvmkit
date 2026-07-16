//! Variable-arity terminator coverage: `switch`, `indirectbr`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{IRBuilder, IntValue, IrError, IsValue, Linkage, Module, PointerValue};

// --------------------------------------------------------------------------
// switch
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` lines 1302-1310:
/// `switch i8 %val, label %defaultdest [ i8 0, label %defaultdest.0
///   i8 1, label %defaultdest.1
///   i8 2, label %defaultdest.2 ]`. Locks the multi-line print form.
#[test]
fn switch_three_cases_print_form() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i8_ty = m.i8_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i8_ty.as_type()], false);
        let f = m.add_function::<(), _>("instructions.terminators", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let default_bb = f.append_basic_block(&m, "defaultdest");
        let case0 = f.append_basic_block(&m, "defaultdest.0");
        let case1 = f.append_basic_block(&m, "defaultdest.1");
        let case2 = f.append_basic_block(&m, "defaultdest.2");
        let default_label = default_bb.label();
        let case0_label = case0.label();
        let case1_label = case1.label();
        let case2_label = case2.label();
        // Seal the case targets with `unreachable` so the verifier accepts them.
        for bb in [default_bb, case0, case1, case2] {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
            bb_b.build_ret_void();
        }
        let val: IntValue<i8> = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, switch) = b.build_switch(val, default_label, "")?;
        let _closed = switch
            .add_case(i8_ty.const_int(0_i8), case0_label)?
            .add_case(i8_ty.const_int(1_i8), case1_label)?
            .add_case(i8_ty.const_int(2_i8), case2_label)?
            .finish();
        let text = format!("{m}");
        // Mirrors the upstream multi-line form (CHECK lines 1303-1310).
        assert!(
            text.contains("switch i8 %0, label %defaultdest ["),
            "got:\n{text}"
        );
        assert!(
            text.contains("    i8 0, label %defaultdest.0"),
            "got:\n{text}"
        );
        assert!(
            text.contains("    i8 1, label %defaultdest.1"),
            "got:\n{text}"
        );
        assert!(
            text.contains("    i8 2, label %defaultdest.2"),
            "got:\n{text}"
        );
        assert!(text.contains("\n  ]\n"), "got:\n{text}");
        Ok(())
    })
}

/// The `cases()` reader round-trips the `(case_value, target)` entries
/// added via `add_case`, in declaration order, on a finished switch.
#[test]
fn switch_cases_reader_round_trips() -> Result<(), IrError> {
    Module::with_new("switch_cases", |m| {
        let i8_ty = m.i8_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i8_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let default_bb = f.append_basic_block(&m, "default");
        let a = f.append_basic_block(&m, "a");
        let bb = f.append_basic_block(&m, "b");
        let default_label = default_bb.label();
        let a_label = a.label();
        let b_label = bb.label();
        for block in [default_bb, a, bb] {
            IRBuilder::new_for::<()>(&m)
                .position_at_end(block)
                .build_ret_void();
        }
        let val: IntValue<i8> = f.param(0)?.try_into()?;
        let builder = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, switch) = builder.build_switch(val, default_label, "")?;
        let closed = switch
            .add_case(i8_ty.const_int(10_i8), a_label)?
            .add_case(i8_ty.const_int(20_i8), b_label)?
            .finish();

        assert_eq!(closed.cases().len(), 2);
        let cases: Vec<_> = closed.cases().collect();
        // Case values round-trip, in order (constants are interned, so the
        // rediscovered value ids equal freshly-built ones).
        assert_eq!(cases[0].0.as_value(), i8_ty.const_int(10_i8).as_value());
        assert_eq!(cases[1].0.as_value(), i8_ty.const_int(20_i8).as_value());
        // Targets round-trip too.
        assert_eq!(cases[0].1.as_value(), a_label.as_value());
        assert_eq!(cases[1].1.as_value(), b_label.as_value());
        Ok(())
    })
}

/// Ports `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, SwitchInst)`
/// — a switch with no cases. Closest upstream functional coverage for an
/// empty `switch` is `test/Assembler/2003-05-15-SwitchBug.ll`:
/// `switch i32 %X, label %dest [\n        ]`. We assert the verifier
/// accepts a switch with only a default destination (no cases).
#[test]
fn switch_no_cases_only_default() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("test", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(dest);
            bb_b.build_ret_void();
        }
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, switch) = b.build_switch(x, dest_label, "")?;
        let _closed = switch.finish();
        m.verify_borrowed()?;
        let text = format!("{m}");
        assert!(
            text.contains("switch i32 %0, label %dest [\n  ]"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Typed `build_switch_typed`: the width `W` is inferred from the typed
/// `i32` condition, and matching-width `i32` cases (a Rust literal and a
/// `ConstantIntValue<i32>`) build, print the `switch i32 ...` form, and
/// verify. The wrong-width negation is the `switch_case_wrong_width`
/// compile-fail fixture (a bare `i64` case has no `IntoIntValue<i32>` impl).
#[test]
fn switch_typed_i32_matching_cases() -> Result<(), IrError> {
    Module::with_new("switch_typed", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let default_bb = f.append_basic_block(&m, "default");
        let a = f.append_basic_block(&m, "a");
        let bb = f.append_basic_block(&m, "b");
        let default_label = default_bb.label();
        let a_label = a.label();
        let b_label = bb.label();
        for block in [default_bb, a, bb] {
            IRBuilder::new_for::<()>(&m)
                .position_at_end(block)
                .build_ret_void();
        }
        let val: IntValue<i32> = f.param(0)?.try_into()?;
        let builder = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        // `W` is inferred as `i32` from `val: IntValue<i32>`.
        let (_sealed, switch) = builder.build_switch_typed(val, default_label, "")?;
        let _closed = switch
            // Rust `i32` literal lifts to the `i32`-width case slot.
            .add_case(10_i32, a_label)?
            // A same-width `ConstantIntValue<i32>` lifts too.
            .add_case(i32_ty.const_int(20_i32), b_label)?
            .finish();
        m.verify_borrowed()?;
        let text = format!("{m}");
        assert!(
            text.contains("switch i32 %0, label %default ["),
            "got:\n{text}"
        );
        assert!(text.contains("    i32 10, label %a"), "got:\n{text}");
        assert!(text.contains("    i32 20, label %b"), "got:\n{text}");
        Ok(())
    })
}

/// The width-erased `build_switch` is unchanged by `SwitchInst<W>`: it still
/// lands in `SwitchInst<IntDyn>` and its runtime-checked `add_case` still
/// rejects a wrong-width case value with the runtime [`IrError::TypeMismatch`]
/// the verifier would raise — a compile error is NOT forced on the erased
/// (parser / SSA-builder) path. Erased matching-width cases and their verify
/// are covered by `switch_three_cases_print_form` / `switch_no_cases_only_default`
/// above; this locks the erased flavour's runtime width check as intact
/// defence in depth beneath what the typed flavour lifts to compile time.
#[test]
fn switch_erased_dyn_wrong_width_case_is_runtime_type_mismatch() -> Result<(), IrError> {
    Module::with_new("switch_erased", |m| {
        let i32_ty = m.i32_type();
        let i8_ty = m.i8_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let default_bb = f.append_basic_block(&m, "default");
        let a = f.append_basic_block(&m, "a");
        let default_label = default_bb.label();
        let a_label = a.label();
        for block in [default_bb, a] {
            IRBuilder::new_for::<()>(&m)
                .position_at_end(block)
                .build_ret_void();
        }
        // Erased condition: `Argument` widens through the `IsValue` path, so
        // the resulting switch is `SwitchInst<IntDyn>` (compiles for any case
        // width; discipline is deferred to the runtime check below).
        let cond = f.param(0)?;
        let builder = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, switch) = builder.build_switch(cond, default_label, "")?;
        // A wrong-width (`i8`) case on the `i32` condition is a RUNTIME
        // `TypeMismatch`, not a compile error (`add_case` consumes `switch`).
        let err = switch
            .add_case(i8_ty.const_int(1_i8), a_label)
            .expect_err("i8 case on i32 switch must be rejected at runtime");
        assert!(matches!(err, IrError::TypeMismatch { .. }), "got: {err:?}");
        Ok(())
    })
}

// --------------------------------------------------------------------------
// indirectbr
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1320:
/// `indirectbr ptr blockaddress(@instructions.terminators, %defaultdest.2),
/// [label %defaultdest.2]`. Note: `blockaddress(...)` constants are not
/// yet supported (Session 2 territory). We exercise the print form using
/// a generic pointer parameter as the address operand; the upstream
/// fixture spells this `blockaddress(...)` because it's the only
/// canonical IR form. The byte-form check verifies the
/// `indirectbr <ptr-ty> <addr>, [label %bb]` skeleton.
#[test]
fn indirectbr_single_destination() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function::<(), _>("g", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(dest);
            bb_b.build_ret_void();
        }
        let addr: PointerValue = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, ibr) = b.build_indirectbr(addr, "")?;
        let _closed = ibr.add_destination(dest_label)?.finish();
        let text = format!("{m}");
        assert!(
            text.contains("indirectbr ptr %0, [label %dest]"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Ports `test/Bitcode/compatibility.ll` line 1322:
/// `indirectbr ptr blockaddress(...), [label %defaultdest.2, label %defaultdest.2]`.
/// Locks the comma-separated multi-destination print form. The duplicated
/// destination is intentional in the upstream fixture (a valid IR form).
#[test]
fn indirectbr_multiple_destinations() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function::<(), _>("g", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let bb1 = f.append_basic_block(&m, "bb1");
        let bb2 = f.append_basic_block(&m, "bb2");
        let bb1_label = bb1.label();
        let bb2_label = bb2.label();
        for bb in [bb1, bb2] {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
            bb_b.build_ret_void();
        }
        let addr: PointerValue = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, ibr) = b.build_indirectbr(addr, "")?;
        let _closed = ibr
            .add_destination(bb1_label)?
            .add_destination(bb2_label)?
            .finish();
        let text = format!("{m}");
        assert!(
            text.contains("indirectbr ptr %0, [label %bb1, label %bb2]"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// OP Slice 2: `build_indirectbr` binds the address by `IntoPointerValue`, so
/// a typed [`PointerValue`] address is accepted directly (identity impl). It
/// builds, prints the `indirectbr ptr ...` skeleton, and `verify()` passes.
#[test]
fn indirectbr_typed_pointer_address_builds_and_verifies() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function::<(), _>("g", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(dest);
            bb_b.build_ret_void();
        }
        // A typed pointer handle: accepted by the identity `IntoPointerValue`.
        let addr: PointerValue = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, ibr) = b.build_indirectbr(addr, "")?;
        let _closed = ibr.add_destination(dest_label)?.finish();
        let text = format!("{m}");
        assert!(
            text.contains("indirectbr ptr %0, [label %dest]"),
            "got:\n{text}"
        );
        m.verify()?;
        Ok(())
    })
}

/// OP Slice 2: an *erased* [`Value`](llvmkit_ir::Value) pointer address (the
/// form the parser feeds `build_indirectbr`) still builds and verifies — the
/// runtime-checked `IntoPointerValue for Value` impl narrows it back to a
/// pointer at *build* time. Proves the tighter bound did not break the erased
/// path.
#[test]
fn indirectbr_erased_value_pointer_address_builds_and_verifies() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function::<(), _>("g", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(dest);
            bb_b.build_ret_void();
        }
        // Erase the pointer param to a bare `Value` before passing it — the
        // runtime-checked `IntoPointerValue for Value` impl narrows it back.
        let addr = f.param(0)?.as_value();
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, ibr) = b.build_indirectbr(addr, "")?;
        let _closed = ibr.add_destination(dest_label)?.finish();
        let text = format!("{m}");
        assert!(
            text.contains("indirectbr ptr %0, [label %dest]"),
            "got:\n{text}"
        );
        m.verify()?;
        Ok(())
    })
}
