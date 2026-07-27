//! llvmkit-specific compile-fail (Doctrine D7): an analysis manager built for
//! one branded module cannot be queried with a function handle from another.
//!
//! The two modules are separated by *named brand types*, so the rejection is a
//! plain type mismatch (`Left` vs `Right`) rather than a region error.

use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, Linkage, Module, ModuleBrand, ModuleView,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Left;
impl ModuleBrand for Left {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Right;
impl ModuleBrand for Right {}

fn manager_for<'ctx, B: ModuleBrand + 'ctx>(
    _module: ModuleView<'ctx, B>,
) -> FunctionAnalysisManager<'ctx, B> {
    FunctionAnalysisManager::new()
}

fn main() {
    let left = Module::branded::<Left, _>("left").unwrap();
    let left_function = left
        .view(
            left.add_typed_function::<(), (), _>("left", Linkage::External)
                .unwrap(),
        )
        .as_function();

    let right = Module::branded::<Right, _>("right").unwrap();
    let mut fam = manager_for(right.as_view());
    let _ = fam.get_result::<DominatorTreeAnalysis, _>(left_function.as_view());
}
