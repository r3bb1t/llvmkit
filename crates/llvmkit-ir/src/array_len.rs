//! Sealed marker types for array lengths.
//!
//! The array analog of [`int_width`](crate::int_width): where that
//! module encodes an integer's bit-width in the type system, this one
//! encodes an array's element count. An `N`-element array marked
//! [`ArrLen<N>`] is a different type from an `M`-element array marked
//! [`ArrLen<M>`], so mixing them at a builder call site is a compile
//! error rather than a runtime shape mismatch.
//!
//! A static length is a `usize`, the type a Rust array's length has, so a
//! `[T; N]`'s `N` names [`ArrLen<N>`] as it is. A run-time length is a
//! `u64` (mirroring `ArrayType::getNumElements`, which returns a 64-bit
//! count), and [`ArrayLen::static_len`] answers in that currency. This is a
//! distinct family from the sibling [`vec_len`](crate::vec_len) marker
//! (whose lane counts are `u32`).
//!
//! Parsed `.ll` whose length is not statically known lands in
//! [`ArrLenDyn`] until the consumer narrows it.
//!
//! The trait is **sealed** — the set of length markers is closed, not
//! an extension point.

use core::fmt;

// A `usize` is at most 64 bits wide on every target this crate builds for,
// so a static length always fits the `u64` a run-time length is.
const _: () = assert!(usize::BITS <= u64::BITS, "a usize must fit in a u64");

/// A static array length as the `u64` a run-time length is. Infallible:
/// `usize::BITS <= u64::BITS` is asserted at compile time above.
pub(crate) fn static_len_u64(len: usize) -> u64 {
    u64::try_from(len)
        .unwrap_or_else(|_| unreachable!("usize::BITS <= u64::BITS, asserted at compile time"))
}

/// Sealed marker trait implemented by every array-length tag.
///
/// The array-length analog of [`IntWidth`](crate::IntWidth).
pub trait ArrayLen: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// Static element count if known at compile time, else `None` (used
    /// by [`ArrLenDyn`]), as the `u64` a run-time length is. Analog of
    /// [`IntWidth::static_bits`](crate::IntWidth::static_bits).
    fn static_len() -> Option<u64>;
}

/// Const-generic marker for a statically known array length — a `usize`,
/// like a Rust array's. Analog of [`Width<N>`](crate::Width).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArrLen<const N: usize>;
impl<const N: usize> ArrayLen for ArrLen<N> {
    #[inline]
    fn static_len() -> Option<u64> {
        Some(static_len_u64(N))
    }
}

/// Length-erased marker. The handle still tracks its element count as
/// runtime data; this marker only signals "the type system does not
/// know how many elements." Analog of [`IntDyn`](crate::IntDyn).
///
/// Used by parsed IR (where the source-level length is whatever the
/// `.ll` says) and by APIs that genuinely cannot be statically typed.
/// Distinct from [`crate::vec_len::LenDyn`] — the array and vector sides
/// each carry their own erasure marker so trait coherence stays sane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArrLenDyn;
impl ArrayLen for ArrLenDyn {
    #[inline]
    fn static_len() -> Option<u64> {
        None
    }
}

/// Sealed: array-length markers whose count is known at compile time.
/// Analog of [`StaticIntWidth`](crate::StaticIntWidth).
///
/// Not implemented for [`ArrLenDyn`] — there is no single static length
/// for an erased array.
pub trait StaticArrayLen: ArrayLen {
    /// Element count at the type level, as the `usize` a Rust array's
    /// length is. Usable as `L::STATIC_LEN` in `const { ... }` assertions
    /// to enforce shape-equality at monomorphisation time. Analog of
    /// [`StaticIntWidth::STATIC_BITS`](crate::StaticIntWidth::STATIC_BITS).
    const STATIC_LEN: usize;
}
impl<const N: usize> StaticArrayLen for ArrLen<N> {
    const STATIC_LEN: usize = N;
}

mod sealed {
    pub trait Sealed {}
    impl<const N: usize> Sealed for super::ArrLen<N> {}
    impl Sealed for super::ArrLenDyn {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arr_len_reports_static_length() {
        assert_eq!(ArrLen::<4>::static_len(), Some(4));
    }

    #[test]
    fn arr_len_dyn_has_no_static_length() {
        assert_eq!(ArrLenDyn::static_len(), None);
    }

    #[test]
    fn static_array_len_exposes_const() {
        assert_eq!(<ArrLen<4> as StaticArrayLen>::STATIC_LEN, 4);
    }
}
