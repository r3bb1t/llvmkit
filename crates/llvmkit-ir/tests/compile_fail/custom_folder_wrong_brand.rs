//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `IRBuilderFolder` in
//! `llvm/include/llvm/IR/IRBuilderFolder.h` returns raw `Value *`; LLVM can only
//! catch a wrong-module folded result later through verifier/module checks.
//! llvmkit makes custom folder hooks return `Value<'ctx, B>`, so a folder result
//! from a different branded [`Module`] cannot satisfy the hook return type.

use llvmkit_ir::{ModuleBrand, Value};

fn return_foreign_folder_value<'ctx, B: ModuleBrand>(foreign: Value<'ctx>) -> Value<'ctx, B> {
    foreign
}

fn main() {}
