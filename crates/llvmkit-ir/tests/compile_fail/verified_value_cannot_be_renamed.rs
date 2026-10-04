//! llvmkit typestate compile-fail (D1, D8): a value minted from a
//! `Module<B, Verified>` is `ReadOnly`, and a `ReadOnly` handle has no setter —
//! even with another module's mutation token in hand.
//!
//! Before the capability parameter, `set_name` asked only for *a*
//! `&Module<B, Unverified>` token, and two modules that share a brand (every
//! `Module::dynamic` is `DynBrand`) type-check against each other's — so the
//! call below compiled, and only a run-time module-id assertion stood between
//! it and a verified module's IR. Now the handle's own capability decides:
//! `set_name` requires `C: CanMutate`, and a verified module's views are
//! `ReadOnly`.
//!
//! No upstream counterpart: `Value::setName` is a plain non-const method and
//! LLVM has no verified-module typestate.
use llvmkit_ir::{IrBuilder, Linkage, Module, NoFolder};

fn main() {
    let a = Module::dynamic("a");
    let i32_ty = a.i32_type();
    let f = a
        .add_function_dyn("f", a.function_type_no_parameters(i32_ty), Linkage::External)
        .expect("f");
    let entry = a.view(f).append_basic_block(&a, "entry");
    let b0 = IrBuilder::with_folder(&a, NoFolder).position_at_end(entry);
    let sum = b0
        .int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "s")
        .expect("add");
    b0.ret(a.view(sum)).expect("ret");
    let a = a.verify().expect("verifies");
    let b = Module::dynamic("b");
    a.view(sum).set_name(&b, "renamed");
}
