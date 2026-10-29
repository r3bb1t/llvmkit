//! llvmkit derive compile-fail: `#[llvmkit(name = "")]` names no struct —
//! `StructType::create(Context, "")` (`lib/IR/Type.cpp`) makes an anonymous
//! one — so the derive refuses it with its own error at the literal, before
//! any type is minted. The hand-written form is `schema_name_empty.rs`.

use llvmkit_ir::IrStruct;

#[derive(IrStruct)]
#[llvmkit(name = "")]
struct Nameless {
    x: i32,
}

fn main() {}
