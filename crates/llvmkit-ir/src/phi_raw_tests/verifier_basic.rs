//! Raw-phi verifier coverage relocated from `tests/verifier_basic.rs`.
//!
//! These cases feed phi incomings from `switch`/`invoke`/`callbr` CFG edges,
//! or are malformed-by-design, so the block-argument authoring surface
//! (`append_block_with_params` + `build_*_with_args`) physically cannot
//! express them. They exercise the raw `build_int_phi`/`add_incoming` API
//! from inside the crate and are kept verbatim from their integration-test
//! origin (only the `llvmkit_ir::` paths are rewritten to `crate::`).

use crate::{IRBuilder, IntValue, IrError, Linkage, Module, VerifierRule};

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor checks
/// using `IR/CFG.h` switch successors: default and case edges both reach
/// the PHI block, so duplicate incoming entries from the same predecessor
/// are valid when they carry the same value.
#[test]
fn verify_phi_predecessors_through_switch_passes() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("phi_switch_ok", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let join_label = join.label();

        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let (_sealed, switch) = IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_switch(x, join_label, "")?;
        let _closed = switch
            .add_case(i32_ty.const_int(0_i32), join_label)?
            .finish();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(x, entry_label)?
            .add_incoming(x, entry_label)?;
        b.build_ret(phi.as_int_value())?;

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor-count
/// rejection through `SwitchInst` CFG edges.
#[test]
fn verify_phi_predecessors_through_switch_rejects_missing_edge() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("phi_switch_bad", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let join_label = join.label();

        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let (_sealed, switch) = IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_switch(x, join_label, "")?;
        let _closed = switch
            .add_case(i32_ty.const_int(0_i32), join_label)?
            .finish();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(x, entry_label)?;
        b.build_ret(phi.as_int_value())?;

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
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor checks
/// using `InvokeInst` normal-edge CFG semantics from `IR/CFG.h`.
#[test]
fn verify_phi_predecessors_through_invoke_passes() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("phi_invoke_ok", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<crate::Type>::new(), false);
        let callee = m.add_function::<(), _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", caller_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let join = f.append_basic_block(&m, "join");
        let unwind = f.append_basic_block(&m, "unwind");
        let entry_label = entry.label();
        let join_label = join.label();
        let unwind_label = unwind.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_invoke_dyn(
                callee,
                Vec::<crate::Value>::new(),
                join_label,
                unwind_label,
                "",
            )?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(unwind)
            .build_ret(x)?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(x, entry_label)?;
        b.build_ret(phi.as_int_value())?;

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor-block
/// rejection for an `InvokeInst` normal destination.
#[test]
fn verify_phi_predecessors_through_invoke_rejects_wrong_block() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("phi_invoke_bad", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<crate::Type>::new(), false);
        let callee = m.add_function::<(), _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", caller_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let join = f.append_basic_block(&m, "join");
        let unwind = f.append_basic_block(&m, "unwind");
        let other = f.append_basic_block(&m, "other");
        let join_label = join.label();
        let unwind_label = unwind.label();
        let other_label = other.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_invoke_dyn(
                callee,
                Vec::<crate::Value>::new(),
                join_label,
                unwind_label,
                "",
            )?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(unwind)
            .build_ret(x)?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(other)
            .build_ret(x)?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(x, other_label)?;
        b.build_ret(phi.as_int_value())?;

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
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor checks
/// using `CallBrInst` default-plus-indirect CFG edges from `IR/CFG.h`.
#[test]
fn verify_phi_predecessors_through_callbr_passes() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("phi_callbr_ok", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<crate::Type>::new(), false);
        let callee = m.add_function::<(), _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", caller_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let join_label = join.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_callbr(
                callee,
                Vec::<crate::Value>::new(),
                join_label,
                [join_label],
                "",
            )?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(x, entry_label)?
            .add_incoming(x, entry_label)?;
        b.build_ret(phi.as_int_value())?;

        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitPHINode` predecessor-count
/// rejection through duplicate `CallBrInst` CFG edges.
#[test]
fn verify_phi_predecessors_through_callbr_rejects_missing_edge() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("phi_callbr_bad", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<crate::Type>::new(), false);
        let callee = m.add_function::<(), _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", caller_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let join_label = join.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_callbr(
                callee,
                Vec::<crate::Value>::new(),
                join_label,
                [join_label],
                "",
            )?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(x, entry_label)?;
        b.build_ret(phi.as_int_value())?;

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
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::verifyDominatesUse` and
/// `llvm/lib/IR/Dominators.cpp`: a PHI incoming value must dominate the
/// edge from its listed predecessor, not just some other predecessor.
#[test]
fn verify_phi_incoming_edge_dominance_fails() -> Result<(), IrError> {
    Module::with_new::<_, _, _>("dom_phi_bad", |m| {
        let i32_ty = m.i32_type();
        let bool_ty = m.bool_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");
        let join = f.append_basic_block(&m, "join");
        let then_label = then_bb.label();
        let else_label = else_bb.label();
        let join_label = join.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let cond: IntValue<bool> = f.param(1)?.try_into()?;

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_cond_br(cond, then_label, else_label)?;
        let bt = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        let y = bt.build_int_add(x, 1_i32, "y")?;
        bt.build_br(join_label)?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(else_bb)
            .build_br(join_label)?;
        let bj = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi = bj
            .build_int_phi::<i32, _>("p")?
            .add_incoming(x, then_label)?
            .add_incoming(y, else_label)?;
        bj.build_ret(phi.as_int_value())?;

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
    })
}
