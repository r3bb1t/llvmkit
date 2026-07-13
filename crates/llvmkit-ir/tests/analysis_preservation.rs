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
    Analyses, DominatorTree, DominatorTreeAnalysis, Dyn, FnCx, FnReport, FunctionPass,
    FunctionView, IRBuilder, InsertPoint, IntValue, IrError, IrResult, Linkage, Module,
    ModuleBrand, ReshapeCfg, Type, run_function_pass,
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
        let merge = f.append_basic_block(&m, "merge");
        let entry_label = entry.label();
        let merge_label = merge.label();

        // entry: %x = add %a, 1 ; br %merge
        let b = IRBuilder::new(&m).position_at_end(entry);
        // Saved while `entry` is genuinely open (before-of-none == end of
        // block) so the pass can reopen `entry` after the split empties it.
        let ip = b.save_insert_point();
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let x = b.build_int_add(a, 1_i32, "x")?;
        b.build_br(merge_label)?;

        // merge: %p = phi i32 [ %x, %entry ] ; ret %p
        let b2 = IRBuilder::new(&m).position_at_end(merge);
        let phi = b2
            .build_int_phi::<i32, _>("p")?
            .add_incoming(x, entry_label)?;
        b2.build_ret(phi.as_int_value())?;

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
