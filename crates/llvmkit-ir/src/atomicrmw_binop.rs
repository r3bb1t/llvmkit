//! `atomicrmw` operation selector. Mirrors
//! `llvm/include/llvm/IR/Instructions.h::AtomicRMWInst::BinOp`.

use core::fmt;

/// `atomicrmw` operation enum. Mirrors `AtomicRMWInst::BinOp` in
/// `Instructions.h`. The keyword spellings come from
/// `AtomicRMWInst::getOperationName` in `lib/IR/Instructions.cpp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtomicRMWBinOp {
    /// `*p = v`
    Xchg,
    /// `*p = old + v`
    Add,
    /// `*p = old - v`
    Sub,
    /// `*p = old & v`
    And,
    /// `*p = ~(old & v)`
    Nand,
    /// `*p = old | v`
    Or,
    /// `*p = old ^ v`
    Xor,
    /// Signed `*p = max(old, v)`
    Max,
    /// Signed `*p = min(old, v)`
    Min,
    /// Unsigned `*p = max(old, v)`
    UMax,
    /// Unsigned `*p = min(old, v)`
    UMin,
    /// `*p = old + v` (FP)
    FAdd,
    /// `*p = old - v` (FP)
    FSub,
    /// `*p = maxnum(old, v)` (FP)
    FMax,
    /// `*p = minnum(old, v)` (FP)
    FMin,
    /// `*p = maximum(old, v)` (FP, IEEE-754 semantics)
    FMaximum,
    /// `*p = minimum(old, v)` (FP, IEEE-754 semantics)
    FMinimum,
    /// `*p = (old u>= v) ? 0 : (old + 1)` (unsigned increment with wrap)
    UIncWrap,
    /// `*p = ((old == 0) || (old u> v)) ? v : (old - 1)` (unsigned decrement with wrap)
    UDecWrap,
    /// `*p = (old u>= v) ? old - v : old` (saturating-conditional unsigned subtract)
    USubCond,
    /// `*p = usub.sat(old, v)` (saturating unsigned subtract)
    USubSat,
}

impl AtomicRMWBinOp {
    /// IR keyword. Mirrors
    /// `AtomicRMWInst::getOperationName` in `lib/IR/Instructions.cpp`.
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Xchg => "xchg",
            Self::Add => "add",
            Self::Sub => "sub",
            Self::And => "and",
            Self::Nand => "nand",
            Self::Or => "or",
            Self::Xor => "xor",
            Self::Max => "max",
            Self::Min => "min",
            Self::UMax => "umax",
            Self::UMin => "umin",
            Self::FAdd => "fadd",
            Self::FSub => "fsub",
            Self::FMax => "fmax",
            Self::FMin => "fmin",
            Self::FMaximum => "fmaximum",
            Self::FMinimum => "fminimum",
            Self::UIncWrap => "uinc_wrap",
            Self::UDecWrap => "udec_wrap",
            Self::USubCond => "usub_cond",
            Self::USubSat => "usub_sat",
        }
    }

    /// `true` if the op operates on floating-point values. Mirrors
    /// `AtomicRMWInst::isFPOperation`.
    pub const fn is_fp_operation(self) -> bool {
        matches!(
            self,
            Self::FAdd | Self::FSub | Self::FMax | Self::FMin | Self::FMaximum | Self::FMinimum
        )
    }
}

impl fmt::Display for AtomicRMWBinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}
