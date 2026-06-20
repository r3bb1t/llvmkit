use llvmkit_ir::{
    FunctionAnalysisManager, IrResult, Module, ModuleAnalysisManager, ModuleBrand, ModulePass,
    ModulePassContext, ModulePassManager, MutatesIr, PreservedAnalyses,
};

struct MutatingPass;

impl<'ctx, B: ModuleBrand + 'ctx> ModulePass<'ctx, B> for MutatingPass {
    fn run(&mut self, cx: &mut ModulePassContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses> {
        cx.module_mut().append_module_asm("side effect");
        Ok(PreservedAnalyses::none())
    }
}

fn main() {
    Module::with_new::<_, _, _>("unverified-output", |module| {
        let verified = module.verify().unwrap();
        let mut mpm = ModulePassManager::<_, MutatesIr>::new_transform();
        mpm.add_pass(MutatingPass);
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        let unverified = mpm.run(verified, &mut mam, &mut fam).unwrap();

        let _ = mpm.run(unverified, &mut mam, &mut fam);
    });
}
