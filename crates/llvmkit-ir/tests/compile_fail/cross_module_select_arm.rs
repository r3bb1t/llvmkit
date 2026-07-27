//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Verifier::visitSelectInst` in
//! `lib/IR/Verifier.cpp` checks select operand type consistency at runtime, and
//! `Verifier::visitGlobalValue` rejects values referenced from a different module.
//! llvmkit pushes the module-provenance part into the Rust type system: both
//! select arms must carry the builder module's brand.
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
    let i32_ty = left.i32_type();
    let f = left
        .add_typed_function::<i32, (), _>("left_f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = left.view(f).append_basic_block(&left, "entry");
    let left_builder = IRBuilder::new_for::<i32>(&left).position_at_end(entry);
    let left_arm = left_builder.view(
        left_builder
            .build_int_add(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "left")
            .unwrap(),
    );

    let right = Module::branded::<Right, _>("right").unwrap();
    let i1_ty = right.bool_type();
    let i32_ty = right.i32_type();
    let cond = i1_ty.const_int(true);
    let f = right
        .add_typed_function::<i32, (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = right.view(f).append_basic_block(&right, "entry");
    let builder = IRBuilder::new_for::<i32>(&right).position_at_end(entry);
    let right_arm = builder
        .build_int_add(i32_ty.const_int(3_i32), i32_ty.const_int(4_i32), "right")
        .unwrap();
    let right_arm = builder.view(right_arm);
    let _ = builder.build_select(cond, left_arm, right_arm, "bad");
}
