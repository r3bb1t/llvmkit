//! Generic-over-marker narrowing: [`IntWidth::narrow`] and
//! [`FloatKind::narrow`].
//!
//! ## Why this file exists
//!
//! `TryFrom<Value> for IntValue<'ctx, W, B>` is implemented **per
//! concrete marker only** (`IntDyn`, the Rust scalars, `Width<N>`).
//! There is no blanket impl for a generic `W`. Reaching `TryFrom` from
//! generic code was therefore possible only by propagating a
//! `where IntValue<'ctx, W, B>: TryFrom<Value<'ctx, B>>` clause through
//! every downstream signature — and not expressible at all where a trait
//! impl fixes the signature for you. In practice the crate did neither:
//! it re-attached the marker by assertion via the `pub(super)`
//! `IntValue::from_value_unchecked`, i.e. a marker applied without
//! proof.
//!
//! The headline tests below (`narrow_generic` / `narrow_generic_float`)
//! narrow behind a **bare** `W: IntWidth` / `K: FloatKind` bound. Every
//! other test here pins the behaviour those helpers inherit from the
//! per-marker `TryFrom` impls they delegate to.
//!
//! ## Upstream provenance
//!
//! The error split mirrors the distinction upstream draws between a
//! wrong-width integer (`Verifier`'s operand-width diagnostics, e.g.
//! `Instructions.cpp` width asserts) and a wrong *kind* entirely
//! (`Type::isIntegerTy` / `Type::isFloatingPointTy` predicate failure).

use llvmkit_ir::{
    ApFloat, ApFloatSemantics, ApFloatSign, BFloat, FloatDyn, FloatKind, FloatValue, Fp128, Half,
    IntDyn, IntValue, IntWidth, IrError, IrResult, Module, ModuleBrand, PpcFp128, TypeKindLabel,
    Value, Width, X86Fp80,
};

// --------------------------------------------------------------------------
// The deliverable: helpers generic over the marker.
// --------------------------------------------------------------------------

/// Generic over `W`. This is the whole point of the slice — before
/// [`IntWidth::narrow`] existed there was no impl this body could call,
/// because `TryFrom` is per-concrete-marker.
fn narrow_generic<'ctx, W: IntWidth, B: ModuleBrand + 'ctx>(
    v: Value<'ctx, B>,
) -> IrResult<IntValue<'ctx, W, B>> {
    W::narrow(v)
}

/// Float mirror of [`narrow_generic`].
fn narrow_generic_float<'ctx, K: FloatKind, B: ModuleBrand + 'ctx>(
    v: Value<'ctx, B>,
) -> IrResult<FloatValue<'ctx, K, B>> {
    K::narrow(v)
}

// --------------------------------------------------------------------------
// Int: the generic helper resolves at every marker family.
// --------------------------------------------------------------------------

/// Exercises the generic helper at **every** `IntWidth` marker, not a
/// sample: all six the `impl_int_width_scalar!` macro expands
/// (`bool`, `i8`, `i16`, `i32`, `i64`, `i128`), the erased marker
/// (`IntDyn`), and the const-generic marker (`Width<7>`) — i.e. all
/// three impl sites, exhaustively.
#[test]
fn narrow_generic_accepts_every_marker_family() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let bool_v = m.bool_type().const_zero().as_value();
        let i8_v = m.i8_type().const_zero().as_value();
        let i16_v = m.i16_type().const_zero().as_value();
        let i32_v = m.i32_type().const_zero().as_value();
        let i64_v = m.i64_type().const_zero().as_value();
        let i128_v = m.i128_type().const_zero().as_value();
        let i7_v = m.int_type_n::<7>().const_zero().as_value();

        // Rust-scalar markers — every marker the macro expands.
        let a: IntValue<'_, bool, _> = narrow_generic::<bool, _>(bool_v)?;
        assert_eq!(a.as_value(), bool_v);
        let b: IntValue<'_, i8, _> = narrow_generic::<i8, _>(i8_v)?;
        assert_eq!(b.as_value(), i8_v);
        let c: IntValue<'_, i16, _> = narrow_generic::<i16, _>(i16_v)?;
        assert_eq!(c.as_value(), i16_v);
        let d: IntValue<'_, i32, _> = narrow_generic::<i32, _>(i32_v)?;
        assert_eq!(d.as_value(), i32_v);
        let e: IntValue<'_, i64, _> = narrow_generic::<i64, _>(i64_v)?;
        assert_eq!(e.as_value(), i64_v);
        let f: IntValue<'_, i128, _> = narrow_generic::<i128, _>(i128_v)?;
        assert_eq!(f.as_value(), i128_v);

        // Erased marker.
        let g: IntValue<'_, IntDyn, _> = narrow_generic::<IntDyn, _>(i32_v)?;
        assert_eq!(g.as_value(), i32_v);

        // Const-generic marker.
        let h: IntValue<'_, Width<7>, _> = narrow_generic::<Width<7>, _>(i7_v)?;
        assert_eq!(h.as_value(), i7_v);

        Ok(())
    })
}

/// `IntDyn` is the erased marker: it accepts any integer width, since
/// its `TryFrom` only checks `TypeData::Integer { .. }`.
#[test]
fn int_dyn_accepts_any_width() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let vals = [
            m.bool_type().const_zero().as_value(),
            m.int_type_n::<7>().const_zero().as_value(),
            m.i32_type().const_zero().as_value(),
            m.i64_type().const_zero().as_value(),
        ];
        for v in vals {
            let narrowed = narrow_generic::<IntDyn, _>(v)?;
            assert_eq!(narrowed.as_value(), v);
        }
        Ok(())
    })
}

// --------------------------------------------------------------------------
// Int: the error split, inherited from the per-marker `TryFrom`.
// --------------------------------------------------------------------------

/// Right kind, wrong width → `OperandWidthMismatch`, carrying the
/// requested width as `lhs` and the actual width as `rhs`.
#[test]
fn wrong_width_is_operand_width_mismatch() {
    Module::with_new("c", |m| {
        let i64_v = m.i64_type().const_zero().as_value();
        let err = narrow_generic::<i32, _>(i64_v).unwrap_err();
        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 32, rhs: 64 });
    })
}

/// The const-generic marker reports the same split, with `N` as `lhs`.
#[test]
fn wrong_width_is_operand_width_mismatch_for_const_generic_marker() {
    Module::with_new("c", |m| {
        let i32_v = m.i32_type().const_zero().as_value();
        let err = narrow_generic::<Width<7>, _>(i32_v).unwrap_err();
        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 7, rhs: 32 });
    })
}

/// Wrong kind entirely (a pointer where an integer was asked for) →
/// `TypeMismatch`, **not** a width mismatch. This is the split the
/// delegation must preserve.
#[test]
fn wrong_kind_is_type_mismatch() {
    Module::with_new("c", |m| {
        let ptr_v = m.ptr_type(0).const_null().as_value();
        let err = narrow_generic::<i32, _>(ptr_v).unwrap_err();
        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: TypeKindLabel::Pointer,
            }
        );
    })
}

/// The erased marker rejects a non-integer too — erasure is over
/// *width*, not over *kind*.
#[test]
fn int_dyn_still_rejects_non_integer() {
    Module::with_new("c", |m| {
        let f32_v = m.f32_type().const_float(1.0_f32).as_value();
        let err = narrow_generic::<IntDyn, _>(f32_v).unwrap_err();
        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: TypeKindLabel::Float,
            }
        );
    })
}

// --------------------------------------------------------------------------
// Float.
// --------------------------------------------------------------------------

/// Float mirror of `narrow_generic_accepts_every_marker_family`:
/// scalar markers (`f32`, `f64`) and the erased marker (`FloatDyn`).
#[test]
fn narrow_generic_float_accepts_every_marker_family() -> Result<(), IrError> {
    Module::with_new("c", |m| {
        let zero = |sem| ApFloat::zero(sem, ApFloatSign::Positive);
        let f32_v = m.f32_type().const_float(1.5_f32).as_value();
        let f64_v = m.f64_type().const_double(2.5_f64).as_value();
        // Exotic kinds have no Rust-scalar constant ctor; go through ApFloat.
        let half_v = m
            .half_type()
            .const_ap_float(&zero(ApFloatSemantics::IeeeHalf))?
            .as_value();
        let bfloat_v = m
            .bfloat_type()
            .const_ap_float(&zero(ApFloatSemantics::BFloat))?
            .as_value();
        let fp128_v = m
            .fp128_type()
            .const_ap_float(&zero(ApFloatSemantics::IeeeQuad))?
            .as_value();
        let x86_v = m
            .x86_fp80_type()
            .const_ap_float(&zero(ApFloatSemantics::X87DoubleExtended))?
            .as_value();
        let ppc_v = m
            .ppc_fp128_type()
            .const_ap_float(&zero(ApFloatSemantics::PpcDoubleDouble))?
            .as_value();

        // Rust-scalar markers.
        let a: FloatValue<'_, f32, _> = narrow_generic_float::<f32, _>(f32_v)?;
        assert_eq!(a.as_value(), f32_v);
        let b: FloatValue<'_, f64, _> = narrow_generic_float::<f64, _>(f64_v)?;
        assert_eq!(b.as_value(), f64_v);

        // Every marker the `decl_struct_kind!` macro expands.
        let c: FloatValue<'_, Half, _> = narrow_generic_float::<Half, _>(half_v)?;
        assert_eq!(c.as_value(), half_v);
        let d: FloatValue<'_, BFloat, _> = narrow_generic_float::<BFloat, _>(bfloat_v)?;
        assert_eq!(d.as_value(), bfloat_v);
        let e: FloatValue<'_, Fp128, _> = narrow_generic_float::<Fp128, _>(fp128_v)?;
        assert_eq!(e.as_value(), fp128_v);
        let f: FloatValue<'_, X86Fp80, _> = narrow_generic_float::<X86Fp80, _>(x86_v)?;
        assert_eq!(f.as_value(), x86_v);
        let g: FloatValue<'_, PpcFp128, _> = narrow_generic_float::<PpcFp128, _>(ppc_v)?;
        assert_eq!(g.as_value(), ppc_v);

        // Erased marker accepts any kind.
        let h: FloatValue<'_, FloatDyn, _> = narrow_generic_float::<FloatDyn, _>(f32_v)?;
        assert_eq!(h.as_value(), f32_v);
        let i: FloatValue<'_, FloatDyn, _> = narrow_generic_float::<FloatDyn, _>(ppc_v)?;
        assert_eq!(i.as_value(), ppc_v);

        Ok(())
    })
}

/// An integer value narrowed at `K = f32` → `TypeMismatch`. The float
/// side has no width-mismatch case: a wrong float kind is a wrong
/// *kind*, since each kind is its own `TypeData` variant.
#[test]
fn integer_at_float_kind_is_type_mismatch() {
    Module::with_new("c", |m| {
        let i32_v = m.i32_type().const_zero().as_value();
        let err = narrow_generic_float::<f32, _>(i32_v).unwrap_err();
        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Float,
                got: TypeKindLabel::Integer,
            }
        );
    })
}

/// A `double` narrowed at `K = f32` is also a kind mismatch — `f32`'s
/// `TryFrom` matches only `TypeData::Float`.
#[test]
fn wrong_float_kind_is_type_mismatch() {
    Module::with_new("c", |m| {
        let f64_v = m.f64_type().const_double(2.5_f64).as_value();
        let err = narrow_generic_float::<f32, _>(f64_v).unwrap_err();
        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Float,
                got: TypeKindLabel::Double,
            }
        );
    })
}

/// `FloatDyn` rejects a non-float, mirroring `int_dyn_still_rejects_non_integer`.
#[test]
fn float_dyn_still_rejects_non_float() {
    Module::with_new("c", |m| {
        let ptr_v = m.ptr_type(0).const_null().as_value();
        let err = narrow_generic_float::<FloatDyn, _>(ptr_v).unwrap_err();
        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Float,
                got: TypeKindLabel::Pointer,
            }
        );
    })
}
