use llvmkit_ir::{FunctionAnalysisManager, FunctionPassManager, Linkage, Module, MutatesIr, Type};

fn main() {
    Module::with_new::<_, _, _>("unverified-function-pass", |module| {
        let i32_ty = module.i32_type();
        let fn_ty = module.fn_type(i32_ty.as_type(), Vec::<Type>::new(), false);
        let function = module
            .add_function::<i32>("f", fn_ty, Linkage::External)
            .unwrap();
        let mut fpm = FunctionPassManager::<_, MutatesIr>::new_transform();
        let mut fam = FunctionAnalysisManager::new();

        let _ = fpm.run(&module, function, &mut fam);
    });
}
