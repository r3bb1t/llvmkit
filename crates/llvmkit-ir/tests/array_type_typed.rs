//! Typed `ArrayType` / `ArrayValue` markers -- the const-generic
//! element + length retrofit (Slice 4 of the const-generic-vec-array plan).
//!
//! The array analog of `vector_type_typed.rs`: a `Module::array_type_n::<E, N>()`
//! constructor yields a statically typed `[N x E]` handle, and a runtime `Value`
//! narrows into the matching `ArrayValue<E, ArrLen<N>>` only when its element
//! type and element count both agree. The erased `ArrayType<'ctx>` /
//! `ArrayValue<'ctx>` forms keep working via the `Dyn` defaults. Arrays differ
//! from vectors only in the `u64` length and the `ArrLen`/`ArrLenDyn` markers;
//! unlike vectors, a zero-length array `[0 x T]` is legal.

use llvmkit_ir::{ArrLen, ArrayType, ArrayValue, IrError, Module};

/// `array_type_n::<i32, 4>()` prints as the canonical `[4 x i32]`, its
/// type-level `static_len()` reports the const-generic parameter, and the
/// erased `ArrayType::try_from` round-trips the same type back to the
/// dynamic handle.
#[test]
fn array_type_n_constructor_prints_and_round_trips() {
    Module::with_new("at", |m| {
        let at = m.array_type_n::<i32, 4>();
        assert_eq!(format!("{}", at.as_type()), "[4 x i32]");
        assert_eq!(at.static_len(), Some(4));
        assert_eq!(at.len(), 4);
        assert!(!at.is_empty());

        // Erased `TryFrom<Type>` still yields the fully dynamic form.
        let erased: ArrayType<'_> = ArrayType::try_from(at.as_type()).unwrap();
        assert_eq!(erased.static_len(), None);
        assert_eq!(erased.len(), 4);
    })
}

/// LLVM permits zero-length arrays `[0 x T]`; `array_type_n::<i32, 0>()` is a
/// valid statically typed handle (no `N > 0` assertion, unlike vectors).
#[test]
fn zero_length_array_is_allowed() {
    Module::with_new("at", |m| {
        let at = m.array_type_n::<i32, 0>();
        assert_eq!(format!("{}", at.as_type()), "[0 x i32]");
        assert_eq!(at.static_len(), Some(0));
        assert!(at.is_empty());
    })
}

/// A `[4 x i32]` runtime `Value` narrows via `try_into()` into the typed
/// `ArrayValue<i32, ArrLen<4>>`, and that typed handle widens back to the
/// erased form.
#[test]
fn value_narrows_to_matching_typed_array() {
    Module::with_new("at", |m| {
        let at = m.array_type_n::<i32, 4>();
        let v = at.as_type().get_poison().as_value();

        let typed: ArrayValue<'_, i32, ArrLen<4>> = v.try_into().expect("[4 x i32] narrows");
        assert_eq!(format!("{}", typed.ty().as_type()), "[4 x i32]");
        assert_eq!(typed.ty().static_len(), Some(4));

        // Static -> Dyn widening.
        let erased: ArrayValue<'_> = typed.into();
        assert_eq!(erased.ty().len(), 4);
    })
}

/// An element-count mismatch (`[2 x i32]` into `ArrLen<4>`) is rejected with
/// `ArrayLengthMismatch`.
#[test]
fn wrong_element_count_is_rejected() {
    Module::with_new("at", |m| {
        let i32_ty = m.i32_type();
        let v = m.array_type(i32_ty, 2).as_type().get_poison().as_value();

        let err = ArrayValue::<i32, ArrLen<4>>::try_from(v)
            .expect_err("[2 x i32] must not narrow to ArrLen<4>");
        assert_eq!(
            err,
            IrError::ArrayLengthMismatch {
                expected: 4,
                got: 2
            }
        );
    })
}

/// An element-type mismatch (`[4 x i64]` into `<i32, ArrLen<4>>`) is rejected
/// with a `TypeMismatch`.
#[test]
fn wrong_element_type_is_rejected() {
    Module::with_new("at", |m| {
        let i64_ty = m.i64_type();
        let v = m.array_type(i64_ty, 4).as_type().get_poison().as_value();

        let err = ArrayValue::<i32, ArrLen<4>>::try_from(v)
            .expect_err("[4 x i64] must not narrow to <i32, ArrLen<4>>");
        assert!(
            matches!(err, IrError::TypeMismatch { .. }),
            "expected TypeMismatch, got {err:?}",
        );
    })
}

/// The erased `ArrayValue<'ctx>` narrowing still accepts any array value
/// regardless of element / length -- the pre-retrofit behaviour is intact.
#[test]
fn erased_narrowing_accepts_any_array() {
    Module::with_new("at", |m| {
        let i64_ty = m.i64_type();
        let v = m.array_type(i64_ty, 3).as_type().get_poison().as_value();

        let erased: ArrayValue<'_> = v.try_into().expect("any array narrows to the dyn form");
        assert_eq!(erased.ty().len(), 3);
        assert_eq!(erased.ty().static_len(), None);
    })
}
