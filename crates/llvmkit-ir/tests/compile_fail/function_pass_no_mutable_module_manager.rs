use llvmkit_ir::{FunctionPass, FunctionPassContext, IrResult, PreservedAnalyses};

struct BadFunctionPass;

impl<'ctx> FunctionPass<'ctx> for BadFunctionPass {
    fn run(&mut self, cx: &mut FunctionPassContext<'_, 'ctx>) -> IrResult<PreservedAnalyses> {
        let _ = cx.module_analysis_manager_mut();
        Ok(PreservedAnalyses::all())
    }
}

fn main() {}
