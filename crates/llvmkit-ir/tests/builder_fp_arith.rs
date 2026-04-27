//! Floating-point arithmetic: `fadd`, `fsub`, `fmul`, `fdiv`, `frem`.
//! Operand kind is statically pinned by the marker; mixing `f32` and
//! `f64` operands is a compile error.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` ports the corresponding `Builder.CreateF*` case from
//! `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`,
//! which exercises `CreateFAdd` / `CreateFSub` / `CreateFMul` /
//! `CreateFDiv` / `CreateFRem` against an `IRBuilder` and inspects the
//! resulting instruction. The fp arithmetic textual form is also
//! pinned by `test/Assembler/fast-math-flags.ll`. The shared
//! `build_f32_fn` helper above factors module setup.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

fn build_f32_fn(op: &str) -> Result<String, IrError> {
    let m = Module::new("fp");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type(), f32_ty.as_type()], false);
    let f = m.add_function::<f32>(op, fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    let x = f.param(0)?;
    let y = f.param(1)?;
    let r = match op {
        "fadd" => b.build_fp_add::<f32, _, _>(x, y, "z")?,
        "fsub" => b.build_fp_sub::<f32, _, _>(x, y, "z")?,
        "fmul" => b.build_fp_mul::<f32, _, _>(x, y, "z")?,
        "fdiv" => b.build_fp_div::<f32, _, _>(x, y, "z")?,
        "frem" => b.build_fp_rem::<f32, _, _>(x, y, "z")?,
        _ => unreachable!(),
    };
    b.build_ret(r)?;
    Ok(format!("{m}"))
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// for the `CreateFAdd` case at `float` width.
#[test]
fn fadd_f32() -> Result<(), IrError> {
    let text = build_f32_fn("fadd")?;
    assert!(text.contains("%z = fadd float %0, %1"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// for the `CreateFSub` case.
#[test]
fn fsub_f32() -> Result<(), IrError> {
    let text = build_f32_fn("fsub")?;
    assert!(text.contains("%z = fsub float %0, %1"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// for the `CreateFMul` case.
#[test]
fn fmul_f32() -> Result<(), IrError> {
    let text = build_f32_fn("fmul")?;
    assert!(text.contains("%z = fmul float %0, %1"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// for the `CreateFDiv` case.
#[test]
fn fdiv_f32() -> Result<(), IrError> {
    let text = build_f32_fn("fdiv")?;
    assert!(text.contains("%z = fdiv float %0, %1"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// for the `CreateFRem` case.
#[test]
fn frem_f32() -> Result<(), IrError> {
    let text = build_f32_fn("frem")?;
    assert!(text.contains("%z = frem float %0, %1"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// for `CreateFAdd` at `double` width — verifies the operand-type
/// marker propagates correctly to the `fadd double` print form.
#[test]
fn fadd_f64() -> Result<(), IrError> {
    let m = Module::new("fp");
    let f64_ty = m.f64_type();
    let fn_ty = m.fn_type(f64_ty, [f64_ty.as_type(), f64_ty.as_type()], false);
    let f = m.add_function::<f64>("fadd", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f64>(&m).position_at_end(entry);
    let r = b.build_fp_add::<f64, _, _>(f.param(0)?, f.param(1)?, "z")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%z = fadd double %0, %1"), "got:\n{text}");
    Ok(())
}
