//! View wrappers around instructions / constant-expressions that
//! share an operator shape. Mirrors `llvm/include/llvm/IR/Operator.h`.
//!
//! The full file is ~700 lines of class hierarchy
//! (`OverflowingBinaryOperator`, `PossiblyExactOperator`,
//! `FPMathOperator`, `GEPOperator`, `BitCastOperator`, ...). This module
//! ships thin views for [`OverflowingBinaryOperator`] (`nuw`/`nsw` on
//! `add`/`sub`/`mul`/`shl`) and [`PossiblyExactOperator`] (`exact` on
//! `udiv`/`sdiv`/`lshr`/`ashr`). The remaining operator classes land as
//! their consumers do.

use crate::instructions::{
    AddInst, AshrInst, LshrInst, MulInst, SdivInst, ShlInst, SubInst, UdivInst,
};
use crate::module::ModuleBrand;

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
/// Mirrors `PossiblyExactOperator` — implemented for [`UdivInst`],
/// [`SdivInst`], [`LshrInst`], and [`AshrInst`].
pub trait PossiblyExactOperator<'ctx> {
    /// `exact` flag.
    fn is_exact(&self) -> bool;
}

impl<'ctx, B: ModuleBrand> OverflowingBinaryOperator<'ctx> for AddInst<'ctx, B> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        AddInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        AddInst::has_no_signed_wrap(self)
    }
}

impl<'ctx, B: ModuleBrand> OverflowingBinaryOperator<'ctx> for SubInst<'ctx, B> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        SubInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        SubInst::has_no_signed_wrap(self)
    }
}

impl<'ctx, B: ModuleBrand> OverflowingBinaryOperator<'ctx> for MulInst<'ctx, B> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        MulInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        MulInst::has_no_signed_wrap(self)
    }
}

impl<'ctx, B: ModuleBrand> OverflowingBinaryOperator<'ctx> for ShlInst<'ctx, B> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        ShlInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        ShlInst::has_no_signed_wrap(self)
    }
}

impl<'ctx, B: ModuleBrand> PossiblyExactOperator<'ctx> for UdivInst<'ctx, B> {
    #[inline]
    fn is_exact(&self) -> bool {
        UdivInst::is_exact(*self)
    }
}
impl<'ctx, B: ModuleBrand> PossiblyExactOperator<'ctx> for SdivInst<'ctx, B> {
    #[inline]
    fn is_exact(&self) -> bool {
        SdivInst::is_exact(*self)
    }
}
impl<'ctx, B: ModuleBrand> PossiblyExactOperator<'ctx> for LshrInst<'ctx, B> {
    #[inline]
    fn is_exact(&self) -> bool {
        LshrInst::is_exact(*self)
    }
}
impl<'ctx, B: ModuleBrand> PossiblyExactOperator<'ctx> for AshrInst<'ctx, B> {
    #[inline]
    fn is_exact(&self) -> bool {
        AshrInst::is_exact(*self)
    }
}
