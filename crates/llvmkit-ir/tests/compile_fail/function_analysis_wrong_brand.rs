use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, Linkage, Module, ModuleBrand, ModuleView,
};

fn manager_for<'ctx, B: ModuleBrand + 'ctx>(
    _module: ModuleView<'ctx, B>,
) -> FunctionAnalysisManager<'ctx, B> {
    FunctionAnalysisManager::new()
}

fn main() {
    Module::with_new::<_, _, _>("left", |left| {
        let left_function = left
            .add_typed_function::<(), (), _>("left", Linkage::External)
            .unwrap()
            .as_function();

        Module::with_new::<_, _, _>("right", |right| {
            let mut fam = manager_for(right.as_view());
            let _ = fam.get_result::<DominatorTreeAnalysis, _>(left_function.as_view());
        });
    });
}
