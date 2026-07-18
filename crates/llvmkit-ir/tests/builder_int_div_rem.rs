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

use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module, SDivFlags, UDivFlags};

fn module_for(op: &str) -> Result<String, IrError> {
    Module::with_new("dr", |m| {
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
        let f = m.add_function::<i64, _>(op, fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
        let x: IntValue<i64> = f.param(0)?.try_into()?;
        let y: IntValue<i64> = f.param(1)?.try_into()?;
        let r = match op {
            "udiv" => b.build_int_udiv(x, y, "z")?,
            "sdiv" => b.build_int_sdiv(x, y, "z")?,
            "urem" => b.build_int_urem(x, y, "z")?,
            "srem" => b.build_int_srem(x, y, "z")?,
            _ => unreachable!(),
        };
        b.build_ret(r)?;
        Ok(format!("{m}"))
    })
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
    Module::with_new("ex", |m| {
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
        let f = m.add_function::<i64, _>("udiv_exact", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
        let lhs: IntValue<i64> = f.param(0)?.try_into()?;
        let rhs: IntValue<i64> = f.param(1)?.try_into()?;
        let r = b.build_int_udiv_with_flags(lhs, rhs, UDivFlags::new().exact(), "z")?;
        b.build_ret(r)?;
        let text = format!("{m}");
        assert!(text.contains("%z = udiv exact i64 %0, %1"), "got:\n{text}");
        Ok(())
    })
}

/// Mirrors `test/Assembler/flags.ll` for the `sdiv exact` variant.
#[test]
fn sdiv_exact() -> Result<(), IrError> {
    Module::with_new("ex", |m| {
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
        let f = m.add_function::<i64, _>("sdiv_exact", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
        let lhs: IntValue<i64> = f.param(0)?.try_into()?;
        let rhs: IntValue<i64> = f.param(1)?.try_into()?;
        let r = b.build_int_sdiv_with_flags(lhs, rhs, SDivFlags::new().exact(), "z")?;
        b.build_ret(r)?;
        let text = format!("{m}");
        assert!(text.contains("%z = sdiv exact i64 %0, %1"), "got:\n{text}");
        Ok(())
    })
}

// `urem` / `srem` accept no flags. There is no `URemFlags` /
// `SRemFlags` type, so the bug "exact on urem" is unspellable. The
// previous `exact_on_urem_rejected` runtime test is replaced by the
// type system itself; attempting `b.build_int_urem_with_flags(...)` is
// a method-not-found compile error.
