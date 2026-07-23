//! llvmkit typestate compile-fail (Doctrine D4), Task 19.
//!
//! `SsaBuilder::def_int_var<W, V>` bounds its value parameter
//! `V: IntoIntValue<'ctx, W, B>`, where `W` is pinned by the target
//! `IntVariable<W>`'s own declared width. `IntoIntValue`'s identity
//! impl is only for the SAME width (`IntValue<'ctx, W, B>: IntoIntValue
//! <'ctx, W, B>`) -- there is no blanket narrowing/widening coercion --
//! so passing an `i64`-typed value into an `IntVariable<i32>` def has
//! ZERO candidate `IntoIntValue<'_, i32, _>` impls and is a compile
//! error (`E0277`), not the runtime `TypeMismatch` the analogous
//! DYN-width seam (`declare_int_var_dyn`) still needs at runtime.
//! Closest upstream behaviour: `Verifier::visitFunction` / instruction
//! type checks reject a mismatched-type SSA def at verify time; llvmkit's
//! statically-widthed variable pushes that same invariant into the type
//! system.

use llvmkit_ir::{Linkage, Module};

fn main() {
    Module::with_new("ssa-def-wrong-width", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let mut b = llvmkit_ir::SsaBuilder::for_function(&m, f).unwrap();
        let entry = b.create_block("entry");

        let x64 = b.declare_int_var::<i64, _>("x64");
        let x32 = b.declare_int_var::<i32, _>("x32");

        let mut b = b.switch_to_block(entry).unwrap();
        b.def_int_var(x64, 5_i64).unwrap();
        let wide = b.use_int_var(x64).unwrap();

        // `wide: IntValue<'_, i64, _>` has no `IntoIntValue<'_, i32, _>`
        // impl -- `x32` expects an `i32`-width value.
        b.def_int_var(x32, wide).unwrap();
    });
}
