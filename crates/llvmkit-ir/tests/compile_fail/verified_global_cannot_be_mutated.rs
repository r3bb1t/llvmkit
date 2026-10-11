//! llvmkit typestate compile-fail (D1, D8) — Task 25's stop condition.
//!
//! Under `DynBrand`, a second unverified module's token used to unlock a
//! verified module: `a.view(g).set_linkage(&b, …)` changed `a` while its type
//! still said `Verified`. A `Module<B, Verified>` now mints `ReadOnly` handles,
//! and a `ReadOnly` handle has no mutators.
use llvmkit_ir::{Linkage, Module};

fn main() {
    let a = Module::dynamic("a");
    let global = a.add_global("g", a.i32_type().const_int(0i32)).expect("global");
    let a = a.verify().expect("verifies");
    let b = Module::dynamic("b");
    a.view(global).set_linkage(&b, Linkage::Internal);
}
