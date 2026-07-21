//! Package 4: framework-*witnessed* analysis preservation across a reshape.
//!
//! A `ReshapeCfg` pass's preservation floor is `none()` — it may rewrite the
//! CFG, so by default every analysis is evicted. Package 4 lets the driver add
//! back exactly the analyses it *watches* repair: the reshape mutator records
//! its edits as `CfgUpdate`s, and at `done()` the driver offers them to each
//! cached CFG-incremental analysis. The dominator tree repairs
//! (correct-by-recompute) and is kept — refreshed to the edited CFG — rather
//! than thrown away. No author ever *claims* the preservation.

use llvmkit_ir::{
    Analyses, BasicBlockLabel, DominatorTree, DominatorTreeAnalysis, Dyn, FnCx, FnReport,
    FunctionPass, FunctionView, IRBuilder, InsertPoint, IntPredicate, IntValue, IrError, IrResult,
    Linkage, Module, ModuleBrand, ReshapeCfg, Type, Value, run_function_pass,
};

/// A `ReshapeCfg` pass that requires the dominator tree and splits the entry
/// block before its terminator — a genuine CFG edit that records `CfgUpdate`s.
struct SplitEntryPass;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for SplitEntryPass {
    type Access = ReshapeCfg;
    type Requires = (DominatorTreeAnalysis,);
    const NAME: &'static str = "split-entry";

    fn run(
        &mut self,
        cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, (DominatorTreeAnalysis,)>,
    ) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has an entry block");
        // Split before the terminator: `br next` (and the only edge into `next`)
        // moves into a fresh block nothing reaches, so `next` becomes unreachable.
        let terminator = entry.instructions().last().expect("entry has a terminator");
        reshape.split_block(&entry, &terminator, "entry.split")?;
        Ok(reshape.done())
    }
}

/// After a `ReshapeCfg` pass whose floor is `none()`, the dominator tree is
/// nonetheless still cached — the driver *witnessed* it repair via the recorded
/// `CfgUpdate`s and kept it — and the kept tree is the REPAIRED one (it reflects
/// the edit: `next` is now unreachable), not a stale survivor.
#[test]
fn reshape_pass_preserves_and_repairs_dominator_tree() -> Result<(), IrError> {
    Module::with_new("witnessed-preservation", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let next = f.append_basic_block(&m, "next");
        let next_label = next.label();

        // entry: br next    next: ret 0
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.build_br(next.label())?;
        let b2 = IRBuilder::new_for::<Dyn>(&m).position_at_end(next);
        b2.build_ret(i32_ty.const_int(0_u32))?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();

        // Run the reshape pass. It prefetches the dominator tree (while `next` is
        // reachable), splits the entry, and reports the ReshapeCfg floor + its
        // edit log.
        let _unverified = run_function_pass(SplitEntryPass, verified, f, &mut analyses)?;

        // The dominator tree survived the reshape — witnessed-preserved, not
        // evicted by the `none()` floor.
        let cached: Option<&DominatorTree> = analyses
            .function_manager()
            .get_cached_result::<DominatorTreeAnalysis, _>(FunctionView::from(f));
        let dt = cached.expect("dominator tree was witnessed-preserved across the reshape");

        // And the kept tree is the REPAIRED one: it reflects the edited CFG, in
        // which `next` is no longer reachable. A stale survivor would say true.
        assert!(!dt.is_reachable_from_entry(next_label));
        Ok(())
    })
}

/// Package 4 / phi-guarantees wave 1, task 1: `split_block` must carry its own
/// phi maintenance — the live bug this test exists to kill.
///
/// `split_block` fires on `entry`, whose old terminator's successors (`[merge]`,
/// captured *before* the split) are what the fix must rewrite. The pass splits
/// at the terminator itself (same split point as
/// `split_block_records_edge_decomposition` in `pass_context.rs`'s in-crate
/// tests): `entry` keeps the `add` and loses its `br %merge` (moved wholesale
/// into `entry.split`), so `entry` is left unterminated — `split_block`'s
/// documented contract is that the caller wires a fresh terminator into the new
/// block. This test does that through a public [`InsertPoint`] saved *before*
/// the pass ran (while `entry` was genuinely open), then restored afterward —
/// [`IRBuilder::restore_insert_point`] is the sanctioned way to reopen a block a
/// structural edit left unterminated, since the pass-context API never hands out
/// an `Unterminated` handle for a block it did not just create.
///
/// Before the phi fix, `merge`'s phi still names `entry` (no longer a
/// predecessor — only `entry.split` is), so `verify()` fails with
/// `PhiPredecessorMismatch` ("phi incoming block %entry is not a predecessor").
/// After the fix, the phi is rewritten in place to name `entry.split` and the
/// module re-verifies clean.
#[test]
fn split_block_rewrites_successor_phi_incoming() -> Result<(), IrError> {
    Module::with_new("split-phi", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        // `Dyn`-marked throughout so every block/label the pass-context API
        // hands back (always `Dyn`) matches the builder's return marker without
        // a widening conversion (there isn't one — see `IntoBasicBlockLabel`).
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        // merge(%p: i32): the phi `[ %x, %entry ]` is authored as a block
        // parameter (head-phi), seeded by `entry` branching to `merge(%x)`.
        let (merge, merge_params) =
            IRBuilder::new(&m).append_block_with_params(f, &[i32_ty.as_type()], "merge")?;
        let merge_label = merge.label();

        // entry: %x = add %a, 1 ; br %merge(%x)
        let b = IRBuilder::new(&m).position_at_end(entry);
        // Saved while `entry` is genuinely open (before-of-none == end of
        // block) so the pass can reopen `entry` after the split empties it.
        let ip = b.save_insert_point();
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let x = b.build_int_add(a, 1_i32, "x")?;
        b.build_br_with_args(merge_label, &[x.into_erased()])?;

        // merge: ret %p (the head-phi param carrying the branch argument).
        let b2 = IRBuilder::new(&m).position_at_end(merge);
        let p: IntValue<i32> = merge_params[0].try_into()?;
        b2.build_ret(p)?;

        /// Splits `entry` at its terminator, then reopens `entry` (through the
        /// pre-saved `ip`) to wire a fresh `br %entry.split` terminator — the
        /// caller-side half of `split_block`'s documented contract.
        struct SplitAtAdd<'ctx, B: ModuleBrand + 'ctx> {
            ip: Option<InsertPoint<'ctx, Dyn, B>>,
        }

        impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for SplitAtAdd<'ctx, B> {
            type Access = ReshapeCfg;
            type Requires = ();
            const NAME: &'static str = "split-at-add";

            fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
                let reshape = cx.mutate();
                let entry = reshape
                    .function()
                    .entry_block()
                    .expect("definition has an entry block");
                let terminator = entry
                    .instructions()
                    .last()
                    .expect("entry is terminated by the br");
                let new_block = reshape.split_block(&entry, &terminator, "entry.split")?;
                let ip = self
                    .ip
                    .take()
                    .expect("insert point stashed before the pass ran");
                let b = reshape.builder_at(ip)?;
                b.build_br(new_block.label())?;
                Ok(reshape.done())
            }
        }

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = SplitAtAdd { ip: Some(ip) };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        // THE assertion: the output must re-verify. Without the phi fix the
        // verifier fails: "phi incoming block %entry is not a predecessor".
        let reverified = out.verify().expect("split output must stay coherent");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("[ %x, %\"entry.split\" ]")
                || printed.contains("[ %x, %entry.split ]"),
            "phi incoming must name the new block, got:\n{printed}"
        );
        Ok(())
    })
}

/// A `ReshapeCfg` pass that inserts a single phi into the merge block named
/// `merge_name`, with the pre-computed `incomings` of type `ty`. It requires the
/// dominator tree, because `FnReshape::insert_phi` witnesses every incoming
/// value's dominance over its edge through `analysis_repaired` before creating
/// the phi. The incomings/labels are stashed at build time (arena ids are stable
/// across `verify()`), mirroring how `SplitAtAdd` stashes its `InsertPoint`.
struct InsertMergePhi<'ctx, B: ModuleBrand + 'ctx> {
    merge_name: &'static str,
    ty: Type<'ctx, B>,
    incomings: Vec<(Value<'ctx, B>, BasicBlockLabel<'ctx, Dyn, B>)>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for InsertMergePhi<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = (DominatorTreeAnalysis,);
    const NAME: &'static str = "insert-merge-phi";

    fn run(
        &mut self,
        cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, (DominatorTreeAnalysis,)>,
    ) -> IrResult<FnReport> {
        let mut reshape = cx.mutate();
        let merge = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.merge_name))
            .expect("merge block is present");
        reshape.insert_phi(&merge, self.ty, &self.incomings)?;
        Ok(reshape.done())
    }
}

/// Build a verifying diamond `entry -> {left, right} -> merge` returning `i32`.
/// `left` defines `%lv = add %a, 10`, `right` defines `%rv = add %a, 20`, and
/// `merge` initially just `ret 0` (no phi yet — a pass inserts one). Returns the
/// function plus the two arm values and the two arm labels the pass will feed as
/// phi incomings.
#[allow(clippy::type_complexity)]
fn build_diamond<'ctx>(
    m: &Module<'ctx, llvmkit_ir::Brand<'ctx>, llvmkit_ir::Unverified>,
) -> IrResult<(
    llvmkit_ir::FunctionValue<'ctx, Dyn>,
    Value<'ctx>,
    BasicBlockLabel<'ctx, Dyn>,
    Value<'ctx>,
    BasicBlockLabel<'ctx, Dyn>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let left = f.append_basic_block(m, "left");
    let right = f.append_basic_block(m, "right");
    let merge = f.append_basic_block(m, "merge");
    // The arm labels are `Dyn` for the `insert_phi` incoming slice (whose pred
    // labels are `Dyn`); the diamond's return marker is `Dyn` too, so the
    // conversion is an identity re-tag rather than an erasure.
    let left_label: BasicBlockLabel<Dyn> = left.label().to_erased().try_into()?;
    let right_label: BasicBlockLabel<Dyn> = right.label().to_erased().try_into()?;

    // entry: br (%a == 0) ? left : right
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let cond = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
    b.build_cond_br(cond, &left, &right)?;

    // left: %lv = add %a, 10 ; br merge
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(left);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let lv = b.build_int_add(a, 10_i32, "lv")?;
    b.build_br(merge.label())?;

    // right: %rv = add %a, 20 ; br merge
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(right);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let rv = b.build_int_add(a, 20_i32, "rv")?;
    b.build_br(merge.label())?;

    // merge: ret 0   (no phi yet — the pass inserts one)
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(merge);
    b.build_ret(i32_ty.const_int(0_u32))?;

    Ok((
        f,
        lv.into_erased(),
        left_label,
        rv.into_erased(),
        right_label,
    ))
}

/// POSITIVE: a `ReshapeCfg` pass inserts a phi into a 2-predecessor merge block
/// with two valid, dominating incomings (each arm's value defined in the arm it
/// flows from, which trivially dominates its own edge). The pass succeeds, the
/// output re-verifies, and the phi prints with both incomings.
#[test]
fn insert_phi_into_merge_block_verifies() -> Result<(), IrError> {
    Module::with_new("insert-phi-merge", |m| {
        let i32_ty = m.i32_type();
        let (f, lv, left_label, rv, right_label) = build_diamond(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = InsertMergePhi {
            merge_name: "merge",
            ty: i32_ty.as_type(),
            incomings: vec![(lv, left_label), (rv, right_label)],
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        // The inserted phi keeps the module coherent: merge's two predecessors
        // {left, right} match the phi's two incomings, and both values dominate
        // their edges.
        let reverified = out.verify().expect("inserted phi must be coherent");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("[ %lv, %left ]"),
            "phi must carry the left incoming, got:\n{printed}"
        );
        assert!(
            printed.contains("[ %rv, %right ]"),
            "phi must carry the right incoming, got:\n{printed}"
        );
        Ok(())
    })
}

/// NEGATIVE (dominance): a `ReshapeCfg` pass tries to insert a phi whose incoming
/// value does NOT dominate its edge — `%lv` (defined in `left`) fed down the
/// `right` edge, and `left` does not dominate `right` in the diamond. The
/// completeness/type checks pass (both blocks are real predecessors, types
/// match), so this genuinely reaches the dominance witness, which rejects it.
#[test]
fn insert_phi_rejects_non_dominating_incoming() -> Result<(), IrError> {
    Module::with_new("insert-phi-nondom", |m| {
        let i32_ty = m.i32_type();
        let (f, lv, left_label, _rv, right_label) = build_diamond(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        // Feed %lv (defined in `left`) down BOTH edges. The `right` edge names a
        // value that does not dominate `right` -> the dominance witness fails.
        let pass = InsertMergePhi {
            merge_name: "merge",
            ty: i32_ty.as_type(),
            incomings: vec![(lv, left_label), (lv, right_label)],
        };
        let err = run_function_pass(pass, verified, f, &mut analyses)
            .err()
            .expect("insert_phi must reject a non-dominating incoming");
        assert!(
            matches!(err, IrError::PhiIncomingNotDominating { .. }),
            "expected PhiIncomingNotDominating, got: {err:?}"
        );
        Ok(())
    })
}

/// NEGATIVE (completeness): a `ReshapeCfg` pass inserts a phi with fewer
/// incomings than the merge block's predecessors — one incoming for the
/// 2-predecessor `merge`. The shared coherence check rejects the incomplete
/// phi before it is created (the count-mismatch arm, which the dominance
/// negative does not exercise).
#[test]
fn insert_phi_rejects_incomplete_incomings() -> Result<(), IrError> {
    Module::with_new("insert-phi-incomplete", |m| {
        let i32_ty = m.i32_type();
        let (f, lv, left_label, _rv, _right_label) = build_diamond(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        // Only one incoming for merge's two predecessors {left, right}.
        let pass = InsertMergePhi {
            merge_name: "merge",
            ty: i32_ty.as_type(),
            incomings: vec![(lv, left_label)],
        };
        let err = run_function_pass(pass, verified, f, &mut analyses)
            .err()
            .expect("insert_phi must reject an incomplete incoming set");
        // Assert on the rendered message rather than the exact variant so the
        // test survives a future refinement of the coherence-error mapping.
        let msg = err.to_string();
        assert!(
            msg.contains("predecessor"),
            "expected an incomplete-phi coherence error mentioning predecessors, got: {err:?}"
        );
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Typed terminator edge edits — `edit_switch`/`edit_cond_br`/`edit_br` narrows
// whose `redirect_*`/`remove_*` ops mechanically maintain successor phis.
//
// These ops perform terminator surgery on a `switch` (whose case list and
// default live behind interior mutability) and mechanically maintain the
// successor phis of every block they touch — the same "the op carries its own
// phi maintenance" contract as wave-1 `split_block`. Since `BranchInstData.kind`
// became interior-mutable, `br`/`cond_br` are edited too: `redirect_then`/
// `redirect` retarget a `cond_br`/`br` successor and `remove_then`/`remove_else`
// collapse a `cond_br` to a `br` (the `br`/`cond_br` tests are at the bottom of
// this file).
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that calls
/// `edit_switch(&from)?.redirect_successor(&old_to, &new_to, ..)`, retargeting
/// the `from_name` block's case edge from `old_to` to `new_to` and seeding
/// `new_to`'s leading phis with the stashed `phi_values`.
struct RedirectSwitchCase<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    old_to: BasicBlockLabel<'ctx, Dyn, B>,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
    phi_values: Vec<Value<'ctx, B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RedirectSwitchCase<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-switch-case";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.edit_switch(&from)?.redirect_successor(
            &self.old_to,
            &self.new_to,
            &self.phi_values,
        )?;
        Ok(reshape.done())
    }
}

/// Build a `switch` whose case-0 edge can be redirected, plus a `new` block
/// already carrying a phi, returning the function, the `old`/`new` `Dyn`
/// labels, and a value (`%ev`, defined in `entry`) to seed `new`'s phi.
///
/// ```text
/// entry(a): %ev = add %a, 3
///           switch i32 %a, label %dflt [ i32 0, label %old ]
/// dflt:     %nd = add %a, 5 ; br %new
/// old:      ret 0
/// new:      %np = phi i32 [ %nd, %dflt ] ; ret %np
/// ```
///
/// `redirect_edge(entry, old, new, [%ev])` retargets the case-0 edge onto `new`
/// and adds `[ %ev, %entry ]` to `new`'s phi.
#[allow(clippy::type_complexity)]
fn build_switch_redirect<'ctx>(
    m: &Module<'ctx, llvmkit_ir::Brand<'ctx>, llvmkit_ir::Unverified>,
) -> IrResult<(
    llvmkit_ir::FunctionValue<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
    Value<'ctx>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let dflt = f.append_basic_block(m, "dflt");
    let old = f.append_basic_block(m, "old");
    // new(%np: i32): the phi `[ %nd, %dflt ]` is authored as a block parameter
    // (head-phi), seeded by `dflt` branching to `new(%nd)`.
    let (new, new_params) =
        IRBuilder::new_for::<Dyn>(m).append_block_with_params(f, &[i32_ty.as_type()], "new")?;

    let dflt_lbl = dflt.label();
    let old_lbl = old.label();
    let new_lbl = new.label();
    let old_dyn: BasicBlockLabel<Dyn> = old_lbl.to_erased().try_into()?;
    let new_dyn: BasicBlockLabel<Dyn> = new_lbl.to_erased().try_into()?;

    // entry: %ev = add %a, 3 ; switch %a, default %dflt [ 0 -> old ]
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let ev = b.build_int_add(a, 3_i32, "ev")?;
    let (_sealed, sw) = b.build_switch_dyn(a, dflt_lbl, "")?;
    sw.add_case(i32_ty.const_int(0_u32), old_lbl)?.finish();

    // dflt: %nd = add %a, 5 ; br new(%nd)
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(dflt);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let nd = b.build_int_add(a, 5_i32, "nd")?;
    b.build_br_with_args(new_lbl, &[nd.into_erased()])?;

    // old: ret 0
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(old);
    b.build_ret(i32_ty.const_int(0_u32))?;

    // new: ret %np (the head-phi param carrying the dflt branch argument).
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(new);
    let np: IntValue<i32> = new_params[0].try_into()?;
    b.build_ret(np)?;

    Ok((f, old_dyn, new_dyn, ev.into_erased()))
}

/// `redirect_successor` retargets the `entry → old` switch case onto `new` AND
/// adds the supplied, typed value as `new`'s phi incoming from `entry`; the
/// output re-verifies.
#[test]
fn redirect_edge_retargets_and_seeds_new_phi() -> Result<(), IrError> {
    Module::with_new("redirect-edge", |m| {
        let (f, old_dyn, new_dyn, ev) = build_switch_redirect(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectSwitchCase {
            from_name: "entry",
            old_to: old_dyn,
            new_to: new_dyn,
            phi_values: vec![ev],
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        let reverified = out
            .verify()
            .expect("redirect_successor output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("[ %ev, %entry ]"),
            "new's phi must gain the supplied incoming from entry, got:\n{printed}"
        );
        assert!(
            printed.contains("i32 0, label %new"),
            "the switch case must now target new, got:\n{printed}"
        );
        assert!(
            !printed.contains("i32 0, label %old"),
            "the switch case must no longer target old, got:\n{printed}"
        );
        Ok(())
    })
}

/// `redirect_successor` rejects a `phi_values` slice whose length differs from
/// the new target's leading-phi count — witnessed at the call, not at `verify()`.
#[test]
fn redirect_edge_rejects_wrong_arity() -> Result<(), IrError> {
    Module::with_new("redirect-edge-arity", |m| {
        let (f, old_dyn, new_dyn, ev) = build_switch_redirect(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        // `new` has one leading phi; supplying two values is a wrong arity.
        let pass = RedirectSwitchCase {
            from_name: "entry",
            old_to: old_dyn,
            new_to: new_dyn,
            phi_values: vec![ev, ev],
        };
        let err = run_function_pass(pass, verified, f, &mut analyses)
            .err()
            .expect("redirect_successor must reject a wrong-arity phi_values");
        assert!(
            matches!(
                err,
                IrError::PhiArgArityMismatch {
                    expected: 1,
                    got: 2
                }
            ),
            "expected PhiArgArityMismatch {{ expected: 1, got: 2 }}, got: {err:?}"
        );
        Ok(())
    })
}

/// `redirect_successor` rejects a `phi_values` entry whose type differs from its
/// target phi — witnessed at the call, not at `verify()`.
#[test]
fn redirect_edge_rejects_wrong_type() -> Result<(), IrError> {
    Module::with_new("redirect-edge-type", |m| {
        let i64_ty = m.i64_type();
        let (f, old_dyn, new_dyn, _ev) = build_switch_redirect(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        // `new`'s phi is i32; an i64 constant is the wrong type for it.
        let wrong: Value = i64_ty.const_int(0_u32).into_erased();
        let pass = RedirectSwitchCase {
            from_name: "entry",
            old_to: old_dyn,
            new_to: new_dyn,
            phi_values: vec![wrong],
        };
        let err = run_function_pass(pass, verified, f, &mut analyses)
            .err()
            .expect("redirect_successor must reject a mistyped phi_values");
        assert!(
            matches!(err, IrError::TypeMismatch { .. }),
            "expected TypeMismatch, got: {err:?}"
        );
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// `br` / `cond_br` edge ops. `BranchInstData.kind` is now `RefCell<BranchKind>`,
// so the reshape mutator can retarget a branch successor (`edit_cond_br`/
// `edit_br` → `redirect_then`/`redirect`) or collapse a `cond_br` to a `br`
// (`remove_then`/`remove_else`) through its `&self` token. The removal ops are
// role-named, so which arm is dropped is chosen at the call site rather than
// inferred from a target — that is what makes dropping the sole edge of an
// unconditional `br` (or an `invoke`/`callbr` edge) a *compile* error, since no
// such method exists on `BrEdit`.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that retargets `from_name`'s `cond_br` then-arm onto
/// `new_to` via `edit_cond_br(&from)?.redirect_then(&new_to, ..)`, seeding
/// `new_to`'s leading phis with the stashed `phi_values`. Propagates the op's
/// error, so a rejected op surfaces as the pass's error.
struct RedirectCondBrThen<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
    phi_values: Vec<Value<'ctx, B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RedirectCondBrThen<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-condbr-then";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape
            .edit_cond_br(&from)?
            .redirect_then(&self.new_to, &self.phi_values)?;
        Ok(reshape.done())
    }
}

/// A `ReshapeCfg` pass that retargets `from_name`'s unconditional `br` onto
/// `new_to` via `edit_br(&from)?.redirect(&new_to, ..)`, seeding `new_to`'s
/// leading phis with the stashed `phi_values`.
struct RedirectBr<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
    phi_values: Vec<Value<'ctx, B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RedirectBr<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-br";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape
            .edit_br(&from)?
            .redirect(&self.new_to, &self.phi_values)?;
        Ok(reshape.done())
    }
}

/// A `ReshapeCfg` pass that removes `from_name`'s `cond_br` else-edge via
/// `edit_cond_br(&from)?.remove_else()`, collapsing the `cond_br` to a `br` to
/// the surviving then-arm.
struct RemoveCondBrElse {
    from_name: &'static str,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RemoveCondBrElse {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-condbr-else";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.edit_cond_br(&from)?.remove_else()?;
        Ok(reshape.done())
    }
}

/// A `ReshapeCfg` pass that removes `from_name`'s `cond_br` then-edge via
/// `edit_cond_br(&from)?.remove_then()`, collapsing the `cond_br` to a `br` to
/// the surviving else-arm.
struct RemoveCondBrThen {
    from_name: &'static str,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RemoveCondBrThen {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-condbr-then";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.edit_cond_br(&from)?.remove_then()?;
        Ok(reshape.done())
    }
}

/// `redirect_then` retargets the then-arm of a `cond_br` onto `new` and
/// seeds `new`'s head-phi with the supplied value; the output re-verifies.
#[test]
fn redirect_edge_retargets_a_cond_br_arm() -> Result<(), IrError> {
    Module::with_new("redirect-condbr", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let old = f.append_basic_block(&m, "old");
        let other = f.append_basic_block(&m, "other");
        // new(%np: i32): a head-phi block param seeded by the redirect.
        let (new, new_params) = IRBuilder::new_for::<Dyn>(&m).append_block_with_params(
            f,
            &[i32_ty.as_type()],
            "new",
        )?;
        let old_lbl = old.label();
        let other_lbl = other.label();
        let new_dyn: BasicBlockLabel<Dyn> = new.label().to_erased().try_into()?;

        // entry: %ev = add %a, 3 ; cond_br (%a == 0) ? old : other
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let ev = b.build_int_add(a, 3_i32, "ev")?;
        let c = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
        b.build_cond_br(c, old_lbl, other_lbl)?;

        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(old);
        b.build_ret(i32_ty.const_int(0_u32))?;
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(other);
        b.build_ret(i32_ty.const_int(1_u32))?;
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(new);
        let np: IntValue<i32> = new_params[0].try_into()?;
        b.build_ret(np)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectCondBrThen {
            from_name: "entry",
            new_to: new_dyn,
            phi_values: vec![ev.into_erased()],
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;
        let reverified = out.verify().expect("redirect_then output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("[ %ev, %entry ]"),
            "new's phi must gain the entry incoming, got:\n{printed}"
        );
        // Pin the WHOLE terminator: only the then-arm may be retargeted. An
        // impl that retargeted both arms would emit `label %new, label %new` and
        // still satisfy a bare `contains("label %new")` + `!contains("label
        // %old")`, so those two checks alone cannot catch it.
        assert!(
            printed.contains("br i1 %c, label %new, label %other"),
            "only the then-arm may be retargeted (else-arm must still be other), got:\n{printed}"
        );
        assert!(
            !printed.contains("label %old"),
            "no branch may still target old, got:\n{printed}"
        );
        Ok(())
    })
}

/// `remove_else` on a `cond_br` collapses it to an unconditional
/// `br` to the surviving then-target, drops the removed successor's
/// `from`-incomings, and deregisters the now-dead condition operand.
///
/// ```text
/// entry(a): %ev = add %a, 3 ; %c = icmp eq %a, 0
///           br %c ? keep() : drop(%ev)
/// keep:     %kv = add %a, 7 ; br drop(%kv)
/// drop(%dp): ret %dp          ; preds {entry, keep}
/// ```
///
/// Removing the else-arm (`drop`) must leave `entry: br label %keep`, `drop`'s
/// phi holding only `[ %kv, %keep ]`, and `%c` with no uses. `drop` keeps a
/// second predecessor (`keep`) on purpose, so the phi genuinely *loses one
/// incoming* rather than being emptied.
#[test]
fn remove_edge_collapses_cond_br_to_br() -> Result<(), IrError> {
    Module::with_new("remove-condbr", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let keep = f.append_basic_block(&m, "keep");
        // drop(%dp: i32): reached from BOTH entry and keep.
        let (drop_bb, drop_params) = IRBuilder::new_for::<Dyn>(&m).append_block_with_params(
            f,
            &[i32_ty.as_type()],
            "drop",
        )?;
        let keep_lbl = keep.label();
        let drop_lbl = drop_bb.label();

        // entry: %ev = add %a, 3 ; br (%a == 0) ? keep() : drop(%ev)
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let ev = b.build_int_add(a, 3_i32, "ev")?;
        let c = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
        b.build_cond_br_with_args(c, keep_lbl, &[], drop_lbl, &[ev.into_erased()])?;

        // keep: %kv = add %a, 7 ; br drop(%kv)
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(keep);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let kv = b.build_int_add(a, 7_i32, "kv")?;
        b.build_br_with_args(drop_lbl, &[kv.into_erased()])?;

        // drop: ret %dp
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(drop_bb);
        let dp: IntValue<i32> = drop_params[0].try_into()?;
        b.build_ret(dp)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RemoveCondBrElse { from_name: "entry" };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        // The condition is no longer an operand of the (now unconditional)
        // branch — the collapse deregistered it. Nothing in `verify()` checks
        // use-list consistency, so assert it directly.
        assert!(
            !c.into_erased().has_uses(),
            "the collapse must deregister the dead condition operand"
        );

        let reverified = out.verify().expect("collapsed output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("br label %keep"),
            "cond_br must collapse to `br label %keep`, got:\n{printed}"
        );
        assert!(
            !printed.contains("br i1"),
            "the conditional branch must be gone, got:\n{printed}"
        );
        assert!(
            !printed.contains("[ %ev, %entry ]"),
            "drop's phi must lose its entry incoming, got:\n{printed}"
        );
        assert!(
            printed.contains("[ %kv, %keep ]"),
            "drop's phi must keep its surviving (keep) incoming, got:\n{printed}"
        );
        Ok(())
    })
}

/// `redirect` retargets an unconditional `br` and seeds the new target's
/// head-phi; the output re-verifies.
#[test]
fn redirect_edge_retargets_an_unconditional_br() -> Result<(), IrError> {
    Module::with_new("redirect-br", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let old = f.append_basic_block(&m, "old");
        let (new, new_params) = IRBuilder::new_for::<Dyn>(&m).append_block_with_params(
            f,
            &[i32_ty.as_type()],
            "new",
        )?;
        let old_lbl = old.label();
        let new_dyn: BasicBlockLabel<Dyn> = new.label().to_erased().try_into()?;

        // entry: %ev = add %a, 3 ; br old
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let ev = b.build_int_add(a, 3_i32, "ev")?;
        b.build_br(old_lbl)?;
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(old);
        b.build_ret(i32_ty.const_int(0_u32))?;
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(new);
        let np: IntValue<i32> = new_params[0].try_into()?;
        b.build_ret(np)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectBr {
            from_name: "entry",
            new_to: new_dyn,
            phi_values: vec![ev.into_erased()],
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;
        let reverified = out.verify().expect("redirect output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("br label %new"),
            "the br must now target new, got:\n{printed}"
        );
        assert!(
            printed.contains("[ %ev, %entry ]"),
            "new's phi must gain the entry incoming, got:\n{printed}"
        );
        Ok(())
    })
}

// Removing the sole edge of an unconditional `br` is no longer a *runtime*
// rejection: `edit_br` yields a `BrEdit`, which carries only `redirect` — there
// is no `remove_*` method to call, so the edit is a *compile* error (`E0599 no
// method`) rather than an `IrError`. The old runtime-rejection test is therefore
// gone; the compile-time guarantee is covered by the typed-edit compile-fail
// fixtures.

/// Builds `entry: br %c ? t : e` with the two arms pointed at `t_name`/`e_name`,
/// plus a spare `new` block — the shared skeleton for the branch-edge rejection
/// guards below. Returns the function and the `Dyn` labels for `old`/`new`.
#[allow(clippy::type_complexity)]
fn build_cond_br_pair<'ctx>(
    m: &Module<'ctx, llvmkit_ir::Brand<'ctx>, llvmkit_ir::Unverified>,
    then_is_new: bool,
) -> IrResult<(
    llvmkit_ir::FunctionValue<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let old = f.append_basic_block(m, "old");
    let new = f.append_basic_block(m, "new");
    let old_lbl = old.label();
    let new_lbl = new.label();
    let old_dyn: BasicBlockLabel<Dyn> = old_lbl.to_erased().try_into()?;
    let new_dyn: BasicBlockLabel<Dyn> = new_lbl.to_erased().try_into()?;

    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let c = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
    if then_is_new {
        // entry: br %c ? old : new  — the else-arm ALREADY reaches `new`.
        b.build_cond_br(c, old_lbl, new_lbl)?;
    } else {
        // entry: br %c ? old : old  — BOTH arms reach `old`.
        b.build_cond_br(c, old_lbl, old_lbl)?;
    }
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(old);
    b.build_ret(i32_ty.const_int(0_u32))?;
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(new);
    b.build_ret(i32_ty.const_int(1_u32))?;

    Ok((f, old_dyn, new_dyn))
}

/// SEMANTIC CHANGE (former error, now valid): removing the then-arm of a
/// `cond_br` whose BOTH arms target the same block collapses it to `br old`.
/// The old dynamic `remove_edge(from, to)` rejected this as ambiguous ("both
/// arms target `to`; removing it would leave no successor"), but the role-named
/// `remove_then` is unambiguous even when `then == else == old` — one arm is
/// named, the survivor (`else`, also `old`) remains — so there is nothing to
/// reject. This is a positive test of that now-valid collapse.
#[test]
fn remove_then_on_cond_br_with_both_arms_to_same_collapses() -> Result<(), IrError> {
    Module::with_new("remove-condbr-both", |m| {
        let (f, _old_dyn, _new_dyn) = build_cond_br_pair(&m, false)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RemoveCondBrThen { from_name: "entry" };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;
        let reverified = out.verify().expect("collapse output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("br label %old"),
            "both-arms-to-old cond_br must collapse to `br label %old`, got:\n{printed}"
        );
        assert!(
            !printed.contains("br i1"),
            "the conditional branch must be gone after the collapse, got:\n{printed}"
        );
        Ok(())
    })
}

/// SEMANTIC CHANGE (former error, now valid): redirecting the then-arm of a
/// `cond_br` whose BOTH arms target `old` retargets exactly that one edge,
/// leaving `br %c ? new : old`. The old dynamic `redirect_edge(from, old, new)`
/// rejected the two-edge `old` as ambiguous ("reaches `old_to` through multiple
/// edges"); the role-named `redirect_then` names the arm, so one edge is
/// retargeted unambiguously. This is a positive test of that now-valid redirect.
#[test]
fn redirect_then_on_cond_br_with_both_arms_to_same_retargets_one() -> Result<(), IrError> {
    Module::with_new("redirect-condbr-multi", |m| {
        let (f, _old_dyn, new_dyn) = build_cond_br_pair(&m, false)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectCondBrThen {
            from_name: "entry",
            new_to: new_dyn,
            phi_values: vec![],
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;
        let reverified = out.verify().expect("redirect_then output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("br i1 %c, label %new, label %old"),
            "only the then-arm may be retargeted (else-arm stays old), got:\n{printed}"
        );
        Ok(())
    })
}

/// `redirect_then` rejects a `cond_br` whose OTHER (else) arm already targets
/// `new_to` — redirect mints a fresh edge, so the new target's phis would
/// otherwise gain a second incoming from the same predecessor.
#[test]
fn redirect_edge_rejects_cond_br_already_reaching_new() -> Result<(), IrError> {
    Module::with_new("redirect-condbr-already", |m| {
        let (f, _old_dyn, new_dyn) = build_cond_br_pair(&m, true)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectCondBrThen {
            from_name: "entry",
            new_to: new_dyn,
            phi_values: vec![],
        };
        let err = run_function_pass(pass, verified, f, &mut analyses)
            .err()
            .expect("an already-reached new_to must be rejected");
        assert!(
            matches!(err, IrError::InvalidOperation { message } if message.contains("already reaches")),
            "expected InvalidOperation about already reaching new_to, got: {err:?}"
        );
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Surviving-parallel-edge phi maintenance on `cond_br %c, X, X`.
//
// The role-named `remove_then`/`redirect_then` reach a case the old
// target-based `remove_edge`/`redirect_edge` rejected: editing ONE arm of a
// `cond_br` whose BOTH arms target the same block `X`. After the edit `from`
// STILL reaches `X` through the untouched parallel arm, so `X`'s phi must retain
// exactly one `from` incoming — dropping all of them (the correct behavior only
// when `from` fully stops reaching `X`) under-counts and makes `verify()` report
// `PhiPredecessorMismatch`. The `build_cond_br_pair`-based tests above pass only
// because their shared target has no phi; these give it one.
// ---------------------------------------------------------------------------

/// Build a `cond_br %c, shared, shared` (both arms → `shared`) whose target
/// carries a head phi, with a third predecessor `keep` so `shared` has three
/// predecessors: `src` (×2, via the parallel arms) and `keep` (×1). Editing one
/// arm of `src`'s branch must leave `shared` reachable from `src` through the
/// surviving arm, so `shared`'s phi must retain exactly one `src` incoming.
///
/// ```text
/// entry(a): %c0 = icmp eq %a, 0 ; cond_br %c0, src, keep
/// src(a):   %sv = add %a, 3 ; %c1 = icmp eq %a, 1
///           cond_br %c1, shared(%sv), shared(%sv)   ; BOTH arms → shared
/// keep(a):  %kv = add %a, 7 ; br shared(%kv)
/// shared(%sp): ret %sp                              ; preds {src, src, keep}
/// ```
///
/// Returns the function plus `new`'s `Dyn` label (a spare, phi-less block for the
/// redirect test — a redirect onto it seeds an empty `phi_values`).
#[allow(clippy::type_complexity)]
fn build_cond_br_both_arms_phi<'ctx>(
    m: &Module<'ctx, llvmkit_ir::Brand<'ctx>, llvmkit_ir::Unverified>,
) -> IrResult<(
    llvmkit_ir::FunctionValue<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let src = f.append_basic_block(m, "src");
    let keep = f.append_basic_block(m, "keep");
    let new = f.append_basic_block(m, "new");
    // shared(%sp: i32): reached from `src` via BOTH cond_br arms and from `keep`.
    let (shared, shared_params) =
        IRBuilder::new_for::<Dyn>(m).append_block_with_params(f, &[i32_ty.as_type()], "shared")?;
    let src_lbl = src.label();
    let keep_lbl = keep.label();
    let shared_lbl = shared.label();
    let new_dyn: BasicBlockLabel<Dyn> = new.label().to_erased().try_into()?;

    // entry: cond_br (%a == 0) ? src : keep
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let c0 = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c0")?;
    b.build_cond_br(c0, src_lbl, keep_lbl)?;

    // src: %sv = add %a, 3 ; cond_br (%a == 1) ? shared(%sv) : shared(%sv)
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(src);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let sv = b.build_int_add(a, 3_i32, "sv")?;
    let c1 = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 1_i32, "c1")?;
    b.build_cond_br_with_args(
        c1,
        shared_lbl,
        &[sv.into_erased()],
        shared_lbl,
        &[sv.into_erased()],
    )?;

    // keep: %kv = add %a, 7 ; br shared(%kv)
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(keep);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let kv = b.build_int_add(a, 7_i32, "kv")?;
    b.build_br_with_args(shared_lbl, &[kv.into_erased()])?;

    // shared: ret %sp
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(shared);
    let sp: IntValue<i32> = shared_params[0].try_into()?;
    b.build_ret(sp)?;

    // new: ret 1  (no phi — a redirect onto it seeds an empty phi_values)
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(new);
    b.build_ret(i32_ty.const_int(1_u32))?;

    Ok((f, new_dyn))
}

/// SURVIVING-PARALLEL-EDGE (remove): `remove_then` on a `cond_br %c, shared,
/// shared` collapses it to `br shared`, so `src` STILL reaches `shared` through
/// the surviving (else) arm. `shared`'s phi must therefore KEEP one `src`
/// incoming (alongside its `keep` incoming), not drop both.
///
/// Before the fix, `remove_slot` dropped every `src` incoming unconditionally,
/// leaving `shared`'s phi with only `[ %kv, %keep ]` (1 entry) against 2
/// predecessors — `verify()` fails with `PhiPredecessorMismatch` ("phi has 1
/// incoming entries but block has 2 predecessors").
#[test]
fn remove_then_keeps_surviving_parallel_edge_phi_incoming() -> Result<(), IrError> {
    Module::with_new("remove-parallel-phi", |m| {
        let (f, _new_dyn) = build_cond_br_both_arms_phi(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RemoveCondBrThen { from_name: "src" };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        let reverified = out
            .verify()
            .expect("collapse must re-verify: shared keeps one src incoming for the surviving arm");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("br label %shared"),
            "both-arms cond_br must collapse to `br label %shared`, got:\n{printed}"
        );
        // shared retains exactly one `src` incoming (surviving arm) plus its
        // `keep` incoming: two entries for two predecessors.
        assert!(
            printed.contains("[ %sv, %src ]"),
            "shared's phi must keep one src incoming for the surviving arm, got:\n{printed}"
        );
        assert!(
            printed.contains("[ %kv, %keep ]"),
            "shared's phi must keep its keep incoming, got:\n{printed}"
        );
        Ok(())
    })
}

/// SURVIVING-PARALLEL-EDGE (redirect): `redirect_then` on a `cond_br %c, shared,
/// shared` retargets only the then-arm onto `new`, leaving the else-arm at
/// `shared`. `src` STILL reaches `shared` through the surviving else-arm, so
/// `shared`'s phi must KEEP one `src` incoming.
///
/// Before the fix, the redirect dropped every `src` incoming from `shared`,
/// leaving `[ %kv, %keep ]` (1 entry) against 2 predecessors — `verify()` fails
/// with `PhiPredecessorMismatch`.
#[test]
fn redirect_then_keeps_surviving_parallel_edge_phi_incoming() -> Result<(), IrError> {
    Module::with_new("redirect-parallel-phi", |m| {
        let (f, new_dyn) = build_cond_br_both_arms_phi(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectCondBrThen {
            from_name: "src",
            new_to: new_dyn,
            phi_values: vec![],
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        let reverified = out
            .verify()
            .expect("redirect must re-verify: shared keeps one src incoming for the surviving arm");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("br i1 %c1, label %new, label %shared"),
            "only the then-arm may be retargeted (else-arm stays shared), got:\n{printed}"
        );
        assert!(
            printed.contains("[ %sv, %src ]"),
            "shared's phi must keep one src incoming for the surviving arm, got:\n{printed}"
        );
        assert!(
            printed.contains("[ %kv, %keep ]"),
            "shared's phi must keep its keep incoming, got:\n{printed}"
        );
        Ok(())
    })
}
