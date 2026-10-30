//! Sealed marker types and trait for a function's return shape.
//!
//! Mirrors the type-level distinction LLVM C++ keeps implicit at the
//! `Function::getReturnType()` boundary: in C++ you read the return
//! type at runtime and pattern-match on it; in Rust we encode it in
//! the type system so the IrBuilder can reject a `ret(int_value)`
//! against a `void`-returning function at compile time.
//!
//! The marker is the bare type the caller already names:
//!
//! - `()` — `void` return; the builder exposes only `ret_void`.
//! - `bool` / `i8` / `i16` / `i32` / `i64` / `i128` / [`crate::IntDyn`]
//!   — `iN` return for the matching width.
//! - `f32` / `f64` / [`crate::Half`] / [`crate::Bfloat`] /
//!   [`crate::Fp128`] / [`crate::X86Fp80`] / [`crate::PpcFp128`] /
//!   [`crate::FloatDyn`] — IEEE/non-IEEE float return.
//! - [`Ptr`] — opaque-pointer return in address space `AS` (`0` when written
//!   plain `Ptr`); its checks require that address space exactly.
//! - [`Dyn`] — anything not statically described (struct, vector,
//!   array, target-ext, parsed IR…). The IrBuilder keeps a runtime
//!   [`crate::IrError::ReturnTypeMismatch`] check exclusively for
//!   `Dyn`.
//!
//! The trait is **sealed** — the closed set of return shapes is part
//! of the IR spec, not an extension point.

use core::fmt;

use super::float_kind::{Bfloat, FloatDyn, Fp128, Half, PpcFp128, X86Fp80};
use super::int_width::{IntDyn, Width};
use crate::addr_space::{AddressSpace, address_space};
use crate::error::TypeKindLabel;
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
    /// A pointer in this address space.
    Ptr(AddressSpace),
    Dyn,
    IntStatic(u32),
    IntDyn,
    FloatStatic(TypeKindLabel),
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
    /// `core::any::TypeSlot`.
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

/// Opaque pointer in address space `AS` — `0`, the default address space, when
/// written plain `Ptr` — as a typed signature or struct-field marker. Its value
/// is a [`PointerValue<'ctx, Space<AS>, …>`](crate::PointerValue), and its type
/// checks require that address space exactly (a `ptr addrspace(1)` is not a
/// `Ptr`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ptr<const AS: u32 = 0>(*mut core::ffi::c_void);
impl<const AS: u32> Ptr<AS> {
    /// Wrap a raw pointer — the Rust-side value of a `Ptr` field in a
    /// `#[derive(IrStruct)]` struct, which is Rust data as well as an IR
    /// schema.
    ///
    /// ```
    /// use llvmkit_ir::Ptr;
    /// let mut byte = 0_u8;
    /// let raw = core::ptr::from_mut(&mut byte).cast::<core::ffi::c_void>();
    /// assert_eq!(Ptr::<1>::new(raw).as_ptr(), raw);
    /// ```
    #[inline]
    pub const fn new(pointer: *mut core::ffi::c_void) -> Self {
        Self(pointer)
    }

    /// The null pointer.
    ///
    /// ```
    /// use llvmkit_ir::Ptr;
    /// assert!(Ptr::<0>::null().as_ptr().is_null());
    /// ```
    #[inline]
    pub const fn null() -> Self {
        Self(core::ptr::null_mut())
    }

    /// The wrapped raw pointer.
    #[inline]
    pub const fn as_ptr(self) -> *mut core::ffi::c_void {
        self.0
    }
}
impl<const AS: u32> sealed::Sealed for Ptr<AS> {}
impl<const AS: u32> ReturnMarker for Ptr<AS> {
    #[inline]
    fn expected_kind() -> ExpectedRetKind {
        ExpectedRetKind::Ptr(address_space::<AS>())
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
                match <$ty as FloatKind>::ieee_kind() {
                    Some(kind) => ExpectedRetKind::FloatStatic(kind),
                    None => ExpectedRetKind::FloatDyn,
                }
            }
        }
    )+ };
}
impl_int_return_marker!(bool, i8, i16, i32, i64, i128, IntDyn);
impl_float_return_marker!(f32, f64, Half, Bfloat, Fp128, X86Fp80, PpcFp128, FloatDyn,);

// `Width<N>` participates as a return marker. Const-generic blanket
// is sound because `Width<N>` doesn't implement `FloatKind`, so no
// overlap with the float-marker impls.
impl<const N: u32> ReturnMarker for Width<N> {
    #[inline]
    fn expected_kind() -> ExpectedRetKind {
        ExpectedRetKind::IntStatic(N)
    }
}

/// Diagnostic label for a return marker's expected type kind. `None`
/// for [`Dyn`], which matches every signature. Crate-internal — used
/// to fix the expected/got duplication in `ReturnTypeMismatch` reports
/// (the dyn call-family builders previously reported `expected` and
/// `got` as the same value, since the marker side had no route to a
/// `TypeKindLabel`).
pub(crate) fn marker_kind_label<R: ReturnMarker>() -> Option<TypeKindLabel> {
    match R::expected_kind() {
        ExpectedRetKind::Void => Some(TypeKindLabel::Void),
        ExpectedRetKind::Ptr(_) => Some(TypeKindLabel::Pointer),
        ExpectedRetKind::IntStatic(_) | ExpectedRetKind::IntDyn => Some(TypeKindLabel::Integer),
        ExpectedRetKind::FloatStatic(kind) => Some(kind),
        ExpectedRetKind::FloatDyn => Some(TypeKindLabel::Float),
        ExpectedRetKind::Dyn => None,
    }
}
