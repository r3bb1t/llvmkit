use llvmkit_ir::{
    FunctionPass, FunctionPassContext, IrResult, ModuleBrand, PreservedAnalyses,
};

struct BadFunctionPass;

impl<'ctx, B: ModuleBrand> FunctionPass<'ctx, B> for BadFunctionPass {
    fn run(&mut self, cx: &mut FunctionPassContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses> {
        let _ = cx.module_analysis_manager_mut();
        Ok(PreservedAnalyses::all())
    }
}

fn main() {}
