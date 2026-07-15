//! Coverage for the TYPED vector-op builders (`build_vec_int_*`,
//! `build_vec_extract` / `build_vec_insert` / `build_vec_splat`), which
//! carry the element marker `E` and lane-count marker `L` in the type
//! system. They lower into the erased builders, so their emitted IR is
//! byte-for-byte identical to the erased family's — this file locks the
//! typed ops against the same golden strings `builder_vector_binop_dyn.rs`
//! asserts, plus checks the element/length inference the typed forms add.
//!
//! The mismatch-is-a-compile-error guarantee lives in the compile-fail
//! fixtures (`tests/compile_fail/vec_binop_*` / `vec_insert_wrong_element`);
//! here we only exercise the well-typed happy path.

use llvmkit_ir::{IRBuilder, IntValue, Len, Linkage, Module, VectorValue};

/// The typed `<2 x i64>` binops emit the same element-wise IR as the erased
/// `build_int_*_dyn` family (golden strings ported from
/// `builder_vector_binop_dyn.rs::vector_binops_emit_elementwise_ir`), but now
/// a length/element mismatch would be a compile error rather than a verifier
/// diagnostic.
#[test]
fn typed_vector_binops_match_dyn_golden() {
    Module::with_new("vtyped", |m| {
        let i64_ty = m.i64_type();
        let vec_ty = m.vector_type(i64_ty.as_type(), 2, false);

        let void_ty = m.void_type();
        let fn_ty = m.fn_type(
            void_ty.as_type(),
            [vec_ty.as_type(), vec_ty.as_type()],
            false,
        );
        let f = m
            .add_function::<(), _>("g", fn_ty, Linkage::External)
            .expect("g");
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);

        // Narrow the erased `<2 x i64>` params into the statically typed handle.
        let a: VectorValue<'_, i64, Len<2>> = f
            .param(0)
            .expect("p0")
            .as_value()
            .try_into()
            .expect("narrow p0");
        let c: VectorValue<'_, i64, Len<2>> = f
            .param(1)
            .expect("p1")
            .as_value()
            .try_into()
            .expect("narrow p1");

        // Both operands are `VectorValue<i64, Len<2>>`; a width/length
        // mismatch here would not compile.
        let x = b.build_vec_int_xor(a, c, "x").expect("xor vec");
        let s = b.build_vec_int_add(x, a, "s").expect("add vec");

        let two = i64_ty.const_int(2i64);
        let shamt_const = vec_ty
            .const_vector::<llvmkit_ir::ConstantIntValue<'_, i64>, _>([two, two])
            .expect("shamt vec");
        let shamt: VectorValue<'_, i64, Len<2>> =
            shamt_const.as_value().try_into().expect("narrow shamt");
        let _sh = b.build_vec_int_shl(s, shamt, "sh").expect("shl vec");

        b.build_ret_void();

        let txt = format!("{m}");
        assert!(
            txt.contains("%x = xor <2 x i64> %0, %1\n"),
            "expected typed vector xor, got:\n{txt}"
        );
        assert!(
            txt.contains("%s = add <2 x i64> %x, %0\n"),
            "expected typed vector add, got:\n{txt}"
        );
        assert!(
            txt.contains("%sh = shl <2 x i64> %s, splat (i64 2)\n"),
            "expected typed vector shl, got:\n{txt}"
        );
    })
}

/// `build_vec_extract` returns the element as its statically typed scalar
/// handle — for a `<2 x i64>` that is `IntValue<'_, i64>`, inferred from the
/// vector's element marker with no turbofish. The `let`-binding annotation is
/// the type assertion: it compiles only if the return type is exactly that.
#[test]
fn typed_extract_returns_typed_element() {
    Module::with_new("vextract", |m| {
        let i64_ty = m.i64_type();
        let vec_ty = m.vector_type(i64_ty.as_type(), 2, false);

        let fn_ty = m.fn_type(i64_ty.as_type(), [vec_ty.as_type()], false);
        let f = m
            .add_function::<i64, _>("g", fn_ty, Linkage::External)
            .expect("g");
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);

        let a: VectorValue<'_, i64, Len<2>> = f
            .param(0)
            .expect("p0")
            .as_value()
            .try_into()
            .expect("narrow p0");

        // Return type inferred from `a`'s element marker: `IntValue<'_, i64>`.
        let e: IntValue<'_, i64> = b
            .build_vec_extract(a, i64_ty.const_int(0_i64), "e")
            .expect("extract");
        assert_eq!(e.ty(), i64_ty, "extracted element must be i64-typed");

        b.build_ret(e).expect("ret");

        let txt = format!("{m}");
        assert!(
            txt.contains("%e = extractelement <2 x i64> %0, i64 0\n"),
            "expected typed extractelement, got:\n{txt}"
        );
    })
}

/// `build_vec_splat` broadcasts an `i32` scalar across a statically-sized
/// `<4 x i32>` vector. `E`/`L` are pinned by the result annotation (`E` cannot
/// be inferred from the scalar — Rust does not invert the `E::Value`
/// projection); `scalar: E::Value` then checks the scalar is an `IntValue<i32>`.
#[test]
fn typed_splat_element_from_scalar_length_free() {
    Module::with_new("vsplat", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i32_ty.as_type()], false);
        let f = m
            .add_function::<(), _>("g", fn_ty, Linkage::External)
            .expect("g");
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);

        let scalar: IntValue<'_, i32> = f.param(0).expect("p0").try_into().expect("i32 scalar");

        // Element `i32` and length `Len<4>` come from the result annotation;
        // `scalar: E::Value` then checks the scalar is an `IntValue<i32>`.
        let sp: VectorValue<'_, i32, Len<4>> = b.build_vec_splat(scalar, "sp").expect("splat");
        let _ = sp.as_value();

        b.build_ret_void();

        let txt = format!("{m}");
        assert!(
            txt.contains("%sp.splatinsert = insertelement <4 x i32> poison, i32 %0, i64 0\n"),
            "expected <4 x i32> splatinsert, got:\n{txt}"
        );
        assert!(
            txt.contains(
                "%sp.splat = shufflevector <4 x i32> %sp.splatinsert, <4 x i32> poison, <4 x i32> zeroinitializer\n"
            ),
            "expected <4 x i32> splat shuffle, got:\n{txt}"
        );
    })
}
