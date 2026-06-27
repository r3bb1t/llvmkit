//! llvmkit typestate compile-fail (Doctrine D4/D7).
//! Closest upstream: `FunctionTest.hasLazyArguments` for ordered arguments;
//! llvmkit keeps unchecked tuple extraction behind `TypedFunctionValue`.

use llvmkit_ir::{FunctionParam, FunctionParamList, Linkage, Module};

fn main() {
    Module::with_new("typed-params-require-facade", |m| {
        let f64_ty = m.f64_type();
        let fn_ty = m.fn_type(m.i32_type(), [f64_ty.as_type()], false);
        let raw = m
            .add_function::<i32, _>("raw", fn_ty, Linkage::External)
            .unwrap();
        let arg = raw.param(0).unwrap();
        let _value = <i32 as FunctionParam>::value_from_argument(arg);
        let _tuple = <(i32,) as FunctionParamList>::values(raw);
        Ok::<(), llvmkit_ir::IrError>(())
    })
    .unwrap();
}
