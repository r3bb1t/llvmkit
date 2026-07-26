//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Verifier::visitGlobalValue` in
//! `lib/IR/Verifier.cpp` rejects a value referenced from a different module at
//! run time. llvmkit rejects the *storable id* form at compile time.
//!
//! This is the compile-time twin of the runtime property locked by
//! `tests/module_ownership.rs::a_stale_id_from_a_dead_generation_is_refused_by_its_successor`:
//!
//! - two **different** named brands are separated statically — an id minted by
//!   the `Left` module is not even the right *type* to hand to the `Right`
//!   module's resolver, which is what this fixture pins;
//! - two **generations of the same** brand share one type, so they can only be
//!   separated by the runtime `ModuleId` tag, which is what the runtime test
//!   pins.
//!
//! Ids are the interesting case precisely because they are `'static` and
//! `Send`: they outlive their module and travel between threads, so the brand
//! is all the static identity they carry.

use llvmkit_ir::{Linkage, Module, ModuleBrand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Left;
impl ModuleBrand for Left {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Right;
impl ModuleBrand for Right {}

fn main() {
    let left = Module::branded::<Left>("left").unwrap();
    let void_ty = left.void_type();
    let fn_ty = left.fn_type_no_params(void_ty, false);
    let left_fn = left
        .add_function_dyn("f", fn_ty, Linkage::External)
        .unwrap();

    let right = Module::branded::<Right>("right").unwrap();
    // `left_fn` is a `FunctionId<Dyn, Left>`; `right` resolves only `…, Right`.
    let _ = right.view(left_fn);
}
