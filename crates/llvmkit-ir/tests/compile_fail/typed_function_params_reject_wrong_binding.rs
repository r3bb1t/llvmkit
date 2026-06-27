//! llvmkit typestate compile-fail (Doctrine D4/D7).
//! Closest upstream: `FunctionTest.hasLazyArguments` for ordered arguments;
//! llvmkit adds typed tuple extraction so wrong value categories are unspellable.

use llvmkit_ir::{FloatValue, Linkage, Module};

fn main() {
    Module::with_new("typed-params-wrong-binding", |m| {
        let f = m
            .add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)
            .unwrap();
        let (_lhs, _rhs): (FloatValue<f32>, _) = f.params();
        Ok::<(), llvmkit_ir::IrError>(())
    })
    .unwrap();
}
