//! Compile-fail lock for the typed vector binops (Slice 5, Doctrine D4).
//! `vector_int_add<E, L>` takes BOTH operands as `VectorValue<'ctx, E, L,
//! B>` — the SAME element marker `E`. Adding a `<4 x i32>` to a `<4 x i64>`
//! therefore cannot unify `E`, and is a compile error (`E0308 mismatched
//! types`, `i32` vs `i64`) instead of the runtime
//! `Verifier::visitBinaryOperator` element-type mismatch the erased
//! `int_add_dyn` would still surface at verify time.

use llvmkit_ir::{Dyn, IrBuilder, Len, Linkage, Module, VectorValue};

fn main() {
    let m = Module::dynamic("vec-elem-mismatch");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let v_i32 = m.vector_type(i32_ty.as_type(), 4);
    let v_i64 = m.vector_type(i64_ty.as_type(), 4);
    let void_ty = m.void_type();
    let fn_ty = m.function_type(
        void_ty.as_type(),
        [v_i32.as_type(), v_i64.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);

    let a: VectorValue<'_, i32, Len<4>, _> =
        m.view(f).param(0).unwrap().as_erased().try_into().unwrap();
    let c: VectorValue<'_, i64, Len<4>, _> =
        m.view(f).param(1).unwrap().as_erased().try_into().unwrap();

    // `i32` and `i64` cannot unify the single `E` the binop demands.
    let _bad = b.vector_int_add(a, c, "x").unwrap(); //~ ERROR mismatched types
}
