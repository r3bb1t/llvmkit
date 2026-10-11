//! llvmkit-specific compile-fail (D3, D10): `Len<0>` can be named, but its
//! lane count cannot be read. `StaticVecLen::STATIC_LEN` is `lanes::<N>()`, so
//! `Len<0>`'s fails with `lanes`'s const-evaluation message rather than
//! reporting a zero-lane vector `FixedVectorType::get` asserts against.

use llvmkit_ir::{Len, StaticVecLen};

fn main() {
    let _ = <Len<0> as StaticVecLen>::STATIC_LEN;
}
