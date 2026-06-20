use llvmkit_ir::{
    IrResult, ModulePass, ModulePassContext, ModulePassManager, PreservedAnalyses,
    PreservesVerification,
};

struct Mutates;

impl<'ctx> ModulePass<'ctx> for Mutates {
    fn run(&mut self, cx: &mut ModulePassContext<'_, 'ctx>) -> IrResult<PreservedAnalyses> {
        cx.module_mut().append_module_asm("side effect");
        Ok(PreservedAnalyses::none())
    }
}

fn main() {
    let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
    mpm.add_pass(Mutates);
}
