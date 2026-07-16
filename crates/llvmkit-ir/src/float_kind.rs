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

use super::r#type::sealed;

/// Sealed marker trait implemented by every IEEE-like float kind tag.
pub trait FloatKind: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// LangRef keyword for this kind. `None` for [`FloatDyn`].
    fn ieee_label() -> Option<&'static str>;

    /// Narrow an erased [`Value`] to this kind, **proving** the marker.
    ///
    /// The generic counterpart of the per-marker
    /// `TryFrom<Value> for FloatValue<'ctx, K, B>` impls, which exist only
    /// per concrete marker — never as a blanket over `K`. A bare
    /// `K: FloatKind` bound therefore affords narrowing here, whereas
    /// reaching `TryFrom` from generic code forces a
    /// `where FloatValue<'ctx, K, B>: TryFrom<Value<'ctx, B>>` clause onto
    /// every downstream signature, and is not expressible at all where a
    /// trait impl fixes the signature for you. Every implementation
    /// delegates to the matching `TryFrom`, so a non-matching value
    /// yields [`IrError::TypeMismatch`] exactly as it does today.
    ///
    /// [`FloatDyn`] accepts any float kind.
    ///
    /// ```
    /// # use llvmkit_ir::{FloatKind, FloatValue, IrResult, ModuleBrand, Value};
    /// fn narrow_generic<'ctx, K: FloatKind, B: ModuleBrand + 'ctx>(
    ///     v: Value<'ctx, B>,
    /// ) -> IrResult<FloatValue<'ctx, K, B>> {
    ///     K::narrow(v)
    /// }
    /// ```
    fn narrow<'ctx, B: ModuleBrand + 'ctx>(
        v: Value<'ctx, B>,
    ) -> IrResult<FloatValue<'ctx, Self, B>>;
}

impl sealed::Sealed for f32 {}
impl FloatKind for f32 {
    #[inline]
    fn ieee_label() -> Option<&'static str> {
        Some("float")
    }
    #[inline]
    fn narrow<'ctx, B: ModuleBrand + 'ctx>(
        v: Value<'ctx, B>,
    ) -> IrResult<FloatValue<'ctx, Self, B>> {
        FloatValue::<'ctx, Self, B>::try_from(v)
    }
}

impl sealed::Sealed for f64 {}
impl FloatKind for f64 {
    #[inline]
    fn ieee_label() -> Option<&'static str> {
        Some("double")
    }
    #[inline]
    fn narrow<'ctx, B: ModuleBrand + 'ctx>(
        v: Value<'ctx, B>,
    ) -> IrResult<FloatValue<'ctx, Self, B>> {
        FloatValue::<'ctx, Self, B>::try_from(v)
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
            #[inline]
            fn narrow<'ctx, B: ModuleBrand + 'ctx>(
                v: Value<'ctx, B>,
            ) -> IrResult<FloatValue<'ctx, Self, B>> {
                FloatValue::<'ctx, Self, B>::try_from(v)
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
    #[inline]
    fn narrow<'ctx, B: ModuleBrand + 'ctx>(
        v: Value<'ctx, B>,
    ) -> IrResult<FloatValue<'ctx, Self, B>> {
        FloatValue::<'ctx, Self, B>::try_from(v)
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
/// Upstream `CastInst::castIsValid` (`lib/IR/Instructions.cpp`) legalizes
/// FPExt/FPTrunc on a strict `getScalarSizeInBits` inequality alone
/// (the FPExt/FPTrunc arms compare `SrcScalarBitSize`/`DstScalarBitSize`,
/// not `getPrimitiveSizeInBits` -- numerically identical for the scalar
/// kinds modeled here, but the precise upstream API name matters for
/// future vector-typed follow-up work), with no restriction on which
/// `FloatKind` participates -- so the non-IEEE layouts (`X86Fp80` = 80
/// bits, `Fp128`/`PpcFp128` = 128 bits) take part in the same total order
/// as the IEEE-binary kinds: half/bfloat (16) < float (32) < double (64)
/// < x86_fp80 (80) < fp128/ppc_fp128 (128). `Fp128` and `PpcFp128` are
/// equal-width and neither is `FloatWiderThan` the other (see the
/// deliberately-absent rows below).
pub trait FloatWiderThan<K: FloatKind>: FloatKind + sealed::Sealed {}

macro_rules! decl_float_wider_than {
    ($wide:ty: $($narrow:ty),+ $(,)?) => {
        $( impl FloatWiderThan<$narrow> for $wide {} )+
    };
}
decl_float_wider_than!(f32: Half, BFloat);
decl_float_wider_than!(f64: Half, BFloat, f32);
decl_float_wider_than!(X86Fp80: Half, BFloat, f32, f64);
decl_float_wider_than!(Fp128: Half, BFloat, f32, f64, X86Fp80);
decl_float_wider_than!(PpcFp128: Half, BFloat, f32, f64, X86Fp80);
// Deliberately absent: Fp128 <-> PpcFp128 (both 128 bits) and
// Half <-> BFloat (both 16 bits) -- `castIsValid` requires a STRICT
// `getScalarSizeInBits` inequality (lib/IR/Instructions.cpp,
// `CastInst::castIsValid`'s FPExt/FPTrunc arms), so neither direction
// is a valid fpext/fptrunc for an equal-width pair.

// --------------------------------------------------------------------------
// IntoConstantFloat: type-driven dispatch for FloatType::const_* lifts
// --------------------------------------------------------------------------

use super::IrError;
use super::IrResult;
use super::constants::ConstantFloatValue;
use super::derived_types::FloatType;
use core::convert::Infallible;

/// Trait implemented by Rust scalar types that can be lifted to a
/// kind-`K` IR floating-point constant. Mirrors the int-side
/// [`crate::IntoConstantInt`] for the float family.
///
/// `Error = Infallible` for the lossless cases (so `f32 -> f32` and
/// `f32 -> f64` widening are infallible). `Error = IrError` for the
/// kind-erased target.
pub trait IntoConstantFloat<'ctx, K: FloatKind, B: ModuleBrand = Brand<'ctx>> {
    type Error;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, K, B>,
    ) -> Result<ConstantFloatValue<'ctx, K, B>, Self::Error>;
}

// f32 -> f32 (exact)
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantFloat<'ctx, f32, B> for f32 {
    type Error = Infallible;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, f32, B>,
    ) -> Result<ConstantFloatValue<'ctx, f32, B>, Infallible> {
        Ok(ty.const_float(self))
    }
}

// f64 -> f64 (exact)
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantFloat<'ctx, f64, B> for f64 {
    type Error = Infallible;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, f64, B>,
    ) -> Result<ConstantFloatValue<'ctx, f64, B>, Infallible> {
        Ok(ty.const_double(self))
    }
}

// f32 -> f64 (widen)
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantFloat<'ctx, f64, B> for f32 {
    type Error = Infallible;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, f64, B>,
    ) -> Result<ConstantFloatValue<'ctx, f64, B>, Infallible> {
        Ok(ty.const_double(f64::from(self)))
    }
}

// f32 -> FloatDyn (kind-erased)
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantFloat<'ctx, FloatDyn, B> for f32 {
    type Error = IrError;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, FloatDyn, B>,
    ) -> IrResult<ConstantFloatValue<'ctx, FloatDyn, B>> {
        Ok(ty.const_from_bits(u128::from(self.to_bits())))
    }
}

// f64 -> FloatDyn (kind-erased)
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantFloat<'ctx, FloatDyn, B> for f64 {
    type Error = IrError;
    fn into_constant_float(
        self,
        ty: FloatType<'ctx, FloatDyn, B>,
    ) -> IrResult<ConstantFloatValue<'ctx, FloatDyn, B>> {
        Ok(ty.const_from_bits(u128::from(self.to_bits())))
    }
}

// --------------------------------------------------------------------------
// IntoFloatValue: ergonomic operand input for the float IRBuilder
// --------------------------------------------------------------------------

use super::argument::Argument;
use super::instruction::{Instruction, state::Attached};
use super::module::{Brand, ModuleBrand, ModuleRef};
use super::value::{FloatValue, Value};

/// Inputs that can be lifted into a [`FloatValue<'ctx, K>`] operand
/// for the IR builder. Mirrors the int-side [`crate::IntoIntValue`]
/// for the float family.
pub trait IntoFloatValue<'ctx, K: FloatKind, B: ModuleBrand = Brand<'ctx>>: Sized {
    fn into_float_value(self, module: ModuleRef<'ctx, B>) -> IrResult<FloatValue<'ctx, K, B>>;
}

// Identity
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> IntoFloatValue<'ctx, K, B>
    for FloatValue<'ctx, K, B>
{
    #[inline]
    fn into_float_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<FloatValue<'ctx, K, B>> {
        Ok(self)
    }
}

// ConstantFloatValue<K> -> FloatValue<K>
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> IntoFloatValue<'ctx, K, B>
    for ConstantFloatValue<'ctx, K, B>
{
    #[inline]
    fn into_float_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<FloatValue<'ctx, K, B>> {
        Ok(FloatValue::<K, B>::from_value_unchecked(
            crate::value::IsValue::as_value(self),
        ))
    }
}

// Rust scalar -> FloatValue<K>
macro_rules! impl_into_float_value_static {
    ($rust_ty:ty, $marker:ty, $ty_method:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> IntoFloatValue<'ctx, $marker, B> for $rust_ty {
            fn into_float_value(
                self,
                module: ModuleRef<'ctx, B>,
            ) -> IrResult<FloatValue<'ctx, $marker, B>> {
                let ty = FloatType::<$marker, B>::new(
                    module.module().$ty_method().as_type().id(),
                    module,
                );
                match self.into_constant_float(ty) {
                    Ok(c) => Ok(FloatValue::<$marker, B>::from_value_unchecked(
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
        impl<'ctx, B: ModuleBrand + 'ctx> IntoFloatValue<'ctx, $k, B> for $source {
            #[inline]
            fn into_float_value(
                self,
                _module: ModuleRef<'ctx, B>,
            ) -> IrResult<FloatValue<'ctx, $k, B>> {
                FloatValue::<'ctx, $k, B>::try_from(self)
            }
        }
    )+ };
}
impl_into_float_value_via_try_from!(
    Argument<'ctx, B>,
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
    Value<'ctx, B>,
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
    Instruction<'ctx, Attached, B>,
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
/// projected from a [`Module`](crate::Module) without an extra runtime parameter.
/// Lets the IR builder accept `b.build_fp_load::<f32, _, _>(p, "v")?`
/// instead of `b.build_fp_load(f32_ty, p, "v")?`.
///
/// Not implemented for [`FloatDyn`] - the dyn-flavour builder
/// methods take an explicit [`FloatType<'ctx, FloatDyn>`] for the
/// runtime kind.
pub trait StaticFloatKind: FloatKind {
    /// Bit width of this IEEE-754 form. Mirrors
    /// `Type::getPrimitiveSizeInBits` in `lib/IR/Type.cpp` for the
    /// statically-known kinds (`half=16`, `bfloat=16`, `float=32`,
    /// `double=64`, `x86_fp80=80`, `fp128=128`, `ppc_fp128=128`).
    /// Usable as `K::STATIC_BITS` in `const { ... }` assertions.
    const STATIC_BITS: u32;

    fn ir_type<'ctx, B: ModuleBrand + 'ctx>(module: ModuleRef<'ctx, B>)
    -> FloatType<'ctx, Self, B>;
}

macro_rules! impl_static_float_kind {
    ($ty:ty, $method:ident, $bits:literal) => {
        impl StaticFloatKind for $ty {
            const STATIC_BITS: u32 = $bits;
            #[inline]
            fn ir_type<'ctx, B: ModuleBrand + 'ctx>(
                module: ModuleRef<'ctx, B>,
            ) -> FloatType<'ctx, Self, B> {
                FloatType::<Self, B>::new(module.module().$method().as_type().id(), module)
            }
        }
    };
}
impl_static_float_kind!(f32, f32_type, 32);
impl_static_float_kind!(f64, f64_type, 64);
impl_static_float_kind!(Half, half_type, 16);
impl_static_float_kind!(BFloat, bfloat_type, 16);
impl_static_float_kind!(Fp128, fp128_type, 128);
impl_static_float_kind!(X86Fp80, x86_fp80_type, 80);
impl_static_float_kind!(PpcFp128, ppc_fp128_type, 128);
