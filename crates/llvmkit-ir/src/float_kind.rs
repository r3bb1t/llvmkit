//! Sealed marker types for IEEE/non-IEEE floating-point kinds.
//!
//! Mirrors the closed set of float kinds in `Type.h` (`HalfTyID`,
//! `BFloatTyID`, `FloatTyID`, `DoubleTyID`, `X86_FP80TyID`,
//! `FP128TyID`, `PPC_FP128TyID`). Encoding the kind in the type
//! system lets `FloatValue<'ctx, f32>` be a different type from
//! `FloatValue<'ctx, f64>` — a `build_fp_add` between them is a
//! compile error rather than a runtime check.
//!
//! ## Rust scalars *are* the markers
//!
//! Whenever LLVM has a kind with a Rust-scalar counterpart, the
//! marker is the Rust type itself: `f32` (binary32) and `f64`
//! (binary64). The exotic kinds without a Rust counterpart use
//! struct markers ([`Half`], [`BFloat`], [`Fp128`], [`X86Fp80`],
//! [`PpcFp128`]).
//!
//! [`FloatDyn`] is the runtime-checked escape hatch for parsed IR and
//! APIs that have not yet been narrowed. It is a dedicated marker for
//! the float side; integer-erased uses [`crate::int_width::IntDyn`].

use core::fmt;

use crate::r#type::sealed;

/// Sealed marker trait implemented by every IEEE-like float kind tag.
pub trait FloatKind: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// LangRef keyword for this kind. `None` for [`FloatDyn`].
    fn ieee_label() -> Option<&'static str>;
}

impl sealed::Sealed for f32 {}
impl FloatKind for f32 {
    #[inline]
    fn ieee_label() -> Option<&'static str> {
        Some("float")
    }
}

impl sealed::Sealed for f64 {}
impl FloatKind for f64 {
    #[inline]
    fn ieee_label() -> Option<&'static str> {
        Some("double")
    }
}

macro_rules! decl_struct_kind {
    ($(#[$attr:meta])* $name:ident, $label:expr) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name;
        impl sealed::Sealed for $name {}
        impl FloatKind for $name {
            #[inline]
            fn ieee_label() -> Option<&'static str> {
                Some($label)
            }
        }
    };
}

decl_struct_kind!(
    /// IEEE 754 binary16. Mirrors `Type::HalfTyID`.
    Half,
    "half"
);
decl_struct_kind!(
    /// Brain-float (1 sign / 8 exp / 7 frac). Mirrors `Type::BFloatTyID`.
    BFloat,
    "bfloat"
);
decl_struct_kind!(
    /// IEEE 754 binary128. Mirrors `Type::FP128TyID`.
    Fp128,
    "fp128"
);
decl_struct_kind!(
    /// X87 80-bit extended precision. Mirrors `Type::X86_FP80TyID`.
    X86Fp80,
    "x86_fp80"
);
decl_struct_kind!(
    /// PowerPC double-double. Mirrors `Type::PPC_FP128TyID`.
    PpcFp128,
    "ppc_fp128"
);

/// Kind-erased marker. The handle still tracks its kind as runtime
/// data; this marker only signals "the type system does not know which
/// IEEE/non-IEEE flavour."
///
/// Used by parsed IR and APIs that genuinely cannot be statically
/// typed. Distinct from [`crate::int_width::IntDyn`] — the integer and
/// float sides each carry their own erasure marker so trait coherence
/// stays sane (a single shared `Dyn` would simultaneously implement
/// `IntWidth` and `FloatKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FloatDyn;
impl sealed::Sealed for FloatDyn {}
impl FloatKind for FloatDyn {
    #[inline]
    fn ieee_label() -> Option<&'static str> {
        None
    }
}

// --------------------------------------------------------------------------
// FloatWiderThan<K>: compile-time precision invariants for fpext / fptrunc
// --------------------------------------------------------------------------

/// Sealed: `Self` (the wider precision) has strictly more precision
/// than `K`. Mirrors the LangRef invariant on `fpext` (destination
/// must be strictly higher precision than source) and `fptrunc`
/// (destination must be strictly lower precision than source).
///
/// We restrict to the IEEE-binary precision order: half/bfloat (16) <
/// float (32) < double (64) < fp128 (128). `X86Fp80` and `PpcFp128`
/// have non-IEEE layouts; cross-format casts go through the `_dyn`
/// fallback rather than this trait.
pub trait FloatWiderThan<K: FloatKind>: FloatKind + sealed::Sealed {}

macro_rules! decl_float_wider_than {
    ($wide:ty: $($narrow:ty),+ $(,)?) => {
        $( impl FloatWiderThan<$narrow> for $wide {} )+
    };
}
decl_float_wider_than!(f32: Half, BFloat);
decl_float_wider_than!(f64: Half, BFloat, f32);
decl_float_wider_than!(Fp128: Half, BFloat, f32, f64);

// --------------------------------------------------------------------------
// IntoConstantFloat: type-driven dispatch for FloatType::const_* lifts
// --------------------------------------------------------------------------

use crate::IrError;
use crate::IrResult;
use crate::constants::ConstantFloatValue;
use crate::derived_types::FloatType;
use core::convert::Infallible;

/// Trait implemented by Rust scalar types that can be lifted to a
/// kind-`K` IR floating-point constant. Mirrors the int-side
/// [`crate::IntoConstantInt`] for the float family.
///
/// `Error = Infallible` for the lossless cases (so `f32 -> f32` and
/// `f32 -> f64` widening are infallible). `Error = IrError` for the
/// kind-erased target.
pub trait IntoConstantFloat<'ctx, K: FloatKind> {
    type Error;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, K>,
    ) -> Result<ConstantFloatValue<'ctx, K>, Self::Error>;
}

// f32 -> f32 (exact)
impl<'ctx> IntoConstantFloat<'ctx, f32> for f32 {
    type Error = Infallible;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, f32>,
    ) -> Result<ConstantFloatValue<'ctx, f32>, Infallible> {
        Ok(ty.const_float(self))
    }
}

// f64 -> f64 (exact)
impl<'ctx> IntoConstantFloat<'ctx, f64> for f64 {
    type Error = Infallible;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, f64>,
    ) -> Result<ConstantFloatValue<'ctx, f64>, Infallible> {
        Ok(ty.const_double(self))
    }
}

// f32 -> f64 (widen)
impl<'ctx> IntoConstantFloat<'ctx, f64> for f32 {
    type Error = Infallible;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, f64>,
    ) -> Result<ConstantFloatValue<'ctx, f64>, Infallible> {
        Ok(ty.const_double(f64::from(self)))
    }
}

// f32 -> FloatDyn (kind-erased)
impl<'ctx> IntoConstantFloat<'ctx, FloatDyn> for f32 {
    type Error = IrError;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, FloatDyn>,
    ) -> IrResult<ConstantFloatValue<'ctx, FloatDyn>> {
        Ok(ty.const_from_bits(u128::from(self.to_bits())))
    }
}

// f64 -> FloatDyn (kind-erased)
impl<'ctx> IntoConstantFloat<'ctx, FloatDyn> for f64 {
    type Error = IrError;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, FloatDyn>,
    ) -> IrResult<ConstantFloatValue<'ctx, FloatDyn>> {
        Ok(ty.const_from_bits(u128::from(self.to_bits())))
    }
}

// --------------------------------------------------------------------------
// IntoFloatValue: ergonomic operand input for the float IRBuilder
// --------------------------------------------------------------------------

use crate::module::Module;
use crate::value::FloatValue;

/// Inputs that can be lifted into a [`FloatValue<'ctx, K>`] operand
/// for the IR builder. Mirrors the int-side [`crate::IntoIntValue`]
/// for the float family.
pub trait IntoFloatValue<'ctx, K: FloatKind>: Sized {
    fn into_float_value(self, module: &'ctx Module<'ctx>) -> IrResult<FloatValue<'ctx, K>>;
}

// Identity
impl<'ctx, K: FloatKind> IntoFloatValue<'ctx, K> for FloatValue<'ctx, K> {
    #[inline]
    fn into_float_value(self, _module: &'ctx Module<'ctx>) -> IrResult<FloatValue<'ctx, K>> {
        Ok(self)
    }
}

// ConstantFloatValue<K> -> FloatValue<K>
impl<'ctx, K: FloatKind> IntoFloatValue<'ctx, K> for ConstantFloatValue<'ctx, K> {
    #[inline]
    fn into_float_value(self, _module: &'ctx Module<'ctx>) -> IrResult<FloatValue<'ctx, K>> {
        Ok(FloatValue::<K>::from_value_unchecked(
            crate::value::IsValue::as_value(self),
        ))
    }
}

// Rust scalar -> FloatValue<K>
macro_rules! impl_into_float_value_static {
    ($rust_ty:ty, $marker:ty, $ty_method:ident) => {
        impl<'ctx> IntoFloatValue<'ctx, $marker> for $rust_ty {
            fn into_float_value(
                self,
                module: &'ctx Module<'ctx>,
            ) -> IrResult<FloatValue<'ctx, $marker>> {
                let ty = module.$ty_method();
                match self.into_constant_float(ty) {
                    Ok(c) => Ok(FloatValue::<$marker>::from_value_unchecked(
                        crate::value::IsValue::as_value(c),
                    )),
                    Err(_) => unreachable!(
                        "IntoConstantFloat for static target is infallible per the trait impls"
                    ),
                }
            }
        }
    };
}
impl_into_float_value_static!(f32, f32, f32_type);
impl_into_float_value_static!(f64, f64, f64_type);
impl_into_float_value_static!(f32, f64, f64_type);

// Erased / heterogeneous handles narrow via TryFrom.
macro_rules! impl_into_float_value_via_try_from {
    ($source:ty, $($k:ty),+ $(,)?) => { $(
        impl<'ctx> IntoFloatValue<'ctx, $k> for $source {
            #[inline]
            fn into_float_value(
                self,
                _module: &'ctx Module<'ctx>,
            ) -> IrResult<FloatValue<'ctx, $k>> {
                FloatValue::<'ctx, $k>::try_from(self)
            }
        }
    )+ };
}
impl_into_float_value_via_try_from!(
    crate::argument::Argument<'ctx>,
    f32,
    f64,
    Half,
    BFloat,
    Fp128,
    X86Fp80,
    PpcFp128,
    FloatDyn,
);
impl_into_float_value_via_try_from!(
    crate::value::Value<'ctx>,
    f32,
    f64,
    Half,
    BFloat,
    Fp128,
    X86Fp80,
    PpcFp128,
    FloatDyn,
);
impl_into_float_value_via_try_from!(
    crate::instruction::Instruction<'ctx>,
    f32,
    f64,
    Half,
    BFloat,
    Fp128,
    X86Fp80,
    PpcFp128,
    FloatDyn,
);

// --------------------------------------------------------------------------
// StaticFloatKind: marker-only type lookup (mirrors StaticIntWidth)
// --------------------------------------------------------------------------

/// Sealed: float-kind markers whose `FloatType<'ctx, Self>` can be
/// projected from a [`Module`] without an extra runtime parameter.
/// Lets the IR builder accept `b.build_fp_load::<f32>(p, "v")?`
/// instead of `b.build_fp_load(f32_ty, p, "v")?`.
///
/// Not implemented for [`FloatDyn`] - the dyn-flavour builder
/// methods take an explicit [`FloatType<'ctx, FloatDyn>`] for the
/// runtime kind.
pub trait StaticFloatKind: FloatKind {
    fn ir_type<'ctx>(module: &'ctx Module<'ctx>) -> FloatType<'ctx, Self>
    where
        Self: Sized;
}

macro_rules! impl_static_float_kind {
    ($ty:ty, $method:ident) => {
        impl StaticFloatKind for $ty {
            #[inline]
            fn ir_type<'ctx>(module: &'ctx Module<'ctx>) -> FloatType<'ctx, Self> {
                module.$method()
            }
        }
    };
}
impl_static_float_kind!(f32, f32_type);
impl_static_float_kind!(f64, f64_type);
impl_static_float_kind!(Half, half_type);
impl_static_float_kind!(BFloat, bfloat_type);
impl_static_float_kind!(Fp128, fp128_type);
impl_static_float_kind!(X86Fp80, x86_fp80_type);
impl_static_float_kind!(PpcFp128, ppc_fp128_type);
