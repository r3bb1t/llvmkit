//! Floating-point compare: `fcmp <pred>` for the standard predicates.
//! Result type is always `i1`.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` exercises one `FcmpInst` predicate. The closest
//! upstream functional coverage is
//! `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`,
//! which uses `Builder.CreateFCmpOEQ` to build an `FcmpInst`. The
//! per-predicate textual print form is locked by
//! `test/Assembler/fast-math-flags.ll` (and the LangRef).

use llvmkit_ir::{
    Constant, ConstantIntValue, Dyn, FloatPredicate, FloatValue, IrBuilder, IrError, Linkage,
    Module, module_new,
};

fn module_with_pred(pred: FloatPredicate, name: &str) -> Result<String, IrError> {
    let m = Module::dynamic("fcmp");
    let f64_ty = m.f64_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(bool_ty, [f64_ty.as_type(), f64_ty.as_type()], false);
    let f = m.add_function_dyn(name, fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: FloatValue<'_, f64, _> = m.view(f).param(0)?.try_into()?;
    let rhs: FloatValue<'_, f64, _> = m.view(f).param(1)?.try_into()?;
    let r = b.build_fp_cmp(pred, lhs, rhs, "r")?;
    b.build_ret(r)?;
    Ok(format!("{m}"))
}

/// llvmkit-specific: AsmWriter parity check for `fcmp oeq`. Closest
/// upstream functional coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// (uses `CreateFCmpOEQ`).
#[test]
fn fcmp_oeq() -> Result<(), IrError> {
    let text = module_with_pred(FloatPredicate::Oeq, "eq_d")?;
    assert!(text.contains("%r = fcmp oeq double %0, %1"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: AsmWriter parity check for `fcmp ogt`. Closest
/// upstream functional coverage: same `IRBuilderTest::FastMathFlags`
/// `FCmp` path with a different predicate.
#[test]
fn fcmp_ogt() -> Result<(), IrError> {
    let text = module_with_pred(FloatPredicate::Ogt, "gt_d")?;
    assert!(text.contains("%r = fcmp ogt double %0, %1"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: AsmWriter parity check for `fcmp olt`. See the
/// `IRBuilderTest::FastMathFlags` `FCmp` path.
#[test]
fn fcmp_olt() -> Result<(), IrError> {
    let text = module_with_pred(FloatPredicate::Olt, "lt_d")?;
    assert!(text.contains("%r = fcmp olt double %0, %1"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: AsmWriter parity check for `fcmp ord`. See the
/// `IRBuilderTest::FastMathFlags` `FCmp` path.
#[test]
fn fcmp_ord() -> Result<(), IrError> {
    let text = module_with_pred(FloatPredicate::Ord, "ord_d")?;
    assert!(text.contains("%r = fcmp ord double %0, %1"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: AsmWriter parity check for `fcmp une`. See the
/// `IRBuilderTest::FastMathFlags` `FCmp` path.
#[test]
fn fcmp_une() -> Result<(), IrError> {
    let text = module_with_pred(FloatPredicate::Une, "ne_d")?;
    assert!(text.contains("%r = fcmp une double %0, %1"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: AsmWriter parity check for `fcmp uno`. See the
/// `IRBuilderTest::FastMathFlags` `FCmp` path.
#[test]
fn fcmp_uno() -> Result<(), IrError> {
    let text = module_with_pred(FloatPredicate::Uno, "uno_d")?;
    assert!(text.contains("%r = fcmp uno double %0, %1"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific regression for
/// `ConstantFold.cpp::ConstantFoldCompareInstruction`: the default builder
/// folder must fold all-constant floating compares to an `i1` constant.
#[test]
fn default_constant_folder_folds_float_compare() -> Result<(), IrError> {
    let m = module_new!("fcmp-fold")?;
    let f64_ty = m.f64_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(bool_ty, Vec::<llvmkit_ir::Type<'_, _>>::new(), false);
    let f = m.add_function_dyn("cmp", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let result = b.build_fp_cmp::<f64, _, _, _>(
        FloatPredicate::Olt,
        f64_ty.const_double(1.0),
        f64_ty.const_double(2.0),
        "is_lt",
    )?;
    let folded =
        ConstantIntValue::<bool, _>::try_from(Constant::try_from(b.view(result).into_erased())?)?;
    assert_eq!(folded.ap_int().try_zext_u64(), Some(1));
    Ok(())
}
