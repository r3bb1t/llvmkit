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

use llvmkit_ir::{
    Constant, ConstantFloatValue, Dyn, FloatValue, IRBuilder, IrError, Linkage, Module,
};

fn build_f32_fn(op: &str) -> Result<String, IrError> {
    Module::with_new("fp", |m| {
        let f32_ty = m.f32_type();
        let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type(), f32_ty.as_type()], false);
        let f = m.add_function_dyn(op, fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: FloatValue<f32> = f.param(0)?.try_into()?;
        let y: FloatValue<f32> = f.param(1)?.try_into()?;
        let r = match op {
            "fadd" => b.build_fp_add(x, y, "z")?,
            "fsub" => b.build_fp_sub(x, y, "z")?,
            "fmul" => b.build_fp_mul(x, y, "z")?,
            "fdiv" => b.build_fp_div(x, y, "z")?,
            "frem" => b.build_fp_rem(x, y, "z")?,
            _ => unreachable!(),
        };
        b.build_ret(r)?;
        Ok(format!("{m}"))
    })
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
    Module::with_new("fp", |m| {
        let f64_ty = m.f64_type();
        let fn_ty = m.fn_type(f64_ty, [f64_ty.as_type(), f64_ty.as_type()], false);
        let f = m.add_function_dyn("fadd", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let lhs: FloatValue<f64> = f.param(0)?.try_into()?;
        let rhs: FloatValue<f64> = f.param(1)?.try_into()?;
        let r = b.build_fp_add(lhs, rhs, "z")?;
        b.build_ret(r)?;
        let text = format!("{m}");
        assert!(text.contains("%z = fadd double %0, %1"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific APFloat regression for
/// `ConstantFold.cpp::ConstantFoldBinaryInstruction`'s floating `fadd` path:
/// the default builder folder delegates FP binops to the shared APFloat folder.
#[test]
fn default_constant_folder_folds_fadd_to_constant() -> Result<(), IrError> {
    Module::with_new("fp-fold", |m| {
        let ty = m.f64_type();
        let fn_ty = m.fn_type(ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("sum", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let result =
            b.build_fp_add::<f64, _, _, _>(ty.const_double(1.5), ty.const_double(2.25), "sum")?;
        let folded =
            ConstantFloatValue::<f64>::try_from(Constant::try_from(result.into_erased())?)?;
        assert!(folded.ap_float().is_exactly_value_f64(3.75));
        Ok(())
    })
}
