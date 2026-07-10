//! View wrappers around instructions / constant-expressions that
//! share an operator shape. Mirrors `llvm/include/llvm/IR/Operator.h`.
//!
//! The full file is ~700 lines of class hierarchy
//! (`OverflowingBinaryOperator`, `PossiblyExactOperator`,
//! `FPMathOperator`, `GEPOperator`, `BitCastOperator`, ...). Phase E
//! ships only the slice we need today: a thin view for
//! [`OverflowingBinaryOperator`] over the binary operators that carry
//! `nuw` / `nsw`. The rest land as their consumers do.

use crate::instructions::{AShrInst, AddInst, LShrInst, MulInst, SDivInst, ShlInst, SubInst, UDivInst};

/// Common interface for the binary operators that carry `nuw`/`nsw`
/// flags. Mirrors `OverflowingBinaryOperator`.
///
/// Implemented for [`AddInst`], [`SubInst`], [`MulInst`], and [`ShlInst`]
/// — the four opcodes LLVM's `OverflowingBinaryOperator::classof` accepts.
pub trait OverflowingBinaryOperator<'ctx> {
    /// `nuw` flag.
    fn has_no_unsigned_wrap(self) -> bool;
    /// `nsw` flag.
    fn has_no_signed_wrap(self) -> bool;
}

/// Common interface for the binary operators that carry the `exact` flag.
/// Mirrors `PossiblyExactOperator` — implemented for [`UDivInst`],
/// [`SDivInst`], [`LShrInst`], and [`AShrInst`].
pub trait PossiblyExactOperator<'ctx> {
    /// `exact` flag.
    fn is_exact(self) -> bool;
}

impl<'ctx> OverflowingBinaryOperator<'ctx> for AddInst<'ctx> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        AddInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        AddInst::has_no_signed_wrap(self)
    }
}

impl<'ctx> OverflowingBinaryOperator<'ctx> for SubInst<'ctx> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        SubInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        SubInst::has_no_signed_wrap(self)
    }
}

impl<'ctx> OverflowingBinaryOperator<'ctx> for MulInst<'ctx> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        MulInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        MulInst::has_no_signed_wrap(self)
    }
}

impl<'ctx> OverflowingBinaryOperator<'ctx> for ShlInst<'ctx> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        ShlInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        ShlInst::has_no_signed_wrap(self)
    }
}

impl<'ctx> PossiblyExactOperator<'ctx> for UDivInst<'ctx> {
    #[inline]
    fn is_exact(self) -> bool {
        UDivInst::is_exact(self)
    }
}
impl<'ctx> PossiblyExactOperator<'ctx> for SDivInst<'ctx> {
    #[inline]
    fn is_exact(self) -> bool {
        SDivInst::is_exact(self)
    }
}
impl<'ctx> PossiblyExactOperator<'ctx> for LShrInst<'ctx> {
    #[inline]
    fn is_exact(self) -> bool {
        LShrInst::is_exact(self)
    }
}
impl<'ctx> PossiblyExactOperator<'ctx> for AShrInst<'ctx> {
    #[inline]
    fn is_exact(self) -> bool {
        AShrInst::is_exact(self)
    }
}
