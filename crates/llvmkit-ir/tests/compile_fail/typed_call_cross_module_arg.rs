//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Verifier::visitGlobalValue` in
//! `lib/IR/Verifier.cpp` rejects a global value referenced from a different
//! module at runtime. llvmkit pushes the same cross-module provenance
//! invariant into the Rust type system: a value produced by one branded
//! [`Module`] cannot be passed as a typed call argument to a builder
//! positioned in a differently branded [`Module`]. Mirrors the shape of
//! `cross_module_value_brand.rs` / `cross_module_select_arm.rs`, applied to
//! `build_call`'s `CallArgs` argument slot instead of `build_int_add` /
//! `build_select`.

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
    let left_value = i32_ty.const_int(1_i32);

    let right = Module::branded::<Right, _>("right").unwrap();
    let callee = right.view(
        right
            .add_typed_function::<i32, (i32,), _>("callee", Linkage::External)
            .unwrap(),
    );
    let caller = right
        .add_typed_function::<i32, (), _>("caller", Linkage::External)
        .unwrap();
    let entry = right.view(caller).append_basic_block(&right, "entry");
    let builder = IRBuilder::new_for::<i32>(&right).position_at_end(entry);
    // `left_value` carries brand `Left`; `builder` is positioned in the
    // `Right`-branded module. Filling `callee`'s call-argument slot with a
    // foreign-module value is rejected.
    let _ = builder.build_call(callee, (left_value,), "bad");
}
