use llvmkit_ir::{
    FunctionAnalysisManager, Module, ModuleAnalysisManager, ModulePassManager, MutatesIr,
};

fn main() {
    Module::with_new::<_, _, _>("unverified", |module| {
        let mut mpm = ModulePassManager::<_, MutatesIr>::new_transform();
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();

        let _ = mpm.run(module, &mut mam, &mut fam);
    });
}
