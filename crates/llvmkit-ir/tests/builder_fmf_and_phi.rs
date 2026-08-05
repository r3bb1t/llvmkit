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
    Dyn, FastMathFlags, FloatValue, InstructionKind, InstructionView, IrBuilder, IrError, Linkage,
    PointerValue, module_new,
};

// --- Builder-context FMF -----------------------------------------------

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
/// FastMathFlags)` (lines 596-620). Upstream sets every builder FMF bit,
/// then checks the next `CreateFAdd` / `CreateFDiv` carry the same `fast`
/// state. llvmkit observes the instruction FMF through exact AsmWriter
/// spelling because FP binop handles currently return typed values.
#[test]
fn fmf_propagates_from_builder_to_fadd() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .with_fast_math_flags(FastMathFlags::fast());
    assert_eq!(b.fast_math_flags(), FastMathFlags::fast());
    assert!(b.fast_math_flags().is_fast());
    let p: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
    let add = b.fp_add(p, p, "add")?;
    let div = b.fp_div(add, add, "div")?;
    b.ret(div)?;
    let text = format!("{m}");
    assert!(
        text.contains("%add = fadd fast float %0, %0\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%div = fdiv fast float %add, %add\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// (lines 622-628): after `Builder.clearFastMathFlags()`, the next
/// `CreateFDiv` carries no `AllowReciprocal` bit. The exact no-FMF print
/// spelling is the observable llvmkit assertion.
#[test]
fn clear_fast_math_flags_drops_flags_from_subsequent_ops() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .with_fast_math_flags(FastMathFlags::fast())
        .clear_fast_math_flags();
    assert!(b.fast_math_flags().is_empty());
    let p: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
    let r = b.fp_div(p, p, "r")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = fdiv float %0, %0\n"), "got:\n{text}");
    Ok(())
}

/// Mirrors `IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// (lines 630-640): after `FMF.setAllowReciprocal()` and
/// `Builder.setFastMathFlags(FMF)`, the next `CreateFDiv` carries the
/// `AllowReciprocal` bit.
#[test]
fn fmf_allow_reciprocal_propagates_to_fdiv() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let fmf = FastMathFlags::ALLOW_RECIPROCAL;
    let b = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .with_fast_math_flags(fmf);
    assert_eq!(b.fast_math_flags(), fmf);
    let p: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
    let r = b.fp_div(p, p, "r")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%r = fdiv arcp float %0, %0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// (lines 642-658): `Builder.CreateFCmpOEQ(F, F)` first carries no
/// `AllowReciprocal` bit after clear, then carries it after
/// `FMF.setAllowReciprocal(); Builder.setFastMathFlags(FMF);`.
#[test]
fn fmf_propagates_to_fcmp_oeq() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let i1_ty = m.bool_type();
    let fn_ty = m.fn_type(i1_ty, [f32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    assert!(b.fast_math_flags().is_empty());
    let p: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
    let c0 = b.fcmp_oeq::<f32, _, _, _>(p, p, "c0")?;
    let fmf = FastMathFlags::ALLOW_RECIPROCAL;
    let b = b.with_fast_math_flags(fmf);
    assert_eq!(b.fast_math_flags(), fmf);
    let c1 = b.fcmp_oeq::<f32, _, _, _>(p, p, "c1")?;
    b.ret(c1)?;
    let text = format!("{m}");
    assert!(
        text.contains("%c0 = fcmp oeq float %0, %0\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%c1 = fcmp arcp oeq float %0, %0\n"),
        "got:\n{text}"
    );
    let _ = c0;
    Ok(())
}

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, RAIIHelpersTest)`
/// (lines 823-856), specifically the `FastMathFlagGuard` arm: snapshot FMF,
/// scope changes to the builder's FMF, exit restores the original. Our
/// consume-self builders provide the same observable round-trip via
/// `fast_math_flags()` (snapshot) + `with_fast_math_flags(orig)` (restore).
#[test]
fn fmf_save_and_restore_round_trip() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    // Original: empty FMF.
    assert!(b.fast_math_flags().is_empty());
    // Snapshot, change to AllowReciprocal, build an op, restore.
    let orig = b.fast_math_flags();
    let b = b.with_fast_math_flags(FastMathFlags::ALLOW_RECIPROCAL);
    let p: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
    let r = b.fp_add(p, p, "r")?;
    assert!(
        b.fast_math_flags()
            .contains(FastMathFlags::ALLOW_RECIPROCAL)
    );
    let b = b.with_fast_math_flags(orig);
    assert!(b.fast_math_flags().is_empty());
    b.ret(r)?;
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
/// both shapes via `fp_neg` / `fp_neg_fmf` and
/// assert the exposed `FnegInst` FMF bits directly.
#[test]
fn fneg_emits_default_then_fmf_form() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
    let n0 = b.fp_neg::<f32, _, _>(p, "n0")?;
    let Some(InstructionKind::FNeg(n0_inst)) =
        InstructionView::try_from(b.view(n0).into_erased())?.kind()
    else {
        panic!("expected n0 to be fneg");
    };
    assert!(n0_inst.fast_math_flags().is_empty());
    let fmf = FastMathFlags::NO_NANS | FastMathFlags::NO_SIGNED_ZEROS;
    let n1 = b.fp_neg_fmf::<f32, _, _>(n0, fmf, "n1")?;
    let Some(InstructionKind::FNeg(n1_inst)) =
        InstructionView::try_from(b.view(n1).into_erased())?.kind()
    else {
        panic!("expected n1 to be fneg");
    };
    assert_eq!(n1_inst.fast_math_flags(), fmf);
    b.ret(n1)?;
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
/// FastMathFlags)` (lines 662-697): the AllowContract / ApproxFunc /
/// AllowReassoc propagation arm. llvmkit observes the instruction bits
/// through exact AsmWriter FMF order.
#[test]
fn fmf_accumulates_contract_approx_reassoc_on_fmul() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f32_ty = m.f32_type();
    let fn_ty = m.fn_type(f32_ty, [f32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    assert!(b.fast_math_flags().is_empty());
    let p: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
    let _ = b.fp_add(p, p, "no_contract")?;

    let contract = FastMathFlags::ALLOW_CONTRACT;
    let b = b.with_fast_math_flags(contract);
    assert_eq!(b.fast_math_flags(), contract);
    let _ = b.fp_add(p, p, "contract")?;

    let contract_afn = contract | FastMathFlags::APPROX_FUNC;
    let b = b.with_fast_math_flags(contract_afn);
    assert_eq!(b.fast_math_flags(), contract_afn);
    let _ = b.fp_mul(p, p, "mul_contract_afn")?;

    let contract_afn_reassoc = contract_afn | FastMathFlags::ALLOW_REASSOC;
    let b = b.with_fast_math_flags(contract_afn_reassoc);
    assert_eq!(b.fast_math_flags(), contract_afn_reassoc);
    let r = b.fp_mul(p, p, "mul_contract_afn_reassoc")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%no_contract = fadd float %0, %0\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%contract = fadd contract float %0, %0\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%mul_contract_afn = fmul contract afn float %0, %0\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%mul_contract_afn_reassoc = fmul reassoc contract afn float %0, %0\n"),
        "got:\n{text}"
    );
    Ok(())
}

// Helper: build a tiny module exercising the fcmp wrapper, assert that
// the AsmWriter emits the expected predicate keyword.
//
// A macro rather than a higher-ranked closure parameter. The callback would
// have to be generic over the module's *brand type*, and Rust cannot quantify a
// `for<..>` bound over a type — only over a lifetime. Expanding the body inline
// sidesteps the bound entirely and lets `'_` shrink to the borrow.
//
// `module_new!` sits *inside* the macro body on purpose: every expansion of
// this macro re-expands it, so each call site still gets its own brand type.
macro_rules! fcmp_predicate_emits {
    ($expected_pred:expr, |$b:ident, $lhs:ident, $rhs:ident| $mk:expr) => {{
        let expected_pred: &str = $expected_pred;
        let m = module_new!("a")?;
        let f32_ty = m.f32_type();
        let i1_ty = m.bool_type();
        let fn_ty = m.fn_type(i1_ty, [f32_ty.as_type(), f32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");
        let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let lhs: FloatValue<'_, f32, _> = m.view(f).param(0)?.try_into()?;
        let rhs: FloatValue<'_, f32, _> = m.view(f).param(1)?.try_into()?;
        let r = {
            let $b = &b;
            let ($lhs, $rhs) = (lhs, rhs);
            $mk
        }?;
        b.ret(r)?;
        let text = format!("{m}");
        let needle = format!(
            "%r = fcmp {expected_pred} float %0, %1
"
        );
        assert!(
            text.contains(&needle),
            "expected `{needle}`; got:
{text}"
        );
        Ok(())
    }};
}

// --- Per-predicate fcmp -------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 1677:
/// `fcmp oeq half %fop1, %fop2`. We use `f32` (`float`) here because the
/// project ships f32/f64 as primary kinds; the predicate routing is
/// agnostic to the float kind.
#[test]
fn build_fcmp_oeq_emits_oeq() -> Result<(), IrError> {
    fcmp_predicate_emits!("oeq", |b, lhs, rhs| {
        b.fcmp_oeq::<f32, _, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1679: `fcmp ogt half %fop1, %fop2`.
#[test]
fn build_fcmp_ogt_emits_ogt() -> Result<(), IrError> {
    fcmp_predicate_emits!("ogt", |b, lhs, rhs| {
        b.fcmp_ogt::<f32, _, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1681: `fcmp oge half %fop1, %fop2`.
#[test]
fn build_fcmp_oge_emits_oge() -> Result<(), IrError> {
    fcmp_predicate_emits!("oge", |b, lhs, rhs| {
        b.fcmp_oge::<f32, _, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1683: `fcmp olt half %fop1, %fop2`.
#[test]
fn build_fcmp_olt_emits_olt() -> Result<(), IrError> {
    fcmp_predicate_emits!("olt", |b, lhs, rhs| {
        b.fcmp_olt::<f32, _, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1685: `fcmp ole half %fop1, %fop2`.
#[test]
fn build_fcmp_ole_emits_ole() -> Result<(), IrError> {
    fcmp_predicate_emits!("ole", |b, lhs, rhs| {
        b.fcmp_ole::<f32, _, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1689: `fcmp ord half %fop1, %fop2`.
#[test]
fn build_fcmp_ord_emits_ord() -> Result<(), IrError> {
    fcmp_predicate_emits!("ord", |b, lhs, rhs| {
        b.fcmp_ord::<f32, _, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1703: `fcmp uno half %fop1, %fop2`.
#[test]
fn build_fcmp_uno_emits_uno() -> Result<(), IrError> {
    fcmp_predicate_emits!("uno", |b, lhs, rhs| {
        b.fcmp_uno::<f32, _, _, _>(lhs, rhs, "r")
    })
}

/// Mirrors `compatibility.ll` line 1691: `fcmp ueq half %fop1, %fop2`.
#[test]
fn build_fcmp_ueq_emits_ueq() -> Result<(), IrError> {
    fcmp_predicate_emits!("ueq", |b, lhs, rhs| {
        b.fcmp_ueq::<f32, _, _, _>(lhs, rhs, "r")
    })
}

// --- Non-int phi handles ----------------------------------------------

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
/// FPMathOperator)` (line 539), which exercises
/// `Builder.CreatePHI(Builder.getDoubleTy(), 0)` -- a `double`-typed phi
/// that subsequently feeds an `FPMathOperator`. We mirror the same
/// `phi double` shape through a `double` block parameter (the head-phi
/// created by `append_block_with_params`).
#[test]
fn build_fp_phi_emits_phi_with_double_kind() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let f64_ty = m.f64_type();
    let fn_ty = m.fn_type(f64_ty, [f64_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    // join(%p: double): a block whose single f64 parameter is the head-phi.
    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (join, params) = bwp.append_block_with_params(m.view(f), &[f64_ty.as_type()], "join")?;
    let join_label = join.id();
    // entry: br join(%0) — the incoming f64 rides the edge into the head-phi.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: FloatValue<'_, f64, _> = m.view(f).param(0)?.try_into()?;
    b.br_with_args(join_label, &[p.into_erased()])?;
    // join: ret %p (the head-phi param, where the phi result was used).
    let b2 = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi: FloatValue<'_, f64, _> = params[0].try_into()?;
    b2.ret(phi)?;
    let text = format!("{m}");
    // The param-phi is unnamed, so assert on the load-bearing `phi double`
    // kind + incoming pair rather than a `%merge =` label.
    assert!(text.contains("phi double [ %0, %entry ]\n"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Verifier/inalloca2.ll` line 35:
/// `%args = phi ptr [ %a, %if ], [ %b, %else ]` -- the canonical
/// upstream IR fixture for a pointer phi. Adapted to a `ptr` block
/// parameter (the head-phi created by `append_block_with_params`).
/// llvmkit-specific scaffold (no dedicated `unittests/IR/*Test.cpp::TEST`
/// for pointer phi).
#[test]
fn build_pointer_phi_emits_phi_with_ptr() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(ptr_ty, [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    // join(%p: ptr): a block whose single pointer parameter is the head-phi.
    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (join, params) = bwp.append_block_with_params(m.view(f), &[ptr_ty.as_type()], "join")?;
    let join_label = join.id();
    // entry: br join(%0) — the incoming ptr rides the edge into the head-phi.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    b.br_with_args(join_label, &[p.into_erased()])?;
    // join: ret %p (the head-phi param, where the phi result was used).
    let b2 = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi: PointerValue<'_, _> = params[0].try_into()?;
    b2.ret(phi)?;
    let text = format!("{m}");
    // The param-phi is unnamed, so assert on the load-bearing `phi ptr`
    // kind + incoming pair rather than a `%merge =` label.
    assert!(text.contains("phi ptr [ %0, %entry ]\n"), "got:\n{text}");
    Ok(())
}
