//! View wrappers around instructions / constant-expressions that
//! share an operator shape. Mirrors `llvm/include/llvm/IR/Operator.h`.
//!
//! The full file is ~700 lines of class hierarchy
//! (`OverflowingBinaryOperator`, `PossiblyExactOperator`,
//! `FPMathOperator`, `GEPOperator`, `BitCastOperator`, ...). Phase E
//! ships only the slice we need today: a thin view for
//! [`OverflowingBinaryOperator`] over the binary operators that carry
//! `nuw` / `nsw`. The rest land as their consumers do.

use crate::instructions::{AddInst, MulInst, SubInst};

/// Common interface for the binary operators that carry `nuw`/`nsw`
/// flags. Mirrors `OverflowingBinaryOperator`.
///
/// Implemented for [`AddInst`], [`SubInst`], [`MulInst`]. Future
/// additions (e.g. `Shl`) extend the trait once their handles land.
pub trait OverflowingBinaryOperator<'ctx> {
    /// `nuw` flag.
    fn has_no_unsigned_wrap(self) -> bool;
    /// `nsw` flag.
    fn has_no_signed_wrap(self) -> bool;
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
