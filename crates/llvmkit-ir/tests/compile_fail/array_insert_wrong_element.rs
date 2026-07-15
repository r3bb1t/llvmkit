//! Compile-fail lock for the typed `build_arr_insert` (Slice 6, Doctrine D4).
//! The element parameter is typed `element: E::Value`, where `E` is pinned by
//! the `array: ArrayValue<'ctx, E, L, B>` argument. Inserting a
//! `FloatValue<f32>` into an `ArrayValue<i32, ArrLen<4>>` therefore demands
//! `E::Value == IntValue<i32>` but is handed a `FloatValue<f32>`, a compile
//! error (`E0308 mismatched types`) instead of the runtime
//! `Verifier::visitInsertValueInst` element-type mismatch the erased
//! `build_insert_value` would still surface at verify time.

use llvmkit_ir::{ArrLen, ArrayValue, FloatValue, IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("arr-insert-wrong-elem", |m| {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let arr_i32 = m.array_type_n::<i32, 4>();
        let fn_ty = m.fn_type(
            arr_i32.as_type(),
            [arr_i32.as_type(), f32_ty.as_type()],
            false,
        );
        let f = m
            .add_function::<llvmkit_ir::marker::Dyn, _>("g", fn_ty, Linkage::External)
            .unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);

        let arr: ArrayValue<'_, i32, ArrLen<4>> =
            f.param(0).unwrap().as_value().try_into().unwrap();
        let wrong: FloatValue<'_, f32> = f.param(1).unwrap().try_into().unwrap();

        // `E` is fixed to `i32` by `arr`, so `element` must be `IntValue<i32>`;
        // a `FloatValue<f32>` does not fit.
        let _bad = b
            .build_arr_insert(arr, wrong, 0, "x") //~ ERROR mismatched types
            .unwrap();
    });
}
