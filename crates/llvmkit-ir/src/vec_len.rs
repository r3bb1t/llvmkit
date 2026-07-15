//! Sealed marker types for fixed-vector lane counts.
//!
//! The vector analog of [`int_width`](crate::int_width): where that
//! module encodes an integer's bit-width in the type system, this one
//! encodes a fixed vector's lane count. An `N`-lane vector marked
//! [`Len<N>`] is a different type from an `M`-lane vector marked
//! [`Len<M>`], so mixing them at a builder call site is a compile error
//! rather than a runtime shape mismatch.
//!
//! Lane counts are `u32` (mirroring `VectorType::getNumElements`'s
//! `ElementCount` count, which is a 32-bit unsigned quantity). The
//! sibling [`array_len`](crate::array_len) marker uses `u64` because
//! `ArrayType` lengths are 64-bit; the two families are deliberately
//! kept distinct so a single trait is not asked to be both `u32` and
//! `u64`.
//!
//! Parsed `.ll` whose lane count is not statically known lands in
//! [`LenDyn`] until the consumer narrows it.
//!
//! The trait is **sealed** — the set of length markers is closed, not
//! an extension point.

use core::fmt;

/// Sealed marker trait implemented by every fixed-vector lane-count tag.
///
/// The lane-count analog of [`IntWidth`](crate::IntWidth).
pub trait VecLen: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// Static lane count if known at compile time, else `None` (used by
    /// [`LenDyn`]). Analog of [`IntWidth::static_bits`](crate::IntWidth::static_bits).
    fn static_len() -> Option<u32>;
}

/// Const-generic marker for a statically known lane count. Analog of
/// [`Width<N>`](crate::Width).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Len<const N: u32>;
impl<const N: u32> VecLen for Len<N> {
    #[inline]
    fn static_len() -> Option<u32> {
        Some(N)
    }
}

/// Lane-count-erased marker. The handle still tracks its lane count as
/// runtime data; this marker only signals "the type system does not
/// know how many lanes." Analog of [`IntDyn`](crate::IntDyn).
///
/// Used by parsed IR (where the source-level lane count is whatever the
/// `.ll` says) and by APIs that genuinely cannot be statically typed.
/// Distinct from [`crate::array_len::ArrLenDyn`] — the vector and array
/// sides each carry their own erasure marker so trait coherence stays
/// sane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LenDyn;
impl VecLen for LenDyn {
    #[inline]
    fn static_len() -> Option<u32> {
        None
    }
}

/// Sealed: lane-count markers whose count is known at compile time.
/// Analog of [`StaticIntWidth`](crate::StaticIntWidth).
///
/// Not implemented for [`LenDyn`] — there is no single static lane count
/// for an erased vector.
pub trait StaticVecLen: VecLen {
    /// Lane count at the type level. Usable as `L::STATIC_LEN` in
    /// `const { ... }` assertions to enforce shape-equality at
    /// monomorphisation time. Analog of
    /// [`StaticIntWidth::STATIC_BITS`](crate::StaticIntWidth::STATIC_BITS).
    const STATIC_LEN: u32;
}
impl<const N: u32> StaticVecLen for Len<N> {
    const STATIC_LEN: u32 = N;
}

mod sealed {
    pub trait Sealed {}
    impl<const N: u32> Sealed for super::Len<N> {}
    impl Sealed for super::LenDyn {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn len_reports_static_lane_count() {
        assert_eq!(Len::<4>::static_len(), Some(4));
    }

    #[test]
    fn len_dyn_has_no_static_lane_count() {
        assert_eq!(LenDyn::static_len(), None);
    }

    #[test]
    fn static_vec_len_exposes_const() {
        assert_eq!(<Len<4> as StaticVecLen>::STATIC_LEN, 4);
    }
}
