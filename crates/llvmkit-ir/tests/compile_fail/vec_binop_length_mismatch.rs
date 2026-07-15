//! Compile-fail lock for the typed vector binops (Slice 5, Doctrine D4).
//! `build_vec_int_add<E, L>` takes BOTH operands as `VectorValue<'ctx, E, L,
//! B>` — the SAME lane-count marker `L`. Adding a `Len<4>` vector to a
//! `Len<2>` vector therefore cannot unify `L`, and is a compile error
//! (`E0308 mismatched types`, `Len<4>` vs `Len<2>`) instead of the runtime
//! `Verifier::visitBinaryOperator` shape mismatch the erased
//! `build_int_add_dyn` would still surface at verify time.

use llvmkit_ir::{IRBuilder, Len, Linkage, Module, VectorValue};

fn main() {
    Module::with_new("vec-len-mismatch", |m| {
        let i32_ty = m.i32_type();
        let v4 = m.vector_type(i32_ty.as_type(), 4, false);
        let v2 = m.vector_type(i32_ty.as_type(), 2, false);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [v4.as_type(), v2.as_type()], false);
        let f = m
            .add_function::<(), _>("g", fn_ty, Linkage::External)
            .unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);

        let a4: VectorValue<'_, i32, Len<4>> =
            f.param(0).unwrap().as_value().try_into().unwrap();
        let a2: VectorValue<'_, i32, Len<2>> =
            f.param(1).unwrap().as_value().try_into().unwrap();

        // `Len<4>` and `Len<2>` cannot unify the single `L` the binop demands.
        let _bad = b.build_vec_int_add(a4, a2, "x").unwrap(); //~ ERROR mismatched types
    });
}
