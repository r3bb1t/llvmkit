//! Sealed marker types for pointer address spaces.
//!
//! The pointer analog of [`int_width`](crate::int_width): where that module
//! encodes an integer's bit-width in the type system, this one encodes the
//! address space a pointer lives in. A pointer marked [`Space<N>`] is a
//! different type from one marked [`Space<M>`], so mixing them where LLVM needs
//! one pointer type (a `select`'s arms, a `pointer_cmp`'s operands) is a
//! compile error rather than a verifier refusal.
//!
//! An LLVM `PointerType *` carries its address space only at run time
//! (`PointerType::getAddressSpace`, `include/llvm/IR/DerivedTypes.h`); the
//! marker is llvmkit's addition, and the run-time address space stays the
//! source of truth — [`PointerType::address_space`](crate::PointerType::address_space)
//! answers it for both markers.
//!
//! A pointer whose address space is known only at run time — the parser's, one
//! made by [`ptr_type`](crate::Module::ptr_type) from a run-time number, a
//! default `alloca` in the DataLayout's alloca address space — is marked
//! [`SpaceDyn`] until the consumer narrows it with `TryFrom`, which checks.
//!
//! The trait is **sealed** — the set of markers is closed, not an extension
//! point.

use core::fmt;

/// Sealed marker trait implemented by every address-space tag.
///
/// The address-space analog of [`IntWidth`](crate::IntWidth).
pub trait AddrSpace: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// The address space when it is known at compile time, else `None`
    /// ([`SpaceDyn`]). Analog of [`IntWidth::static_bits`](crate::IntWidth::static_bits).
    ///
    /// ```
    /// # use llvmkit_ir::{AddrSpace, Space, SpaceDyn};
    /// assert_eq!(Space::<3>::static_space(), Some(3));
    /// assert_eq!(SpaceDyn::static_space(), None);
    /// ```
    fn static_space() -> Option<u32>;
}

/// Const-generic marker for a statically known address space. Analog of
/// [`Width<N>`](crate::Width).
///
/// `Space<0>` is the default address space — what `ptr` means in textual IR
/// and what [`Ptr`](crate::Ptr) means in a typed signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Space<const N: u32>;
impl<const N: u32> AddrSpace for Space<N> {
    #[inline]
    fn static_space() -> Option<u32> {
        Some(N)
    }
}

/// Address-space-erased marker. The handle still carries its address space as
/// run-time data; this marker only says the type system does not know it.
/// Analog of [`IntDyn`](crate::IntDyn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpaceDyn;
impl AddrSpace for SpaceDyn {
    #[inline]
    fn static_space() -> Option<u32> {
        None
    }
}

mod sealed {
    pub trait Sealed {}
    impl<const N: u32> Sealed for super::Space<N> {}
    impl Sealed for super::SpaceDyn {}
}
