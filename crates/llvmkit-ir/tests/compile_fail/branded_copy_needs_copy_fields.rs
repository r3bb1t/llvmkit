//! `#[derive(Branded)]` cannot mint a wrong `Copy`: the derive emits a bare
//! `impl Copy`, and the compiler still checks every field (`E0204`). A type
//! holding a `String` that requests the default full-six set is rejected —
//! the macro's bound-freedom never becomes structural unsoundness.

use std::marker::PhantomData;

use llvmkit_macros::Branded;

#[derive(Branded)]
struct HoldsString<B> {
    text: String,
    _brand: PhantomData<fn(B) -> B>,
}

fn main() {}
