//! Sealed marker types for a function's return shape.
//!
//! Mirrors the type-level distinction LLVM C++ keeps implicit at the
//! `Function::getReturnType()` boundary: in C++ you read the return
//! type at runtime and pattern-match on it; in Rust we encode it in
//! the type system so the IRBuilder can reject a `build_ret(int_value)`
//! against a `void`-returning function at compile time.
//!
//! The static markers cover the cases where the Rust caller already
//! knows the shape:
//!
//! - [`RVoid`] — `void` return; the builder exposes only `build_ret_void`.
//! - [`RInt<W>`] — `iN` return for a static width.
//! - [`RFloat<K>`] — IEEE/non-IEEE float return for a static kind.
//! - [`RPtr`] — opaque pointer return (any address space).
//!
//! Aggregate, vector, struct, target-ext, and parsed-IR cases use the
//! [`RDyn`] runtime-checked marker — `build_ret` falls back to the
//! current runtime equality check for those.
//!
//! The trait is **sealed** — the closed set of return shapes is part
//! of the IR spec, not an extension point.

use core::fmt;
use core::marker::PhantomData;

use crate::float_kind::FloatKind;
use crate::int_width::IntWidth;
use crate::r#type::sealed;

/// Sealed marker trait for a function's return shape.
pub trait ReturnMarker: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// `true` if the function returns `void`.
    fn is_void() -> bool {
        false
    }
}

/// `void` return. The IRBuilder exposes only `build_ret_void` for
/// functions tagged `RVoid`; `build_ret(value)` is a compile error.
#[derive(Debug, Clone, Copy)]
pub struct RVoid;
impl sealed::Sealed for RVoid {}
impl ReturnMarker for RVoid {
    #[inline]
    fn is_void() -> bool {
        true
    }
}

/// `iN` return for a static width. The IRBuilder's `build_ret`
/// requires an [`IntValue<'ctx, W>`](crate::IntValue) "" mismatched
/// widths are a compile error.
pub struct RInt<W: IntWidth>(PhantomData<W>);

impl<W: IntWidth> Clone for RInt<W> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<W: IntWidth> Copy for RInt<W> {}
impl<W: IntWidth> fmt::Debug for RInt<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RInt")
            .field("width", &W::static_bits())
            .finish()
    }
}
impl<W: IntWidth> sealed::Sealed for RInt<W> {}
impl<W: IntWidth> ReturnMarker for RInt<W> {}

/// IEEE / non-IEEE float return for a static kind.
pub struct RFloat<K: FloatKind>(PhantomData<K>);

impl<K: FloatKind> Clone for RFloat<K> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: FloatKind> Copy for RFloat<K> {}
impl<K: FloatKind> fmt::Debug for RFloat<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RFloat")
            .field("kind", &K::ieee_label())
            .finish()
    }
}
impl<K: FloatKind> sealed::Sealed for RFloat<K> {}
impl<K: FloatKind> ReturnMarker for RFloat<K> {}

/// Opaque-pointer return (any address space).
#[derive(Debug, Clone, Copy)]
pub struct RPtr;
impl sealed::Sealed for RPtr {}
impl ReturnMarker for RPtr {}

/// Anything not statically described (struct, vector, array,
/// target-ext, parsed IR…). The runtime path uses this; the IRBuilder
/// keeps its current runtime [`IrError::ReturnTypeMismatch`](
/// crate::IrError::ReturnTypeMismatch) check exclusively for `RDyn`.
#[derive(Debug, Clone, Copy)]
pub struct RDyn;
impl sealed::Sealed for RDyn {}
impl ReturnMarker for RDyn {}
