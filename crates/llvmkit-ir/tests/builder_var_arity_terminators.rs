//! Variable-arity terminator coverage: `switch`, `indirectbr`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module, PointerValue};

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
        // Seal the case targets with `unreachable` so the verifier accepts them.
        for bb in [default_bb, case0, case1, case2] {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
            bb_b.build_ret_void();
        }
        let val: IntValue<i8> = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, switch) = b.build_switch(val, default_bb, "")?;
        let _closed = switch
            .add_case(i8_ty.const_int(0_i8), case0)?
            .add_case(i8_ty.const_int(1_i8), case1)?
            .add_case(i8_ty.const_int(2_i8), case2)?
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
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(dest);
            bb_b.build_ret_void();
        }
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, switch) = b.build_switch(x, dest, "")?;
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
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(dest);
            bb_b.build_ret_void();
        }
        let addr: PointerValue = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, ibr) = b.build_indirectbr(addr, "")?;
        let _closed = ibr.add_destination(dest)?.finish();
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
        for bb in [bb1, bb2] {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
            bb_b.build_ret_void();
        }
        let addr: PointerValue = f.param(0)?.try_into()?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, ibr) = b.build_indirectbr(addr, "")?;
        let _closed = ibr.add_destination(bb1)?.add_destination(bb2)?.finish();
        let text = format!("{m}");
        assert!(
            text.contains("indirectbr ptr %0, [label %bb1, label %bb2]"),
            "got:\n{text}"
        );
        Ok(())
    })
}
