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
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let next = f.append_basic_block(&m, "next");
        let next_label = next.label();

        // entry: br next    next: ret 0
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_br(next.label())?;
        let b2 = IRBuilder::new_for::<i32>(&m).position_at_end(next);
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
        let f = m.add_function::<Dyn, _>("f", fn_ty, Linkage::External)?;
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
        b.build_br_with_args(merge_label, &[x.as_value()])?;

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
                let b = IRBuilder::new(reshape.module_mut()).restore_insert_point(ip)?;
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
    llvmkit_ir::FunctionValue<'ctx, i32>,
    Value<'ctx>,
    BasicBlockLabel<'ctx, Dyn>,
    Value<'ctx>,
    BasicBlockLabel<'ctx, Dyn>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let left = f.append_basic_block(m, "left");
    let right = f.append_basic_block(m, "right");
    let merge = f.append_basic_block(m, "merge");
    // Erase the arm labels to `Dyn` for the `insert_phi` incoming slice (whose
    // pred labels are `Dyn`); the diamond's return marker is `i32`, so
    // `label()` alone would hand back `i32`-marked labels.
    let left_label: BasicBlockLabel<Dyn> = left.label().as_value().try_into()?;
    let right_label: BasicBlockLabel<Dyn> = right.label().as_value().try_into()?;

    // entry: br (%a == 0) ? left : right
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let cond = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
    b.build_cond_br(cond, &left, &right)?;

    // left: %lv = add %a, 10 ; br merge
    let b = IRBuilder::new_for::<i32>(m).position_at_end(left);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let lv = b.build_int_add(a, 10_i32, "lv")?;
    b.build_br(merge.label())?;

    // right: %rv = add %a, 20 ; br merge
    let b = IRBuilder::new_for::<i32>(m).position_at_end(right);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let rv = b.build_int_add(a, 20_i32, "rv")?;
    b.build_br(merge.label())?;

    // merge: ret 0   (no phi yet — the pass inserts one)
    let b = IRBuilder::new_for::<i32>(m).position_at_end(merge);
    b.build_ret(i32_ty.const_int(0_u32))?;

    Ok((f, lv.as_value(), left_label, rv.as_value(), right_label))
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
// `FnReshape::remove_edge` / `redirect_edge` — edge ops that mechanically
// maintain successor phis.
//
// These ops perform terminator surgery on a `switch` (whose case list and
// default live behind interior mutability) and mechanically maintain the
// successor phis of every block they touch — the same "the op carries its own
// phi maintenance" contract as wave-1 `split_block`. `br`/`cond_br` are not
// edited (their target payload is not interior-mutable), so the fixtures below
// build `switch`-terminated blocks.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that calls [`FnReshape::redirect_edge`], retargeting the
/// `from_name` block's edge from `old_to` to `new_to` and seeding `new_to`'s
/// leading phis with the stashed `phi_values`.
struct RedirectSwitchEdge<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    old_to: BasicBlockLabel<'ctx, Dyn, B>,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
    phi_values: Vec<Value<'ctx, B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RedirectSwitchEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-switch-edge";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.redirect_edge(&from, &self.old_to, &self.new_to, &self.phi_values)?;
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
    llvmkit_ir::FunctionValue<'ctx, i32>,
    BasicBlockLabel<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
    Value<'ctx>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let dflt = f.append_basic_block(m, "dflt");
    let old = f.append_basic_block(m, "old");
    // new(%np: i32): the phi `[ %nd, %dflt ]` is authored as a block parameter
    // (head-phi), seeded by `dflt` branching to `new(%nd)`.
    let (new, new_params) =
        IRBuilder::new_for::<i32>(m).append_block_with_params(f, &[i32_ty.as_type()], "new")?;

    let dflt_lbl = dflt.label();
    let old_lbl = old.label();
    let new_lbl = new.label();
    let old_dyn: BasicBlockLabel<Dyn> = old_lbl.as_value().try_into()?;
    let new_dyn: BasicBlockLabel<Dyn> = new_lbl.as_value().try_into()?;

    // entry: %ev = add %a, 3 ; switch %a, default %dflt [ 0 -> old ]
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let ev = b.build_int_add(a, 3_i32, "ev")?;
    let (_sealed, sw) = b.build_switch(a, dflt_lbl, "")?;
    sw.add_case(i32_ty.const_int(0_u32), old_lbl)?.finish();

    // dflt: %nd = add %a, 5 ; br new(%nd)
    let b = IRBuilder::new_for::<i32>(m).position_at_end(dflt);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let nd = b.build_int_add(a, 5_i32, "nd")?;
    b.build_br_with_args(new_lbl, &[nd.as_value()])?;

    // old: ret 0
    let b = IRBuilder::new_for::<i32>(m).position_at_end(old);
    b.build_ret(i32_ty.const_int(0_u32))?;

    // new: ret %np (the head-phi param carrying the dflt branch argument).
    let b = IRBuilder::new_for::<i32>(m).position_at_end(new);
    let np: IntValue<i32> = new_params[0].try_into()?;
    b.build_ret(np)?;

    Ok((f, old_dyn, new_dyn, ev.as_value()))
}

/// `redirect_edge` retargets the `entry → old` switch case onto `new` AND adds
/// the supplied, typed value as `new`'s phi incoming from `entry`; the output
/// re-verifies.
#[test]
fn redirect_edge_retargets_and_seeds_new_phi() -> Result<(), IrError> {
    Module::with_new("redirect-edge", |m| {
        let (f, old_dyn, new_dyn, ev) = build_switch_redirect(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectSwitchEdge {
            from_name: "entry",
            old_to: old_dyn,
            new_to: new_dyn,
            phi_values: vec![ev],
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        let reverified = out.verify().expect("redirect_edge output must re-verify");
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

/// `redirect_edge` rejects a `phi_values` slice whose length differs from the
/// new target's leading-phi count — witnessed at the call, not at `verify()`.
#[test]
fn redirect_edge_rejects_wrong_arity() -> Result<(), IrError> {
    Module::with_new("redirect-edge-arity", |m| {
        let (f, old_dyn, new_dyn, ev) = build_switch_redirect(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        // `new` has one leading phi; supplying two values is a wrong arity.
        let pass = RedirectSwitchEdge {
            from_name: "entry",
            old_to: old_dyn,
            new_to: new_dyn,
            phi_values: vec![ev, ev],
        };
        let err = run_function_pass(pass, verified, f, &mut analyses)
            .err()
            .expect("redirect_edge must reject a wrong-arity phi_values");
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

/// `redirect_edge` rejects a `phi_values` entry whose type differs from its
/// target phi — witnessed at the call, not at `verify()`.
#[test]
fn redirect_edge_rejects_wrong_type() -> Result<(), IrError> {
    Module::with_new("redirect-edge-type", |m| {
        let i64_ty = m.i64_type();
        let (f, old_dyn, new_dyn, _ev) = build_switch_redirect(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        // `new`'s phi is i32; an i64 constant is the wrong type for it.
        let wrong: Value = i64_ty.const_int(0_u32).as_value();
        let pass = RedirectSwitchEdge {
            from_name: "entry",
            old_to: old_dyn,
            new_to: new_dyn,
            phi_values: vec![wrong],
        };
        let err = run_function_pass(pass, verified, f, &mut analyses)
            .err()
            .expect("redirect_edge must reject a mistyped phi_values");
        assert!(
            matches!(err, IrError::TypeMismatch { .. }),
            "expected TypeMismatch, got: {err:?}"
        );
        Ok(())
    })
}
