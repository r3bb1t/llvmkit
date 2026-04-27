//! Sealed marker types and trait for a function's return shape.
//!
//! Mirrors the type-level distinction LLVM C++ keeps implicit at the
//! `Function::getReturnType()` boundary: in C++ you read the return
//! type at runtime and pattern-match on it; in Rust we encode it in
//! the type system so the IRBuilder can reject a `build_ret(int_value)`
//! against a `void`-returning function at compile time.
//!
//! The marker is the bare type the caller already names:
//!
//! - `()` — `void` return; the builder exposes only `build_ret_void`.
//! - `bool` / `i8` / `i16` / `i32` / `i64` / `i128` / [`crate::IntDyn`]
//!   — `iN` return for the matching width.
//! - `f32` / `f64` / [`crate::Half`] / [`crate::BFloat`] /
//!   [`crate::Fp128`] / [`crate::X86Fp80`] / [`crate::PpcFp128`] /
//!   [`crate::FloatDyn`] — IEEE/non-IEEE float return.
//! - [`Ptr`] — opaque-pointer return (any address space).
//! - [`Dyn`] — anything not statically described (struct, vector,
//!   array, target-ext, parsed IR…). The IRBuilder keeps a runtime
//!   [`crate::IrError::ReturnTypeMismatch`] check exclusively for
//!   `Dyn`.
//!
//! The trait is **sealed** — the closed set of return shapes is part
//! of the IR spec, not an extension point.

use core::fmt;

use crate::float_kind::FloatKind;
use crate::int_width::IntWidth;
use crate::r#type::sealed;

/// Crate-internal projection: the kind of return type a marker
/// expects at runtime. Drives
/// [`crate::function::signature_matches_marker`] and any future
/// surface that needs to compare a marker against the actual
/// `TypeData`. Public-but-`#[doc(hidden)]` because it surfaces in
/// the [`ReturnMarker::expected_kind`] return type; users don't
/// construct or pattern-match it.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedRetKind {
    Void,
    Ptr,
    Dyn,
    IntStatic(u32),
    IntDyn,
    FloatStatic(&'static str),
    FloatDyn,
}

/// Sealed marker trait for a function's return shape.
pub trait ReturnMarker: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// `true` if the function returns `void`.
    fn is_void() -> bool {
        false
    }

    /// Crate-internal: project the marker into the
    /// [`ExpectedRetKind`] discriminator so the runtime
    /// signature-validation path can inspect it without resorting to
    /// `core::any::TypeId`.
    #[doc(hidden)]
    fn expected_kind() -> ExpectedRetKind;
}

// `()` — void return.
impl sealed::Sealed for () {}
impl ReturnMarker for () {
    #[inline]
    fn is_void() -> bool {
        true
    }
    #[inline]
    fn expected_kind() -> ExpectedRetKind {
        ExpectedRetKind::Void
    }
}

/// Opaque-pointer return (any address space).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ptr;
impl sealed::Sealed for Ptr {}
impl ReturnMarker for Ptr {
    #[inline]
    fn expected_kind() -> ExpectedRetKind {
        ExpectedRetKind::Ptr
    }
}

/// Fully-erased return marker. Anything not statically described
/// (struct, vector, array, target-ext, parsed IR) lands here. The
/// builder keeps the runtime [`crate::IrError::ReturnTypeMismatch`]
/// check exclusively for this marker.
///
/// Distinct from [`crate::IntDyn`] (integer-width-erased) and
/// [`crate::FloatDyn`] (float-kind-erased) — those still constrain
/// the return type to the *category*, while top-level `Dyn` makes no
/// category guarantee at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dyn;
impl sealed::Sealed for Dyn {}
impl ReturnMarker for Dyn {
    #[inline]
    fn expected_kind() -> ExpectedRetKind {
        ExpectedRetKind::Dyn
    }
}
// Per-type `ReturnMarker` impls. Rust's coherence checker rejects two
// blanket `impl<W: IntWidth>` + `impl<K: FloatKind>` even though no
// type implements both traits, so we expand by hand. Each impl is
// `#[inline]` and projects through the corresponding `IntWidth` /
// `FloatKind` accessor — no behavioural difference from a blanket.
macro_rules! impl_int_return_marker {
    ($($ty:ty),+ $(,)?) => { $(
        impl ReturnMarker for $ty {
            #[inline]
            fn expected_kind() -> ExpectedRetKind {
                match <$ty as IntWidth>::static_bits() {
                    Some(b) => ExpectedRetKind::IntStatic(b),
                    None => ExpectedRetKind::IntDyn,
                }
            }
        }
    )+ };
}
macro_rules! impl_float_return_marker {
    ($($ty:ty),+ $(,)?) => { $(
        impl ReturnMarker for $ty {
            #[inline]
            fn expected_kind() -> ExpectedRetKind {
                match <$ty as FloatKind>::ieee_label() {
                    Some(label) => ExpectedRetKind::FloatStatic(label),
                    None => ExpectedRetKind::FloatDyn,
                }
            }
        }
    )+ };
}
impl_int_return_marker!(bool, i8, i16, i32, i64, i128, crate::int_width::IntDyn);
impl_float_return_marker!(
    f32,
    f64,
    crate::float_kind::Half,
    crate::float_kind::BFloat,
    crate::float_kind::Fp128,
    crate::float_kind::X86Fp80,
    crate::float_kind::PpcFp128,
    crate::float_kind::FloatDyn,
);

// `Width<N>` participates as a return marker. Const-generic blanket
// is sound because `Width<N>` doesn't implement `FloatKind`, so no
// overlap with the float-marker impls.
impl<const N: u32> ReturnMarker for crate::int_width::Width<N> {
    #[inline]
    fn expected_kind() -> ExpectedRetKind {
        ExpectedRetKind::IntStatic(N)
    }
}
