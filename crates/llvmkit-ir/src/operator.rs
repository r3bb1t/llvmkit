//! View wrappers around instructions / constant-expressions that
//! share an operator shape. Mirrors `llvm/include/llvm/IR/Operator.h`.
//!
//! The full file is ~700 lines of class hierarchy
//! (`OverflowingBinaryOperator`, `PossiblyExactOperator`,
//! `FPMathOperator`, `GEPOperator`, `BitCastOperator`, ...). This module
//! ships thin views for [`OverflowingBinaryOperator`] (`nuw`/`nsw` on
//! `add`/`sub`/`mul`/`shl`) and [`PossiblyExactOperator`] (`exact` on
//! `udiv`/`sdiv`/`lshr`/`ashr`), and `FPMathOperator`'s `classof` with its
//! type predicate [`is_supported_floating_point_type`]. The remaining operator
//! classes land as their consumers do.

use crate::capability::Capability;
use crate::derived_types::AnyTypeEnum;
use crate::fmf::FastMathFlags;
use crate::instr_types::CastOpcode;
use crate::instruction::InstructionKindData;
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
fn is_composed_of_homogeneous_floating_point_types<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ty: Type<'ctx, B, C>,
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
///
/// A read, so a type of either capability is accepted.
pub fn is_supported_floating_point_type<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    ty: Type<'ctx, B, C>,
) -> bool {
    ty.is_float_or_float_vector() || is_composed_of_homogeneous_floating_point_types(ty)
}

/// The fast-math flags of an instruction that is an `FPMathOperator`, or
/// `None` for one that is not: `dyn_cast<FPMathOperator>(I)` followed by
/// `getFastMathFlags()`.
///
/// Ports `FPMathOperator::classof`'s `switch`, in its order. `fneg`, the five
/// floating-point binary operators, `fptrunc`, `fpext` and `fcmp` always are
/// one. A `phi`, a `select` and a `call` are one when `ty`, the instruction's
/// result type, passes [`is_supported_floating_point_type`]. Every other
/// opcode is not. `classof` admits only an `Instruction`, so a constant
/// expression is never one, and this takes an instruction's payload.
///
/// Upstream reads the flags out of `SubclassOptionalData`, which each
/// `FPMathOperator` keeps its flags in. llvmkit keeps them in the payloads
/// that can carry them, which are exactly the opcodes `classof` admits.
pub(crate) fn fp_math_operator_flags<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
    instruction: &InstructionKindData,
    ty: Type<'ctx, B, C>,
) -> Option<FastMathFlags> {
    match instruction {
        // `case Instruction::FNeg:`
        InstructionKindData::Fneg(data) => Some(data.fmf),
        // `case Instruction::FAdd: case Instruction::FSub: case
        // Instruction::FMul: case Instruction::FDiv: case Instruction::FRem:`
        InstructionKindData::Fadd(data)
        | InstructionKindData::Fsub(data)
        | InstructionKindData::Fmul(data)
        | InstructionKindData::Fdiv(data)
        | InstructionKindData::Frem(data) => Some(data.fmf),
        // `case Instruction::FPTrunc: case Instruction::FPExt:`
        InstructionKindData::Cast(data)
            if matches!(data.kind, CastOpcode::FpTrunc | CastOpcode::FpExt) =>
        {
            Some(data.fmf.get())
        }
        // `case Instruction::FCmp: return true;`
        InstructionKindData::Fcmp(data) => Some(data.fmf),
        // `case Instruction::PHI: case Instruction::Select: case
        // Instruction::Call: return isSupportedFloatingPointType(V->getType());`
        InstructionKindData::Phi(data) => {
            is_supported_floating_point_type(ty).then(|| data.fmf.get())
        }
        InstructionKindData::Select(data) => {
            is_supported_floating_point_type(ty).then(|| data.fmf.get())
        }
        InstructionKindData::Call(data) => is_supported_floating_point_type(ty).then_some(data.fmf),
        // `default: return false;` — matched exhaustively, so a new opcode has
        // to be classified rather than defaulted.
        InstructionKindData::Add(_)
        | InstructionKindData::Sub(_)
        | InstructionKindData::Mul(_)
        | InstructionKindData::Udiv(_)
        | InstructionKindData::Sdiv(_)
        | InstructionKindData::Urem(_)
        | InstructionKindData::Srem(_)
        | InstructionKindData::Shl(_)
        | InstructionKindData::Lshr(_)
        | InstructionKindData::Ashr(_)
        | InstructionKindData::And(_)
        | InstructionKindData::Or(_)
        | InstructionKindData::Xor(_)
        | InstructionKindData::Cast(_)
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Gep(_)
        | InstructionKindData::Icmp(_)
        | InstructionKindData::Freeze(_)
        | InstructionKindData::VaArg(_)
        | InstructionKindData::ExtractValue(_)
        | InstructionKindData::InsertValue(_)
        | InstructionKindData::ExtractElement(_)
        | InstructionKindData::InsertElement(_)
        | InstructionKindData::ShuffleVector(_)
        | InstructionKindData::Fence(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRmw(_)
        | InstructionKindData::Switch(_)
        | InstructionKindData::IndirectBr(_)
        | InstructionKindData::Invoke(_)
        | InstructionKindData::CallBr(_)
        | InstructionKindData::LandingPad(_)
        | InstructionKindData::Resume(_)
        | InstructionKindData::CleanupPad(_)
        | InstructionKindData::CatchPad(_)
        | InstructionKindData::CatchReturn(_)
        | InstructionKindData::CleanupReturn(_)
        | InstructionKindData::CatchSwitch(_)
        | InstructionKindData::Ret(_)
        | InstructionKindData::Br(_)
        | InstructionKindData::Unreachable(_) => None,
    }
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

impl<'ctx, B: ModuleBrand, C: Capability> OverflowingBinaryOperator<'ctx> for AddInst<'ctx, B, C> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        AddInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        AddInst::has_no_signed_wrap(self)
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> OverflowingBinaryOperator<'ctx> for SubInst<'ctx, B, C> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        SubInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        SubInst::has_no_signed_wrap(self)
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> OverflowingBinaryOperator<'ctx> for MulInst<'ctx, B, C> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        MulInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        MulInst::has_no_signed_wrap(self)
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> OverflowingBinaryOperator<'ctx> for ShlInst<'ctx, B, C> {
    #[inline]
    fn has_no_unsigned_wrap(self) -> bool {
        ShlInst::has_no_unsigned_wrap(self)
    }
    #[inline]
    fn has_no_signed_wrap(self) -> bool {
        ShlInst::has_no_signed_wrap(self)
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> PossiblyExactOperator<'ctx> for UdivInst<'ctx, B, C> {
    #[inline]
    fn is_exact(&self) -> bool {
        UdivInst::is_exact(*self)
    }
}
impl<'ctx, B: ModuleBrand, C: Capability> PossiblyExactOperator<'ctx> for SdivInst<'ctx, B, C> {
    #[inline]
    fn is_exact(&self) -> bool {
        SdivInst::is_exact(*self)
    }
}
impl<'ctx, B: ModuleBrand, C: Capability> PossiblyExactOperator<'ctx> for LshrInst<'ctx, B, C> {
    #[inline]
    fn is_exact(&self) -> bool {
        LshrInst::is_exact(*self)
    }
}
impl<'ctx, B: ModuleBrand, C: Capability> PossiblyExactOperator<'ctx> for AshrInst<'ctx, B, C> {
    #[inline]
    fn is_exact(&self) -> bool {
        AshrInst::is_exact(*self)
    }
}
