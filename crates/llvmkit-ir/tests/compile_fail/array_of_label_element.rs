//! llvmkit-specific compile-fail (D3, D10): `ArrayType::of` takes only an
//! `ArrayElement`, so a `label` element is a compile error rather than
//! `ArrayType::get`'s `isValidElementType` assert. Locks the trait's
//! `#[diagnostic::on_unimplemented]` message.

use llvmkit_ir::{ArrayType, Module};

fn main() {
    let m = Module::dynamic("c");
    let _ = ArrayType::of(m.label_type(), 2);
}
