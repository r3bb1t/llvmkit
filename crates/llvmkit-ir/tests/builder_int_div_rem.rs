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

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module, SDivFlags, UDivFlags};

fn module_for(op: &str) -> Result<String, IrError> {
    let m = Module::new("dr");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>(op, fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let x = f.param(0)?;
    let y = f.param(1)?;
    let r = match op {
        "udiv" => b.build_int_udiv::<i64, _, _>(x, y, "z")?,
        "sdiv" => b.build_int_sdiv::<i64, _, _>(x, y, "z")?,
        "urem" => b.build_int_urem::<i64, _, _>(x, y, "z")?,
        "srem" => b.build_int_srem::<i64, _, _>(x, y, "z")?,
        _ => unreachable!(),
    };
    b.build_ret(r)?;
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
    let m = Module::new("ex");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>("udiv_exact", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let r = b.build_int_udiv_with_flags::<i64, _, _>(
        f.param(0)?,
        f.param(1)?,
        UDivFlags::new().exact(),
        "z",
    )?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = udiv exact i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for the `sdiv exact` variant.
#[test]
fn sdiv_exact() -> Result<(), IrError> {
    let m = Module::new("ex");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>("sdiv_exact", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let r = b.build_int_sdiv_with_flags::<i64, _, _>(
        f.param(0)?,
        f.param(1)?,
        SDivFlags::new().exact(),
        "z",
    )?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = sdiv exact i64 %0, %1"), "got:\n{text}");
    Ok(())
}

// `urem` / `srem` accept no flags. There is no `URemFlags` /
// `SRemFlags` type, so the bug "exact on urem" is unspellable. The
// previous `exact_on_urem_rejected` runtime test is replaced by the
// type system itself; attempting `b.build_int_urem_with_flags(...)` is
// a method-not-found compile error.
