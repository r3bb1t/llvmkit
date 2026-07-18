//! Sealed marker types for integer bit-widths.
//!
//! Mirrors the static-width split LLVM C++ keeps implicit
//! (`Type::isIntegerTy(32)`). In Rust we encode the width in the type
//! system: an [`IntType<'ctx, i32>`](crate::IntType) is a different
//! type from [`IntType<'ctx, i64>`] and mixing them at a builder call
//! site is a compile error rather than a runtime
//! [`IrError::OperandWidthMismatch`].
//!
//! ## Rust scalars *are* the markers
//!
//! Whenever LLVM has a width with a Rust-scalar counterpart, the
//! marker is the Rust type itself: `bool` (i1), `i8`, `i16`, `i32`,
//! `i64`, `i128`. There are no `B1` / `B8` / `B32` newtype wrappers —
//! the Rust scalar already names the width. Only [`IntDyn`] needs a
//! dedicated struct, since "width unknown at compile time" has no
//! Rust counterpart.
//!
//! Marker signedness is **not** semantic — LLVM IR has no signedness
//! at the type level. The signed Rust scalars (`i8` … `i128`) are
//! used as canonical markers because they are the natural literal
//! type for typed-builder call sites; unsigned Rust scalars
//! (`u8` … `u128`) still implement [`IntoConstantInt`] so a `u32`
//! literal lifts to a constant in the matching `i32`-marked IR type.
//! Operations select signedness on a per-call basis (e.g. `udiv`
//! vs `sdiv`, `IntPredicate::Slt` vs `IntPredicate::Ult`).
//!
//! Parsed `.ll` always lands in [`IntDyn`] until the consumer narrows
//! it via [`TryFrom`].
//!
//! The trait is **sealed** — the closed set of LLVM widths is part of
//! the IR spec, not an extension point.

use core::fmt;

use super::constants::ConstantIntValue;
use super::module::{Brand, ModuleBrand, ModuleRef};
use super::r#type::sealed;
use super::value::{IntValue, IsValue, Value};

/// Sealed marker trait implemented by every integer width tag.
pub trait IntWidth: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// Static bit-width if known at compile time, else `None` (used by
    /// [`IntDyn`]).
    fn static_bits() -> Option<u32>;

    /// Narrow an erased [`Value`] to this width, **proving** the marker.
    ///
    /// The generic counterpart of the per-marker
    /// `TryFrom<Value> for IntValue<'ctx, W, B>` impls, which exist only
    /// per concrete marker ([`IntDyn`], the Rust scalars, [`Width<N>`](Width))
    /// — never as a blanket over `W`. A bare `W: IntWidth` bound therefore
    /// affords narrowing here, whereas reaching `TryFrom` from generic code
    /// forces a `where IntValue<'ctx, W, B>: TryFrom<Value<'ctx, B>>` clause
    /// onto every downstream signature, and is not expressible at all where
    /// a trait impl fixes the signature for you. Every implementation
    /// delegates to the matching `TryFrom`, so the error split is
    /// inherited rather than restated:
    ///
    /// - right kind, wrong width → [`IrError::OperandWidthMismatch`]
    /// - wrong kind entirely → [`IrError::TypeMismatch`]
    ///
    /// [`IntDyn`] accepts any integer width.
    ///
    /// ```
    /// # use llvmkit_ir::{IntValue, IntWidth, IrResult, ModuleBrand, Value};
    /// fn narrow_generic<'ctx, W: IntWidth, B: ModuleBrand + 'ctx>(
    ///     v: Value<'ctx, B>,
    /// ) -> IrResult<IntValue<'ctx, W, B>> {
    ///     W::narrow(v)
    /// }
    /// ```
    fn narrow<'ctx, B: ModuleBrand + 'ctx>(v: Value<'ctx, B>) -> IrResult<IntValue<'ctx, Self, B>>;
}

macro_rules! impl_int_width_scalar {
    ($rust_ty:ty, $bits:expr) => {
        impl sealed::Sealed for $rust_ty {}
        impl IntWidth for $rust_ty {
            #[inline]
            fn static_bits() -> Option<u32> {
                Some($bits)
            }
            #[inline]
            fn narrow<'ctx, B: ModuleBrand + 'ctx>(
                v: Value<'ctx, B>,
            ) -> IrResult<IntValue<'ctx, Self, B>> {
                IntValue::<'ctx, Self, B>::try_from(v)
            }
        }
    };
}

impl_int_width_scalar!(bool, 1);
impl_int_width_scalar!(i8, 8);
impl_int_width_scalar!(i16, 16);
impl_int_width_scalar!(i32, 32);
impl_int_width_scalar!(i64, 64);
impl_int_width_scalar!(i128, 128);

/// Width-erased marker. The handle still tracks its width as runtime
/// data; this marker only signals "the type system does not know which
/// width."
///
/// Used by parsed IR (where the source-level width is whatever the
/// `.ll` says) and by APIs that genuinely cannot be statically typed.
/// Distinct from [`crate::float_kind::FloatDyn`] — the integer and
/// float sides each carry their own erasure marker so trait coherence
/// stays sane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IntDyn;
impl sealed::Sealed for IntDyn {}
impl IntWidth for IntDyn {
    #[inline]
    fn static_bits() -> Option<u32> {
        None
    }
    #[inline]
    fn narrow<'ctx, B: ModuleBrand + 'ctx>(v: Value<'ctx, B>) -> IrResult<IntValue<'ctx, Self, B>> {
        IntValue::<'ctx, Self, B>::try_from(v)
    }
}

/// Const-generic marker for arbitrary integer widths. Use this when
/// the width is statically known but doesn't have a Rust scalar
/// counterpart - e.g. an `i7` for a packed bit-field, an `i48`
/// for a 6-byte hash, or any other LLVM-permitted custom width.
///
/// `Width<N>` participates in the same trait surface as the Rust
/// scalar markers: it implements [`IntWidth`] and [`StaticIntWidth`],
/// gets a [`crate::IntType`] / [`crate::IntValue`] like every other
/// width, and lifts Rust scalar literals via [`IntoConstantInt`] when
/// `N` is large enough to fit them losslessly.
///
/// `N` must lie in the LLVM-permitted range
/// `[`[`crate::MIN_INT_BITS`]`..=`[`crate::MAX_INT_BITS`]`]`. The check is
/// `const`-evaluated at instantiation by
/// [`crate::Module::int_type_n`]; an out-of-range `N` is a *compile*
/// error.
///
/// ## Limitations on stable Rust
///
/// Rust stable does not allow const-evaluated bounds in trait impls
/// (`{ M > N }` in a `where` clause needs unstable
/// `generic_const_exprs`). Consequently:
/// - [`WiderThan`] is **not** implemented between `Width<M>` and
///   `Width<N>` (or between `Width<N>` and the Rust-scalar markers).
///   Cross-marker truncate / extend casts go through the runtime-
///   checked `_dyn` builder methods.
/// - Lift-to-constant impls (`IntoConstantInt<'ctx, Width<N>>`) use
///   `const { assert!(N >= ...) }` to guarantee a lossless fit at
///   monomorphisation time - invalid `N` is a compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Width<const N: u32>;
impl<const N: u32> sealed::Sealed for Width<N> {}
impl<const N: u32> IntWidth for Width<N> {
    #[inline]
    fn static_bits() -> Option<u32> {
        Some(N)
    }
    #[inline]
    fn narrow<'ctx, B: ModuleBrand + 'ctx>(v: Value<'ctx, B>) -> IrResult<IntValue<'ctx, Self, B>> {
        IntValue::<'ctx, Self, B>::try_from(v)
    }
}

// --------------------------------------------------------------------------
// IntoConstantInt: type-driven dispatch for IntType::const_int
// --------------------------------------------------------------------------

use super::IrError;
use super::IrResult;
use super::ap_int::ApInt;
use super::derived_types::IntType;
use core::convert::Infallible;

/// Split into (lo, hi) 64-bit halves. Invariant: mask/shift bound each
/// half to 64 bits, so the narrowing conversions cannot fail.
fn u128_halves(bits: u128) -> (u64, u64) {
    let lo = u64::try_from(bits & u128::from(u64::MAX))
        .unwrap_or_else(|_| unreachable!("masked to 64 bits"));
    let hi = u64::try_from(bits >> 64).unwrap_or_else(|_| unreachable!("shifted to 64 bits"));
    (lo, hi)
}

/// Trait implemented by Rust scalar types that can be lifted to a
/// width-`W` IR integer constant. The Rust input type drives the
/// extension scheme: `iN` impls sign-extend, `uN` impls zero-extend,
/// `bool` becomes `i1` true/false.
///
/// `Error = Infallible` for the lossless cases (so [`IntType::const_int`](
/// crate::IntType::const_int) is infallible). `Error = IrError` for
/// cases that require runtime fit checking (narrowing or [`IntDyn`]).
pub trait IntoConstantInt<'ctx, W: IntWidth, B: ModuleBrand = Brand<'ctx>> {
    type Error;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, W, B>,
    ) -> Result<ConstantIntValue<'ctx, W, B>, Self::Error>;
}

// ---- Width-exact infallible cases (Error = Infallible) ----

impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, bool, B> for bool {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, bool, B>,
    ) -> Result<ConstantIntValue<'ctx, bool, B>, Infallible> {
        Ok(ty
            .const_int_raw(u64::from(self), false)
            .unwrap_or_else(|_| unreachable!("bool fits in i1")))
    }
}

macro_rules! impl_into_constant_int_signed_exact {
    ($rust_ty:ty) => {
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, $rust_ty, B> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, $rust_ty, B>,
            ) -> Result<ConstantIntValue<'ctx, $rust_ty, B>, Infallible> {
                let raw = i64::from(self).cast_unsigned();
                Ok(ty
                    .const_int_raw(raw, true)
                    .unwrap_or_else(|_| unreachable!("native signed int fits exactly")))
            }
        }
    };
}
macro_rules! impl_into_constant_int_unsigned_exact {
    ($rust_ty:ty, $marker:ty) => {
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, $marker, B> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, $marker, B>,
            ) -> Result<ConstantIntValue<'ctx, $marker, B>, Infallible> {
                Ok(ty
                    .const_int_raw(u64::from(self), false)
                    .unwrap_or_else(|_| unreachable!("native unsigned int fits exactly")))
            }
        }
    };
}
impl_into_constant_int_signed_exact!(i8);
impl_into_constant_int_signed_exact!(i16);
impl_into_constant_int_signed_exact!(i32);
// i64 -> i64 marker: const_int_raw takes a u64, no sign-extend, but we
// want the bit-pattern interpreted as signed for diagnostics.
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, i64, B> for i64 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, i64, B>,
    ) -> Result<ConstantIntValue<'ctx, i64, B>, Infallible> {
        let raw = self.cast_unsigned();
        Ok(ty
            .const_int_raw(raw, false)
            .unwrap_or_else(|_| unreachable!("i64 fits exactly in i64")))
    }
}
impl_into_constant_int_unsigned_exact!(u8, i8);
impl_into_constant_int_unsigned_exact!(u16, i16);
impl_into_constant_int_unsigned_exact!(u32, i32);
impl_into_constant_int_unsigned_exact!(u64, i64);

// i128/u128 use arbitrary-precision path
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, i128, B> for i128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, i128, B>,
    ) -> Result<ConstantIntValue<'ctx, i128, B>, Infallible> {
        let (lo, hi) = u128_halves(self.cast_unsigned());
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("i128 fits in i128")))
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, i128, B> for u128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, i128, B>,
    ) -> Result<ConstantIntValue<'ctx, i128, B>, Infallible> {
        let (lo, hi) = u128_halves(self);
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("u128 fits in i128")))
    }
}

// ---- Widening (smaller signed Rust int → wider static W); infallible
// ----
macro_rules! impl_into_constant_int_signed_widen {
    ($rust_ty:ty, $($marker:ty),+) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, $marker, B> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, $marker, B>)
                -> Result<ConstantIntValue<'ctx, $marker, B>, Infallible>
            {
                let widened = i64::from(self).cast_unsigned();
                Ok(ty.const_int_raw(widened, true).unwrap_or_else(|_| {
                    unreachable!("signed Rust int fits losslessly when sign-extending to wider W")
                }))
            }
        }
    )+ };
}
macro_rules! impl_into_constant_int_unsigned_widen {
    ($rust_ty:ty, $($marker:ty),+) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, $marker, B> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, $marker, B>)
                -> Result<ConstantIntValue<'ctx, $marker, B>, Infallible>
            {
                Ok(ty.const_int_raw(u64::from(self), false).unwrap_or_else(|_| {
                    unreachable!("unsigned Rust int fits losslessly when zero-extending to wider W")
                }))
            }
        }
    )+ };
}
impl_into_constant_int_signed_widen!(i8, i16, i32, i64);
impl_into_constant_int_signed_widen!(i16, i32, i64);
impl_into_constant_int_signed_widen!(i32, i64);
impl_into_constant_int_unsigned_widen!(u8, i16, i32, i64);
impl_into_constant_int_unsigned_widen!(u16, i32, i64);
impl_into_constant_int_unsigned_widen!(u32, i64);
// For i8..i64 -> i128 use arbitrary-precision:
macro_rules! impl_into_constant_int_signed_widen_b128 {
    ($($rust_ty:ty),+) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, i128, B> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, i128, B>)
                -> Result<ConstantIntValue<'ctx, i128, B>, Infallible>
            {
                let (lo, hi) = u128_halves(i128::from(self).cast_unsigned());
                Ok(ty.const_int_arbitrary_precision(&[lo, hi]).unwrap_or_else(|_| {
                    unreachable!("signed Rust int fits in i128")
                }))
            }
        }
    )+ };
}
macro_rules! impl_into_constant_int_unsigned_widen_b128 {
    ($($rust_ty:ty),+) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, i128, B> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, i128, B>)
                -> Result<ConstantIntValue<'ctx, i128, B>, Infallible>
            {
                let (lo, hi) = u128_halves(u128::from(self));
                Ok(ty.const_int_arbitrary_precision(&[lo, hi]).unwrap_or_else(|_| {
                    unreachable!("unsigned Rust int fits in i128")
                }))
            }
        }
    )+ };
}
impl_into_constant_int_signed_widen_b128!(i8, i16, i32, i64);
impl_into_constant_int_unsigned_widen_b128!(u8, u16, u32, u64);

// ---- Width<N> targets: const-asserted lossless lift ----
//
// Each Rust scalar lifts to `Width<N>` losslessly when `N >= bit_width
// (T)`. The check is `const`-evaluated at monomorphisation; an
// instantiation with insufficient `N` is a compile error.
//
// Sources whose value range is bound by Rust's type are listed
// individually so the const block names the right minimum bits.

impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>, B> for bool {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>, B>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>, B>, Infallible> {
        const {
            assert!(N >= 1, "Width<N> requires N >= 1");
        }
        Ok(ty
            .const_int_raw(u64::from(self), false)
            .unwrap_or_else(|_| unreachable!("bool fits in Width<N>, N >= 1")))
    }
}

macro_rules! impl_into_constant_int_width_signed {
    ($rust_ty:ty, $min_bits:literal) => {
        impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>, B>
            for $rust_ty
        {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, Width<N>, B>,
            ) -> Result<ConstantIntValue<'ctx, Width<N>, B>, Infallible> {
                const {
                    assert!(
                        N >= $min_bits,
                        concat!(
                            stringify!($rust_ty),
                            " lift to Width<N> requires N >= ",
                            stringify!($min_bits),
                        ),
                    );
                }
                let widened = i64::from(self).cast_unsigned();
                Ok(ty.const_int_raw(widened, true).unwrap_or_else(|_| {
                    unreachable!("signed Rust int fits losslessly in Width<N>")
                }))
            }
        }
    };
}
macro_rules! impl_into_constant_int_width_unsigned {
    ($rust_ty:ty, $min_bits:literal) => {
        impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>, B>
            for $rust_ty
        {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, Width<N>, B>,
            ) -> Result<ConstantIntValue<'ctx, Width<N>, B>, Infallible> {
                const {
                    assert!(
                        N >= $min_bits,
                        concat!(
                            stringify!($rust_ty),
                            " lift to Width<N> requires N >= ",
                            stringify!($min_bits),
                        ),
                    );
                }
                Ok(ty
                    .const_int_raw(u64::from(self), false)
                    .unwrap_or_else(|_| {
                        unreachable!("unsigned Rust int fits losslessly in Width<N>")
                    }))
            }
        }
    };
}
impl_into_constant_int_width_signed!(i8, 8);
impl_into_constant_int_width_signed!(i16, 16);
impl_into_constant_int_width_signed!(i32, 32);
impl_into_constant_int_width_unsigned!(u8, 8);
impl_into_constant_int_width_unsigned!(u16, 16);
impl_into_constant_int_width_unsigned!(u32, 32);
// i64/u64 -> Width<N>: special-cased because `From<i64> for i64` is
// identity (no `from`), and we need to preserve the signed bit pattern.
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>, B> for i64 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>, B>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>, B>, Infallible> {
        const {
            assert!(N >= 64, "i64 lift to Width<N> requires N >= 64");
        }
        // Sign-extend (mirrors ConstantInt::getSigned): a negative i64
        // must fill the upper bits of any N > 64 target. For N == 64 the
        // signed interpretation is the identity on the bit pattern.
        let raw = self.cast_unsigned();
        Ok(ty
            .const_int_raw(raw, true)
            .unwrap_or_else(|_| unreachable!("i64 fits in Width<N>, N >= 64")))
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>, B> for u64 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>, B>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>, B>, Infallible> {
        const {
            assert!(N >= 64, "u64 lift to Width<N> requires N >= 64");
        }
        Ok(ty
            .const_int_raw(self, false)
            .unwrap_or_else(|_| unreachable!("u64 fits in Width<N>, N >= 64")))
    }
}
// i128/u128 use the arbitrary-precision path.
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>, B> for i128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>, B>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>, B>, Infallible> {
        const {
            assert!(N >= 128, "i128 lift to Width<N> requires N >= 128");
        }
        // Sign-extend above bit 128 (mirrors `ConstantInt::getSigned` /
        // `APInt(bits, val, /*isSigned=*/true)`); the `u128` lift below
        // zero-extends instead.
        let (lo, hi) = u128_halves(self.cast_unsigned());
        let pattern = ApInt::from_words(128, &[lo, hi])
            .sext(N)
            .unwrap_or_else(|| unreachable!("N >= 128 permits sign-extension"));
        Ok(ty
            .const_ap_int(&pattern)
            .unwrap_or_else(|_| unreachable!("sign-extended i128 has width N")))
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>, B> for u128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>, B>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>, B>, Infallible> {
        const {
            assert!(N >= 128, "u128 lift to Width<N> requires N >= 128");
        }
        let (lo, hi) = u128_halves(self);
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("u128 fits in Width<N>, N >= 128")))
    }
}

// ---- IntDyn target: runtime fit-check ----
macro_rules! impl_into_constant_int_dyn {
    (signed $($rust_ty:ty),+) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, IntDyn, B> for $rust_ty {
            type Error = IrError;
            fn into_constant_int(self, ty: IntType<'ctx, IntDyn, B>) -> IrResult<ConstantIntValue<'ctx, IntDyn, B>> {
                ty.const_int_raw(i64::from(self).cast_unsigned(), true)
            }
        }
    )+ };
    (unsigned $($rust_ty:ty),+) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, IntDyn, B> for $rust_ty {
            type Error = IrError;
            fn into_constant_int(self, ty: IntType<'ctx, IntDyn, B>) -> IrResult<ConstantIntValue<'ctx, IntDyn, B>> {
                ty.const_int_raw(u64::from(self), false)
            }
        }
    )+ };
}
impl_into_constant_int_dyn!(signed i8, i16, i32);
// i64 to IntDyn sign-extends like its signed siblings above: wider
// runtime targets get the sign bits, and narrower targets fit-check the
// SIGNED value (so -1 fits any width; mirrors ConstantInt::getSigned).
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, IntDyn, B> for i64 {
    type Error = IrError;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, IntDyn, B>,
    ) -> IrResult<ConstantIntValue<'ctx, IntDyn, B>> {
        ty.const_int_raw(self.cast_unsigned(), true)
    }
}
impl_into_constant_int_dyn!(unsigned u8, u16, u32, u64);
impl<'ctx, B: ModuleBrand + 'ctx> IntoConstantInt<'ctx, IntDyn, B> for bool {
    type Error = IrError;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, IntDyn, B>,
    ) -> IrResult<ConstantIntValue<'ctx, IntDyn, B>> {
        ty.const_int_raw(u64::from(self), false)
    }
}

// --------------------------------------------------------------------------
// WiderThan<W>: compile-time width invariants for cast operations
// --------------------------------------------------------------------------

/// Sealed: `Self` (the wider side) has strictly more bits than `W`.
///
/// Generated for every `(Wider, Narrower)` pair of static markers via
/// macro. Mirrors the runtime check
/// `IntegerType::getBitWidth() > other.getBitWidth()`
/// (`DerivedTypes.h`). The trait is the bound used by
/// [`IRBuilder::build_trunc`](crate::IRBuilder::build_trunc) (where
/// `Src: WiderThan<Dst>`) and the inverse on
/// [`IRBuilder::build_zext`](crate::IRBuilder::build_zext) /
/// [`build_sext`](crate::IRBuilder::build_sext) (where
/// `Dst: WiderThan<Src>`).
///
/// Signed-only: every `WiderThan` involves two static markers.
/// Dynamic-width casts use the `_dyn` builder methods, which keep the
/// runtime check.
pub trait WiderThan<W: IntWidth>: IntWidth + sealed::Sealed {}

macro_rules! decl_wider_than {
    ($wide:ty: $($narrow:ty),+ $(,)?) => {
        $( impl WiderThan<$narrow> for $wide {} )+
    };
}

decl_wider_than!(i8: bool);
decl_wider_than!(i16: bool, i8);
decl_wider_than!(i32: bool, i8, i16);
decl_wider_than!(i64: bool, i8, i16, i32);
decl_wider_than!(i128: bool, i8, i16, i32, i64);

// --------------------------------------------------------------------------
// IntoIntValue: ergonomic operand input for the IRBuilder
// --------------------------------------------------------------------------

/// Inputs that can be lifted into an [`IntValue<'ctx, W>`] operand
/// for the IR builder.
///
/// Implemented by:
/// - [`IntValue<'ctx, W>`] (identity).
/// - [`crate::ConstantIntValue<'ctx, W>`] (lift).
/// - Every kept Rust scalar (`bool`, `i8`..`i128`, `u8`..`u128`), each
///   mapping to exactly its own `iN` marker.
///
/// The trait is **sealed**. An erased [`Value`] / [`Argument`] /
/// [`Instruction`] no longer lifts silently: narrow it explicitly with
/// [`IntValue::try_from`] (or [`IsValue`]-erased `_dyn` builders). The
/// `module` argument exists so Rust-scalar inputs can route through the
/// right [`IntType<'ctx, W>`] constructor; impls for value handles
/// ignore it.
pub trait IntoIntValue<'ctx, W: IntWidth, B: ModuleBrand = Brand<'ctx>>:
    Sized + into_int_value_sealed::Sealed
{
    fn into_int_value(self, module: ModuleRef<'ctx, B>) -> IrResult<IntValue<'ctx, W, B>>;
}

/// Seals [`IntoIntValue`] to the identity/lift handles plus the kept
/// exact-width Rust scalars. Erased `Value`/`Argument`/`Instruction` are
/// deliberately absent.
mod into_int_value_sealed {
    pub trait Sealed {}
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> into_int_value_sealed::Sealed
    for IntValue<'ctx, W, B>
{
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> into_int_value_sealed::Sealed
    for ConstantIntValue<'ctx, W, B>
{
}

macro_rules! impl_into_int_value_sealed_scalar {
    ($($t:ty),+ $(,)?) => { $(
        impl into_int_value_sealed::Sealed for $t {}
    )+ };
}
impl_into_int_value_sealed_scalar!(bool, i8, i16, i32, i64, i128, u8, u16, u32, u64, u128);

// ---- Identity ---------------------------------------------------------
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IntoIntValue<'ctx, W, B> for IntValue<'ctx, W, B> {
    #[inline]
    fn into_int_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<IntValue<'ctx, W, B>> {
        Ok(self)
    }
}

// ---- ConstantIntValue lift -------------------------------------------
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IntoIntValue<'ctx, W, B>
    for ConstantIntValue<'ctx, W, B>
{
    #[inline]
    fn into_int_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<IntValue<'ctx, W, B>> {
        Ok(IntValue::<W, B>::from_value_unchecked(IsValue::as_value(
            self,
        )))
    }
}

// ---- Rust scalar -> typed IntValue (via IntoConstantInt) -------------
//
// Each impl picks the matching `Module::*_type()` constructor and
// routes the literal through `IntoConstantInt`, then wraps the
// resulting constant as an `IntValue`. Static (non-IntDyn) targets reuse
// the infallible `IntoConstantInt` impls; the IntDyn-target impls
// surface the underlying width-fit check via `IrResult`.
macro_rules! impl_into_int_value_static {
    ($rust_ty:ty, $marker:ty, $ty_method:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> IntoIntValue<'ctx, $marker, B> for $rust_ty {
            fn into_int_value(
                self,
                module: ModuleRef<'ctx, B>,
            ) -> IrResult<IntValue<'ctx, $marker, B>> {
                let ty =
                    IntType::<$marker, B>::new(module.module().$ty_method().as_type().id(), module);
                match self.into_constant_int(ty) {
                    Ok(c) => Ok(IntValue::<$marker, B>::from_value_unchecked(
                        IsValue::as_value(c),
                    )),
                    Err(_) => unreachable!(
                        "IntoConstantInt for static target is infallible per the trait impls"
                    ),
                }
            }
        }
    };
}

// `bool` -> bool marker
impl_into_int_value_static!(bool, bool, bool_type);
// signed exact: each Rust width maps to exactly its own `iN` marker.
// (Widening removed in the "no silent erasure" strict cut: a literal in a
// wider slot must name its width, e.g. `2_i64` rather than `2_i32`.)
impl_into_int_value_static!(i8, i8, i8_type);
impl_into_int_value_static!(i16, i16, i16_type);
impl_into_int_value_static!(i32, i32, i32_type);
impl_into_int_value_static!(i64, i64, i64_type);
impl_into_int_value_static!(i128, i128, i128_type);
// unsigned exact: same bit width as the matching signed `iN` marker.
impl_into_int_value_static!(u8, i8, i8_type);
impl_into_int_value_static!(u16, i16, i16_type);
impl_into_int_value_static!(u32, i32, i32_type);
impl_into_int_value_static!(u64, i64, i64_type);
impl_into_int_value_static!(u128, i128, i128_type);

// --------------------------------------------------------------------------
// StaticIntWidth: marker-only type lookup
// --------------------------------------------------------------------------

/// Sealed: integer-width markers whose width is known at compile time
/// AND whose `IntType<'ctx, Self>` can be projected from a
/// [`Module`](crate::Module) without an extra runtime parameter. Lets the IR
/// builder accept `b.build_int_phi::<i32, _>("acc")?` instead of
/// `b.build_int_phi(i32_ty, "acc")?`.
///
/// Not implemented for [`IntDyn`] - there is no single "dyn
/// integer type" in a module; the dyn-flavour builder methods take an
/// explicit [`IntType<'ctx, IntDyn>`] for the runtime width.
pub trait StaticIntWidth: IntWidth {
    /// Bit width of the integer type at the type level. Mirrors
    /// `Type::getIntegerBitWidth` in `lib/IR/Type.cpp` for the cases
    /// where the width is statically known. Usable as `W::STATIC_BITS`
    /// in `const { ... }` assertions to enforce width-equality at
    /// monomorphisation time.
    const STATIC_BITS: u32;
    /// Project the marker into the matching [`IntType`] from the
    /// caller's module.
    fn ir_type<'ctx, B: ModuleBrand + 'ctx>(module: ModuleRef<'ctx, B>) -> IntType<'ctx, Self, B>;
}

macro_rules! impl_static_int_width {
    ($ty:ty, $method:ident, $bits:literal) => {
        impl StaticIntWidth for $ty {
            const STATIC_BITS: u32 = $bits;
            #[inline]
            fn ir_type<'ctx, B: ModuleBrand + 'ctx>(
                module: ModuleRef<'ctx, B>,
            ) -> IntType<'ctx, Self, B> {
                IntType::<Self, B>::new(module.module().$method().as_type().id(), module)
            }
        }
    };
}
impl_static_int_width!(bool, bool_type, 1);
impl_static_int_width!(i8, i8_type, 8);
impl_static_int_width!(i16, i16_type, 16);
impl_static_int_width!(i32, i32_type, 32);
impl_static_int_width!(i64, i64_type, 64);
impl_static_int_width!(i128, i128_type, 128);

// `Width<N>` static-width impl: routes through the const-generic
// `Module::int_type_n::<N>()` constructor, which performs the
// `MIN_INT_BITS..=MAX_INT_BITS` range check at monomorphisation.
impl<const N: u32> StaticIntWidth for Width<N> {
    const STATIC_BITS: u32 = N;
    #[inline]
    fn ir_type<'ctx, B: ModuleBrand + 'ctx>(module: ModuleRef<'ctx, B>) -> IntType<'ctx, Self, B> {
        IntType::<Self, B>::new(module.module().int_type_n::<N>().as_type().id(), module)
    }
}

// NOTE: the `impl<const N> IntoIntValue<Width<N>> for <rust scalar>` lifts
// were removed in the "no silent erasure" strict cut (task #72). A Rust
// scalar now maps to exactly its own `iN` marker, so `W` in
// `build_int_add(2i32, 3i32, "n")` infers uniquely with no turbofish; a
// `Width<N>` slot must be fed a typed `IntValue<Width<N>>` /
// `ConstantIntValue<Width<N>>` (both still lift via the identity/const
// impls above), not a bare Rust literal.
