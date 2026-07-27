//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Verifier::visitGlobalValue` in
//! `lib/IR/Verifier.cpp` rejects a global value referenced from a different
//! module at runtime. llvmkit pushes the same cross-module provenance invariant
//! into the Rust type system: values carrying one [`Module`] brand cannot be
//! used as operands by an `IRBuilder` positioned in another branded module.
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
    let left = Module::branded::<Left, _>("left").unwrap();
    let left_value = left.i64_type().const_int(1_i64);

    let right = Module::branded::<Right, _>("right").unwrap();
    let function = right
        .add_typed_function::<i64, (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = right.view(function).append_basic_block(&right, "entry");
    let builder = IRBuilder::new_for::<i64>(&right).position_at_end(entry);
    let _ = builder.build_int_add(left_value, left_value, "bad");
}
