//! llvmkit typestate compile-fail (Doctrine D4/D7).
//! Closest upstream: `FunctionTest.hasLazyArguments` for ordered arguments;
//! llvmkit adds typed tuple extraction so wrong value categories are unspellable.

use llvmkit_ir::{FloatValue, Linkage, Module};

fn main() {
    let m = Module::dynamic("typed-params-wrong-binding");
    let f = m
        .add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)
        .unwrap();
    let (_lhs, _rhs): (FloatValue<f32, _>, _) = m.view(f).params();
    Ok::<(), llvmkit_ir::IrError>(())
    .unwrap();
}
