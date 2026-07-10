//! llvmkit-specific Pass API v2 lock (Doctrine D1 — make invalid states
//! unrepresentable), Package 4.
//!
//! On a `ReshapeCfg` mutator a CFG analysis can be read only through
//! `analysis_repaired`, whose returned reference is tied to the `&mut self`
//! receiver. Holding that reference across a later structural edit
//! (`split_block`, which borrows the mutator again) is a borrow-check error, so
//! the classic mid-reshape stale read — read a dominator tree, reshape the CFG,
//! then use the now-stale tree — cannot be written down. `FnReshape`
//! deliberately does not `Deref` to `FnPatch`, so the `&'r`-borrowed
//! `FnPatch::analysis` (which *would* outlive the edit) is not reachable here.

use llvmkit_ir::{
    DominatorTreeAnalysis, FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, ReshapeCfg,
};

struct StaleRead;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for StaleRead {
    type Access = ReshapeCfg;
    type Requires = (DominatorTreeAnalysis,);
    const NAME: &'static str = "stale-read";

    fn run(
        &mut self,
        cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, (DominatorTreeAnalysis,)>,
    ) -> IrResult<FnReport> {
        let mut reshape = cx.mutate();
        let entry = reshape.function().entry_block().expect("definition");
        let before = entry.instructions().next().expect("an instruction");

        // Read a CFG analysis mid-reshape (reference tied to `&mut reshape`)...
        let dt = reshape.analysis_repaired::<DominatorTreeAnalysis, _>();
        // ...then reshape the CFG while still holding it: `reshape` is already
        // mutably borrowed by `dt`, so this cannot compile.
        let _ = reshape.split_block(&entry, &before, "split");
        // The stale use that keeps `dt` live across the edit.
        let _ = dt.is_reachable_from_entry(entry);

        Ok(reshape.done())
    }
}

fn main() {}
