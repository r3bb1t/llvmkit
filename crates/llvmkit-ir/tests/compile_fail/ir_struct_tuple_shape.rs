//! llvmkit derive compile-fail (Doctrine D6).
//!
//! Closest upstream behaviour: LLVM identified structs are positional bodies,
//! while this derive intentionally maps only named Rust fields to positions.

use llvmkit_ir::IrStruct;

#[derive(IrStruct)]
struct TuplePoint(i32, i32);

fn main() {}
