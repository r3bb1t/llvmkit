//! llvmkit-specific compile-fail (Doctrine D8), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `PassManager<Function>::run` in
//! `llvm/include/llvm/IR/PassManager.h` has no type-level notion of a
//! "verified" module at all -- LLVM re-verifies (or not) at the caller's
//! discretion, entirely at runtime. llvmkit's typed function pipeline
//! (`function_pipeline`, `pass_manager.rs`) instead derives its `run` output
//! type from its members' effects (D8): a pipeline containing any
//! `MutatesIr` member (here `DcePass`, migrated to `TypedFunctionPass` in
//! Task 7) always yields `Module<Unverified>`, never `Module<Verified>`,
//! regardless of what the caller hopes to bind it as.

use llvmkit_ir::{
    DcePass, FunctionAnalysisManager, IRBuilder, Linkage, Module, Type, Verified,
    function_pipeline,
};

fn main() {
    Module::with_new("mixed", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_ret(i32_ty.const_int(1_u32)).unwrap();

        let verified = m.verify().unwrap();
        let mut fam = FunctionAnalysisManager::new();
        let mut pipe = function_pipeline((DcePass,));
        // MutatesIr pipeline: the result is Module<Unverified>; binding it as
        // Verified must not compile.
        let _wrong: Module<'_, _, Verified> = pipe.run(verified, f, &mut fam).unwrap();
    });
}
