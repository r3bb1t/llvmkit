//! Compile-fail lock for the typed vector binops (Slice 5, Doctrine D4).
//! `build_vec_int_add<E, L>` takes BOTH operands as `VectorValue<'ctx, E, L,
//! B>` — the SAME element marker `E`. Adding a `<4 x i32>` to a `<4 x i64>`
//! therefore cannot unify `E`, and is a compile error (`E0308 mismatched
//! types`, `i32` vs `i64`) instead of the runtime
//! `Verifier::visitBinaryOperator` element-type mismatch the erased
//! `build_int_add_dyn` would still surface at verify time.

use llvmkit_ir::{IRBuilder, Len, Linkage, Module, VectorValue};

fn main() {
    Module::with_new("vec-elem-mismatch", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let v_i32 = m.vector_type(i32_ty.as_type(), 4, false);
        let v_i64 = m.vector_type(i64_ty.as_type(), 4, false);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(
            void_ty.as_type(),
            [v_i32.as_type(), v_i64.as_type()],
            false,
        );
        let f = m
            .add_function::<(), _>("g", fn_ty, Linkage::External)
            .unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);

        let a: VectorValue<'_, i32, Len<4>> =
            f.param(0).unwrap().as_value().try_into().unwrap();
        let c: VectorValue<'_, i64, Len<4>> =
            f.param(1).unwrap().as_value().try_into().unwrap();

        // `i32` and `i64` cannot unify the single `E` the binop demands.
        let _bad = b.build_vec_int_add(a, c, "x").unwrap(); //~ ERROR mismatched types
    });
}
