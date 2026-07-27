//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! LLVM has no Rust `ModuleBrand`; the closest parity anchors are
//! `GlobalVariable::GlobalVariable` / `setInitializer` in `lib/IR/Globals.cpp`,
//! which assert initializer type compatibility, and `Verifier::visitGlobalValue`
//! in `lib/IR/Verifier.cpp`, which rejects globals referenced from a different
//! module at runtime. llvmkit makes the stronger module-provenance rule static:
//! a constant produced through one branded [`Module`] cannot initialize a global
//! in a differently branded [`Module`].
//!
//! The two modules are separated by *named brand types*, so the rejection is a
//! plain type mismatch (`Left` vs `Right`) rather than a region error.

use llvmkit_ir::{Module, ModuleBrand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Left;
impl ModuleBrand for Left {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Right;
impl ModuleBrand for Right {}

fn main() {
    let left = Module::branded::<Left, _>("left").unwrap();
    let left_init = left.i32_type().const_int(1_i32);
    let right = Module::branded::<Right, _>("right").unwrap();
    let _ = right.add_global("g", left_init);
}
