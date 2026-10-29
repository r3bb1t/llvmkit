//! llvmkit-specific compile-fail (D3, D10): a `TypedPointerType` is an
//! analysis-only type and does not implement `IrType`, so it cannot be a
//! function parameter — LLVM's `TypedPointerType.h` says it is "not legal to
//! use this type, or derived types containing this type, in LLVM IR", and
//! `Value::Value`'s `checkType` asserts no value has it. Locks `IrType`'s
//! `#[diagnostic::on_unimplemented]` message.

use llvmkit_ir::{Module, TypedPointerType};

fn main() {
    let m = Module::dynamic("c");
    let tp = TypedPointerType::of(m.i32_type(), 0);
    let _ = m.function_type(m.void_type(), [tp]);
}
