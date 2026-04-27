//! Integer shift opcodes: `shl`, `lshr`, `ashr` plus the `nuw`/`nsw`
//! variants on `shl` and the `exact` variant on `lshr`/`ashr`.
//!
//! ## Upstream provenance
//!
//! Print-form fixtures locked from `test/Assembler/flags.ll`. The
//! flagged variants additionally mirror the `Builder.CreateShl(..., NUW, NSW)`
//! path of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`.

use llvmkit_ir::{AShrFlags, IRBuilder, IrError, LShrFlags, Linkage, Module, ShlFlags};

/// Mirrors `test/Assembler/flags.ll` for `shl` print form.
#[test]
fn shl_plain() -> Result<(), IrError> {
    let m = Module::new("shifts");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>("shl_plain", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let r = b.build_int_shl::<i64, _, _>(f.param(0)?, f.param(1)?, "z")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = shl i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for `lshr` print form.
#[test]
fn lshr_plain() -> Result<(), IrError> {
    let m = Module::new("shifts");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>("lshr_plain", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let r = b.build_int_lshr::<i64, _, _>(f.param(0)?, f.param(1)?, "z")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = lshr i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for `ashr` print form.
#[test]
fn ashr_plain() -> Result<(), IrError> {
    let m = Module::new("shifts");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>("ashr_plain", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let r = b.build_int_ashr::<i64, _, _>(f.param(0)?, f.param(1)?, "z")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = ashr i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`
/// for the `Builder.CreateShl(..., NUW=true, NSW=true)` case.
#[test]
fn shl_nuw_nsw() -> Result<(), IrError> {
    let m = Module::new("shifts");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>("shl_both", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let r = b.build_int_shl_with_flags::<i64, _, _>(
        f.param(0)?,
        f.param(1)?,
        ShlFlags::new().nuw().nsw(),
        "z",
    )?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = shl nuw nsw i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for the `lshr exact` variant.
#[test]
fn lshr_exact() -> Result<(), IrError> {
    let m = Module::new("shifts");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>("lshr_exact", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let r = b.build_int_lshr_with_flags::<i64, _, _>(
        f.param(0)?,
        f.param(1)?,
        LShrFlags::new().exact(),
        "z",
    )?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = lshr exact i64 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for the `ashr exact` variant.
#[test]
fn ashr_exact() -> Result<(), IrError> {
    let m = Module::new("shifts");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i64>("ashr_exact", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let r = b.build_int_ashr_with_flags::<i64, _, _>(
        f.param(0)?,
        f.param(1)?,
        AShrFlags::new().exact(),
        "z",
    )?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = ashr exact i64 %0, %1"), "got:\n{text}");
    Ok(())
}

// `add` / `sub` / `mul` / `shl` accept only `nuw`/`nsw`; they have no
// `.exact()` setter on their flag types. `ashr`/`lshr`/`udiv`/`sdiv`
// accept only `.exact()`; their flag types have no `.nuw()`/`.nsw()`
// setter. The previous `exact_on_add_rejected` runtime test is replaced
// by the type system: `AddFlags::new().exact()` is a method-not-found
// compile error.
