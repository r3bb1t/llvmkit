//! An analysis *observes*; it never edits. `ModuleAnalysis::run` is handed a
//! read-only [`ModuleView`], and the mutating setters live only on the
//! `Unverified`-module surface a pass rung holds — so "an analysis quietly
//! rewrote the IR it was asked to describe", the classic source of stale
//! analysis results, is a type error rather than a convention.
//!
//! The signatures below deliberately mirror the trait's own: the view region
//! `'v` is the caller's, distinct from the manager's `'ctx`. Keeping them in
//! step is what makes the single error this fixture locks be `set_linkage`
//! missing on a read-only view, rather than an incidental signature mismatch.

use llvmkit_ir::{
    IrResult, Linkage, ModuleAnalysis, ModuleAnalysisInvalidator, ModuleAnalysisManager,
    ModuleAnalysisResult, ModuleBrand, ModuleView, PreservedAnalyses,
};

struct MutatingGlobalAnalysis;
struct MutatingGlobalResult;

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysis<'ctx, B> for MutatingGlobalAnalysis {
    type Result = MutatingGlobalResult;

    fn run<'v>(
        &self,
        module: ModuleView<'v, B>,
        _am: &mut ModuleAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result>
    where
        'ctx: 'v,
    {
        if let Some(global) = module.globals().next() {
            global.set_linkage(Linkage::Internal);
        }
        Ok(MutatingGlobalResult)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysisResult<'ctx, B> for MutatingGlobalResult {
    fn invalidate<'v>(
        &mut self,
        _module: ModuleView<'v, B>,
        _pa: &PreservedAnalyses,
        _inv: &mut ModuleAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool>
    where
        'ctx: 'v,
    {
        Ok(false)
    }
}

fn main() {}
