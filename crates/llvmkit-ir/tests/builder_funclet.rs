//! Funclet op coverage: `cleanuppad`, `cleanupret`, `catchpad`,
//! `catchret`, `catchswitch`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

// --------------------------------------------------------------------------
// catchswitch + catchpad
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1351:
/// `%cs1 = catchswitch within none [label %catchpad1] unwind to caller`.
/// Locks the `catchswitch within none` print form with one handler and
/// `unwind to caller`.
#[test]
fn catchswitch_within_none_unwind_to_caller() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("instructions.funclets", fn_ty, Linkage::External)?;
    let cs1_block = f.append_basic_block("catchswitch1");
    let cp1_block = f.append_basic_block("catchpad1");
    {
        // Stub a terminator on the handler so the block is well-formed.
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(cp1_block);
        bb_b.build_unreachable();
    }
    let b = IRBuilder::new_for::<()>(&m).position_at_end(cs1_block);
    let (_sealed, cs) = b.build_catch_switch::<llvmkit_ir::Unsealed>(None, None, "cs1")?;
    let _closed = cs.add_handler(cp1_block)?.finish();
    let text = format!("{m}");
    assert!(
        text.contains("%cs1 = catchswitch within none [label %catchpad1] unwind to caller"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 1354:
/// `catchpad within %cs1 []`. Locks the empty-args print form.
#[test]
fn catchpad_within_catchswitch_empty_args() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let cs_block = f.append_basic_block("cs");
    let cp_block = f.append_basic_block("cp");
    let exit = f.append_basic_block("exit");
    {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(exit);
        bb_b.build_ret_void();
    }
    let b_cs = IRBuilder::new_for::<()>(&m).position_at_end(cs_block);
    let (_sealed, cs) = b_cs.build_catch_switch::<llvmkit_ir::Unsealed>(None, None, "cs1")?;
    let cs_closed = cs.add_handler(cp_block)?.finish();
    let cs_value = cs_closed.as_instruction().as_value();
    let b_cp = IRBuilder::new_for::<()>(&m).position_at_end(cp_block);
    let _cp = b_cp.build_catch_pad(cs_value, Vec::<llvmkit_ir::value::Value>::new(), "")?;
    b_cp.build_unreachable();
    let text = format!("{m}");
    assert!(text.contains("catchpad within %cs1 []"), "got:\n{text}");
    Ok(())
}

// --------------------------------------------------------------------------
// cleanuppad + cleanupret
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1378:
/// `%clean.1 = cleanuppad within none []`.
#[test]
fn cleanuppad_within_none_empty_args() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let _ = b.build_cleanup_pad(None, Vec::<llvmkit_ir::value::Value>::new(), "clean.1")?;
    b.build_unreachable();
    let text = format!("{m}");
    assert!(
        text.contains("%clean.1 = cleanuppad within none []"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` line 1397:
/// `cleanupret from %clean unwind to caller`. Locks the print form
/// for the unwind-to-caller variant of `cleanupret`.
#[test]
fn cleanupret_unwind_to_caller() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let cp = b.build_cleanup_pad(None, Vec::<llvmkit_ir::value::Value>::new(), "clean")?;
    let _ =
        b.build_cleanup_ret::<llvmkit_ir::Unsealed>(cp.as_instruction().as_value(), None, "")?;
    let text = format!("{m}");
    assert!(
        text.contains("cleanupret from %clean unwind to caller"),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// catchret
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1412:
/// `catchret from %catch to label %return`. Uses a placeholder catchpad
/// since the upstream fixture builds one inside an
/// invoke-funclet-protected block.
#[test]
fn catchret_to_label() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let cs_block = f.append_basic_block("cs_block");
    let cp_block = f.append_basic_block("cp_block");
    let return_block = f.append_basic_block("return");
    {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(return_block);
        bb_b.build_ret_void();
    }
    let b_cs = IRBuilder::new_for::<()>(&m).position_at_end(cs_block);
    let (_sealed, cs) = b_cs.build_catch_switch::<llvmkit_ir::Unsealed>(None, None, "cs")?;
    let cs_closed = cs.add_handler(cp_block)?.finish();
    let cs_value = cs_closed.as_instruction().as_value();
    let b_cp = IRBuilder::new_for::<()>(&m).position_at_end(cp_block);
    let cp = b_cp.build_catch_pad(cs_value, Vec::<llvmkit_ir::value::Value>::new(), "catch")?;
    let _ = b_cp.build_catch_ret(cp.as_instruction().as_value(), return_block, "")?;
    let text = format!("{m}");
    assert!(
        text.contains("catchret from %catch to label %return"),
        "got:\n{text}"
    );
    Ok(())
}
