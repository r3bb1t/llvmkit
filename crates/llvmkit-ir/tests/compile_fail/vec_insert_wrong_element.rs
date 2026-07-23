//! Compile-fail lock for the typed `build_vec_insert` (Slice 5, Doctrine
//! D4). The element parameter is typed `element: E::Value`, where `E` is
//! pinned by the `vec: VectorValue<'ctx, E, L, B>` argument. Inserting a
//! `FloatValue<f32>` into a `VectorValue<i32, Len<4>>` therefore demands
//! `E::Value == IntValue<i32>` but is handed a `FloatValue<f32>`, a compile
//! error (`E0308 mismatched types`) instead of the runtime
//! `Verifier::visitInsertElementInst` element-type mismatch the erased
//! `build_insert_element` would still surface at verify time.

use llvmkit_ir::{FloatValue, IRBuilder, Len, Linkage, Module, VectorValue};

fn main() {
    Module::with_new("vec-insert-wrong-elem", |m| {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let v_i32 = m.vector_type(i32_ty.as_type(), 4, false);
        let fn_ty = m.fn_type(v_i32.as_type(), [v_i32.as_type(), f32_ty.as_type()], false);
        let f = m.add_function_dyn("g", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);

        let vec: VectorValue<'_, i32, Len<4>> =
            f.param(0).unwrap().into_erased().try_into().unwrap();
        let wrong: FloatValue<'_, f32> = f.param(1).unwrap().try_into().unwrap();

        // `E` is fixed to `i32` by `vec`, so `element` must be `IntValue<i32>`;
        // a `FloatValue<f32>` does not fit.
        let _bad = b
            .build_vec_insert(vec, wrong, i32_ty.const_int(0_i32), "x") //~ ERROR mismatched types
            .unwrap();
    });
}
