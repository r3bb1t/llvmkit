//! llvmkit-specific Pass API v2 lock (Doctrine D1/D7), not a 1:1 LLVM port.
//!
//! A pass's analysis access is tied to its own `type Requires` tuple: the
//! infallible `FnCx::analysis::<A, _>()` accessor (`pass_context.rs`) is bounded
//! by `R: AnalysisSelector<'ctx, B, A, _>` (`analysis.rs`), and only the analyses
//! actually listed in `Requires` have an `AnalysisSelector` impl. A pass that
//! declares `type Requires = ()` therefore has no
//! `AnalysisSelector<DominatorTreeAnalysis, _>` for `()`, so querying an
//! undeclared analysis is a compile error carrying our own stable
//! `#[diagnostic::on_unimplemented]` message, not the runtime "analysis not
//! registered" null-deref UB LLVM's `AnalysisManager::getResult` risks.
//!
//! (Re-expresses the deleted old-API `typed_context_undeclared_analysis.rs` for
//! the capability-graded Pass API v2.)

use llvmkit_ir::{DominatorTreeAnalysis, FnCx, FnReport, FunctionPass, Inspect, IrResult, ModuleBrand};

struct UndeclaredAnalysis;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for UndeclaredAnalysis {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "undeclared-analysis";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport> {
        // `Requires = ()` has no `AnalysisSelector<DominatorTreeAnalysis, _>`
        // impl, so this access is unspellable at compile time.
        let _dt = cx.analysis::<DominatorTreeAnalysis, _>();
        Ok(cx.done())
    }
}

fn main() {}
