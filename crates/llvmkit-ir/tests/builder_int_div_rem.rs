//! Integer divide/remainder opcodes: `udiv`, `sdiv`, `urem`, `srem`,
//! plus the `exact` variants on `udiv`/`sdiv`.
//!
//! ## Upstream provenance
//!
//! Print-form fixtures locked from `test/Assembler/flags.ll`. The
//! `*_exact` cases additionally mirror the `exact` flag handling in
//! `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`
//! (which exercises `Builder.Create*` flag setters of the same shape).
//! The shared `module_for` helper above factors module setup.

use llvmkit_ir::{
    Dyn, IntValue, IrBuilder, IrError, Linkage, Module, SdivFlags, UdivFlags, module_new,
};

fn module_for(op: &str) -> Result<String, IrError> {
    let m = Module::dynamic("dr");
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()]);
    let f = m.add_function_dyn(op, fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i64, _> = m.view(f).param(0)?.try_into()?;
    let y: IntValue<'_, i64, _> = m.view(f).param(1)?.try_into()?;
    let r = match op {
        "udiv" => b.int_udiv(x, y, "z")?,
        "sdiv" => b.int_sdiv(x, y, "z")?,
        "urem" => b.int_urem(x, y, "z")?,
        "srem" => b.int_srem(x, y, "z")?,
        _ => unreachable!(),
    };
    b.ret(r)?;
    Ok(format!("{m}"))
}

/// Mirrors `test/Assembler/flags.ll` for `udiv` print form. Closest
/// upstream functional coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`.
#[test]
fn udiv_plain() -> Result<(), IrError> {
    let text = module_for("udiv")?;
    assert!(text.contains("%z = udiv i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for `sdiv` print form.
#[test]
fn sdiv_plain() -> Result<(), IrError> {
    let text = module_for("sdiv")?;
    assert!(text.contains("%z = sdiv i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for `urem` print form.
#[test]
fn urem_plain() -> Result<(), IrError> {
    let text = module_for("urem")?;
    assert!(text.contains("%z = urem i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for `srem` print form.
#[test]
fn srem_plain() -> Result<(), IrError> {
    let text = module_for("srem")?;
    assert!(text.contains("%z = srem i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for the `udiv exact` variant.
#[test]
fn udiv_exact() -> Result<(), IrError> {
    let m = module_new!("ex")?;
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()]);
    let f = m.add_function_dyn("udiv_exact", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i64, _> = m.view(f).param(0)?.try_into()?;
    let rhs: IntValue<'_, i64, _> = m.view(f).param(1)?.try_into()?;
    let r = b.int_udiv_with_flags(lhs, rhs, UdivFlags::new().exact(), "z")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = udiv exact i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for the `sdiv exact` variant.
#[test]
fn sdiv_exact() -> Result<(), IrError> {
    let m = module_new!("ex")?;
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()]);
    let f = m.add_function_dyn("sdiv_exact", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i64, _> = m.view(f).param(0)?.try_into()?;
    let rhs: IntValue<'_, i64, _> = m.view(f).param(1)?.try_into()?;
    let r = b.int_sdiv_with_flags(lhs, rhs, SdivFlags::new().exact(), "z")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = sdiv exact i64 %0, %1"), "got:\n{text}");
    Ok(())
}

// `urem` / `srem` accept no flags. There is no `URemFlags` /
// `SRemFlags` type, so the bug "exact on urem" is unspellable. The
// previous `exact_on_urem_rejected` runtime test is replaced by the
// type system itself; attempting `b.int_urem_with_flags(...)` is
// a method-not-found compile error.
