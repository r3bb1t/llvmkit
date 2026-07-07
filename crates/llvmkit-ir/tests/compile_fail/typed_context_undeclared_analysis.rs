//! llvmkit-specific compile-fail (Doctrine D1, D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `AnalysisManager::getResult` in
//! `llvm/include/llvm/IR/PassManager.h` computes (or fetches a cached) result
//! for any analysis ID a pass asks for at runtime -- there is no static list
//! of which analyses a given pass is allowed to query. llvmkit's typed
//! function-pass context (`TypedFunctionPassContext::analysis`,
//! `pass_context.rs`) instead ties analysis access to the pass's own
//! `type Requires` tuple via the sealed `AnalysisSelector` trait
//! (`analysis.rs`): a pass that declares `Requires = ()` has no
//! `AnalysisSelector<DominatorTreeAnalysis, _>` impl for `()`, so calling
//! `cx.analysis::<DominatorTreeAnalysis, _>()` from such a pass is
//! unspellable at compile time rather than a runtime "analysis not
//! registered" error.

use llvmkit_ir::{
    DominatorTreeAnalysis, IrError, ModuleBrand, PreservedAnalyses, TypedFunctionPass,
    TypedFunctionPassContext,
};

struct BadPass;

impl<'ctx, B: ModuleBrand + 'ctx> TypedFunctionPass<'ctx, B> for BadPass {
    type Effect = llvmkit_ir::PreservesVerification;
    type Requires = ();
    type MinPreserves = ();
    const NAME: &'static str = "bad-pass";

    fn run(
        &mut self,
        cx: &mut TypedFunctionPassContext<'_, '_, 'ctx, B, (), llvmkit_ir::PreservesVerification>,
    ) -> Result<PreservedAnalyses, IrError> {
        // `Requires = ()` has no `AnalysisSelector<DominatorTreeAnalysis, _>`
        // impl, so this access must not compile.
        let _dt = cx.analysis::<DominatorTreeAnalysis, _>();
        Ok(PreservedAnalyses::all())
    }
}

fn main() {}
