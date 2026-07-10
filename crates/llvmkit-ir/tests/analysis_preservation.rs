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
    Analyses, DominatorTree, DominatorTreeAnalysis, FnCx, FnReport, FunctionPass, FunctionView,
    IRBuilder, IrError, IrResult, Linkage, Module, ModuleBrand, ReshapeCfg, Type,
    run_function_pass,
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
