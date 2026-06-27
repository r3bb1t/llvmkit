//! llvmkit derive compile-fail (Doctrine D6).
//!
//! LLVM IR struct layout is positional; field-level rename/skip/default helper
//! attributes would obscure layout changes and are rejected in this slice.

use llvmkit_ir::IrStruct;

#[derive(IrStruct)]
struct Point {
    #[llvmkit(rename = "xx")]
    x: i32,
    y: i32,
}

fn main() {}
