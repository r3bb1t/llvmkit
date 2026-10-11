//! llvmkit-specific compile-fail (D3, D10): `VectorType::of` takes only a
//! `VectorElement`, so a `void` element is a compile error rather than
//! `FixedVectorType::get`'s `isValidElementType` assert. Locks the trait's
//! `#[diagnostic::on_unimplemented]` message.

use llvmkit_ir::{Module, VectorType, lanes};

fn main() {
    let m = Module::dynamic("c");
    let _ = VectorType::of(m.void_type(), lanes::<4>());
}
