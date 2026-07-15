//! Regression coverage for the zero-incoming-phi round-trip hole.
//!
//! When [`FnReshape::remove_edge`](crate::pass_context) /
//! [`redirect_edge`](crate::pass_context) drops the *only* predecessor of a
//! block, that block's leading phis are left with zero incomings. A phi with no
//! incomings has no legal textual form (`%p = phi i32` with no `[ … ]` pairs),
//! which LLVM's own LL parser rejects, so the module no longer round-trips. The
//! edge ops now mirror LLVM `BasicBlock::removePredecessor`: an emptied phi is
//! RAUW'd with poison of its own type and erased.
//!
//! This case builds a `cond_br` whose then-arm target has `entry` as its ONLY
//! predecessor, so removing that edge empties the target's head phi — which
//! block arguments cannot express with a raw single-incoming phi, hence the
//! in-crate home.

use crate::{
    Analyses, BasicBlockLabel, Dyn, FnCx, FnReport, FunctionPass, IRBuilder, IntValue, IrError,
    IrResult, Linkage, Module, ModuleBrand, ReshapeCfg, VerifierRule, run_function_pass,
};

/// A `ReshapeCfg` pass that removes the `from_name → to` edge. The `to` label is
/// stashed at build time (arena ids are stable across `verify()`).
struct RemoveEdge<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    to: BasicBlockLabel<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RemoveEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-edge-empty-phi";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.remove_edge(&from, &self.to)?;
        Ok(reshape.done())
    }
}

/// A `ReshapeCfg` pass that retargets the `from_name → old_to` `cond_br` arm
/// onto `new_to`. `new_to` is authored with no leading phis, so the
/// `redirect_edge` `phi_values` slice is empty. The labels are stashed at build
/// time (arena ids are stable across `verify()`).
struct RedirectEmptyEdge<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    old_to: BasicBlockLabel<'ctx, Dyn, B>,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RedirectEmptyEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-edge-empty-phi";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        // `new_to` has no leading phis, so no incoming values are supplied.
        reshape.redirect_edge(&from, &self.old_to, &self.new_to, &[])?;
        Ok(reshape.done())
    }
}

/// Build a `cond_br`-fed block whose ONLY predecessor is `entry`, so removing
/// the `entry → to` edge empties `to`'s single-incoming head phi. Returns the
/// function and `to`'s `Dyn` label.
///
/// ```text
/// entry(a): %x = add %a, 7
///           %c = icmp slt %a, 5
///           cond_br %c, to, other
/// to:       %p = phi i32 [ %x, entry ]
///           %u = add %p, 1 ; ret %u
/// other:    ret 0
/// ```
fn build_single_pred_phi<'ctx>(
    m: &Module<'ctx, crate::Brand<'ctx>, crate::Unverified>,
) -> IrResult<(crate::FunctionValue<'ctx, i32>, BasicBlockLabel<'ctx, Dyn>)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let to = f.append_basic_block(m, "to");
    let other = f.append_basic_block(m, "other");

    let entry_lbl = entry.label();
    let to_lbl = to.label();
    let other_lbl = other.label();
    let to_dyn: BasicBlockLabel<Dyn> = to_lbl.as_value().try_into()?;

    // entry: %x = add %a, 7 ; %c = icmp slt %a, 5 ; cond_br %c, to, other
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let x = b.build_int_add(a, 7_i32, "x")?;
    let c = b.build_icmp_slt(a, 5_i32, "c")?;
    b.build_cond_br(c, to_lbl, other_lbl)?;

    // to: %p = phi i32 [ %x, entry ] ; %u = add %p, 1 ; ret %u
    let b = IRBuilder::new_for::<i32>(m).position_at_end(to);
    let p = b.build_int_phi::<i32, _>("p")?.add_incoming(x, entry_lbl)?;
    let u = b.build_int_add(p.as_int_value(), 1_i32, "u")?;
    b.build_ret(u)?;

    // other: ret 0
    let b = IRBuilder::new_for::<i32>(m).position_at_end(other);
    b.build_ret(i32_ty.const_int(0_u32))?;

    Ok((f, to_dyn))
}

/// Removing `entry → to` — `entry` being `to`'s only predecessor — empties
/// `to`'s head phi. The op must erase that phi (LLVM `removePredecessor`
/// parity), RAUW'ing its sole user onto poison, so the output re-verifies AND
/// round-trips (no bracket-less `phi i32` is printed).
///
/// Without the fix the phi survives with zero incomings: `verify()` still
/// accepts it (0 == 0) but the printed IR carries a `phi` LLVM's parser rejects,
/// so the `!contains("phi")` assertion below fails.
#[test]
fn remove_edge_emptying_phi_erases_it_with_poison() -> Result<(), IrError> {
    Module::with_new("remove-edge-empty-phi", |m| {
        let (f, to_dyn) = build_single_pred_phi(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RemoveEdge {
            from_name: "entry",
            to: to_dyn,
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        let reverified = out
            .verify()
            .expect("remove_edge output must re-verify after emptying a phi");
        let printed = format!("{reverified}");
        // The emptied phi is erased entirely — never left as a bracket-less
        // `phi i32`, the shape LLVM's LL parser rejects. (Match the instruction
        // form, not the bare word: the module name also contains "phi".)
        assert!(
            !printed.contains("= phi"),
            "the emptied phi must have been erased, got:\n{printed}"
        );
        // Its sole user (`%u = add %p, 1`) was RAUW'd onto poison of the phi's
        // own type before the phi was detached.
        assert!(
            printed.contains("add i32 poison"),
            "the phi's user must now reference poison, got:\n{printed}"
        );
        Ok(())
    })
}

/// Build a `cond_br`-fed layout whose then-arm target (`old_to`) has `entry` as
/// its ONLY predecessor, plus a phi-free `new_to` that `entry` does not yet
/// reach. Redirecting the `entry → old_to` arm onto `new_to` strips `old_to`'s
/// only predecessor, emptying its single-incoming head phi. Returns the function
/// and the `old_to` / `new_to` `Dyn` labels.
///
/// ```text
/// entry(a): %x = add %a, 7
///           %c = icmp slt %a, 5
///           cond_br %c, old_to, other
/// old_to:   %p = phi i32 [ %x, entry ]
///           %u = add %p, 1 ; ret %u
/// other:    ret 0
/// new_to:   ret 1   ; no leading phi -> redirect's `phi_values` slice is empty
/// ```
fn build_redirect_single_pred_phi<'ctx>(
    m: &Module<'ctx, crate::Brand<'ctx>, crate::Unverified>,
) -> IrResult<(
    crate::FunctionValue<'ctx, i32>,
    BasicBlockLabel<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let old_to = f.append_basic_block(m, "old_to");
    let other = f.append_basic_block(m, "other");
    let new_to = f.append_basic_block(m, "new_to");

    let entry_lbl = entry.label();
    let old_to_lbl = old_to.label();
    let other_lbl = other.label();
    let new_to_lbl = new_to.label();
    let old_to_dyn: BasicBlockLabel<Dyn> = old_to_lbl.as_value().try_into()?;
    let new_to_dyn: BasicBlockLabel<Dyn> = new_to_lbl.as_value().try_into()?;

    // entry: %x = add %a, 7 ; %c = icmp slt %a, 5 ; cond_br %c, old_to, other
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let x = b.build_int_add(a, 7_i32, "x")?;
    let c = b.build_icmp_slt(a, 5_i32, "c")?;
    b.build_cond_br(c, old_to_lbl, other_lbl)?;

    // old_to: %p = phi i32 [ %x, entry ] ; %u = add %p, 1 ; ret %u
    let b = IRBuilder::new_for::<i32>(m).position_at_end(old_to);
    let p = b.build_int_phi::<i32, _>("p")?.add_incoming(x, entry_lbl)?;
    let u = b.build_int_add(p.as_int_value(), 1_i32, "u")?;
    b.build_ret(u)?;

    // other: ret 0
    let b = IRBuilder::new_for::<i32>(m).position_at_end(other);
    b.build_ret(i32_ty.const_int(0_u32))?;

    // new_to: ret 1  (no leading phi -> redirect's `phi_values` slice is empty)
    let b = IRBuilder::new_for::<i32>(m).position_at_end(new_to);
    b.build_ret(i32_ty.const_int(1_u32))?;

    Ok((f, old_to_dyn, new_to_dyn))
}

/// Redirecting `entry → old_to` onto `new_to` — `entry` being `old_to`'s only
/// predecessor — empties `old_to`'s head phi. `redirect_edge` must erase that
/// phi (LLVM `removePredecessor` parity), RAUW'ing its sole user onto poison of
/// the phi's own type, so the output re-verifies AND round-trips (no
/// bracket-less `phi i32` is printed).
///
/// This is the `redirect_edge` twin of
/// [`remove_edge_emptying_phi_erases_it_with_poison`]: both reach the shared
/// `drop_incoming_from_pred` empty-phi erase, but this drives it through
/// `redirect_edge`'s `old_to` argument rather than `remove_edge`'s `to` — the
/// path a final review flagged as covered only transitively.
///
/// Without the fix the phi survives with zero incomings in the now-unreachable
/// `old_to`: `verify()` still accepts it (0 == 0, and an unreachable block keeps
/// the reachable-block backstop quiet), but the printed IR carries a
/// bracket-less `phi` and its user (`%u`) still names `%p`, so the two
/// assertions below fail — red-without-the-fix by construction.
#[test]
fn redirect_edge_emptying_phi_erases_it_with_poison() -> Result<(), IrError> {
    Module::with_new("redirect-edge-empty-phi", |m| {
        let (f, old_to_dyn, new_to_dyn) = build_redirect_single_pred_phi(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectEmptyEdge {
            from_name: "entry",
            old_to: old_to_dyn,
            new_to: new_to_dyn,
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        let reverified = out
            .verify()
            .expect("redirect_edge output must re-verify after emptying a phi");
        let printed = format!("{reverified}");
        // The emptied phi is erased entirely — never left as a bracket-less
        // `phi i32`, the shape LLVM's LL parser rejects. (Match the instruction
        // form, not the bare word: the module name also contains "phi".)
        assert!(
            !printed.contains("= phi"),
            "the emptied phi must have been erased, got:\n{printed}"
        );
        // Its sole user (`%u = add %p, 1`) was RAUW'd onto poison of the phi's
        // own type before the phi was detached.
        assert!(
            printed.contains("add i32 poison"),
            "the phi's user must now reference poison, got:\n{printed}"
        );
        Ok(())
    })
}

/// Defensive verifier backstop: a phi with **zero** incomings sitting in a
/// block **reachable from entry** is rejected with `PhiEmptyInReachableBlock`,
/// however it arose. The public mutation path (Slice A) already erases such
/// phis, but a phi authored directly through the raw builder with no
/// `add_incoming` — the shape block arguments cannot express — must still be
/// caught by `verify()`.
///
/// Shape: `entry` unconditionally branches to `b`; `b` opens with a raw
/// `phi i32` carrying no incomings, then a terminator. `b` is reachable
/// (entry → b). The new check runs *before* the count-mismatch delegation, so
/// it fires first — otherwise this block (0 incomings vs 1 predecessor) would
/// be reported as `PhiPredecessorMismatch`, which is exactly what makes this
/// test red-for-the-right-reason before the check exists.
#[test]
fn zero_incoming_phi_in_reachable_block_is_rejected() -> Result<(), IrError> {
    Module::with_new("zero_incoming_reachable", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = f.append_basic_block(&m, "b");
        let b_label = b.label();

        // entry: br b   (so `b` is reachable from entry)
        let bld = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        bld.build_br(b_label)?;

        // b: %p = phi i32   (no add_incoming) ; ret 0
        let bld = IRBuilder::new_for::<i32>(&m).position_at_end(b);
        let _p = bld.build_int_phi::<i32, _>("p")?.finish();
        bld.build_ret(i32_ty.const_int(0_u32))?;

        let err = m
            .verify_borrowed()
            .expect_err("a zero-incoming phi in a reachable block must be rejected");
        assert!(
            matches!(
                err,
                IrError::VerifierFailure {
                    rule: VerifierRule::PhiEmptyInReachableBlock,
                    ..
                }
            ),
            "expected PhiEmptyInReachableBlock, got {err:?}"
        );
        Ok(())
    })
}

/// Contrast that proves the reachability gate: the *same* zero-incoming phi in
/// an **unreachable** block (no predecessors, no edge into it) is NOT rejected
/// by this rule. An unreachable block may legitimately have zero predecessors,
/// so its head phi carrying zero incomings passes the shared count check
/// (`0 == 0`) and the reachable-gate suppresses the new backstop. `verify()`
/// accepts the module.
#[test]
fn zero_incoming_phi_in_unreachable_block_is_accepted() -> Result<(), IrError> {
    Module::with_new("zero_incoming_unreachable", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        // `u` has no edge into it — unreachable from entry.
        let u = f.append_basic_block(&m, "u");

        // entry: ret 0
        let bld = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        bld.build_ret(i32_ty.const_int(0_u32))?;

        // u: %q = phi i32   (no add_incoming) ; ret 0
        let bld = IRBuilder::new_for::<i32>(&m).position_at_end(u);
        let _q = bld.build_int_phi::<i32, _>("q")?.finish();
        bld.build_ret(i32_ty.const_int(0_u32))?;

        m.verify_borrowed()
            .expect("a zero-incoming phi in an unreachable block is not this rule's concern");
        Ok(())
    })
}
