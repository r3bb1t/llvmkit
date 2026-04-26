//! Shared payload structs for instructions. Mirrors
//! `llvm/include/llvm/IR/InstrTypes.h`.
//!
//! The upstream `InstrTypes.h` is a grab-bag of base classes
//! (`UnaryInstruction`, `BinaryOperator`, `CastInst`, `CallBase`, ...)
//! that lift shared layout out of [`Instructions.h`](crate::instructions).
//! Phase E of the foundation only ships the binary-operator shape and
//! the return-instruction shape; `CastInst`, `CallBase`, etc. are
//! follow-up.
//!
//! The structs here are **storage payloads**, not public handles. They
//! are stored inside the per-instruction storage record
//! in the value arena. Public per-opcode handles live in
//! [`crate::instructions`].

use crate::value::ValueId;

/// Storage payload for the binary-operator opcodes (`add`, `sub`,
/// `mul`, ...). Mirrors the operand/flag layout of `BinaryOperator`
/// (`InstrTypes.h`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BinaryOpData {
    pub(crate) lhs: ValueId,
    pub(crate) rhs: ValueId,
    /// `nuw` (no-unsigned-wrap) flag. Mirrors
    /// `OverflowingBinaryOperator::NoUnsignedWrap`.
    pub(crate) no_unsigned_wrap: bool,
    /// `nsw` (no-signed-wrap) flag. Mirrors
    /// `OverflowingBinaryOperator::NoSignedWrap`.
    pub(crate) no_signed_wrap: bool,
}

impl BinaryOpData {
    pub(crate) fn new(lhs: ValueId, rhs: ValueId) -> Self {
        Self {
            lhs,
            rhs,
            no_unsigned_wrap: false,
            no_signed_wrap: false,
        }
    }
}

/// Storage payload for the `ret` terminator. `value: None` is `ret
/// void`. Mirrors `ReturnInst`'s operand layout (`Instructions.h`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ReturnOpData {
    pub(crate) value: Option<ValueId>,
}

/// Closed enum mirroring the cast opcodes in
/// `Instruction::CastOps` (`Instructions.h`). The set is fixed by the
/// IR spec; new entries are not expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CastOpcode {
    /// `trunc` — narrow an integer to a smaller width.
    Trunc,
    /// `zext` — widen an integer with zero-extension.
    ZExt,
    /// `sext` — widen an integer with sign-extension.
    SExt,
    /// `fptrunc` — narrow a float kind.
    FpTrunc,
    /// `fpext` — widen a float kind.
    FpExt,
    /// `fptoui` — float to unsigned integer.
    FpToUI,
    /// `fptosi` — float to signed integer.
    FpToSI,
    /// `uitofp` — unsigned integer to float.
    UIToFp,
    /// `sitofp` — signed integer to float.
    SIToFp,
    /// `ptrtoint` — pointer to integer.
    PtrToInt,
    /// `inttoptr` — integer to pointer.
    IntToPtr,
    /// `bitcast` — same-size bit reinterpretation.
    BitCast,
    /// `addrspacecast` — address-space change on a pointer.
    AddrSpaceCast,
}

impl CastOpcode {
    /// `.ll` keyword for this cast opcode.
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Trunc => "trunc",
            Self::ZExt => "zext",
            Self::SExt => "sext",
            Self::FpTrunc => "fptrunc",
            Self::FpExt => "fpext",
            Self::FpToUI => "fptoui",
            Self::FpToSI => "fptosi",
            Self::UIToFp => "uitofp",
            Self::SIToFp => "sitofp",
            Self::PtrToInt => "ptrtoint",
            Self::IntToPtr => "inttoptr",
            Self::BitCast => "bitcast",
            Self::AddrSpaceCast => "addrspacecast",
        }
    }
}

/// Storage payload for a cast instruction. The destination type is
/// carried in the host [`crate::value::ValueData::ty`] field; this
/// payload only needs the source operand and which opcode to emit.
/// Mirrors the operand layout of `CastInst` (`InstrTypes.h`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CastOpData {
    pub(crate) kind: CastOpcode,
    pub(crate) src: crate::value::ValueId,
}
