use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, Linkage, Module, ModuleBrand, ModuleView, Type,
};

fn manager_for<'ctx, B: ModuleBrand + 'ctx>(
    _module: ModuleView<'ctx, B>,
) -> FunctionAnalysisManager<'ctx, B> {
    FunctionAnalysisManager::new()
}

fn main() {
    Module::with_new::<_, _, _>("left", |left| {
        let void_ty = left.void_type();
        let params = Vec::<Type<'_, _>>::new();
        let fn_ty = left.fn_type(void_ty.as_type(), params, false);
        let left_function = left.add_function::<(), _>("left", fn_ty, Linkage::External).unwrap();

        Module::with_new::<_, _, _>("right", |right| {
            let mut fam = manager_for(right.as_view());
            let _ = fam.get_result::<DominatorTreeAnalysis, _>(left_function.as_view());
        });
    });
}
