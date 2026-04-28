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

use crate::r#type::sealed;

/// Sealed marker trait implemented by every integer width tag.
pub trait IntWidth: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// Static bit-width if known at compile time, else `None` (used by
    /// [`IntDyn`]).
    fn static_bits() -> Option<u32>;
}

macro_rules! impl_int_width_scalar {
    ($rust_ty:ty, $bits:expr) => {
        impl sealed::Sealed for $rust_ty {}
        impl IntWidth for $rust_ty {
            #[inline]
            fn static_bits() -> Option<u32> {
                Some($bits)
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
/// cases that require runtime fit checking (narrowing or [`IntDyn`]).
pub trait IntoConstantInt<'ctx, W: IntWidth> {
    type Error;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, W>,
    ) -> Result<ConstantIntValue<'ctx, W>, Self::Error>;
}

// ---- Width-exact infallible cases (Error = Infallible) ----

impl<'ctx> IntoConstantInt<'ctx, bool> for bool {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, bool>,
    ) -> Result<ConstantIntValue<'ctx, bool>, Infallible> {
        Ok(ty
            .const_int_raw(u64::from(self), false)
            .unwrap_or_else(|_| unreachable!("bool fits in i1")))
    }
}

macro_rules! impl_into_constant_int_signed_exact {
    ($rust_ty:ty) => {
        impl<'ctx> IntoConstantInt<'ctx, $rust_ty> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, $rust_ty>,
            ) -> Result<ConstantIntValue<'ctx, $rust_ty>, Infallible> {
                let raw = i64::from(self) as u64;
                Ok(ty
                    .const_int_raw(raw, true)
                    .unwrap_or_else(|_| unreachable!("native signed int fits exactly")))
            }
        }
    };
}
macro_rules! impl_into_constant_int_unsigned_exact {
    ($rust_ty:ty, $marker:ty) => {
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
impl_into_constant_int_signed_exact!(i8);
impl_into_constant_int_signed_exact!(i16);
impl_into_constant_int_signed_exact!(i32);
// i64 -> i64 marker: const_int_raw takes a u64, no sign-extend, but we
// want the bit-pattern interpreted as signed for diagnostics.
impl<'ctx> IntoConstantInt<'ctx, i64> for i64 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, i64>,
    ) -> Result<ConstantIntValue<'ctx, i64>, Infallible> {
        let raw = self as u64;
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
impl<'ctx> IntoConstantInt<'ctx, i128> for i128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, i128>,
    ) -> Result<ConstantIntValue<'ctx, i128>, Infallible> {
        let bits = self as u128;
        let lo = (bits & 0xffff_ffff_ffff_ffff) as u64;
        let hi = (bits >> 64) as u64;
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("i128 fits in i128")))
    }
}
impl<'ctx> IntoConstantInt<'ctx, i128> for u128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, i128>,
    ) -> Result<ConstantIntValue<'ctx, i128>, Infallible> {
        let lo = (self & 0xffff_ffff_ffff_ffff) as u64;
        let hi = (self >> 64) as u64;
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("u128 fits in i128")))
    }
}

// ---- Widening (smaller signed Rust int → wider static W); infallible
// ----
macro_rules! impl_into_constant_int_signed_widen {
    ($rust_ty:ty, $($marker:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, $marker> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, $marker>)
                -> Result<ConstantIntValue<'ctx, $marker>, Infallible>
            {
                let widened = i64::from(self) as u64;
                Ok(ty.const_int_raw(widened, true).unwrap_or_else(|_| {
                    unreachable!("signed Rust int fits losslessly when sign-extending to wider W")
                }))
            }
        }
    )+ };
}
macro_rules! impl_into_constant_int_unsigned_widen {
    ($rust_ty:ty, $($marker:ty),+) => { $(
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
impl_into_constant_int_signed_widen!(i8, i16, i32, i64);
impl_into_constant_int_signed_widen!(i16, i32, i64);
impl_into_constant_int_signed_widen!(i32, i64);
impl_into_constant_int_unsigned_widen!(u8, i16, i32, i64);
impl_into_constant_int_unsigned_widen!(u16, i32, i64);
impl_into_constant_int_unsigned_widen!(u32, i64);
// For i8..i64 -> i128 use arbitrary-precision:
macro_rules! impl_into_constant_int_signed_widen_b128 {
    ($($rust_ty:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, i128> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, i128>)
                -> Result<ConstantIntValue<'ctx, i128>, Infallible>
            {
                let v = i128::from(self) as u128;
                let lo = (v & 0xffff_ffff_ffff_ffff) as u64;
                let hi = (v >> 64) as u64;
                Ok(ty.const_int_arbitrary_precision(&[lo, hi]).unwrap_or_else(|_| {
                    unreachable!("signed Rust int fits in i128")
                }))
            }
        }
    )+ };
}
macro_rules! impl_into_constant_int_unsigned_widen_b128 {
    ($($rust_ty:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, i128> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(self, ty: IntType<'ctx, i128>)
                -> Result<ConstantIntValue<'ctx, i128>, Infallible>
            {
                let v = u128::from(self);
                let lo = (v & 0xffff_ffff_ffff_ffff) as u64;
                let hi = (v >> 64) as u64;
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

impl<'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>> for bool {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>>, Infallible> {
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
        impl<'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, Width<N>>,
            ) -> Result<ConstantIntValue<'ctx, Width<N>>, Infallible> {
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
                let widened = i64::from(self) as u64;
                Ok(ty.const_int_raw(widened, true).unwrap_or_else(|_| {
                    unreachable!("signed Rust int fits losslessly in Width<N>")
                }))
            }
        }
    };
}
macro_rules! impl_into_constant_int_width_unsigned {
    ($rust_ty:ty, $min_bits:literal) => {
        impl<'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>> for $rust_ty {
            type Error = Infallible;
            fn into_constant_int(
                self,
                ty: IntType<'ctx, Width<N>>,
            ) -> Result<ConstantIntValue<'ctx, Width<N>>, Infallible> {
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
impl<'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>> for i64 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>>, Infallible> {
        const {
            assert!(N >= 64, "i64 lift to Width<N> requires N >= 64");
        }
        let raw = self as u64;
        Ok(ty
            .const_int_raw(raw, false)
            .unwrap_or_else(|_| unreachable!("i64 fits in Width<N>, N >= 64")))
    }
}
impl<'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>> for u64 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>>, Infallible> {
        const {
            assert!(N >= 64, "u64 lift to Width<N> requires N >= 64");
        }
        Ok(ty
            .const_int_raw(self, false)
            .unwrap_or_else(|_| unreachable!("u64 fits in Width<N>, N >= 64")))
    }
}
// i128/u128 use the arbitrary-precision path.
impl<'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>> for i128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>>, Infallible> {
        const {
            assert!(N >= 128, "i128 lift to Width<N> requires N >= 128");
        }
        let bits = self as u128;
        let lo = (bits & 0xffff_ffff_ffff_ffff) as u64;
        let hi = (bits >> 64) as u64;
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("i128 fits in Width<N>, N >= 128")))
    }
}
impl<'ctx, const N: u32> IntoConstantInt<'ctx, Width<N>> for u128 {
    type Error = Infallible;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, Width<N>>,
    ) -> Result<ConstantIntValue<'ctx, Width<N>>, Infallible> {
        const {
            assert!(N >= 128, "u128 lift to Width<N> requires N >= 128");
        }
        let lo = (self & 0xffff_ffff_ffff_ffff) as u64;
        let hi = (self >> 64) as u64;
        Ok(ty
            .const_int_arbitrary_precision(&[lo, hi])
            .unwrap_or_else(|_| unreachable!("u128 fits in Width<N>, N >= 128")))
    }
}

// ---- IntDyn target: runtime fit-check ----
macro_rules! impl_into_constant_int_dyn {
    (signed $($rust_ty:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, IntDyn> for $rust_ty {
            type Error = IrError;
            fn into_constant_int(self, ty: IntType<'ctx, IntDyn>) -> IrResult<ConstantIntValue<'ctx, IntDyn>> {
                ty.const_int_raw(i64::from(self) as u64, true)
            }
        }
    )+ };
    (unsigned $($rust_ty:ty),+) => { $(
        impl<'ctx> IntoConstantInt<'ctx, IntDyn> for $rust_ty {
            type Error = IrError;
            fn into_constant_int(self, ty: IntType<'ctx, IntDyn>) -> IrResult<ConstantIntValue<'ctx, IntDyn>> {
                ty.const_int_raw(u64::from(self), false)
            }
        }
    )+ };
}
impl_into_constant_int_dyn!(signed i8, i16, i32);
// i64 to IntDyn passes through the u64 bit-pattern.
impl<'ctx> IntoConstantInt<'ctx, IntDyn> for i64 {
    type Error = IrError;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, IntDyn>,
    ) -> IrResult<ConstantIntValue<'ctx, IntDyn>> {
        ty.const_int_raw(self as u64, false)
    }
}
impl_into_constant_int_dyn!(unsigned u8, u16, u32, u64);
impl<'ctx> IntoConstantInt<'ctx, IntDyn> for bool {
    type Error = IrError;
    fn into_constant_int(
        self,
        ty: IntType<'ctx, IntDyn>,
    ) -> IrResult<ConstantIntValue<'ctx, IntDyn>> {
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

use crate::module::Module;
use crate::value::IntValue;

/// Inputs that can be lifted into an [`IntValue<'ctx, W>`] operand
/// for the IR builder.
///
/// Implemented by:
/// - [`IntValue<'ctx, W>`] (identity).
/// - [`crate::ConstantIntValue<'ctx, W>`] (lift).
/// - Every Rust scalar that already implements
///   [`IntoConstantInt<'ctx, W>`] for the matching marker.
///
/// The trait shape mirrors LLVM IRBuilder's `Value*`-typed operand
/// slots: anything that can spell an `iN` value at the call site is
/// accepted. The `module` argument exists so Rust-scalar inputs can
/// route through the right [`IntType<'ctx, W>`] constructor; impls
/// for value handles ignore it.
pub trait IntoIntValue<'ctx, W: IntWidth>: Sized {
    fn into_int_value(self, module: &'ctx Module<'ctx>) -> IrResult<IntValue<'ctx, W>>;
}

// ---- Identity ---------------------------------------------------------
impl<'ctx, W: IntWidth> IntoIntValue<'ctx, W> for IntValue<'ctx, W> {
    #[inline]
    fn into_int_value(self, _module: &'ctx Module<'ctx>) -> IrResult<IntValue<'ctx, W>> {
        Ok(self)
    }
}

// ---- ConstantIntValue lift -------------------------------------------
impl<'ctx, W: IntWidth> IntoIntValue<'ctx, W> for ConstantIntValue<'ctx, W> {
    #[inline]
    fn into_int_value(self, _module: &'ctx Module<'ctx>) -> IrResult<IntValue<'ctx, W>> {
        Ok(IntValue::<W>::from_value_unchecked(
            crate::value::IsValue::as_value(self),
        ))
    }
}

// ---- Erased / heterogeneous handles --------------------------------------
// Anything that already has a `TryFrom<...> for IntValue<W>` impl lifts
// to the trait via the same path. These impls eliminate the
// `try_into()?` boilerplate at call sites: `b.build_int_add(f.param(0)?,
// 1_i32, "r")` reads the `Argument` directly.
//
// Per-W expansion (no W-blanket) so coherence with the identity blanket
// `impl<W: IntWidth> IntoIntValue<W> for IntValue<W>` stays sane: each
// concrete impl below fixes a distinct `Self` type, so no overlap.
macro_rules! impl_into_int_value_via_try_from {
    ($source:ty, $($w:ty),+ $(,)?) => { $(
        impl<'ctx> IntoIntValue<'ctx, $w> for $source {
            #[inline]
            fn into_int_value(
                self,
                _module: &'ctx Module<'ctx>,
            ) -> IrResult<IntValue<'ctx, $w>> {
                IntValue::<'ctx, $w>::try_from(self)
            }
        }
    )+ };
}
impl_into_int_value_via_try_from!(
    crate::argument::Argument<'ctx>,
    bool,
    i8,
    i16,
    i32,
    i64,
    i128,
    IntDyn
);
impl_into_int_value_via_try_from!(
    crate::value::Value<'ctx>,
    bool,
    i8,
    i16,
    i32,
    i64,
    i128,
    IntDyn
);
impl_into_int_value_via_try_from!(
    crate::instruction::Instruction<'ctx>,
    bool,
    i8,
    i16,
    i32,
    i64,
    i128,
    IntDyn
);

// ---- Rust scalar -> typed IntValue (via IntoConstantInt) -------------
//
// Each impl picks the matching `Module::*_type()` constructor and
// routes the literal through `IntoConstantInt`, then wraps the
// resulting constant as an `IntValue`. Static (non-IntDyn) targets reuse
// the infallible `IntoConstantInt` impls; the IntDyn-target impls
// surface the underlying width-fit check via `IrResult`.
macro_rules! impl_into_int_value_static {
    ($rust_ty:ty, $marker:ty, $ty_method:ident) => {
        impl<'ctx> IntoIntValue<'ctx, $marker> for $rust_ty {
            fn into_int_value(
                self,
                module: &'ctx Module<'ctx>,
            ) -> IrResult<IntValue<'ctx, $marker>> {
                let ty = module.$ty_method();
                match self.into_constant_int(ty) {
                    Ok(c) => Ok(IntValue::<$marker>::from_value_unchecked(
                        crate::value::IsValue::as_value(c),
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
// signed exact + widening
impl_into_int_value_static!(i8, i8, i8_type);
impl_into_int_value_static!(i8, i16, i16_type);
impl_into_int_value_static!(i8, i32, i32_type);
impl_into_int_value_static!(i8, i64, i64_type);
impl_into_int_value_static!(i8, i128, i128_type);
impl_into_int_value_static!(i16, i16, i16_type);
impl_into_int_value_static!(i16, i32, i32_type);
impl_into_int_value_static!(i16, i64, i64_type);
impl_into_int_value_static!(i16, i128, i128_type);
impl_into_int_value_static!(i32, i32, i32_type);
impl_into_int_value_static!(i32, i64, i64_type);
impl_into_int_value_static!(i32, i128, i128_type);
impl_into_int_value_static!(i64, i64, i64_type);
impl_into_int_value_static!(i64, i128, i128_type);
impl_into_int_value_static!(i128, i128, i128_type);
// unsigned widening (zero-extend)
impl_into_int_value_static!(u8, i8, i8_type);
impl_into_int_value_static!(u8, i16, i16_type);
impl_into_int_value_static!(u8, i32, i32_type);
impl_into_int_value_static!(u8, i64, i64_type);
impl_into_int_value_static!(u8, i128, i128_type);
impl_into_int_value_static!(u16, i16, i16_type);
impl_into_int_value_static!(u16, i32, i32_type);
impl_into_int_value_static!(u16, i64, i64_type);
impl_into_int_value_static!(u16, i128, i128_type);
impl_into_int_value_static!(u32, i32, i32_type);
impl_into_int_value_static!(u32, i64, i64_type);
impl_into_int_value_static!(u32, i128, i128_type);
impl_into_int_value_static!(u64, i64, i64_type);
impl_into_int_value_static!(u64, i128, i128_type);
impl_into_int_value_static!(u128, i128, i128_type);

// --------------------------------------------------------------------------
// StaticIntWidth: marker-only type lookup
// --------------------------------------------------------------------------

/// Sealed: integer-width markers whose width is known at compile time
/// AND whose `IntType<'ctx, Self>` can be projected from a [`Module`]
/// without an extra runtime parameter. Lets the IR builder accept
/// `b.build_int_phi::<i32>("acc")?` instead of
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
    fn ir_type<'ctx>(module: &'ctx Module<'ctx>) -> IntType<'ctx, Self>
    where
        Self: Sized;
}

macro_rules! impl_static_int_width {
    ($ty:ty, $method:ident, $bits:literal) => {
        impl StaticIntWidth for $ty {
            const STATIC_BITS: u32 = $bits;
            #[inline]
            fn ir_type<'ctx>(module: &'ctx Module<'ctx>) -> IntType<'ctx, Self> {
                module.$method()
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
    fn ir_type<'ctx>(module: &'ctx Module<'ctx>) -> IntType<'ctx, Self> {
        module.int_type_n::<N>()
    }
}

// IntoIntValue for Width<N>: each Rust scalar lifts when N >= bit_width(scalar).
// Each impl carries a `const { assert!(N >= ...) }` so an under-sized N
// is a compile error at the call site.
macro_rules! impl_into_int_value_width {
    ($rust_ty:ty, $min_bits:literal) => {
        impl<'ctx, const N: u32> IntoIntValue<'ctx, Width<N>> for $rust_ty {
            fn into_int_value(
                self,
                module: &'ctx Module<'ctx>,
            ) -> IrResult<IntValue<'ctx, Width<N>>> {
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
                let ty: IntType<'ctx, Width<N>> = module.int_type_n::<N>();
                match self.into_constant_int(ty) {
                    Ok(c) => Ok(IntValue::<Width<N>>::from_value_unchecked(
                        crate::value::IsValue::as_value(c),
                    )),
                    Err(_) => unreachable!(
                        "IntoConstantInt for Width<N> with N >= min_bits is infallible"
                    ),
                }
            }
        }
    };
}
impl_into_int_value_width!(bool, 1);
impl_into_int_value_width!(i8, 8);
impl_into_int_value_width!(i16, 16);
impl_into_int_value_width!(i32, 32);
impl_into_int_value_width!(i64, 64);
impl_into_int_value_width!(i128, 128);
impl_into_int_value_width!(u8, 8);
impl_into_int_value_width!(u16, 16);
impl_into_int_value_width!(u32, 32);
impl_into_int_value_width!(u64, 64);
impl_into_int_value_width!(u128, 128);
