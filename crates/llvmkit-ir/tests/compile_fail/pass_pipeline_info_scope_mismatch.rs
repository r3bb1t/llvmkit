use llvmkit_ir::{
    FunctionPassManager, IrResult, ModulePassScope, PassName, PassPipelineInfo, PreservedAnalyses,
    PreservesVerification, ReadOnlyFunctionPass, ReadOnlyFunctionPassContext,
};

struct WrongScopeFunctionPass;

impl<'ctx> ReadOnlyFunctionPass<'ctx> for WrongScopeFunctionPass {
    fn run(
        &mut self,
        _cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx>,
    ) -> IrResult<PreservedAnalyses> {
        Ok(PreservedAnalyses::all())
    }
}

impl PassPipelineInfo for WrongScopeFunctionPass {
    type Scope = ModulePassScope;
    const PIPELINE_NAME: PassName<Self::Scope> =
        panic!("the fixture must fail before evaluating this module-pass name");
}

fn main() {
    let mut fpm = FunctionPassManager::<_, PreservesVerification>::new_read_only();
    fpm.add_pipeline_pass(WrongScopeFunctionPass);
}
