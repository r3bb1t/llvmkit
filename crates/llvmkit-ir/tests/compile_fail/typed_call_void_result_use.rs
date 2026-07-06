//! llvmkit-specific compile-fail (Doctrine D4), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: LLVM's `CallInst::getType()` returns
//! `Type::getVoidTy()` for a void callee, and any caller that tries to use
//! the "result" as an operand fails a runtime type check (or UB in
//! optimized builds that skip verification). llvmkit's typed
//! `TypedCallInst::result()` narrows a void callee's result to an actual
//! `()` value via the `CallResult` GAT -- `()` does not implement
//! `IntoIntValue<'_, i32, _>`, so feeding it into `build_int_add` is a
//! compile error, not a runtime/UB path.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let void_callee = m
            .add_typed_function::<(), (), _>("sink", Linkage::External)
            .unwrap();
        let caller = m
            .add_typed_function::<i32, (i32,), _>("caller", Linkage::External)
            .unwrap();
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (n,) = caller.params();
        let void_call = b.build_call(void_callee, (), "").unwrap();
        // `void_call.result()` is `()`, not an `IntoIntValue<'_, i32, _>`.
        let _ = b.build_int_add::<i32, _, _, _>(void_call.result(), n, "x");
    });
}
