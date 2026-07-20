//! Coverage for the TYPED array-op builders (`build_arr_extract` /
//! `build_arr_insert`), which carry the element marker `E` and length marker
//! `L` in the type system. They lower into the erased aggregate ops
//! (`build_extract_value` / `build_insert_value`), so their emitted IR is
//! byte-for-byte identical to the erased family's — this file locks the typed
//! ops against the same golden `extractvalue`/`insertvalue` strings
//! `builder_aggregate_vector.rs` asserts, plus checks the element inference the
//! typed forms add. Also exercises `build_alloca` on a statically-typed array
//! type (the retrofitted handle still implements `IrType`).
//!
//! The wrong-element-is-a-compile-error guarantee lives in the compile-fail
//! fixture (`tests/compile_fail/array_insert_wrong_element`); here we only
//! exercise the well-typed happy path.

use llvmkit_ir::{ArrLen, ArrayValue, Dyn, IRBuilder, IntValue, Linkage, Module};

/// `build_arr_extract` at index 2 on a `[4 x i32]` typed `ArrayValue` returns
/// the element as its statically typed scalar handle — `IntValue<'_, i32>`,
/// inferred from the array's element marker with no turbofish. The
/// `let`-binding annotation is the type assertion. The emitted IR matches the
/// golden `extractvalue [4 x i32] ..., 2` the erased `build_extract_value`
/// produces.
#[test]
fn typed_arr_extract_returns_typed_element() {
    Module::with_new("aextract", |m| {
        let i32_ty = m.i32_type();
        let arr_ty = m.array_type_n::<i32, 4>();

        let fn_ty = m.fn_type(i32_ty.as_type(), [arr_ty.as_type()], false);
        let f = m
            .add_function_dyn("g", fn_ty, Linkage::External)
            .expect("g");
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);

        // Narrow the erased `[4 x i32]` param into the statically typed handle.
        let a: ArrayValue<'_, i32, ArrLen<4>> = f
            .param(0)
            .expect("p0")
            .as_value()
            .try_into()
            .expect("narrow p0");

        // Return type inferred from `a`'s element marker: `IntValue<'_, i32>`.
        let e: IntValue<'_, i32> = b.build_arr_extract(a, 2, "e").expect("extract");
        assert_eq!(e.ty(), i32_ty, "extracted element must be i32-typed");

        b.build_ret(e).expect("ret");

        let txt = format!("{m}");
        assert!(
            txt.contains("%e = extractvalue [4 x i32] %0, 2\n"),
            "expected typed extractvalue, got:\n{txt}"
        );
    })
}

/// `build_arr_insert` writes a typed `IntValue<i32>` into a `[4 x i32]` typed
/// array and returns the same-marker `ArrayValue<i32, ArrLen<4>>`, so the
/// result round-trips into further typed ops. The emitted IR matches the
/// golden `insertvalue [4 x i32] ..., i32 7, 1` the erased `build_insert_value`
/// produces.
#[test]
fn typed_arr_insert_round_trips() {
    Module::with_new("ainsert", |m| {
        let i32_ty = m.i32_type();
        let arr_ty = m.array_type_n::<i32, 4>();
        let void_ty = m.void_type();

        let fn_ty = m.fn_type(void_ty.as_type(), [arr_ty.as_type()], false);
        let f = m
            .add_function_dyn("g", fn_ty, Linkage::External)
            .expect("g");
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);

        let a: ArrayValue<'_, i32, ArrLen<4>> = f
            .param(0)
            .expect("p0")
            .as_value()
            .try_into()
            .expect("narrow p0");

        let seven: IntValue<'_, i32> = i32_ty
            .const_int(7_i32)
            .as_value()
            .try_into()
            .expect("i32 const");

        // Result keeps the `i32` / `ArrLen<4>` markers, so it feeds straight
        // back into another typed array op.
        let updated: ArrayValue<'_, i32, ArrLen<4>> =
            b.build_arr_insert(a, seven, 1, "u").expect("insert");
        let back: IntValue<'_, i32> = b.build_arr_extract(updated, 1, "back").expect("extract");
        assert_eq!(back.ty(), i32_ty, "round-tripped element must be i32-typed");

        b.build_ret_void().expect("ret void");

        let txt = format!("{m}");
        assert!(
            txt.contains("%u = insertvalue [4 x i32] %0, i32 7, 1\n"),
            "expected typed insertvalue, got:\n{txt}"
        );
        assert!(
            txt.contains("%back = extractvalue [4 x i32] %u, 1\n"),
            "expected round-trip extractvalue, got:\n{txt}"
        );
    })
}

/// `build_alloca` accepts a statically-typed `[4 x i32]` array type directly —
/// the retrofitted `ArrayType<'ctx, E, ArrLen<N>, B>` still implements
/// `IrType`, so no dedicated typed-alloca method is needed. The result is
/// still an erased `PointerValue` (pointee typing is separate future work),
/// and the emitted IR is the canonical `alloca [4 x i32]`.
#[test]
fn typed_array_type_allocas() {
    Module::with_new("aalloca", |m| {
        let arr_ty = m.array_type_n::<i32, 4>();
        let void_ty = m.void_type();

        let fn_ty = m.fn_type_no_params(void_ty.as_type(), false);
        let f = m
            .add_function_dyn("g", fn_ty, Linkage::External)
            .expect("g");
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);

        // `build_alloca` takes any `IrType`; the typed array handle qualifies.
        let slot = b.build_alloca(arr_ty, "slot").expect("alloca");
        let _ = slot.as_value();

        b.build_ret_void().expect("ret void");

        let txt = format!("{m}");
        assert!(
            txt.contains("%slot = alloca [4 x i32]"),
            "expected typed-array alloca, got:\n{txt}"
        );
    })
}
