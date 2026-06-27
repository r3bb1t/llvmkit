//! llvmkit derive compile-fail (Doctrine D3).
//!
//! Generic Rust schemas would require an erased type map; this slice keeps the
//! LLVM struct schema source of truth explicit and monomorphic.

use llvmkit_ir::IrStruct;

#[derive(IrStruct)]
struct Boxed<T> {
    value: T,
}

fn main() {}
