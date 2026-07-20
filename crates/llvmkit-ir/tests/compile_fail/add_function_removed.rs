//! Compile-fail lock for the strict cut (cycle B, slice B3e): the
//! erased-signature + typed-return constructor
//! `Module::add_function::<R>(name, fn_ty, linkage)` is GONE from the
//! public surface. Typed declarations derive their signature from the
//! markers (`add_typed_function::<Ret, Params, _>` — a marker/signature
//! mismatch is unrepresentable at declaration time, superseding the
//! deleted runtime-rejection test
//! `vertical_slice.rs::typed_add_function_rejects_mismatched_return_marker`),
//! and erased declarations take a runtime `FunctionType` behind
//! `add_function_dyn`, which returns `FunctionValue<Dyn>`. Erasure is
//! still available, but it must be spelled.

use llvmkit_ir::{IrError, Linkage, Module};

fn main() -> Result<(), IrError> {
    Module::with_new("lock", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let _ = m.add_function::<i32, _>("bad", fn_ty, Linkage::External);
        Ok(())
    })
}
