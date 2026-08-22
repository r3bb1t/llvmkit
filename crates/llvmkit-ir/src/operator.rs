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

use crate::derived_types::AnyTypeEnum;
use crate::instructions::{
    AddInst, AshrInst, LshrInst, MulInst, SdivInst, ShlInst, SubInst, UdivInst,
};
use crate::module::ModuleBrand;
use crate::r#type::Type;

/// `true` when `ty` is composed of a single kind of floating-point type,
/// possibly repeated within an aggregate. Mirrors
/// `FPMathOperator::isComposedOfHomogeneousFloatingPointTypes`, which is
/// private to `FPMathOperator` upstream and so is private here.
///
/// A **literal** struct qualifies when `containsHomogeneousTypes` holds and
/// its first element is FP-or-FP-vector; an array qualifies when the type
/// reached by peeling every array level is. An identified struct never does.
fn is_composed_of_homogeneous_floating_point_types<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
) -> bool {
    let mut ty = ty;
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Struct(struct_ty) => {
            if !struct_ty.is_literal() || !struct_ty.contains_homogeneous_types() {
                return false;
            }
            // `Ty = StructTy->elements().front()`. `containsHomogeneousTypes`
            // has just established the body is non-empty, so the miss arm is
            // dead by construction rather than a second rejection.
            let Some(front) = struct_ty.field_type(0) else {
                return false;
            };
            ty = front;
        }
        AnyTypeEnum::Array(_) => {
            // `do { Ty = ArrayTy->getElementType(); } while (dyn_cast<ArrayType>(Ty))`.
            while let AnyTypeEnum::Array(array_ty) = AnyTypeEnum::from(ty) {
                ty = array_ty.element();
            }
        }
        _ => {}
    }
    ty.is_float_or_float_vector()
}

/// `true` for the types a `phi`, `select` or `call` may carry fast-math flags
/// on. Mirrors `FPMathOperator::isSupportedFloatingPointType`:
/// `Ty->isFPOrFPVectorTy() || isComposedOfHomogeneousFloatingPointTypes(Ty)`.
///
/// This is the predicate `FPMathOperator::classof`'s `PHI` / `Select` / `Call`
/// arm asks, and therefore the one behind
/// `LLParser::parseInstruction`'s two fast-math guards and `parseCall`'s third.
/// It is **wider** than [`Type::is_float_or_float_vector`], which is what the
/// `fcmp` and `atomicrmw` operand checks ask instead.
pub fn is_supported_floating_point_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> bool {
    ty.is_float_or_float_vector() || is_composed_of_homogeneous_floating_point_types(ty)
}

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
