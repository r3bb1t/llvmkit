//! llvmkit-specific compile-fail (Doctrine D7/D4), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `CallInst::init`'s "Calling a function with a
//! bad signature!" assertion (`lib/IR/Instructions.cpp`) and
//! `Verifier::visitCallBase`'s per-argument type check reject a
//! wrong-typed call argument *at runtime*. llvmkit's typed `build_call`
//! pushes that same invariant into the Rust type system: an `f64` value
//! does not satisfy `IntoCallArg<'_, i32, _>`, so filling an `i32` call-
//! argument slot with it is a compile error, not a build-time `IrError`.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let callee = m
            .add_typed_function::<i32, (i32,), _>("callee", Linkage::External)
            .unwrap();
        let caller = m
            .add_typed_function::<i32, (f64,), _>("caller", Linkage::External)
            .unwrap();
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (x,) = caller.params();
        // `x` is `FloatValue<f64>`; `callee`'s single parameter schema is
        // `i32`. `FloatValue<f64>` does not implement
        // `IntoCallArg<'_, i32, _>`.
        let _ = b.build_call(callee, (x,), "bad");
    });
}
