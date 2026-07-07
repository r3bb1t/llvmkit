//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `PassManager<Function>::run` in
//! `llvm/include/llvm/IR/PassManager.h` takes a bare `Function&` and an
//! `AnalysisManager&`, with nothing stopping a caller from passing a function
//! that belongs to a different `Module` than the one the analysis manager was
//! built for -- the mismatch would only surface later, if at all, as a subtle
//! wrong-result bug. llvmkit's typed function pipeline instead threads a
//! single `ModuleBrand` type parameter `B` through the module, the function
//! view, and the `FunctionAnalysisManager` (`FunctionPipeline::run` in
//! `pass_manager.rs`); each `Module::with_new` session mints a fresh,
//! generative brand (`docs/type-safety-vs-llvm.md` section 1), so a function
//! handle from one module cannot unify with a verified module/analysis
//! manager pair from another.

use llvmkit_ir::{
    DcePass, FunctionAnalysisManager, IRBuilder, Linkage, Module, ModuleBrand, ModuleView, Type,
    function_pipeline,
};

/// Brand-generic constructor for the analysis manager, mirroring
/// `function_analysis_wrong_brand.rs`'s `manager_for` helper: taking a
/// `ModuleView<'ctx, B>` ties the returned manager's brand to `right`'s
/// brand via type inference, without borrowing `right` itself.
fn manager_for<'ctx, B: ModuleBrand + 'ctx>(
    _module: ModuleView<'ctx, B>,
) -> FunctionAnalysisManager<'ctx, B> {
    FunctionAnalysisManager::new()
}

fn main() {
    Module::with_new::<_, _, _>("left", |left| {
        let i32_ty = left.i32_type();
        let fn_ty = left.fn_type(i32_ty, Vec::<Type<'_, _>>::new(), false);
        let left_function = left
            .add_function::<i32, _>("f", fn_ty, Linkage::External)
            .unwrap();
        let entry = left_function.append_basic_block(&left, "entry");
        let b = IRBuilder::new_for::<i32>(&left).position_at_end(entry);
        b.build_ret(i32_ty.const_int(1_u32)).unwrap();
        let left_verified = left.verify().unwrap();

        Module::with_new::<_, _, _>("right", |right| {
            let mut right_fam = manager_for(right.as_view());
            let mut pipe = function_pipeline((DcePass,));
            // Brand mismatch: `left_verified`'s module brand disagrees with
            // `right_fam`'s brand. Must not compile.
            let _ = pipe.run(left_verified, left_function, &mut right_fam);
        });
    });
}
