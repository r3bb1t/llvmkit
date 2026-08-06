//! `atomicrmw` operation selector. Mirrors
//! `llvm/include/llvm/IR/Instructions.h::AtomicRMWInst::BinOp`.

use core::fmt;

/// `atomicrmw` operation enum. Mirrors `AtomicRMWInst::BinOp` in
/// `Instructions.h`. The keyword spellings come from
/// `AtomicRMWInst::getOperationName` in `lib/IR/Instructions.cpp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtomicRmwBinOp {
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
    Umax,
    /// Unsigned `*p = min(old, v)`
    Umin,
    /// `*p = old + v` (FP)
    Fadd,
    /// `*p = old - v` (FP)
    Fsub,
    /// `*p = maxnum(old, v)` (FP)
    Fmax,
    /// `*p = minnum(old, v)` (FP)
    Fmin,
    /// `*p = maximum(old, v)` (FP, IEEE-754 semantics)
    Fmaximum,
    /// `*p = minimum(old, v)` (FP, IEEE-754 semantics)
    Fminimum,
    /// `*p = (old u>= v) ? 0 : (old + 1)` (unsigned increment with wrap)
    UincWrap,
    /// `*p = ((old == 0) || (old u> v)) ? v : (old - 1)` (unsigned decrement with wrap)
    UdecWrap,
    /// `*p = (old u>= v) ? old - v : old` (saturating-conditional unsigned subtract)
    UsubCond,
    /// `*p = usub.sat(old, v)` (saturating unsigned subtract)
    UsubSat,
}

impl AtomicRmwBinOp {
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
            Self::Umax => "umax",
            Self::Umin => "umin",
            Self::Fadd => "fadd",
            Self::Fsub => "fsub",
            Self::Fmax => "fmax",
            Self::Fmin => "fmin",
            Self::Fmaximum => "fmaximum",
            Self::Fminimum => "fminimum",
            Self::UincWrap => "uinc_wrap",
            Self::UdecWrap => "udec_wrap",
            Self::UsubCond => "usub_cond",
            Self::UsubSat => "usub_sat",
        }
    }

    /// `true` if the op operates on floating-point values. Mirrors
    /// `AtomicRMWInst::isFPOperation`.
    pub const fn is_fp_operation(self) -> bool {
        matches!(
            self,
            Self::Fadd | Self::Fsub | Self::Fmax | Self::Fmin | Self::Fmaximum | Self::Fminimum
        )
    }
}

impl fmt::Display for AtomicRmwBinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}
