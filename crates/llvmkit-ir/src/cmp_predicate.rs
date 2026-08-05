//! Compare predicates. Mirrors `llvm/include/llvm/IR/CmpPredicate.h` and the
//! `Predicate` enum in `llvm/include/llvm/IR/InstrTypes.h` (`InstrTypes.h`:
//! 670-710).
//!
//! `IntPredicate` and `FloatPredicate` are distinct Rust types, so passing
//! `FloatPredicate::OEQ` to a method that
//! expects an integer predicate is a compile error. The raw discriminants
//! match the upstream `CmpInst::Predicate` enum so a downstream parser /
//! AsmWriter port can round-trip via `as u8` / `from_raw`.
//!
//! ## Why signedness lives on the predicate, not on values
//!
//! LLVM IR values are sign-agnostic --- a 32-bit register's bit pattern
//! can be interpreted either way (`lib/IR/Constants.h` canonicalises
//! every `ConstantInt` to an unsigned `APInt` internally). The
//! signedness of an integer comparison therefore lives in the
//! *predicate*, not in the operands. [`IntPredicate`] separates the
//! signedness-irrelevant predicates (`Eq`, `Ne`) from the unsigned
//! family (`Ult`/`Ule`/`Ugt`/`Uge`) and the signed family
//! (`Slt`/`Sle`/`Sgt`/`Sge`). For ergonomics,
//! [`crate::IrBuilder`] also ships per-predicate convenience methods
//! (`icmp_eq`, `icmp_slt`, ...) that bake the predicate
//! into the method name --- see `IRBuilder::CreateICmp{EQ,SLT,...}` in
//! `IRBuilder.h` for the upstream parallel.

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpPredicate {
    Int(IntPredicate),
    Float(FloatPredicate),
}

impl From<IntPredicate> for CmpPredicate {
    fn from(value: IntPredicate) -> Self {
        Self::Int(value)
    }
}

impl From<FloatPredicate> for CmpPredicate {
    fn from(value: FloatPredicate) -> Self {
        Self::Float(value)
    }
}

/// A comparison predicate together with the `samesign` flag of the `icmp` it
/// came from.
///
/// Ports `llvm::CmpPredicate` (`CmpPredicate.h`), which is a `Predicate` plus
/// one `bool`. llvmkit's [`CmpPredicate`] is the same int-or-float union
/// *without* the flag; the two are separate types because `samesign` is
/// meaningless on an `fcmp` and every operation that reads it — [`Self::matching`],
/// [`Self::preferred_signed_predicate`], [`Self::drop_same_sign`] — is
/// integer-only.
///
/// A predicate with the flag set claims both operands carry the same sign, so
/// the signed and unsigned readings of the comparison agree. That is what lets
/// [`Self::matching`] pair a signed predicate with its unsigned twin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PredicateWithSameSign {
    predicate: CmpPredicate,
    same_sign: bool,
}

impl PredicateWithSameSign {
    /// An integer predicate with no `samesign` claim.
    #[inline]
    pub const fn int(predicate: IntPredicate) -> Self {
        Self {
            predicate: CmpPredicate::Int(predicate),
            same_sign: false,
        }
    }

    /// An integer predicate whose `icmp` carried `samesign`.
    #[inline]
    pub const fn int_same_sign(predicate: IntPredicate) -> Self {
        Self {
            predicate: CmpPredicate::Int(predicate),
            same_sign: true,
        }
    }

    /// A floating-point predicate. `samesign` never applies.
    #[inline]
    pub const fn float(predicate: FloatPredicate) -> Self {
        Self {
            predicate: CmpPredicate::Float(predicate),
            same_sign: false,
        }
    }

    /// The predicate, without the flag.
    #[inline]
    pub const fn predicate(self) -> CmpPredicate {
        self.predicate
    }

    /// The integer predicate, or `None` for a floating-point one.
    #[inline]
    pub const fn as_int(self) -> Option<IntPredicate> {
        match self.predicate {
            CmpPredicate::Int(predicate) => Some(predicate),
            CmpPredicate::Float(_) => None,
        }
    }

    /// The floating-point predicate, or `None` for an integer one.
    #[inline]
    pub const fn as_float(self) -> Option<FloatPredicate> {
        match self.predicate {
            CmpPredicate::Float(predicate) => Some(predicate),
            CmpPredicate::Int(_) => None,
        }
    }

    /// Whether the originating `icmp` carried `samesign`.
    ///
    /// Ports `CmpPredicate::hasSameSign`.
    #[inline]
    pub const fn has_same_sign(self) -> bool {
        self.same_sign
    }

    /// The bare predicate, discarding any `samesign` claim.
    ///
    /// Ports `CmpPredicate::dropSameSign`.
    #[inline]
    pub const fn drop_same_sign(self) -> CmpPredicate {
        self.predicate
    }

    /// Under `samesign`, the signed reading of the predicate; otherwise the
    /// predicate unchanged.
    ///
    /// Ports `CmpPredicate::getPreferredSignedPredicate`, whose body is
    /// `HasSameSign ? IcmpInst::getSignedPredicate(Pred) : Pred`.
    #[inline]
    pub const fn preferred_signed_predicate(self) -> CmpPredicate {
        match self.predicate {
            CmpPredicate::Int(predicate) if self.same_sign => {
                CmpPredicate::Int(predicate.signed_predicate())
            }
            other => other,
        }
    }

    /// The predicate both `a` and `b` can be read as, if there is one.
    ///
    /// Ports `CmpPredicate::getMatching`. Equal predicates match, keeping the
    /// flag only when both carry it; otherwise a `samesign` predicate matches
    /// its opposite-signedness twin, which is exactly what the flag licenses.
    /// Floating-point predicates only ever match themselves.
    #[inline]
    pub fn matching(a: Self, b: Self) -> Option<Self> {
        if a.predicate == b.predicate {
            return Some(if a.same_sign == b.same_sign {
                a
            } else {
                Self {
                    predicate: a.predicate,
                    same_sign: false,
                }
            });
        }
        let (CmpPredicate::Int(a_int), CmpPredicate::Int(b_int)) = (a.predicate, b.predicate)
        else {
            return None;
        };
        if a.same_sign && a_int == b_int.flip_signedness() {
            return Some(Self::int(b_int));
        }
        if b.same_sign && b_int == a_int.flip_signedness() {
            return Some(Self::int(a_int));
        }
        None
    }

    /// Whether `first` being true forces `second` to be true, false, or neither,
    /// for two comparisons over the *same* operands.
    ///
    /// Ports `IcmpInst::isImpliedByMatchingCmp` together with the two static
    /// helpers it delegates to, `isImpliedTrueByMatchingCmp` and
    /// `isImpliedFalseByMatchingCmp` (`Instructions.cpp`) — the latter is the
    /// former against the inverse of `second`.
    #[inline]
    pub fn implied_by_matching_comparison(first: Self, second: Self) -> Option<bool> {
        if Self::implied_true_by_matching(first, second) {
            return Some(true);
        }
        if Self::implied_true_by_matching(first, second.inverse()) {
            return Some(false);
        }
        None
    }

    /// The inverse predicate, keeping the `samesign` claim — inverting a
    /// comparison does not change what its operands' signs are.
    #[inline]
    pub const fn inverse(self) -> Self {
        let predicate = match self.predicate {
            CmpPredicate::Int(predicate) => CmpPredicate::Int(predicate.inverse()),
            CmpPredicate::Float(predicate) => CmpPredicate::Float(predicate.inverse()),
        };
        Self {
            predicate,
            same_sign: self.same_sign,
        }
    }

    /// The predicate yielded by swapping the comparison's operands, keeping the
    /// `samesign` claim.
    #[inline]
    pub const fn swapped(self) -> Self {
        let predicate = match self.predicate {
            CmpPredicate::Int(predicate) => CmpPredicate::Int(predicate.swapped()),
            CmpPredicate::Float(predicate) => CmpPredicate::Float(predicate.swapped()),
        };
        Self {
            predicate,
            same_sign: self.same_sign,
        }
    }

    /// Ports `isImpliedTrueByMatchingCmp`.
    fn implied_true_by_matching(mut first: Self, mut second: Self) -> bool {
        // Matching predicates: the first condition makes the second true.
        if Self::matching(first, second).is_some() {
            return true;
        }

        // Under `samesign`, read whichever side carries the flag in the other
        // side's signedness so the table below can compare like with like.
        if let (Some(first_int), Some(second_int)) = (first.as_int(), second.as_int()) {
            if first.same_sign && second_int.is_signed() {
                first = Self::int(first_int.flip_signedness());
            } else if second.same_sign && first_int.is_signed() {
                second = Self::int(second_int.flip_signedness());
            }
        }

        let (Some(first), Some(second)) = (first.as_int(), second.as_int()) else {
            return false;
        };
        match first {
            // A == B implies A >=u B, A <=u B, A >=s B and A <=s B.
            IntPredicate::Eq => matches!(
                second,
                IntPredicate::Uge | IntPredicate::Ule | IntPredicate::Sge | IntPredicate::Sle
            ),
            // A >u B implies A != B and A >=u B.
            IntPredicate::Ugt => matches!(second, IntPredicate::Ne | IntPredicate::Uge),
            // A <u B implies A != B and A <=u B.
            IntPredicate::Ult => matches!(second, IntPredicate::Ne | IntPredicate::Ule),
            // A >s B implies A != B and A >=s B.
            IntPredicate::Sgt => matches!(second, IntPredicate::Ne | IntPredicate::Sge),
            // A <s B implies A != B and A <=s B.
            IntPredicate::Slt => matches!(second, IntPredicate::Ne | IntPredicate::Sle),
            _ => false,
        }
    }
}

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
    pub const fn as_str(self) -> &'static str {
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

    /// Whether the predicate is ordered — false whenever either operand is
    /// NaN. Mirrors the FCMP arm of `CmpInst::isOrdered` (`Instructions.cpp`).
    #[inline]
    pub const fn is_ordered(self) -> bool {
        matches!(
            self,
            Self::Oeq | Self::One | Self::Ogt | Self::Olt | Self::Oge | Self::Ole | Self::Ord
        )
    }

    /// Whether the predicate is unordered — true whenever either operand is
    /// NaN. Mirrors `CmpInst::isUnordered`.
    #[inline]
    pub const fn is_unordered(self) -> bool {
        matches!(
            self,
            Self::Ueq | Self::Une | Self::Ugt | Self::Ult | Self::Uge | Self::Ule | Self::Uno
        )
    }

    /// Whether the predicate tests equality in either direction. Mirrors the
    /// FCMP arm of `CmpInst::isEquality`.
    #[inline]
    pub const fn is_equality(self) -> bool {
        matches!(self, Self::Oeq | Self::One | Self::Ueq | Self::Une)
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
        f.write_str(self.as_str())
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
    pub const fn as_str(self) -> &'static str {
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

    /// `true` iff this predicate tests equality. Mirrors the ICMP arm of
    /// `CmpInst::isEquality`.
    #[inline]
    pub const fn is_equality(self) -> bool {
        matches!(self, Self::Eq | Self::Ne)
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

    /// The signed reading of this predicate; `eq`/`ne` and the already-signed
    /// predicates are returned unchanged. Mirrors `IcmpInst::getSignedPredicate`.
    #[inline]
    pub const fn signed_predicate(self) -> Self {
        match self {
            Self::Ugt => Self::Sgt,
            Self::Ult => Self::Slt,
            Self::Uge => Self::Sge,
            Self::Ule => Self::Sle,
            other => other,
        }
    }

    /// If signed, return the unsigned counterpart (and vice versa).
    /// `eq`/`ne` are returned unchanged. Mirrors the
    /// `getSignedPredicate` / `getUnsignedPredicate` pair on `IcmpInst`.
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
        f.write_str(self.as_str())
    }
}

/// Upstream provenance: mirrors `CmpInst::Predicate` /
/// `IcmpInst::Predicate` / `FcmpInst::Predicate` from
/// `include/llvm/IR/InstrTypes.h` and `lib/IR/Instructions.cpp`,
/// exercised at runtime by `unittests/IR/InstructionsTest.cpp`.
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: enum round-trip. Mirrors `FcmpInst::Predicate`
    /// numeric stability in `include/llvm/IR/InstrTypes.h`.
    #[test]
    fn float_round_trip() {
        for p in FloatPredicate::all() {
            assert_eq!(FloatPredicate::from_raw(p.as_raw()), Some(p));
        }
    }

    /// llvmkit-specific: enum round-trip. Mirrors `IcmpInst::Predicate`
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
