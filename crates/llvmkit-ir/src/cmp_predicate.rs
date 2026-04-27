//! Compare predicates. Mirrors `llvm/include/llvm/IR/CmpPredicate.h` and the
//! `Predicate` enum in `llvm/include/llvm/IR/InstrTypes.h` (`InstrTypes.h`:
//! 670-710).
//!
//! Per the IR foundation plan (Pivot 4), `IntPredicate` and `FloatPredicate`
//! are distinct Rust types so passing `FloatPredicate::OEQ` to a method that
//! expects an integer predicate is a compile error. The raw discriminants
//! match the upstream `CmpInst::Predicate` enum so a downstream parser /
//! AsmWriter port can round-trip via `as u8` / `from_raw`.

use core::fmt;

/// Floating-point comparison predicate.
///
/// Discriminants (0..15) match LLVM's `FCMP_*` exactly; bit pattern is
/// `U L G E` with one bit per ordered/less/greater/equal slot
/// (`InstrTypes.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FloatPredicate {
    /// `FCMP_FALSE`: always false.
    False = 0,
    /// `FCMP_OEQ`: ordered and equal.
    Oeq = 1,
    /// `FCMP_OGT`: ordered and greater than.
    Ogt = 2,
    /// `FCMP_OGE`: ordered and greater than or equal.
    Oge = 3,
    /// `FCMP_OLT`: ordered and less than.
    Olt = 4,
    /// `FCMP_OLE`: ordered and less than or equal.
    Ole = 5,
    /// `FCMP_ONE`: ordered and unequal.
    One = 6,
    /// `FCMP_ORD`: ordered (no NaNs).
    Ord = 7,
    /// `FCMP_UNO`: unordered (`isnan(x) | isnan(y)`).
    Uno = 8,
    /// `FCMP_UEQ`: unordered or equal.
    Ueq = 9,
    /// `FCMP_UGT`: unordered or greater than.
    Ugt = 10,
    /// `FCMP_UGE`: unordered, greater than or equal.
    Uge = 11,
    /// `FCMP_ULT`: unordered or less than.
    Ult = 12,
    /// `FCMP_ULE`: unordered, less than or equal.
    Ule = 13,
    /// `FCMP_UNE`: unordered or not equal.
    Une = 14,
    /// `FCMP_TRUE`: always true.
    True = 15,
}

impl FloatPredicate {
    /// Smallest valid raw value (`FIRST_FCMP_PREDICATE`).
    pub const MIN_RAW: u8 = 0;
    /// Largest valid raw value (`LAST_FCMP_PREDICATE`).
    pub const MAX_RAW: u8 = 15;

    /// Construct from the raw `FCMP_*` discriminant. Returns `None` if
    /// the value is outside `0..=15`.
    #[inline]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::False,
            1 => Self::Oeq,
            2 => Self::Ogt,
            3 => Self::Oge,
            4 => Self::Olt,
            5 => Self::Ole,
            6 => Self::One,
            7 => Self::Ord,
            8 => Self::Uno,
            9 => Self::Ueq,
            10 => Self::Ugt,
            11 => Self::Uge,
            12 => Self::Ult,
            13 => Self::Ule,
            14 => Self::Une,
            15 => Self::True,
            _ => return None,
        })
    }

    /// Raw `FCMP_*` discriminant.
    #[inline]
    pub const fn as_raw(self) -> u8 {
        match self {
            Self::False => 0,
            Self::Oeq => 1,
            Self::Ogt => 2,
            Self::Oge => 3,
            Self::Olt => 4,
            Self::Ole => 5,
            Self::One => 6,
            Self::Ord => 7,
            Self::Uno => 8,
            Self::Ueq => 9,
            Self::Ugt => 10,
            Self::Uge => 11,
            Self::Ult => 12,
            Self::Ule => 13,
            Self::Une => 14,
            Self::True => 15,
        }
    }

    /// Mnemonic suffix as it appears in `.ll` syntax (`oeq`, `ord`, …).
    /// Mirrors `CmpInst::getPredicateName` (`Instructions.cpp`).
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::False => "false",
            Self::Oeq => "oeq",
            Self::Ogt => "ogt",
            Self::Oge => "oge",
            Self::Olt => "olt",
            Self::Ole => "ole",
            Self::One => "one",
            Self::Ord => "ord",
            Self::Uno => "uno",
            Self::Ueq => "ueq",
            Self::Ugt => "ugt",
            Self::Uge => "uge",
            Self::Ult => "ult",
            Self::Ule => "ule",
            Self::Une => "une",
            Self::True => "true",
        }
    }

    /// Inverse predicate (`x != y` becomes the negation). Mirrors
    /// the FCMP arm of `CmpInst::getInversePredicate`
    /// (`Instructions.cpp`); the LLVM source spells it as `XOR 0b1111`,
    /// we spell it as a direct mapping match (no `as` cast required).
    #[inline]
    pub const fn inverse(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::Oeq => Self::Une,
            Self::Ogt => Self::Ule,
            Self::Oge => Self::Ult,
            Self::Olt => Self::Uge,
            Self::Ole => Self::Ugt,
            Self::One => Self::Ueq,
            Self::Ord => Self::Uno,
            Self::Uno => Self::Ord,
            Self::Ueq => Self::One,
            Self::Ugt => Self::Ole,
            Self::Uge => Self::Olt,
            Self::Ult => Self::Oge,
            Self::Ule => Self::Ogt,
            Self::Une => Self::Oeq,
            Self::True => Self::False,
        }
    }

    /// Predicate yielded by swapping the comparison operands.
    /// Mirrors `CmpInst::getSwappedPredicate` (`Instructions.cpp`).
    #[inline]
    pub const fn swapped(self) -> Self {
        match self {
            Self::False
            | Self::True
            | Self::Oeq
            | Self::One
            | Self::Ueq
            | Self::Une
            | Self::Ord
            | Self::Uno => self,
            Self::Ogt => Self::Olt,
            Self::Olt => Self::Ogt,
            Self::Oge => Self::Ole,
            Self::Ole => Self::Oge,
            Self::Ugt => Self::Ult,
            Self::Ult => Self::Ugt,
            Self::Uge => Self::Ule,
            Self::Ule => Self::Uge,
        }
    }

    /// Iterate over every variant in canonical (`as_raw`) order.
    pub fn all() -> impl Iterator<Item = Self> {
        (Self::MIN_RAW..=Self::MAX_RAW).map(|r| Self::from_raw(r).expect("contiguous"))
    }
}

impl fmt::Display for FloatPredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Integer / pointer comparison predicate.
///
/// Discriminants match LLVM's `ICMP_*` (range `32..=41`,
/// `InstrTypes.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum IntPredicate {
    /// `ICMP_EQ`: equal.
    Eq = 32,
    /// `ICMP_NE`: not equal.
    Ne = 33,
    /// `ICMP_UGT`: unsigned greater than.
    Ugt = 34,
    /// `ICMP_UGE`: unsigned greater or equal.
    Uge = 35,
    /// `ICMP_ULT`: unsigned less than.
    Ult = 36,
    /// `ICMP_ULE`: unsigned less or equal.
    Ule = 37,
    /// `ICMP_SGT`: signed greater than.
    Sgt = 38,
    /// `ICMP_SGE`: signed greater or equal.
    Sge = 39,
    /// `ICMP_SLT`: signed less than.
    Slt = 40,
    /// `ICMP_SLE`: signed less or equal.
    Sle = 41,
}

impl IntPredicate {
    /// Smallest valid raw value (`FIRST_ICMP_PREDICATE`).
    pub const MIN_RAW: u8 = 32;
    /// Largest valid raw value (`LAST_ICMP_PREDICATE`).
    pub const MAX_RAW: u8 = 41;

    /// Construct from the raw `ICMP_*` discriminant. Returns `None` if
    /// the value is outside `32..=41`.
    #[inline]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            32 => Self::Eq,
            33 => Self::Ne,
            34 => Self::Ugt,
            35 => Self::Uge,
            36 => Self::Ult,
            37 => Self::Ule,
            38 => Self::Sgt,
            39 => Self::Sge,
            40 => Self::Slt,
            41 => Self::Sle,
            _ => return None,
        })
    }

    /// Raw `ICMP_*` discriminant.
    #[inline]
    pub const fn as_raw(self) -> u8 {
        match self {
            Self::Eq => 32,
            Self::Ne => 33,
            Self::Ugt => 34,
            Self::Uge => 35,
            Self::Ult => 36,
            Self::Ule => 37,
            Self::Sgt => 38,
            Self::Sge => 39,
            Self::Slt => 40,
            Self::Sle => 41,
        }
    }

    /// Mnemonic suffix as it appears in `.ll` syntax (`eq`, `slt`, …).
    /// Mirrors `CmpInst::getPredicateName`.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Ugt => "ugt",
            Self::Uge => "uge",
            Self::Ult => "ult",
            Self::Ule => "ule",
            Self::Sgt => "sgt",
            Self::Sge => "sge",
            Self::Slt => "slt",
            Self::Sle => "sle",
        }
    }

    /// `true` iff this predicate is signed (`s*`).
    #[inline]
    pub const fn is_signed(self) -> bool {
        matches!(self, Self::Sgt | Self::Sge | Self::Slt | Self::Sle)
    }

    /// `true` iff this predicate is unsigned (`u*`); `eq`/`ne` aren't
    /// signed *or* unsigned, mirroring `CmpInst::isUnsigned`.
    #[inline]
    pub const fn is_unsigned(self) -> bool {
        matches!(self, Self::Ugt | Self::Uge | Self::Ult | Self::Ule)
    }

    /// Inverse predicate. Mirrors the ICMP arm of
    /// `CmpInst::getInversePredicate` (`Instructions.cpp`).
    #[inline]
    pub const fn inverse(self) -> Self {
        match self {
            Self::Eq => Self::Ne,
            Self::Ne => Self::Eq,
            Self::Ugt => Self::Ule,
            Self::Ult => Self::Uge,
            Self::Uge => Self::Ult,
            Self::Ule => Self::Ugt,
            Self::Sgt => Self::Sle,
            Self::Slt => Self::Sge,
            Self::Sge => Self::Slt,
            Self::Sle => Self::Sgt,
        }
    }

    /// Predicate yielded by swapping the comparison operands.
    /// Mirrors `CmpInst::getSwappedPredicate` (`Instructions.cpp`).
    #[inline]
    pub const fn swapped(self) -> Self {
        match self {
            Self::Eq | Self::Ne => self,
            Self::Sgt => Self::Slt,
            Self::Slt => Self::Sgt,
            Self::Sge => Self::Sle,
            Self::Sle => Self::Sge,
            Self::Ugt => Self::Ult,
            Self::Ult => Self::Ugt,
            Self::Uge => Self::Ule,
            Self::Ule => Self::Uge,
        }
    }

    /// If signed, return the unsigned counterpart (and vice versa).
    /// `eq`/`ne` are returned unchanged. Mirrors the
    /// `getSignedPredicate` / `getUnsignedPredicate` pair on `ICmpInst`.
    #[inline]
    pub const fn flip_signedness(self) -> Self {
        match self {
            Self::Eq | Self::Ne => self,
            Self::Sgt => Self::Ugt,
            Self::Slt => Self::Ult,
            Self::Sge => Self::Uge,
            Self::Sle => Self::Ule,
            Self::Ugt => Self::Sgt,
            Self::Ult => Self::Slt,
            Self::Uge => Self::Sge,
            Self::Ule => Self::Sle,
        }
    }

    /// Iterate over every variant in canonical (`as_raw`) order.
    pub fn all() -> impl Iterator<Item = Self> {
        (Self::MIN_RAW..=Self::MAX_RAW).map(|r| Self::from_raw(r).expect("contiguous"))
    }
}

impl fmt::Display for IntPredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Upstream provenance: mirrors `CmpInst::Predicate` /
/// `ICmpInst::Predicate` / `FCmpInst::Predicate` from
/// `include/llvm/IR/InstrTypes.h` and `lib/IR/Instructions.cpp`,
/// exercised at runtime by `unittests/IR/InstructionsTest.cpp`.
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: enum round-trip. Mirrors `FCmpInst::Predicate`
    /// numeric stability in `include/llvm/IR/InstrTypes.h`.
    #[test]
    fn float_round_trip() {
        for p in FloatPredicate::all() {
            assert_eq!(FloatPredicate::from_raw(p.as_raw()), Some(p));
        }
    }

    /// llvmkit-specific: enum round-trip. Mirrors `ICmpInst::Predicate`
    /// numeric stability in `include/llvm/IR/InstrTypes.h`.
    #[test]
    fn int_round_trip() {
        for p in IntPredicate::all() {
            assert_eq!(IntPredicate::from_raw(p.as_raw()), Some(p));
        }
    }

    /// Mirrors `CmpInst::getInversePredicate` (XOR-with-15 trick) for FCmp
    /// in `lib/IR/Instructions.cpp`.
    #[test]
    fn float_inverse_is_xor_15() {
        for p in FloatPredicate::all() {
            assert_eq!(p.inverse().as_raw(), p.as_raw() ^ 0b1111);
            assert_eq!(p.inverse().inverse(), p);
        }
    }

    /// Mirrors `CmpInst::getInversePredicate` involution for ICmp in
    /// `lib/IR/Instructions.cpp`.
    #[test]
    fn int_inverse_involutive() {
        for p in IntPredicate::all() {
            assert_eq!(p.inverse().inverse(), p);
        }
    }

    /// Mirrors `CmpInst::getSwappedPredicate` involution for ICmp in
    /// `lib/IR/Instructions.cpp`.
    #[test]
    fn int_swapped_involutive() {
        for p in IntPredicate::all() {
            assert_eq!(p.swapped().swapped(), p);
        }
    }

    /// Mirrors `CmpInst::getSwappedPredicate` involution for FCmp in
    /// `lib/IR/Instructions.cpp`.
    #[test]
    fn float_swapped_involutive() {
        for p in FloatPredicate::all() {
            assert_eq!(p.swapped().swapped(), p);
        }
    }

    /// Mirrors `CmpInst::isSigned` / `isUnsigned` partition for ICmp in
    /// `lib/IR/Instructions.cpp`.
    #[test]
    fn int_signedness_partition() {
        // eq / ne are neither signed nor unsigned; the rest are exactly one.
        for p in IntPredicate::all() {
            let s = p.is_signed();
            let u = p.is_unsigned();
            match p {
                IntPredicate::Eq | IntPredicate::Ne => assert!(!s && !u),
                _ => assert!(s ^ u),
            }
        }
    }

    /// Mirrors `CmpInst::getPredicateName` in `lib/IR/Instructions.cpp`;
    /// rendered shape matches `test/Assembler/*.ll` icmp/fcmp fixtures.
    #[test]
    fn display_matches_llvm() {
        // Spot-check a handful — the exhaustive list lives in the source.
        assert_eq!(format!("{}", FloatPredicate::Oeq), "oeq");
        assert_eq!(format!("{}", FloatPredicate::True), "true");
        assert_eq!(format!("{}", IntPredicate::Eq), "eq");
        assert_eq!(format!("{}", IntPredicate::Slt), "slt");
    }

    /// llvmkit-specific: enum range guard. Closest upstream:
    /// `CmpInst::Predicate` enum in `include/llvm/IR/InstrTypes.h`.
    #[test]
    fn from_raw_rejects_out_of_range() {
        assert_eq!(FloatPredicate::from_raw(16), None);
        assert_eq!(IntPredicate::from_raw(31), None);
        assert_eq!(IntPredicate::from_raw(42), None);
    }
}
