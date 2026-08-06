//! `#[branded(…, Ord)]` cannot be spelled without its supertraits. `Ord: Eq +
//! PartialOrd`, so emitting the `Ord` impl alone would fail at the *use* site
//! with an unresolved bound far from the derive. The macro rejects the list up
//! front instead, at the attribute that is actually wrong.

use std::marker::PhantomData;

use llvmkit_macros::Branded;

#[derive(Branded)]
#[branded(Clone, Copy, PartialEq, Ord)]
struct Ranked<B> {
    n: u32,
    _brand: PhantomData<fn(B) -> B>,
}

fn main() {}
