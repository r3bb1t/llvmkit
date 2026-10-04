//! llvmkit typestate compile-fail (D1, D8): reordering a value's use list is
//! a mutation, and a value a `Module<B, Verified>` mints is `ReadOnly`.
//!
//! `sort_use_list` and `sort_use_list_by` take no `&Module<B, Unverified>`
//! token, so before the capability parameter nothing stopped them on a
//! verified module's values: the order `users()` walks, and the
//! `uselistorder` directives the printer emits when asked to preserve it,
//! could be rewritten after `verify()`. The handle's own capability is now
//! the only guard — and the one this fixture holds in place.
//!
//! No upstream counterpart: `Value::sortUseList` is a plain non-const method
//! and LLVM has no verified-module typestate.
use llvmkit_ir::{IrBuilder, Linkage, Module, NoFolder};

fn main() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f = m
        .add_function_dyn("f", m.function_type_no_parameters(i32_ty), Linkage::External)
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let one = i32_ty.const_int(1_i32);
    let sum = b.int_add::<i32, _, _, _>(one, one, "s").expect("add");
    b.ret(m.view(sum)).expect("ret");
    let m = m.verify().expect("verifies");
    let _ = m.view(sum).as_erased().sort_use_list(&[0]);
}
