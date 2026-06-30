use llvmkit_ir::{
    INSTCOMBINE, IrResult, ModulePassManager, PassName, PreservedAnalyses,
    PreservesVerification, ReadOnlyModulePass, ReadOnlyModulePassContext,
};

struct ModuleReportPass;

impl<'ctx> ReadOnlyModulePass<'ctx> for ModuleReportPass {
    fn run(
        &mut self,
        _cx: &mut ReadOnlyModulePassContext<'_, 'ctx>,
    ) -> IrResult<PreservedAnalyses> {
        Ok(PreservedAnalyses::all())
    }
}

fn main() {
    let _typed_name: PassName<llvmkit_ir::FunctionPassScope> = INSTCOMBINE;
    let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
    mpm.add_named_pass(INSTCOMBINE, ModuleReportPass);
}
