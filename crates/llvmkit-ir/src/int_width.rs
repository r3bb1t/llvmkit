//! Sealed marker types for integer bit-widths.
//!
//! Mirrors the static-width split LLVM C++ keeps implicit (`Type::isIntegerTy(32)`).
//! In Rust we encode it in the type system: an [`IntType<'ctx, B32>`](
//! crate::IntType) is a different type from [`IntType<'ctx, B64>`] and
//! mixing them at a builder call site is a compile error rather than a
//! runtime [`IrError::OperandWidthMismatch`].
//!
//! The static markers ([`B1`], [`B8`], [`B16`], [`B32`], [`B64`],
//! [`B128`]) cover every width that has a native Rust scalar
//! counterpart. Other static widths are uncommon enough that the
//! [`BDyn`] runtime-checked marker is the right tool — it preserves the
//! `iN` width as runtime data while keeping the same handle type
//! family. Parsed `.ll` always lands in [`BDyn`] until the consumer
//! narrows it via [`TryFrom`].
//!
//! The trait is **sealed** — the closed set of LLVM widths is part of
//! the IR spec, not an extension point. The marker structs themselves
//! are user-visible (so external callers can spell `IntType<'ctx, B32>`
//! in their own type signatures).

use core::fmt;

use crate::r#type::sealed;

/// Sealed marker trait implemented by every integer width tag.
pub trait IntWidth: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// Static bit-width if known at compile time, else `None` (used by
    /// [`BDyn`]).
    fn static_bits() -> Option<u32>;
}

macro_rules! decl_static_width {
    ($(#[$attr:meta])* $name:ident, $bits:expr) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name;
        impl sealed::Sealed for $name {}
        impl IntWidth for $name {
            #[inline]
            fn static_bits() -> Option<u32> { Some($bits) }
        }
    };
}

decl_static_width!(
    /// `i1` width marker. Mirrors `Type::getInt1Ty`.
    B1, 1
);
decl_static_width!(
    /// `i8` width marker.
    B8, 8
);
decl_static_width!(
    /// `i16` width marker.
    B16, 16
);
decl_static_width!(
    /// `i32` width marker.
    B32, 32
);
decl_static_width!(
    /// `i64` width marker.
    B64, 64
);
decl_static_width!(
    /// `i128` width marker.
    B128, 128
);

/// Width-erased marker. The handle still tracks its width as runtime
/// data; this marker only signals "the type system does not know which
/// width."
///
/// Used by parsed IR (where the source-level width is whatever the
/// `.ll` says) and by APIs that genuinely cannot be statically typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BDyn;
impl sealed::Sealed for BDyn {}
impl IntWidth for BDyn {
    #[inline]
    fn static_bits() -> Option<u32> {
        None
    }
}

// --------------------------------------------------------------------------
// IntoConstantInt: type-driven dispatch for IntType::const_int
// --------------------------------------------------------------------------

use crate::IrError;
use crate::IrResult;
use crate::constants::ConstantIntValue;
use crate::derived_types::IntType;
use core::convert::Infallible;

/// Trait implemented by Rust scalar types that can be lifted to a
/// width-`W` IR integer constant. The Rust input type drives the
/// extension scheme: `iN` impls sign-extend, `uN` impls zero-extend,
/// `bool` becomes `i1` true/false.
///
/// `Error = Infallible` for the lossless cases (so [`IntType::const_int`](
/// crate::IntType::const_int) is infallible). `Error = IrError` for
/// cases that require runtime fit checking (narrowing or [`BDyn`]).
pub trait IntoConstantInt<'ctx, W: IntWidth> {
    type Error;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, W>,
    ) -> Result<ConstantIntValue<'ctx, W>, Self::Error>;
}

// ---- Width-exact infallible cases (Error = Infallible) ----

impl<'ctx> IntoConstantInt<'ctx, B1> for bool {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, B1>,
    ) -> Result<ConstantIntValue<'ctx, B1>, Infallible> {
        Ok(ty
            .const_int_raw(u64::from(self), false)
            .unwrap_or_else(|_| unreachable!("bool fits in i1")))
    }
}

macro_rules! impl_into_constant_int_signed_exact {
    ($rust_ty:ty, $marker:ident) => {
        impl<'ctx> IntoConstantInt<'ctx, $marker> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, $marker>,
            ) -> Result<ConstantIntValue<'ctx, $marker>, Infallible> {
                let raw = self as i64 as u64;
                Ok(ty
                    .const_int_raw(raw, true)
                    .unwrap_or_else(|_| unreachable!("native signed int fits exactly")))
            }
        }
    };
}
macro_rules! impl_into_constant_int_unsigned_exact {
    ($rust_ty:ty, $marker:ident) => {
        impl<'ctx> IntoConstantInt<'ctx, $marker> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, $marker>,
            ) -> Result<ConstantIntValue<'ctx, $marker>, Infallible> {
                Ok(ty
                    .const_int_raw(u64::from(self), false)
                    .unwrap_or_else(|_| unreachable!("native unsigned int fits exactly")))
            }
        }
    };
}
impl_into_constant_int_signed_exact!(i8, B8);
impl_into_constant_int_signed_exact!(i16, B16);
impl_into_constant_int_signed_exact!(i32, B32);
impl_into_constant_int_signed_exact!(i64, B64);
impl_into_constant_int_unsigned_exact!(u8, B8);
impl_into_constant_int_unsigned_exact!(u16, B16);
impl_into_constant_int_unsigned_exact!(u32, B32);
impl_into_constant_int_unsigned_exact!(u64, B64);

// i128/u128 "" use arbitrary-precision path
impl<'ctx> IntoConstantInt<'ctx, B128> for i128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, B128>,
    ) -> Result<ConstantIntValue<'ctx, B128>, Infallible> {
        let bits = self as u128;
        let lo = (bits & 0xffff_ffff_ffff_ffff) as u64;
        let hi = (bits >> 64) as u64;
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("i128 fits in B128")))
    }
}
impl<'ctx> IntoConstantInt<'ctx, B128> for u128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, B128>,
    ) -> Result<ConstantIntValue<'ctx, B128>, Infallible> {
        let lo = (self & 0xffff_ffff_ffff_ffff) as u64;
        let hi = (self >> 64) as u64;
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("u128 fits in B128")))
    }
}

// ---- Widening (smaller signed Rust int "" wider static W); infallible
// ----
macro_rules! impl_into_constant_int_signed_widen {
    ($rust_ty:ty, $($marker:ident),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, $marker> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, $marker>)
                -> Result<ConstantIntValue<'ctx, $marker>, Infallible>
            {
                let widened = self as i64 as u64;
                Ok(ty.const_int_raw(widened, true).unwrap_or_else(|_| {
                    unreachable!("signed Rust int fits losslessly when sign-extending to wider W")
                }))
            }
        }
    )+ };
}
macro_rules! impl_into_constant_int_unsigned_widen {
    ($rust_ty:ty, $($marker:ident),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, $marker> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, $marker>)
                -> Result<ConstantIntValue<'ctx, $marker>, Infallible>
            {
                Ok(ty.const_int_raw(u64::from(self), false).unwrap_or_else(|_| {
                    unreachable!("unsigned Rust int fits losslessly when zero-extending to wider W")
                }))
            }
        }
    )+ };
}
impl_into_constant_int_signed_widen!(i8, B16, B32, B64);
impl_into_constant_int_signed_widen!(i16, B32, B64);
impl_into_constant_int_signed_widen!(i32, B64);
impl_into_constant_int_unsigned_widen!(u8, B16, B32, B64);
impl_into_constant_int_unsigned_widen!(u16, B32, B64);
impl_into_constant_int_unsigned_widen!(u32, B64);
// Widen i128-source-with-narrower-width to B128: handled by exact above.
// For i8..i64 -> B128 use arbitrary-precision:
macro_rules! impl_into_constant_int_signed_widen_b128 {
    ($($rust_ty:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, B128> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, B128>)
                -> Result<ConstantIntValue<'ctx, B128>, Infallible>
            {
                let v = self as i128 as u128;
                let lo = (v & 0xffff_ffff_ffff_ffff) as u64;
                let hi = (v >> 64) as u64;
                Ok(ty.const_int_arbitrary_precision(&[lo, hi]).unwrap_or_else(|_| {
                    unreachable!("signed Rust int fits in B128")
                }))
            }
        }
    )+ };
}
macro_rules! impl_into_constant_int_unsigned_widen_b128 {
    ($($rust_ty:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, B128> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, B128>)
                -> Result<ConstantIntValue<'ctx, B128>, Infallible>
            {
                let v = u128::from(self);
                let lo = (v & 0xffff_ffff_ffff_ffff) as u64;
                let hi = (v >> 64) as u64;
                Ok(ty.const_int_arbitrary_precision(&[lo, hi]).unwrap_or_else(|_| {
                    unreachable!("unsigned Rust int fits in B128")
                }))
            }
        }
    )+ };
}
impl_into_constant_int_signed_widen_b128!(i8, i16, i32, i64);
impl_into_constant_int_unsigned_widen_b128!(u8, u16, u32, u64);

// ---- BDyn target: runtime fit-check ----
macro_rules! impl_into_constant_int_dyn {
    (signed $($rust_ty:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, BDyn> for $rust_ty {
            type Error = IrError;
            fn into_constant_int(self, ty: IntType<'ctx, BDyn>) -> IrResult<ConstantIntValue<'ctx, BDyn>> {
                ty.const_int_raw(self as i64 as u64, true)
            }
        }
    )+ };
    (unsigned $($rust_ty:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, BDyn> for $rust_ty {
            type Error = IrError;
            fn into_constant_int(self, ty: IntType<'ctx, BDyn>) -> IrResult<ConstantIntValue<'ctx, BDyn>> {
                ty.const_int_raw(u64::from(self), false)
            }
        }
    )+ };
}
impl_into_constant_int_dyn!(signed i8, i16, i32, i64);
impl_into_constant_int_dyn!(unsigned u8, u16, u32, u64);
impl<'ctx> IntoConstantInt<'ctx, BDyn> for bool {
    type Error = IrError;
    fn into_constant_int(self, ty: IntType<'ctx, BDyn>) -> IrResult<ConstantIntValue<'ctx, BDyn>> {
        ty.const_int_raw(u64::from(self), false)
    }
}
