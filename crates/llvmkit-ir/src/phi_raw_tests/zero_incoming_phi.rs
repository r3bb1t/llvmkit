//! Regression coverage for the phi a dropped edge empties.
//!
//! When the typed edge-edit ops (`edit_cond_br(..).remove_then` /
//! `redirect_then`) drop the *only* predecessor of a block, that block's
//! leading phis are left with zero incomings and their users still reading
//! them. The edge ops mirror LLVM `BasicBlock::removePredecessor`, which
//! removes each incoming through `PHINode::removeIncomingValue` with
//! `DeletePHIIfEmpty` set: "If the PHI node is dead, because it has zero
//! entries, nuke it now" — `replaceAllUsesWith(PoisonValue::get(getType()))`
//! then `eraseFromParent()`. No verifier rule stands behind it: the block has
//! lost its only predecessor, so a leftover zero-entry phi passes
//! `visitBasicBlock`'s `numIncoming == numPreds` on `0 == 0`. The assertions
//! are the guard.
//!
//! This case builds a `cond_br` whose then-arm target has `entry` as its ONLY
//! predecessor, so removing that edge empties the target's head phi — which
//! block arguments cannot express with a raw single-incoming phi, hence the
//! in-crate home.

use crate::{
    Analyses, BlockId, Dyn, FnCx, FnReport, FunctionPass, IntValue, IrBuilder, IrError, IrResult,
    Linkage, Module, ModuleBrand, ReshapeCfg, VerifierRule, run_function_pass,
};

/// `Dyn`-marked block id in the fixture's brand: the storable branch-target
/// currency these fixtures hand back for the pass-side redirect surface.
type DynBlockId<B> = BlockId<Dyn, B>;

/// Return of `build_redirect_single_pred_phi`: the function plus the `to` and
/// `new_to` `Dyn` labels. Named so the signature stays under clippy's
/// `type_complexity` threshold without an `#[allow]` (the repo bans them).
type RedirectFixture<'ctx, B> = (crate::FunctionId<Dyn, B>, DynBlockId<B>, DynBlockId<B>);

/// A `ReshapeCfg` pass that removes the `from_name` block's `cond_br` then-edge
/// (its target `to` is the then-arm by construction), collapsing the `cond_br`
/// to a `br` to the surviving else-arm.
struct RemoveEdge {
    from_name: &'static str,
}

impl<B: ModuleBrand> FunctionPass<B> for RemoveEdge {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-edge-empty-phi";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.edit_cond_br(from.id())?.remove_then()?;
        Ok(reshape.done())
    }
}

/// A `ReshapeCfg` pass that retargets the `from_name` block's `cond_br` then-arm
/// (its target `old_to` by construction) onto `new_to`. `new_to` is authored
/// with no leading phis, so the `redirect_then` `phi_values` slice is empty. The
/// label is stashed at build time (arena ids are stable across `verify()`).
struct RedirectEmptyEdge<B: ModuleBrand> {
    from_name: &'static str,
    new_to: BlockId<Dyn, B>,
}

impl<B: ModuleBrand> FunctionPass<B> for RedirectEmptyEdge<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-edge-empty-phi";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        // `new_to` has no leading phis, so no incoming values are supplied.
        reshape
            .edit_cond_br(from.id())?
            .redirect_then(self.new_to, &[])?;
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
fn build_single_pred_phi<'ctx, B: crate::ModuleBrand + 'ctx>(
    m: &'ctx Module<B, crate::Unverified>,
) -> IrResult<(crate::FunctionId<Dyn, B>, BlockId<Dyn, B>)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let to = m.view(f).append_basic_block(m, "to");
    let other = m.view(f).append_basic_block(m, "other");

    let entry_lbl = entry.id();
    let to_lbl = to.id();
    let other_lbl = other.id();

    // entry: %x = add %a, 7 ; %c = icmp slt %a, 5 ; cond_br %c, to, other
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let x = b.int_add(a, 7_i32, "x")?;
    let c = b.icmp_slt(a, 5_i32, "c")?;
    b.cond_br(c, to_lbl, other_lbl)?;

    // to: %p = phi i32 [ %x, entry ] ; %u = add %p, 1 ; ret %u
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(to);
    let p = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(x, entry_lbl)?;
    let u = b.int_add(p.as_int_value(), 1_i32, "u")?;
    b.ret(u)?;

    // other: ret 0
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(other);
    b.ret(i32_ty.const_int(0_u32))?;

    Ok((f, to_lbl))
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
    let m = crate::module_new!("remove-edge-empty-phi")?;
    let (f, _to_dyn) = build_single_pred_phi(&m)?;

    let verified = m.verify()?;
    let mut analyses = Analyses::new();
    let pass = RemoveEdge { from_name: "entry" };
    let out = run_function_pass(pass, verified, f, &mut analyses)?;

    let reverified = out
        .verify()
        .expect("remove_then output must re-verify after emptying a phi");
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
fn build_redirect_single_pred_phi<'ctx, B: crate::ModuleBrand + 'ctx>(
    m: &'ctx Module<B, crate::Unverified>,
) -> IrResult<RedirectFixture<'ctx, B>> {
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let old_to = m.view(f).append_basic_block(m, "old_to");
    let other = m.view(f).append_basic_block(m, "other");
    let new_to = m.view(f).append_basic_block(m, "new_to");

    let entry_lbl = entry.id();
    let old_to_lbl = old_to.id();
    let other_lbl = other.id();
    let new_to_lbl = new_to.id();

    // entry: %x = add %a, 7 ; %c = icmp slt %a, 5 ; cond_br %c, old_to, other
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let x = b.int_add(a, 7_i32, "x")?;
    let c = b.icmp_slt(a, 5_i32, "c")?;
    b.cond_br(c, old_to_lbl, other_lbl)?;

    // old_to: %p = phi i32 [ %x, entry ] ; %u = add %p, 1 ; ret %u
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(old_to);
    let p = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(x, entry_lbl)?;
    let u = b.int_add(p.as_int_value(), 1_i32, "u")?;
    b.ret(u)?;

    // other: ret 0
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(other);
    b.ret(i32_ty.const_int(0_u32))?;

    // new_to: ret 1  (no leading phi -> redirect's `phi_values` slice is empty)
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(new_to);
    b.ret(i32_ty.const_int(1_u32))?;

    Ok((f, old_to_lbl, new_to_lbl))
}

/// Redirecting `entry → old_to` onto `new_to` — `entry` being `old_to`'s only
/// predecessor — empties `old_to`'s head phi. `redirect_then` must erase that
/// phi (LLVM `removePredecessor` parity), RAUW'ing its sole user onto poison of
/// the phi's own type, so the output re-verifies AND round-trips (no
/// bracket-less `phi i32` is printed).
///
/// This is the `redirect_then` twin of
/// [`remove_edge_emptying_phi_erases_it_with_poison`]: both reach the shared
/// `drop_incoming_from_pred` empty-phi erase, but this drives it through the
/// redirected then-arm's old target rather than the removed then-edge — the
/// path a final review flagged as covered only transitively.
///
/// Without the fix the phi survives with zero incomings in the now-unreachable
/// `old_to`: `verify()` still accepts it (0 == 0, and an unreachable block keeps
/// the reachable-block backstop quiet), but the printed IR carries a
/// bracket-less `phi` and its user (`%u`) still names `%p`, so the two
/// assertions below fail — red-without-the-fix by construction.
#[test]
fn redirect_edge_emptying_phi_erases_it_with_poison() -> Result<(), IrError> {
    let m = crate::module_new!("redirect-edge-empty-phi")?;
    let (f, _old_to_dyn, new_to_dyn) = build_redirect_single_pred_phi(&m)?;

    let verified = m.verify()?;
    let mut analyses = Analyses::new();
    let pass = RedirectEmptyEdge {
        from_name: "entry",
        new_to: new_to_dyn,
    };
    let out = run_function_pass(pass, verified, f, &mut analyses)?;

    let reverified = out
        .verify()
        .expect("redirect_then output must re-verify after emptying a phi");
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
}

/// A phi with **zero** incomings in a block that *has* a predecessor is
/// rejected by the one length guard upstream has — `Verifier::visitBasicBlock`'s
/// `Check(PN.getNumIncomingValues() == Preds.size(), "PHINode should have one
/// entry for each predecessor of its parent basic block!", &PN)`. The public
/// mutation path (Slice A) already erases such phis, but a phi authored
/// directly through the raw builder with no `add_incoming` — the shape block
/// arguments cannot express — must still be caught by `verify()`.
///
/// Shape: `entry` unconditionally branches to `b`; `b` opens with a raw
/// `phi i32` carrying no incomings, then a terminator. `b` therefore has one
/// predecessor and zero incoming entries.
///
/// The `CHECK` text is asserted, not just the rule: the rule enum is llvmkit's
/// category label and says nothing about the literal a
/// `llvm/test/Verifier/*.ll` fixture matches.
#[test]
fn zero_incoming_phi_in_a_block_with_a_predecessor_is_rejected() -> Result<(), IrError> {
    let m = crate::module_new!("zero_incoming_reachable")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = m.view(f).append_basic_block(&m, "b");
    let b_label = b.id();

    // entry: br b   (so `b` has exactly one predecessor)
    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    bld.br(b_label)?;

    // b: %p = phi i32   (no add_incoming) ; ret 0
    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(b);
    let _p = bld.view(bld.int_phi::<i32, _>("p")?);
    bld.ret(i32_ty.const_int(0_u32))?;

    let err = m
        .verify_borrowed()
        .expect_err("0 incomings against 1 predecessor must be rejected");
    match &err {
        IrError::VerifierFailure { rule, message, .. } => {
            assert_eq!(*rule, VerifierRule::PhiPredecessorMismatch, "got {err:?}");
            assert!(
                message.starts_with(
                    "PHINode should have one entry for each predecessor of its parent basic block!"
                ),
                "message {message:?} lacks upstream's Check literal"
            );
        }
        other => panic!("expected a verifier failure, got {other:?}"),
    }
    Ok(())
}

/// The contrast, and the case llvmkit used to get wrong: a zero-incoming phi in
/// a block with **zero** predecessors verifies clean, because
/// `visitBasicBlock`'s count guard passes on `0 == 0` and nothing else in
/// `Verifier` looks at a phi's length.
///
/// Both predecessor-less shapes are covered, since llvmkit's superseded
/// `PhiEmptyInReachableBlock` rule split exactly here — it rejected the entry
/// block and spared the unreachable one:
///
/// - `u`, unreachable, with no edge into it. This shape *is* an upstream
///   fixture, `test/Assembler/zero-input-phi.ll`, already ported at
///   `crates/llvmkit-asmparser/tests/parser_remaining_opcodes.rs::phi_int_round_trips`
///   and in the corpus manifest; its `%r = phi i32` sits in a `return` block
///   nothing branches to.
/// - `entry` itself, reachable by definition and predecessor-free — the shape
///   the old rule rejected and LLVM accepts. No upstream fixture writes it:
///   `grep -raInE '(^|[^a-zA-Z_])phi[[:space:]]+[^[:space:],]+[[:space:]]*$'
///   test/Verifier test/Assembler` under
///   `orig_cpp/llvm-project-llvmorg-22.1.4/llvm` matches `zero-input-phi.ll`
///   alone, so the oracle for this half is `Verifier::visitBasicBlock` read
///   directly.
#[test]
fn zero_incoming_phi_without_predecessors_verifies() -> Result<(), IrError> {
    let m = crate::module_new!("zero_incoming_no_preds")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    // `u` has no edge into it — unreachable from entry.
    let u = m.view(f).append_basic_block(&m, "u");

    // entry: %e = phi i32   (no add_incoming) ; ret 0
    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let _e = bld.view(bld.int_phi::<i32, _>("e")?);
    bld.ret(i32_ty.const_int(0_u32))?;

    // u: %q = phi i32   (no add_incoming) ; ret 0
    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(u);
    let _q = bld.view(bld.int_phi::<i32, _>("q")?);
    bld.ret(i32_ty.const_int(0_u32))?;

    m.verify_borrowed()
        .expect("a phi with no incomings and no predecessors is what LLVM accepts");
    Ok(())
}

/// The printed half of the round trip that makes accepting an empty phi safe —
/// the half the withdrawn `PhiEmptyInReachableBlock` rule assumed did not
/// exist.
///
/// `AsmWriter`'s phi arm prints the result type and then an empty
/// `ListSeparator` loop, so `%e = phi i32` is exactly what LLVM emits too. The
/// reading half already exists in the parser crate:
/// `parser_remaining_opcodes.rs::phi_int_round_trips` (the ported
/// `test/Assembler/zero-input-phi.ll`) and
/// `phi_real_incomings.rs::zero_input_phi_still_parses`.
///
/// No upstream counterpart: this is llvmkit's own print/parse idempotence law,
/// asserted on the shape the verifier rule above stopped rejecting.
#[test]
fn an_empty_phi_prints_its_type_and_no_pairs() -> Result<(), IrError> {
    let m = crate::module_new!("empty_phi_round_trip")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let bld = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let _e = bld.view(bld.int_phi::<i32, _>("e")?);
    bld.ret(i32_ty.const_int(0_u32))?;

    let printed = format!("{m}");
    let phi_line = printed
        .lines()
        .find(|l| l.contains("= phi "))
        .expect("the phi is printed");
    // `Out << ' '; TypePrinter.print(I.getType(), Out); Out << ' ';` and then a
    // `ListSeparator` loop that emits nothing — the trailing space is
    // upstream's too.
    assert_eq!(phi_line.trim_start(), "%e = phi i32 ", "got:\n{printed}");
    Ok(())
}
