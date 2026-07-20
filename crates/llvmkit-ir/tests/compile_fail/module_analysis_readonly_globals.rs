use llvmkit_ir::{
    IrResult, Linkage, ModuleAnalysis, ModuleAnalysisInvalidator, ModuleAnalysisManager,
    ModuleAnalysisResult, ModuleBrand, ModuleView, PreservedAnalyses,
};

struct MutatingGlobalAnalysis;
struct MutatingGlobalResult;

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysis<'ctx, B> for MutatingGlobalAnalysis {
    type Result = MutatingGlobalResult;

    fn run(
        &self,
        module: ModuleView<'ctx, B>,
        _am: &mut ModuleAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result> {
        if let Some(global) = module.globals().next() {
            global.set_linkage(Linkage::Internal);
        }
        Ok(MutatingGlobalResult)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysisResult<'ctx, B> for MutatingGlobalResult {
    fn invalidate(
        &mut self,
        _module: ModuleView<'ctx, B>,
        _pa: &PreservedAnalyses,
        _inv: &mut ModuleAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        Ok(false)
    }
}

fn main() {}
