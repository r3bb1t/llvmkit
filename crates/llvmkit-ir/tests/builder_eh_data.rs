//! EH-data coverage: `landingpad`, `resume`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{Dyn, IrBuilder, IrError, Linkage, module_new};

// --------------------------------------------------------------------------
// landingpad
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll::@f.no_personality` block
/// `exception:` — `%cleanup = landingpad i8 cleanup`. Locks the print form for the
/// `cleanup`-only landingpad (no clauses).
#[test]
fn landingpad_cleanup_only() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let fn_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("f.no_personality", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let exception = m.view(f).append_basic_block(&m, "exception");
    {
        let bb_b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        bb_b.ret_void()?;
    }
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(exception);
    let lp = b.landingpad(i8_ty.as_type(), true, "cleanup")?;
    let _closed = lp.finish();
    b.ret_void()?;
    let text = format!("{m}");
    // Mirrors `; CHECK: %cleanup = landingpad i8` followed by
    // `; CHECK: cleanup` (the instruction text of `@f.no_personality`'s
    // `exception:` block, laid out by the upstream `printInstruction`
    // LandingPadInst arm).
    assert!(
        text.contains("%cleanup = landingpad i8\n          cleanup"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll::@instructions.landingpad` block
/// `catch2:` —
/// `landingpad i32\n             cleanup\n             catch ptr null`.
/// Locks the print form for a landingpad with a `catch` clause. (`catch3:`
/// is the two-`catch` block and is *not* what this builds.)
#[test]
fn landingpad_cleanup_plus_catch() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("instructions.landingpad", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let catch2 = m.view(f).append_basic_block(&m, "catch2");
    {
        let bb_b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        bb_b.ret_void()?;
    }
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(catch2);
    let null_ptr = ptr_ty.const_null();
    let lp = b.landingpad(i32_ty.as_type(), true, "")?;
    let _closed = lp.add_catch_clause(null_ptr)?.finish();
    b.ret_void()?;
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

/// Ports `test/Bitcode/compatibility.ll::@instructions.terminators` block
/// `exc:` — `resume i32 undef`. Locks the print form for a resume with an
/// undef operand.
#[test]
fn resume_i32_undef() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let exc = m.view(f).append_basic_block(&m, "exc");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(exc);
    let undef = i32_ty.as_type().undef();
    let _ = b.resume(undef, "")?;
    let text = format!("{m}");
    assert!(text.contains("resume i32 undef"), "got:\n{text}");
    Ok(())
}

/// Ports `test/Bitcode/compatibility.ll::@instructions.terminators` block
/// `exc:` — `%cleanup = landingpad i32 cleanup` followed by
/// `resume i32 undef`. Verifies the `\n          ` continuation
/// indentation on the landingpad print does not break the following
/// instruction.
#[test]
fn landingpad_followed_by_resume() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let exc = m.view(f).append_basic_block(&m, "exc");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(exc);
    let lp = b.landingpad(i32_ty.as_type(), true, "cleanup")?;
    let _closed = lp.finish();
    let undef = i32_ty.as_type().undef();
    let _ = b.resume(undef, "")?;
    let text = format!("{m}");
    assert!(
        text.contains("%cleanup = landingpad i32\n          cleanup"),
        "got:\n{text}"
    );
    assert!(text.contains("resume i32 undef"), "got:\n{text}");
    Ok(())
}
