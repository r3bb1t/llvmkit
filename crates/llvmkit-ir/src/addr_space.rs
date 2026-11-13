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
//!
//! [`AddressSpace`] is the run-time number itself, and it holds only what LLVM
//! can: 24 bits. Every route that names an address space takes, returns or yields
//! one, so a wider number is refused once, where it becomes an [`AddressSpace`],
//! and nothing past that point can fail. [`AddressSpace::get`] and `u32::from` are
//! the deliberate way back to a number. Two commands check the rest, and both
//! print nothing, measured on top of `de3f63c`:
//! `rg -n "(addr|address)_spaces?\w*: *(Vec<|&\[|Option<)?u32" crates` for what a
//! route takes or holds, and
//! `rg -nU "fn \w*space\w*(<[^>]*>)?\([^)]*\)\s*->\s*[^{;]*\bu32\b" crates` for what
//! it returns or yields. They search by name, so they do not reach
//! [`IrError::AddressSpaceMismatch`](crate::IrError::AddressSpaceMismatch), whose
//! fields are `expected` and `got`. Those are [`AddressSpace`] too.

use core::fmt;

/// An address space LLVM can hold: a number below `1 << 24`.
///
/// `Type` keeps a pointer's address space in its 24-bit `SubclassData`, and
/// `Type::setSubclassData` (`include/llvm/IR/Type.h`) asserts that it fits;
/// `LLParser::parseOptionalAddrSpace` and `DataLayout`'s `parseAddrSpace` refuse
/// anything wider. So a wider pointer type is one no parser reads back, and this
/// type cannot name one. [`address_space`] checks a known number at compile time
/// and [`new`](Self::new) a run-time one, as `lanes` and `NonZeroU32::new` do for
/// a vector's length.
///
/// ```
/// # use llvmkit_ir::{AddressSpace, Module, address_space};
/// let m = Module::dynamic("doc");
/// assert_eq!(m.ptr_type(address_space::<3>()).to_string(), "ptr addrspace(3)");
/// assert_eq!(AddressSpace::new(1 << 24), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AddressSpace(u32);

impl AddressSpace {
    /// Address space 0, which is what `ptr` means in textual IR.
    pub const DEFAULT: Self = Self(0);

    /// The largest address space: `(1 << 24) - 1`.
    pub const MAX: Self = Self((1 << 24) - 1);

    /// `value` as an address space, or `None` when it needs more than 24 bits.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX.0 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The address space as a number.
    #[inline]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for AddressSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Eight bits always fit.
impl From<u8> for AddressSpace {
    #[inline]
    fn from(value: u8) -> Self {
        Self(u32::from(value))
    }
}

impl From<AddressSpace> for u32 {
    #[inline]
    fn from(space: AddressSpace) -> Self {
        space.0
    }
}

impl PartialEq<u32> for AddressSpace {
    #[inline]
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

impl PartialEq<AddressSpace> for u32 {
    #[inline]
    fn eq(&self, other: &AddressSpace) -> bool {
        *self == other.0
    }
}

/// The address space `N` as an [`AddressSpace`], checked at compile time:
/// `address_space::<16777216>()` does not compile.
pub const fn address_space<const N: u32>() -> AddressSpace {
    const {
        match AddressSpace::new(N) {
            Some(space) => space,
            None => panic!(
                "an address space is a 24-bit number: `Type::setSubclassData` asserts it fits"
            ),
        }
    }
}

/// Sealed marker trait implemented by every address-space tag.
///
/// The address-space analog of [`IntWidth`](crate::IntWidth).
pub trait AddrSpace: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// The address space when it is known at compile time, else `None`
    /// ([`SpaceDyn`]). Analog of [`IntWidth::static_bits`](crate::IntWidth::static_bits).
    ///
    /// ```
    /// # use llvmkit_ir::{AddrSpace, Space, SpaceDyn, address_space};
    /// assert_eq!(Space::<3>::static_space(), Some(address_space::<3>()));
    /// assert_eq!(SpaceDyn::static_space(), None);
    /// ```
    fn static_space() -> Option<AddressSpace>;
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
    fn static_space() -> Option<AddressSpace> {
        Some(address_space::<N>())
    }
}

/// Address-space-erased marker. The handle still carries its address space as
/// run-time data; this marker only says the type system does not know it.
/// Analog of [`IntDyn`](crate::IntDyn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpaceDyn;
impl AddrSpace for SpaceDyn {
    #[inline]
    fn static_space() -> Option<AddressSpace> {
        None
    }
}

mod sealed {
    pub trait Sealed {}
    impl<const N: u32> Sealed for super::Space<N> {}
    impl Sealed for super::SpaceDyn {}
}
