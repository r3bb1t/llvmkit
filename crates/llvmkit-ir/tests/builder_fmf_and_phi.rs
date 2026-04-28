//! Phase-A3 coverage: builder-context FMF, per-predicate fcmp wrappers,
//! and non-int phi handles (`FpPhiInst`, `PointerPhiInst`).
//!
//! Each `#[test]` cites its upstream source (Doctrine D11). The FMF
//! tests port `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
//! FastMathFlags)`. The fcmp predicate tests mirror the
//! `test/Bitcode/compatibility.ll` fcmp fixture lines 1677-1706. The
//! non-int phi tests adapt `unittests/IR/InstructionsTest.cpp` phi
//! coverage to the typed-marker API.

use llvmkit_ir::{
    FastMathFlags, FloatValue, IRBuilder, IntValue, IrError, Linkage, Module, PointerValue,
};

// --- Builder-context FMF -----------------------------------------------

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
/// FastMathFlags)` (line 557). The upstream test sets all flags via
/// `FMF.setFast(); Builder.setFastMathFlags(FMF);` then expects the next
/// `CreateFAdd` to carry every FMF bit. We mirror the construct with the
/// `with_fast_math_flags` builder method (consume-self port of the
/// upstream void mutator).
#[test]
fn fmf_propagates_from_builder_to_fadd() -> Result<(), IrError> {
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<f32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m)
        .position_at_end(entry)
        .with_fast_math_flags(FastMathFlags::fast());
    let p: FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_fp_add(p, p, "r")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%r = fadd fast float %0, %0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// (line 622-628): after `Builder.clearFastMathFlags()`, the next op
/// carries no FMF.
#[test]
fn clear_fast_math_flags_drops_flags_from_subsequent_ops() -> Result<(), IrError> {
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<f32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m)
        .position_at_end(entry)
        .with_fast_math_flags(FastMathFlags::fast())
        .clear_fast_math_flags();
    let p: FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_fp_div(p, p, "r")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = fdiv float %0, %0\n"), "got:\n{text}");
    assert!(!text.contains("fast"), "no FMF expected; got:\n{text}");
    Ok(())
}

/// Mirrors `IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// (line 643-647): `Builder.CreateFCmpOEQ(F, F)` carries the builder's
/// FMF (an `FPMathOperator` upstream).
#[test]
fn fmf_propagates_to_fcmp_oeq() -> Result<(), IrError> {
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let i1_ty = m.bool_type();
    let fn_ty = m.fn_type(i1_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<bool>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    // Match upstream's `FMF.setAllowReciprocal()` exactly (line 650).
    let fmf = FastMathFlags::ALLOW_RECIPROCAL;
    let b = IRBuilder::new_for::<bool>(&m)
        .position_at_end(entry)
        .with_fast_math_flags(fmf);
    let p: FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_fcmp_oeq::<f32, _, _>(p, p, "r")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%r = fcmp arcp oeq float %0, %0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, RAIIHelpersTest)`
/// (lines 823-856), specifically the `FastMathFlagGuard` arm: snapshot FMF,
/// scope changes to the builder's FMF, exit restores the original. Our
/// consume-self builders provide the same observable round-trip via
/// `fast_math_flags()` (snapshot) + `with_fast_math_flags(orig)` (restore).
#[test]
fn fmf_save_and_restore_round_trip() -> Result<(), IrError> {
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<f32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    // Original: empty FMF.
    assert!(b.fast_math_flags().is_empty());
    // Snapshot, change to AllowReciprocal, build an op, restore.
    let orig = b.fast_math_flags();
    let b = b.with_fast_math_flags(FastMathFlags::ALLOW_RECIPROCAL);
    let p: FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_fp_add(p, p, "r")?;
    assert!(
        b.fast_math_flags()
            .contains(FastMathFlags::ALLOW_RECIPROCAL)
    );
    let b = b.with_fast_math_flags(orig);
    assert!(b.fast_math_flags().is_empty());
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%r = fadd arcp float %0, %0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, UnaryOperators)`
/// (line 535-555): `Builder.CreateUnOp(Instruction::FNeg, V)` followed by
/// `Builder.CreateFNegFMF(V, I)` where `I` carries `nnan` + `nsz`. We mirror
/// both shapes via `build_float_neg` / `build_float_neg_with_flags`.
#[test]
fn fneg_emits_default_then_fmf_form() -> Result<(), IrError> {
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<f32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    let p: FloatValue<f32> = f.param(0)?.try_into()?;
    // Default: no FMF.
    let n0 = b.build_float_neg::<f32, _>(p, "n0")?;
    // With FMF: nnan + nsz (mirrors upstream `setHasNoNaNs(true);
    // setHasNoSignedZeros(true)`, lines 548-549).
    let fmf = FastMathFlags::NO_NANS | FastMathFlags::NO_SIGNED_ZEROS;
    let n1 = b.build_float_neg_with_flags::<f32, _>(n0, fmf, "n1")?;
    b.build_ret(n1)?;
    let text = format!("{m}");
    assert!(
        text.contains("%n0 = fneg float %0\n"),
        "default fneg missing; got:\n{text}"
    );
    assert!(
        text.contains("%n1 = fneg nnan nsz float %n0\n"),
        "fneg-fmf missing; got:\n{text}"
    );
    Ok(())
}

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
/// FastMathFlags)` (lines 663-697): the AllowContract / ApproxFunc /
/// AllowReassoc propagation arm. Builder-context FMF accumulates these
/// flags and they appear on the resulting fmul.
#[test]
fn fmf_accumulates_contract_approx_reassoc_on_fmul() -> Result<(), IrError> {
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<f32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let fmf =
        FastMathFlags::APPROX_FUNC | FastMathFlags::ALLOW_CONTRACT | FastMathFlags::ALLOW_REASSOC;
    let b = IRBuilder::new_for::<f32>(&m)
        .position_at_end(entry)
        .with_fast_math_flags(fmf);
    let p: FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_fp_mul(p, p, "r")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    // Mirrors `lib/IR/AsmWriter.cpp::FastMathFlags::print` order:
    // reassoc, nnan, ninf, nsz, arcp, contract, afn.
    assert!(
        text.contains("%r = fmul reassoc contract afn float %0, %0\n"),
        "got:\n{text}"
    );
    Ok(())
}

// --- Per-predicate fcmp -------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 1677:
/// `fcmp oeq half %fop1, %fop2`. We use `f32` (`float`) here because the
/// project ships f32/f64 as primary kinds; the predicate routing is
/// agnostic to the float kind.
#[test]
fn build_fcmp_oeq_emits_oeq() -> Result<(), IrError> {
    fcmp_predicate_emits("oeq", |b, lhs, rhs| {
        b.build_fcmp_oeq::<f32, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1679: `fcmp ogt half %fop1, %fop2`.
#[test]
fn build_fcmp_ogt_emits_ogt() -> Result<(), IrError> {
    fcmp_predicate_emits("ogt", |b, lhs, rhs| {
        b.build_fcmp_ogt::<f32, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1681: `fcmp oge half %fop1, %fop2`.
#[test]
fn build_fcmp_oge_emits_oge() -> Result<(), IrError> {
    fcmp_predicate_emits("oge", |b, lhs, rhs| {
        b.build_fcmp_oge::<f32, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1683: `fcmp olt half %fop1, %fop2`.
#[test]
fn build_fcmp_olt_emits_olt() -> Result<(), IrError> {
    fcmp_predicate_emits("olt", |b, lhs, rhs| {
        b.build_fcmp_olt::<f32, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1685: `fcmp ole half %fop1, %fop2`.
#[test]
fn build_fcmp_ole_emits_ole() -> Result<(), IrError> {
    fcmp_predicate_emits("ole", |b, lhs, rhs| {
        b.build_fcmp_ole::<f32, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1689: `fcmp ord half %fop1, %fop2`.
#[test]
fn build_fcmp_ord_emits_ord() -> Result<(), IrError> {
    fcmp_predicate_emits("ord", |b, lhs, rhs| {
        b.build_fcmp_ord::<f32, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1703: `fcmp uno half %fop1, %fop2`.
#[test]
fn build_fcmp_uno_emits_uno() -> Result<(), IrError> {
    fcmp_predicate_emits("uno", |b, lhs, rhs| {
        b.build_fcmp_uno::<f32, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1691: `fcmp ueq half %fop1, %fop2`.
#[test]
fn build_fcmp_ueq_emits_ueq() -> Result<(), IrError> {
    fcmp_predicate_emits("ueq", |b, lhs, rhs| {
        b.build_fcmp_ueq::<f32, _, _>(lhs, rhs, "r")
    })
}

// Helper: build a tiny module exercising the fcmp wrapper, assert that
// the AsmWriter emits the expected predicate keyword.
fn fcmp_predicate_emits<F>(expected_pred: &str, mk: F) -> Result<(), IrError>
where
    F: for<'ctx> FnOnce(
        &IRBuilder<
            'ctx,
            llvmkit_ir::ir_builder::constant_folder::ConstantFolder,
            llvmkit_ir::ir_builder::Positioned,
            bool,
        >,
        FloatValue<'ctx, f32>,
        FloatValue<'ctx, f32>,
    ) -> Result<IntValue<'ctx, bool>, IrError>,
{
    let m = Module::new("a");
    let f32_ty = m.f32_type();
    let i1_ty = m.bool_type();
    let fn_ty = m.fn_type(i1_ty, [f32_ty.as_type(), f32_ty.as_type()], false);
    let f = m.add_function::<bool>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
    let lhs: FloatValue<f32> = f.param(0)?.try_into()?;
    let rhs: FloatValue<f32> = f.param(1)?.try_into()?;
    let r = mk(&b, lhs, rhs)?;
    b.build_ret(r)?;
    let text = format!("{m}");
    let needle = format!("%r = fcmp {expected_pred} float %0, %1\n");
    assert!(text.contains(&needle), "expected `{needle}`; got:\n{text}");
    Ok(())
}

// --- Non-int phi handles ----------------------------------------------

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
/// FPMathOperator)` (line 539), which exercises
/// `Builder.CreatePHI(Builder.getDoubleTy(), 0)` -- a `double`-typed phi
/// that subsequently feeds an `FPMathOperator`. We mirror the same
/// `phi double` shape through the typed `build_fp_phi::<f64>` wrapper.
#[test]
fn build_fp_phi_emits_phi_with_double_kind() -> Result<(), IrError> {
    let m = Module::new("a");
    let f64_ty = m.f64_type();
    let fn_ty = m.fn_type(f64_ty, [f64_ty.as_type()], false);
    let f = m.add_function::<f64>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let join = f.append_basic_block("join");
    let b = IRBuilder::new_for::<f64>(&m).position_at_end(entry);
    let p: FloatValue<f64> = f.param(0)?.try_into()?;
    let (entry_sealed, _br) = b.build_br(join)?;
    let _ = entry_sealed;
    let b2 = IRBuilder::new_for::<f64>(&m).position_at_end(join);
    let phi = b2
        .build_fp_phi::<f64>("merge")?
        .add_incoming(p, entry)?
        .finish();
    b2.build_ret(phi.as_float_value())?;
    let text = format!("{m}");
    assert!(
        text.contains("%merge = phi double [ %0, %entry ]\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Verifier/inalloca2.ll` line 35:
/// `%args = phi ptr [ %a, %if ], [ %b, %else ]` -- the canonical
/// upstream IR fixture for a pointer phi. Adapted to the typed
/// `build_pointer_phi` wrapper. llvmkit-specific scaffold (no
/// dedicated `unittests/IR/*Test.cpp::TEST` for pointer phi).
#[test]
fn build_pointer_phi_emits_phi_with_ptr() -> Result<(), IrError> {
    let m = Module::new("a");
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(ptr_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<llvmkit_ir::marker::Ptr>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let join = f.append_basic_block("join");
    let b = IRBuilder::new_for::<llvmkit_ir::marker::Ptr>(&m).position_at_end(entry);
    let p: PointerValue = f.param(0)?.try_into()?;
    let (entry_sealed, _br) = b.build_br(join)?;
    let _ = entry_sealed;
    let b2 = IRBuilder::new_for::<llvmkit_ir::marker::Ptr>(&m).position_at_end(join);
    let phi = b2
        .build_pointer_phi("merge")?
        .add_incoming(p, entry)?
        .finish();
    b2.build_ret(phi.as_pointer_value())?;
    let text = format!("{m}");
    assert!(
        text.contains("%merge = phi ptr [ %0, %entry ]\n"),
        "got:\n{text}"
    );
    Ok(())
}
