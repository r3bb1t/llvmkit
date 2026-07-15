//! Typed `VectorType` / `VectorValue` markers -- the const-generic
//! element + length retrofit (Slice 3 of the const-generic-vec-array plan).
//!
//! Mirrors the shape of `custom_width.rs` (the `IntType<'ctx, Width<N>>`
//! smoke test): a `Module::vector_type_n::<E, N>()` constructor yields a
//! statically typed `<N x E>` handle, and a runtime `Value` narrows into
//! the matching `VectorValue<E, Len<N>>` only when its element type and
//! lane count both agree. The erased `VectorType<'ctx>` / `VectorValue<'ctx>`
//! forms keep working via the `Dyn` defaults.

use llvmkit_ir::{IrError, Len, Module, VectorType, VectorValue};

/// `vector_type_n::<i32, 4>()` prints as the canonical `<4 x i32>`, its
/// type-level `static_len()` reports the const-generic parameter, and the
/// erased `VectorType::try_from` round-trips the same type back to the
/// dynamic handle.
#[test]
fn vector_type_n_constructor_prints_and_round_trips() {
    Module::with_new("vt", |m| {
        let vt = m.vector_type_n::<i32, 4>();
        assert_eq!(format!("{}", vt.as_type()), "<4 x i32>");
        assert_eq!(vt.static_len(), Some(4));
        assert_eq!(vt.min_len(), 4);
        assert!(!vt.is_scalable());

        // Erased `TryFrom<Type>` still yields the fully dynamic form.
        let erased: VectorType<'_> = VectorType::try_from(vt.as_type()).unwrap();
        assert_eq!(erased.static_len(), None);
        assert_eq!(erased.min_len(), 4);
    })
}

/// A `<4 x i32>` runtime `Value` narrows via `try_into()` into the typed
/// `VectorValue<i32, Len<4>>`, and that typed handle widens back to the
/// erased form.
#[test]
fn value_narrows_to_matching_typed_vector() {
    Module::with_new("vt", |m| {
        let vt = m.vector_type_n::<i32, 4>();
        let v = vt.as_type().get_poison().as_value();

        let typed: VectorValue<'_, i32, Len<4>> = v.try_into().expect("<4 x i32> narrows");
        assert_eq!(format!("{}", typed.ty().as_type()), "<4 x i32>");
        assert_eq!(typed.ty().static_len(), Some(4));

        // Static -> Dyn widening.
        let erased: VectorValue<'_> = typed.into();
        assert_eq!(erased.ty().min_len(), 4);
    })
}

/// A lane-count mismatch (`<2 x i32>` into `Len<4>`) is rejected with the
/// vector-length arm of `OperandWidthMismatch`.
#[test]
fn wrong_lane_count_is_rejected() {
    Module::with_new("vt", |m| {
        let i32_ty = m.i32_type();
        let v = m
            .vector_type(i32_ty, 2, false)
            .as_type()
            .get_poison()
            .as_value();

        let err = VectorValue::<i32, Len<4>>::try_from(v)
            .expect_err("<2 x i32> must not narrow to Len<4>");
        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 4, rhs: 2 });
    })
}

/// An element-type mismatch (`<4 x i64>` into `<i32, Len<4>>`) is rejected
/// with a `TypeMismatch`.
#[test]
fn wrong_element_type_is_rejected() {
    Module::with_new("vt", |m| {
        let i64_ty = m.i64_type();
        let v = m
            .vector_type(i64_ty, 4, false)
            .as_type()
            .get_poison()
            .as_value();

        let err = VectorValue::<i32, Len<4>>::try_from(v)
            .expect_err("<4 x i64> must not narrow to <i32, Len<4>>");
        assert!(
            matches!(err, IrError::TypeMismatch { .. }),
            "expected TypeMismatch, got {err:?}",
        );
    })
}

/// The erased `VectorValue<'ctx>` narrowing still accepts any vector value
/// regardless of element / length -- the pre-retrofit behaviour is intact.
#[test]
fn erased_narrowing_accepts_any_vector() {
    Module::with_new("vt", |m| {
        let i64_ty = m.i64_type();
        let v = m
            .vector_type(i64_ty, 3, false)
            .as_type()
            .get_poison()
            .as_value();

        let erased: VectorValue<'_> = v.try_into().expect("any vector narrows to the dyn form");
        assert_eq!(erased.ty().min_len(), 3);
        assert_eq!(erased.ty().static_len(), None);
    })
}
