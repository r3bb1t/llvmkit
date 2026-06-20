use llvmkit_ir::{
    FunctionAnalysisManager, IrResult, Module, ModuleAnalysisManager, ModulePass,
    ModulePassContext, ModulePassManager, MutatesIr, PreservedAnalyses,
};

struct TouchesModule;

impl<'ctx> ModulePass<'ctx> for TouchesModule {
    fn run(&mut self, cx: &mut ModulePassContext<'_, 'ctx>) -> IrResult<PreservedAnalyses> {
        cx.module_mut().append_module_asm("side effect");
        Ok(PreservedAnalyses::none())
    }
}

fn main() -> IrResult<()> {
    Module::with_new::<_, _, _>("transform-output", |module| {
        let verified = module.verify()?;
        let mut mpm = ModulePassManager::<_, MutatesIr>::new_transform();
        mpm.add_pass(TouchesModule);
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        let unverified = mpm.run(verified, &mut mam, &mut fam)?;
        mpm.run(unverified, &mut mam, &mut fam)?;
        Ok(())
    })
}
