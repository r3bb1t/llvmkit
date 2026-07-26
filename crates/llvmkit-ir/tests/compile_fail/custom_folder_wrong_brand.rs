//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `IRBuilderFolder` in
//! `llvm/include/llvm/IR/IRBuilderFolder.h` returns raw `Value *`; LLVM can only
//! catch a wrong-module folded result later through verifier/module checks.
//! llvmkit makes custom folder hooks return `Value<'ctx, B>`, so a folder result
//! from a different branded [`Module`] cannot satisfy the hook return type.
//!
//! This fixture is deliberately **brand-specific and stays that way**: the whole
//! point is that a value carrying one concrete brand (`Foreign`) is not the
//! caller's `B`. Generalising it to a brand-agnostic value would prove nothing.

use llvmkit_ir::{ModuleBrand, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Foreign;
impl ModuleBrand for Foreign {}

fn return_foreign_folder_value<'ctx, B: ModuleBrand>(
    foreign: Value<'ctx, Foreign>,
) -> Value<'ctx, B> {
    foreign
}

fn main() {}
