//! llvmkit-specific compile-fail (D3, D10): `lanes::<0>()` is a compile error,
//! pulling `FixedVectorType::get`'s `NumElts > 0` assert forward to
//! monomorphisation. Runs under the harness's `cargo build` mode (see
//! `typestate_compile_fail.rs`), since the `const` block is evaluated at
//! codegen.

use llvmkit_ir::{Module, VectorType, lanes};

fn main() {
    let m = Module::dynamic("c");
    let _ = VectorType::of(m.i32_type(), lanes::<0>());
}
