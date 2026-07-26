//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Verifier::visitTerminator` / successor checks in
//! `lib/IR/Verifier.cpp` reject malformed cross-function or cross-module control
//! flow at runtime. llvmkit pushes the module-provenance part into the Rust type
//! system: a branch target carrying one [`Module`] brand cannot be used by an
//! `IRBuilder` positioned in another branded [`Module`].
//!
//! The two modules are separated by *named brand types*, so the rejection is a
//! plain type mismatch (`Left` vs `Right`) rather than a region error.

use llvmkit_ir::{IRBuilder, Linkage, Module, ModuleBrand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Left;
impl ModuleBrand for Left {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Right;
impl ModuleBrand for Right {}

fn main() {
    let left = Module::branded::<Left>("left").unwrap();
    let f = left
        .add_typed_function::<(), (), _>("left_f", Linkage::External)
        .unwrap()
        .as_function();
    let left_target = left.view(f).append_basic_block(&left, "target");

    let right = Module::branded::<Right>("right").unwrap();
    let f = right
        .add_typed_function::<(), (), _>("right_f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = right.view(f).append_basic_block(&right, "entry");
    let builder = IRBuilder::new_for::<()>(&right).position_at_end(entry);
    let _ = builder.build_br(left_target);
}
