//! EH-data coverage: `landingpad`, `resume`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

// --------------------------------------------------------------------------
// landingpad
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 789:
/// `%cleanup = landingpad i8 cleanup`. Locks the print form for the
/// `cleanup`-only landingpad (no clauses).
#[test]
fn landingpad_cleanup_only() -> Result<(), IrError> {
    let m = Module::new("a");
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f.no_personality", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let exception = f.append_basic_block("exception");
    {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        bb_b.build_ret_void();
    }
    let b = IRBuilder::new_for::<()>(&m).position_at_end(exception);
    let lp = b.build_landingpad(i8_ty.as_type(), true, "cleanup")?;
    let _closed = lp.finish();
    b.build_ret_void();
    let text = format!("{m}");
    // Mirrors `; CHECK: %cleanup = landingpad i8` followed by
    // `; CHECK: cleanup` (compatibility.ll lines 789 + the upstream
    // `printInstruction` LandingPadInst arm).
    assert!(
        text.contains("%cleanup = landingpad i8\n          cleanup"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` lines 1782-1786:
/// `landingpad i32\n             cleanup\n             catch ptr null`.
/// Locks the print form for a landingpad with a `catch` clause.
#[test]
fn landingpad_cleanup_plus_catch() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("instructions.landingpad", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let catch3 = f.append_basic_block("catch3");
    {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        bb_b.build_ret_void();
    }
    let b = IRBuilder::new_for::<()>(&m).position_at_end(catch3);
    let null_ptr = ptr_ty.const_null();
    let lp = b.build_landingpad(i32_ty.as_type(), true, "")?;
    let _closed = lp.add_catch_clause(null_ptr)?.finish();
    b.build_ret_void();
    let text = format!("{m}");
    // Mirrors `landingpad i32\n          cleanup\n          catch ptr null`.
    assert!(
        text.contains("landingpad i32\n          cleanup\n          catch ptr null"),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// resume
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1332:
/// `resume i32 undef`. Locks the print form for a resume with an
/// undef operand.
#[test]
fn resume_i32_undef() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let exc = f.append_basic_block("exc");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(exc);
    let undef = i32_ty.as_type().get_undef();
    let _ = b.build_resume(undef, "")?;
    let text = format!("{m}");
    assert!(text.contains("resume i32 undef"), "got:\n{text}");
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll` lines 1330-1332 — a landingpad
/// followed by `resume`. Verifies the `\n          ` continuation
/// indentation on the landingpad print does not break the following
/// instruction.
#[test]
fn landingpad_followed_by_resume() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let exc = f.append_basic_block("exc");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(exc);
    let lp = b.build_landingpad(i32_ty.as_type(), true, "cleanup")?;
    let _closed = lp.finish();
    let undef = i32_ty.as_type().get_undef();
    let _ = b.build_resume(undef, "")?;
    let text = format!("{m}");
    assert!(
        text.contains("%cleanup = landingpad i32\n          cleanup"),
        "got:\n{text}"
    );
    assert!(text.contains("resume i32 undef"), "got:\n{text}");
    Ok(())
}
