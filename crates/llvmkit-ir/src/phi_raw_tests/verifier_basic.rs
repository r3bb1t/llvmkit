//! Raw-phi verifier coverage relocated from `tests/verifier_basic.rs`.
//!
//! Most feed phi incomings from `switch`/`invoke`/`callbr` CFG edges, which the
//! block-argument authoring surface (`append_block_with_params` +
//! `*_with_args`, which only carry `br`/`cond_br` edges) cannot express;
//! the rest are malformed-by-design verifier-negative cases for which the raw
//! path is the natural way to author a deliberately-broken phi. They exercise
//! the raw `int_phi`/`add_incoming` API
//! from inside the crate and are kept verbatim from their integration-test
//! origin (only the `llvmkit_ir::` paths are rewritten to `crate::`).

use crate::{Dyn, IntValue, IrBuilder, IrError, Linkage, VerifierRule};

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor checks
/// using `IR/CFG.h` switch successors: default and case edges both reach
/// the PHI block, so duplicate incoming entries from the same predecessor
/// are valid when they carry the same value.
#[test]
fn verify_phi_predecessors_through_switch_passes() -> Result<(), IrError> {
    let m = crate::module_new!("phi_switch_ok")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let join = m.view(f).append_basic_block(&m, "join");
    let entry_label = entry.id();
    let join_label = join.id();

    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let (_sealed, switch) = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .switch_dyn(x, join_label, "")?;
    let _closed = switch
        .add_case(i32_ty.const_int(0_i32), join_label)?
        .finish();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(x, entry_label)?
        .add_incoming(x, entry_label)?;
    b.ret(phi.as_int_value())?;

    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor-count
/// rejection through `SwitchInst` CFG edges.
#[test]
fn verify_phi_predecessors_through_switch_rejects_missing_edge() -> Result<(), IrError> {
    let m = crate::module_new!("phi_switch_bad")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let join = m.view(f).append_basic_block(&m, "join");
    let entry_label = entry.id();
    let join_label = join.id();

    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let (_sealed, switch) = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .switch_dyn(x, join_label, "")?;
    let _closed = switch
        .add_case(i32_ty.const_int(0_i32), join_label)?
        .finish();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(x, entry_label)?;
    b.ret(phi.as_int_value())?;

    let err = m
        .verify_borrowed()
        .expect_err("missing switch incoming must fail");
    assert!(
        matches!(
            err,
            IrError::VerifierFailure {
                rule: VerifierRule::PhiPredecessorMismatch,
                ..
            }
        ),
        "got {err:?}"
    );
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor checks
/// using `InvokeInst` normal-edge CFG semantics from `IR/CFG.h`.
///
/// The `unwind` block starts with a `landingpad` and the function carries a
/// `personality` because `Verifier::visitInvokeInst` requires the unwind
/// destination to be an EH pad (`The unwind destination does not have an
/// exception handling instruction!`) and `visitLandingPadInst` requires the
/// personality. Neither is what this test is about; without them the module is
/// not IR upstream accepts, and it used to be accepted here only because both
/// `Check`s were unported.
#[test]
fn verify_phi_predecessors_through_invoke_passes() -> Result<(), IrError> {
    let m = crate::module_new!("phi_invoke_ok")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let callee_ty = m.function_type(void_ty.as_type(), Vec::<crate::Type<'_, _>>::new());
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", caller_ty, Linkage::External)?;
    // `personality ptr null`, the spelling
    // `test/Verifier/inline-asm-indirect-operand.ll` uses: the `Check` is
    // `F->hasPersonalityFn()`, which says nothing about what the personality
    // is.
    m.view(f)
        .set_personality_fn(&m, m.ptr_type(0).const_null())?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let join = m.view(f).append_basic_block(&m, "join");
    let unwind = m.view(f).append_basic_block(&m, "unwind");
    let entry_label = entry.id();
    let join_label = join.id();
    let unwind_label = unwind.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .invoke_dyn(
            m.view(callee),
            Vec::<crate::Value<'_, _>>::new(),
            join_label,
            unwind_label,
            "",
        )?;
    {
        // `Verifier::visitInvokeInst`: the unwind destination must start with
        // an EH pad. A `cleanup`-only `landingpad` is the smallest one.
        let unwind_builder = IrBuilder::new_for::<Dyn>(&m).position_at_end(unwind);
        let _closed = unwind_builder
            .landingpad(i32_ty.as_type(), true, "lp")?
            .finish();
        unwind_builder.ret(x)?;
    }

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(x, entry_label)?;
    b.ret(phi.as_int_value())?;

    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor-block
/// rejection for an `InvokeInst` normal destination.
#[test]
fn verify_phi_predecessors_through_invoke_rejects_wrong_block() -> Result<(), IrError> {
    let m = crate::module_new!("phi_invoke_bad")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let callee_ty = m.function_type(void_ty.as_type(), Vec::<crate::Type<'_, _>>::new());
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", caller_ty, Linkage::External)?;
    // `personality ptr null`, the spelling
    // `test/Verifier/inline-asm-indirect-operand.ll` uses: the `Check` is
    // `F->hasPersonalityFn()`, which says nothing about what the personality
    // is.
    m.view(f)
        .set_personality_fn(&m, m.ptr_type(0).const_null())?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let join = m.view(f).append_basic_block(&m, "join");
    let unwind = m.view(f).append_basic_block(&m, "unwind");
    let other = m.view(f).append_basic_block(&m, "other");
    let join_label = join.id();
    let unwind_label = unwind.id();
    let other_label = other.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .invoke_dyn(
            m.view(callee),
            Vec::<crate::Value<'_, _>>::new(),
            join_label,
            unwind_label,
            "",
        )?;
    {
        // `Verifier::visitInvokeInst`: the unwind destination must start with
        // an EH pad. A `cleanup`-only `landingpad` is the smallest one.
        let unwind_builder = IrBuilder::new_for::<Dyn>(&m).position_at_end(unwind);
        let _closed = unwind_builder
            .landingpad(i32_ty.as_type(), true, "lp")?
            .finish();
        unwind_builder.ret(x)?;
    }
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(other)
        .ret(x)?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(x, other_label)?;
    b.ret(phi.as_int_value())?;

    let err = m
        .verify_borrowed()
        .expect_err("wrong invoke incoming block must fail");
    assert!(
        matches!(
            err,
            IrError::VerifierFailure {
                rule: VerifierRule::PhiPredecessorMismatch,
                ..
            }
        ),
        "got {err:?}"
    );
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor checks
/// using `CallBrInst` default-plus-indirect CFG edges from `IR/CFG.h`.
#[test]
fn verify_phi_predecessors_through_callbr_passes() -> Result<(), IrError> {
    let m = crate::module_new!("phi_callbr_ok")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    // `Verifier::visitCallBrInst`'s non-inline-asm arm ends in `default:
    // CheckFailed("Callbr currently only supports asm-goto and selected
    // intrinsics")`, so a `callbr` to an ordinary function is not IR upstream
    // accepts. The callee is inline asm with one label constraint, matching
    // the one indirect destination — the shape `test/Verifier/callbr.ll`
    // spells `@correct_label_constraints`.
    let asm_ty = m.function_type(void_ty.as_type(), Vec::<crate::Type<'_, _>>::new());
    let asm = m.inline_asm(
        asm_ty,
        "",
        "!i",
        crate::InlineAsmOptions::new().side_effects(),
    );
    let caller_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", caller_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let join = m.view(f).append_basic_block(&m, "join");
    let entry_label = entry.id();
    let join_label = join.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let _ = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .inline_asm_callbr::<(), _, _, _, _, _, _>(
            asm,
            Vec::<crate::Value<'_, _>>::new(),
            join_label,
            [join_label],
            "",
        )?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(x, entry_label)?
        .add_incoming(x, entry_label)?;
    b.ret(phi.as_int_value())?;

    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor-count
/// rejection through duplicate `CallBrInst` CFG edges.
#[test]
fn verify_phi_predecessors_through_callbr_rejects_missing_edge() -> Result<(), IrError> {
    let m = crate::module_new!("phi_callbr_bad")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    // `Verifier::visitCallBrInst`'s non-inline-asm arm ends in `default:
    // CheckFailed("Callbr currently only supports asm-goto and selected
    // intrinsics")`, so a `callbr` to an ordinary function is not IR upstream
    // accepts. The callee is inline asm with one label constraint, matching
    // the one indirect destination — the shape `test/Verifier/callbr.ll`
    // spells `@correct_label_constraints`.
    let asm_ty = m.function_type(void_ty.as_type(), Vec::<crate::Type<'_, _>>::new());
    let asm = m.inline_asm(
        asm_ty,
        "",
        "!i",
        crate::InlineAsmOptions::new().side_effects(),
    );
    let caller_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", caller_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let join = m.view(f).append_basic_block(&m, "join");
    let entry_label = entry.id();
    let join_label = join.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let _ = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .inline_asm_callbr::<(), _, _, _, _, _, _>(
            asm,
            Vec::<crate::Value<'_, _>>::new(),
            join_label,
            [join_label],
            "",
        )?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(x, entry_label)?;
    b.ret(phi.as_int_value())?;

    let err = m
        .verify_borrowed()
        .expect_err("missing callbr incoming must fail");
    assert!(
        matches!(
            err,
            IrError::VerifierFailure {
                rule: VerifierRule::PhiPredecessorMismatch,
                ..
            }
        ),
        "got {err:?}"
    );
    Ok(())
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse` and
/// `llvm/lib/IR/Dominators.cpp`: a PHI incoming value must dominate the
/// edge from its listed predecessor, not just some other predecessor.
#[test]
fn verify_phi_incoming_edge_dominance_fails() -> Result<(), IrError> {
    let m = crate::module_new!("dom_phi_bad")?;
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_bb = m.view(f).append_basic_block(&m, "then");
    let else_bb = m.view(f).append_basic_block(&m, "else");
    let join = m.view(f).append_basic_block(&m, "join");
    let then_label = then_bb.id();
    let else_label = else_bb.id();
    let join_label = join.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let cond: IntValue<'_, bool, _> = m.view(f).param(1)?.try_into()?;

    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .cond_br(cond, then_label, else_label)?;
    let bt = IrBuilder::new_for::<Dyn>(&m).position_at_end(then_bb);
    let y = bt.int_add(x, 1_i32, "y")?;
    bt.br(join_label)?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(else_bb)
        .br(join_label)?;
    let bj = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = bj
        .view(bj.int_phi::<i32, _>("p")?)
        .add_incoming(x, then_label)?
        .add_incoming(y, else_label)?;
    bj.ret(phi.as_int_value())?;

    let err = m
        .verify_borrowed()
        .expect_err("non-dominating phi incoming value must fail");
    assert!(
        matches!(
            err,
            IrError::VerifierFailure {
                rule: VerifierRule::UseBeforeDef,
                ..
            }
        ),
        "got {err:?}"
    );
    Ok(())
}
