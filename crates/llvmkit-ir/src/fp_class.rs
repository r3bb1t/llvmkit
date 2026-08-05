//! The floating-point classification lattice.
//!
//! [`FpClassTest`] ports `llvm::FPClassTest` (`llvm/ADT/FloatingPointMode.h`)
//! — the ten-bit mask naming every class an IEEE value can fall into, split by
//! sign. [`KnownFpClass`] ports `llvm::KnownFPClass`
//! (`llvm/Support/KnownFPClass.h`), which pairs that mask with a separately
//! tracked sign bit and is to `computeKnownFPClass` what
//! [`KnownBits`](crate::KnownBits) is to `computeKnownBits`.
//!
//! The sign bit is tracked *beside* the mask rather than derived from it
//! because the two disagree on NaN: `fcNan` covers both signs, so a value known
//! to be a negative NaN has a mask that admits positive classes while its sign
//! bit is definitely set. Upstream's `knownNot` is the one place the two are
//! reconciled, and only in the direction the mask can justify.
//!
//! The module is complete against both upstream files: every predicate the
//! header declares inline, and every operation `llvm/lib/Support/KnownFPClass.cpp`
//! defines out of line — `fmul`, `square`, `sqrt`, `log`, `exp`, `fpext`,
//! `roundToIntegral`, `canonicalize`, `minMaxLike`, `propagateDenormal` and
//! `propagateCanonicalizingSrc`.

use crate::ap_float::{ApFloat, ApFloatSemantics};
use crate::denormal_mode::{DenormalMode, DenormalModeKind};
use core::fmt;

/// The floating-point classes a value may belong to, as a bitmask.
///
/// Ports `llvm::FPClassTest`. Each of the ten primitive classes gets one bit;
/// the named unions below are exactly upstream's, in upstream's order.
///
/// A bare `u32` newtype rather than a Rust `enum`: upstream's is an `enum`
/// only because C++ has no other way to spell a named bitmask, and every
/// operation on it is a bitwise combination that no enum variant list can
/// close over. `LLVM_DECLARE_ENUM_AS_BITMASK` is upstream saying the same
/// thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FpClassTest(u32);

impl FpClassTest {
    /// No class at all — the empty mask. Ports `fcNone`.
    pub const NONE: Self = Self(0);

    /// Signaling NaN. Ports `fcSNan`.
    pub const SIGNALING_NAN: Self = Self(0x0001);
    /// Quiet NaN. Ports `fcQNan`.
    pub const QUIET_NAN: Self = Self(0x0002);
    /// Negative infinity. Ports `fcNegInf`.
    pub const NEGATIVE_INFINITY: Self = Self(0x0004);
    /// Negative normal. Ports `fcNegNormal`.
    pub const NEGATIVE_NORMAL: Self = Self(0x0008);
    /// Negative subnormal. Ports `fcNegSubnormal`.
    pub const NEGATIVE_SUBNORMAL: Self = Self(0x0010);
    /// Negative zero. Ports `fcNegZero`.
    pub const NEGATIVE_ZERO: Self = Self(0x0020);
    /// Positive zero. Ports `fcPosZero`.
    pub const POSITIVE_ZERO: Self = Self(0x0040);
    /// Positive subnormal. Ports `fcPosSubnormal`.
    pub const POSITIVE_SUBNORMAL: Self = Self(0x0080);
    /// Positive normal. Ports `fcPosNormal`.
    pub const POSITIVE_NORMAL: Self = Self(0x0100);
    /// Positive infinity. Ports `fcPosInf`.
    pub const POSITIVE_INFINITY: Self = Self(0x0200);

    /// Either NaN. Ports `fcNan`.
    pub const NAN: Self = Self(Self::SIGNALING_NAN.0 | Self::QUIET_NAN.0);
    /// Either infinity. Ports `fcInf`.
    pub const INFINITY: Self = Self(Self::POSITIVE_INFINITY.0 | Self::NEGATIVE_INFINITY.0);
    /// Either normal. Ports `fcNormal`.
    pub const NORMAL: Self = Self(Self::POSITIVE_NORMAL.0 | Self::NEGATIVE_NORMAL.0);
    /// Either subnormal. Ports `fcSubnormal`.
    pub const SUBNORMAL: Self = Self(Self::POSITIVE_SUBNORMAL.0 | Self::NEGATIVE_SUBNORMAL.0);
    /// Either zero. Ports `fcZero`.
    pub const ZERO: Self = Self(Self::POSITIVE_ZERO.0 | Self::NEGATIVE_ZERO.0);
    /// Any finite positive value, zero included. Ports `fcPosFinite`.
    pub const POSITIVE_FINITE: Self =
        Self(Self::POSITIVE_NORMAL.0 | Self::POSITIVE_SUBNORMAL.0 | Self::POSITIVE_ZERO.0);
    /// Any finite negative value, zero included. Ports `fcNegFinite`.
    pub const NEGATIVE_FINITE: Self =
        Self(Self::NEGATIVE_NORMAL.0 | Self::NEGATIVE_SUBNORMAL.0 | Self::NEGATIVE_ZERO.0);
    /// Any finite value. Ports `fcFinite`.
    pub const FINITE: Self = Self(Self::POSITIVE_FINITE.0 | Self::NEGATIVE_FINITE.0);
    /// Any value whose sign bit is clear, NaN excluded. Ports `fcPositive`.
    pub const POSITIVE: Self = Self(Self::POSITIVE_FINITE.0 | Self::POSITIVE_INFINITY.0);
    /// Any value whose sign bit is set, NaN excluded. Ports `fcNegative`.
    pub const NEGATIVE: Self = Self(Self::NEGATIVE_FINITE.0 | Self::NEGATIVE_INFINITY.0);

    /// Every class. Ports `fcAllFlags`.
    pub const ALL: Self = Self(Self::NAN.0 | Self::INFINITY.0 | Self::FINITE.0);

    /// The raw bits. Exposed because `@llvm.is.fpclass` takes this mask as an
    /// immediate operand, so a caller printing or parsing one needs the number.
    #[inline]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// A mask from raw bits, or `None` when a bit outside [`Self::ALL`] is set.
    ///
    /// Fallible where [`Self::bits`] is not: a mask read out of IR is
    /// caller-supplied, and `@llvm.is.fpclass`'s verifier rejects out-of-range
    /// values rather than truncating them.
    #[inline]
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits & !Self::ALL.0 == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Whether this mask names no class at all.
    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }

    /// Whether this mask names every class.
    #[inline]
    pub const fn is_all(self) -> bool {
        self.0 == Self::ALL.0
    }

    /// The classes in both masks. Ports `operator&`.
    #[inline]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The classes in either mask. Ports `operator|`.
    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The classes in this mask but not the other.
    #[inline]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Every class this mask does not name. Ports `operator~`, which
    /// `LLVM_DECLARE_ENUM_AS_BITMASK` bounds to the declared bits — so this
    /// complements within [`Self::ALL`], not within `u32`.
    #[inline]
    pub const fn complement(self) -> Self {
        Self(!self.0 & Self::ALL.0)
    }

    /// Whether every class in `other` is also in this mask.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the two masks share any class.
    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// The mask that holds after negating the value.
    ///
    /// Ports `llvm::fneg` (`FloatingPointMode.cpp`): each signed class swaps
    /// for its opposite, and NaN — which `fneg` flips the sign bit of without
    /// changing the class — passes through.
    #[inline]
    pub const fn negated(self) -> Self {
        let mut bits = self.0 & Self::NAN.0;
        bits |= swap_if_set(self.0, Self::NEGATIVE_INFINITY.0, Self::POSITIVE_INFINITY.0);
        bits |= swap_if_set(self.0, Self::NEGATIVE_NORMAL.0, Self::POSITIVE_NORMAL.0);
        bits |= swap_if_set(
            self.0,
            Self::NEGATIVE_SUBNORMAL.0,
            Self::POSITIVE_SUBNORMAL.0,
        );
        bits |= swap_if_set(self.0, Self::NEGATIVE_ZERO.0, Self::POSITIVE_ZERO.0);
        bits |= swap_if_set(self.0, Self::POSITIVE_ZERO.0, Self::NEGATIVE_ZERO.0);
        bits |= swap_if_set(
            self.0,
            Self::POSITIVE_SUBNORMAL.0,
            Self::NEGATIVE_SUBNORMAL.0,
        );
        bits |= swap_if_set(self.0, Self::POSITIVE_NORMAL.0, Self::NEGATIVE_NORMAL.0);
        bits |= swap_if_set(self.0, Self::POSITIVE_INFINITY.0, Self::NEGATIVE_INFINITY.0);
        Self(bits)
    }

    /// The mask an input must satisfy for `fabs` of it to satisfy this one.
    ///
    /// Ports `llvm::inverse_fabs`. `fabs` maps both signs onto the positive
    /// class, so each positive class in this mask admits either sign of its
    /// input; a negative class admits nothing, because `fabs` never produces
    /// one.
    #[inline]
    pub const fn inverse_absolute(self) -> Self {
        let mut bits = self.0 & Self::NAN.0;
        bits |= widen_if_set(self.0, Self::POSITIVE_ZERO.0, Self::ZERO.0);
        bits |= widen_if_set(self.0, Self::POSITIVE_SUBNORMAL.0, Self::SUBNORMAL.0);
        bits |= widen_if_set(self.0, Self::POSITIVE_NORMAL.0, Self::NORMAL.0);
        bits |= widen_if_set(self.0, Self::POSITIVE_INFINITY.0, Self::INFINITY.0);
        Self(bits)
    }

    /// This mask with every class widened to both signs.
    ///
    /// Ports `llvm::unknown_sign`: the same magnitudes, sign forgotten.
    #[inline]
    pub const fn with_unknown_sign(self) -> Self {
        let mut bits = self.0 & Self::NAN.0;
        bits |= widen_if_set(self.0, Self::ZERO.0, Self::ZERO.0);
        bits |= widen_if_set(self.0, Self::SUBNORMAL.0, Self::SUBNORMAL.0);
        bits |= widen_if_set(self.0, Self::NORMAL.0, Self::NORMAL.0);
        bits |= widen_if_set(self.0, Self::INFINITY.0, Self::INFINITY.0);
        Self(bits)
    }

    /// The single class `value` belongs to.
    ///
    /// Ports `APFloat::classify` (`APFloat.cpp`). Its closing
    /// `assert(isNaN() && "Other class of FP constant")` is the fallthrough
    /// here: the five tests are exhaustive over IEEE categories, so the last
    /// arm needs no test of its own.
    pub fn of(value: &ApFloat) -> Self {
        let negative = value.is_negative();
        if value.is_zero() {
            return if negative {
                Self::NEGATIVE_ZERO
            } else {
                Self::POSITIVE_ZERO
            };
        }
        if value.is_denormal() {
            return if negative {
                Self::NEGATIVE_SUBNORMAL
            } else {
                Self::POSITIVE_SUBNORMAL
            };
        }
        if value.is_infinity() {
            return if negative {
                Self::NEGATIVE_INFINITY
            } else {
                Self::POSITIVE_INFINITY
            };
        }
        if value.is_nan() {
            return if value.is_signaling() {
                Self::SIGNALING_NAN
            } else {
                Self::QUIET_NAN
            };
        }
        // Finite, non-zero and not subnormal: normal.
        if negative {
            Self::NEGATIVE_NORMAL
        } else {
            Self::POSITIVE_NORMAL
        }
    }
}

/// `to` when `bit` is set in `bits`, otherwise nothing. The shape every arm of
/// [`FpClassTest::negated`] and its two siblings has.
#[inline]
const fn swap_if_set(bits: u32, bit: u32, to: u32) -> u32 {
    if bits & bit != 0 { to } else { 0 }
}

/// [`swap_if_set`] under a different name, for the arms that *widen* a class to
/// both signs rather than swapping it to the other one.
#[inline]
const fn widen_if_set(bits: u32, bit: u32, to: u32) -> u32 {
    swap_if_set(bits, bit, to)
}

/// The names upstream prints a mask with, in the order it consumes them.
///
/// Ports `NoFPClassName` (`FloatingPointMode.cpp`), whose own comment records
/// the rule: "Names should be listed in order of preference, with higher
/// popcounts listed first", and "Bits are consumed as printed."
const CLASS_NAMES: &[(FpClassTest, &str)] = &[
    (FpClassTest::ALL, "all"),
    (FpClassTest::NAN, "nan"),
    (FpClassTest::SIGNALING_NAN, "snan"),
    (FpClassTest::QUIET_NAN, "qnan"),
    (FpClassTest::INFINITY, "inf"),
    (FpClassTest::NEGATIVE_INFINITY, "ninf"),
    (FpClassTest::POSITIVE_INFINITY, "pinf"),
    (FpClassTest::ZERO, "zero"),
    (FpClassTest::NEGATIVE_ZERO, "nzero"),
    (FpClassTest::POSITIVE_ZERO, "pzero"),
    (FpClassTest::SUBNORMAL, "sub"),
    (FpClassTest::NEGATIVE_SUBNORMAL, "nsub"),
    (FpClassTest::POSITIVE_SUBNORMAL, "psub"),
    (FpClassTest::NORMAL, "norm"),
    (FpClassTest::NEGATIVE_NORMAL, "nnorm"),
    (FpClassTest::POSITIVE_NORMAL, "pnorm"),
];

impl fmt::Display for FpClassTest {
    /// Ports `operator<<(raw_ostream &, FPClassTest)`: greedily consume the
    /// widest name that fits, space-separated, and print `none` for the empty
    /// mask.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            return f.write_str("none");
        }
        let mut remaining = *self;
        let mut first = true;
        for (mask, name) in CLASS_NAMES {
            if !remaining.contains(*mask) {
                continue;
            }
            if !first {
                f.write_str(" ")?;
            }
            f.write_str(name)?;
            first = false;
            remaining = remaining.difference(*mask);
            if remaining.is_none() {
                break;
            }
        }
        Ok(())
    }
}

impl core::ops::BitAnd for FpClassTest {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        self.intersection(rhs)
    }
}

impl core::ops::BitOr for FpClassTest {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl core::ops::Not for FpClassTest {
    type Output = Self;
    #[inline]
    fn not(self) -> Self {
        self.complement()
    }
}

impl core::ops::BitAndAssign for FpClassTest {
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        *self = self.intersection(rhs);
    }
}

impl core::ops::BitOrAssign for FpClassTest {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        *self = self.union(rhs);
    }
}

/// What is known about a floating-point value's class and sign.
///
/// Ports `llvm::KnownFPClass` (`llvm/Support/KnownFPClass.h`). The default is
/// "nothing known": every class possible, sign unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownFpClass {
    classes: FpClassTest,
    sign_bit: Option<bool>,
}

impl Default for KnownFpClass {
    fn default() -> Self {
        Self {
            classes: FpClassTest::ALL,
            sign_bit: None,
        }
    }
}

impl KnownFpClass {
    /// The classes ordered less than zero. Ports
    /// `KnownFPClass::OrderedLessThanZeroMask`.
    pub const ORDERED_LESS_THAN_ZERO: FpClassTest = FpClassTest(
        FpClassTest::NEGATIVE_SUBNORMAL.0
            | FpClassTest::NEGATIVE_NORMAL.0
            | FpClassTest::NEGATIVE_INFINITY.0,
    );

    /// The classes ordered greater than zero. Ports
    /// `KnownFPClass::OrderedGreaterThanZeroMask`.
    pub const ORDERED_GREATER_THAN_ZERO: FpClassTest = FpClassTest(
        FpClassTest::POSITIVE_SUBNORMAL.0
            | FpClassTest::POSITIVE_NORMAL.0
            | FpClassTest::POSITIVE_INFINITY.0,
    );

    /// Nothing known. Ports the default-constructed `KnownFPClass`.
    #[inline]
    pub fn unknown() -> Self {
        Self::default()
    }

    /// Exactly these classes, with the sign left unknown.
    ///
    /// Ports `KnownFPClass(FPClassTest Known)` at its defaulted `Sign = {}`.
    #[inline]
    pub const fn from_classes(classes: FpClassTest) -> Self {
        Self {
            classes,
            sign_bit: None,
        }
    }

    /// Exactly these classes, with a known sign.
    ///
    /// Ports the two-argument `KnownFPClass(FPClassTest, std::optional<bool>)`.
    #[inline]
    pub const fn new(classes: FpClassTest, sign_bit: Option<bool>) -> Self {
        Self { classes, sign_bit }
    }

    /// Everything a constant tells us: its exact class, and its exact sign.
    ///
    /// Ports `KnownFPClass::KnownFPClass(const APFloat &C)`.
    pub fn of(value: &ApFloat) -> Self {
        Self {
            classes: FpClassTest::of(value),
            sign_bit: Some(value.is_negative()),
        }
    }

    /// The classes the value may belong to. Ports the `KnownFPClasses` field.
    #[inline]
    pub const fn classes(self) -> FpClassTest {
        self.classes
    }

    /// `None` if the sign bit is unknown, otherwise whether it is set. Ports
    /// the `SignBit` field.
    #[inline]
    pub const fn sign_bit(self) -> Option<bool> {
        self.sign_bit
    }

    /// Whether the value can never be in any class in `mask`.
    ///
    /// Ports `isKnownNever`.
    #[inline]
    pub const fn is_known_never(self, mask: FpClassTest) -> bool {
        self.classes.intersection(mask).is_none()
    }

    /// Whether the value must always be in some class in `mask`.
    ///
    /// Ports `isKnownAlways`, which is `isKnownNever(~Mask)`.
    #[inline]
    pub const fn is_known_always(self, mask: FpClassTest) -> bool {
        self.is_known_never(mask.complement())
    }

    /// Whether nothing at all is known. Ports `isUnknown`.
    #[inline]
    pub const fn is_unknown(self) -> bool {
        self.classes.is_all() && self.sign_bit.is_none()
    }

    /// Ports `isKnownNeverNaN`.
    #[inline]
    pub const fn is_known_never_nan(self) -> bool {
        self.is_known_never(FpClassTest::NAN)
    }

    /// Ports `isKnownAlwaysNaN`.
    #[inline]
    pub const fn is_known_always_nan(self) -> bool {
        self.is_known_always(FpClassTest::NAN)
    }

    /// Ports `isKnownNeverInfinity`.
    #[inline]
    pub const fn is_known_never_infinity(self) -> bool {
        self.is_known_never(FpClassTest::INFINITY)
    }

    /// Ports `isKnownNeverInfOrNaN`.
    #[inline]
    pub const fn is_known_never_infinity_or_nan(self) -> bool {
        self.is_known_never(FpClassTest::INFINITY.union(FpClassTest::NAN))
    }

    /// Ports `isKnownNeverPosInfinity`.
    #[inline]
    pub const fn is_known_never_positive_infinity(self) -> bool {
        self.is_known_never(FpClassTest::POSITIVE_INFINITY)
    }

    /// Ports `isKnownNeverNegInfinity`.
    #[inline]
    pub const fn is_known_never_negative_infinity(self) -> bool {
        self.is_known_never(FpClassTest::NEGATIVE_INFINITY)
    }

    /// Ports `isKnownNeverSubnormal`.
    #[inline]
    pub const fn is_known_never_subnormal(self) -> bool {
        self.is_known_never(FpClassTest::SUBNORMAL)
    }

    /// Ports `isKnownNeverPosSubnormal`.
    #[inline]
    pub const fn is_known_never_positive_subnormal(self) -> bool {
        self.is_known_never(FpClassTest::POSITIVE_SUBNORMAL)
    }

    /// Ports `isKnownNeverNegSubnormal`.
    #[inline]
    pub const fn is_known_never_negative_subnormal(self) -> bool {
        self.is_known_never(FpClassTest::NEGATIVE_SUBNORMAL)
    }

    /// Whether the value can never be a literal `[+-]0`.
    ///
    /// Ports `isKnownNeverZero`. Upstream's comment is load-bearing: this does
    /// *not* include a subnormal that a denormal-flushing mode would treat as
    /// zero — [`Self::is_known_never_logical_zero`] is the question that does.
    #[inline]
    pub const fn is_known_never_zero(self) -> bool {
        self.is_known_never(FpClassTest::ZERO)
    }

    /// Ports `isKnownNeverPosZero`.
    #[inline]
    pub const fn is_known_never_positive_zero(self) -> bool {
        self.is_known_never(FpClassTest::POSITIVE_ZERO)
    }

    /// Ports `isKnownNeverNegZero`.
    #[inline]
    pub const fn is_known_never_negative_zero(self) -> bool {
        self.is_known_never(FpClassTest::NEGATIVE_ZERO)
    }

    /// Whether the value can never be *interpreted* as a zero under `mode`.
    ///
    /// Ports `isKnownNeverLogicalZero` (`KnownFPClass.cpp`), which extends
    /// [`Self::is_known_never_zero`] to the case where the function's denormal
    /// mode flushes subnormal inputs to zero.
    #[inline]
    pub fn is_known_never_logical_zero(self, mode: DenormalMode) -> bool {
        self.is_known_never_zero()
            && (self.is_known_never_subnormal() || input_denormal_is_ieee(mode))
    }

    /// Ports `isKnownNeverLogicalNegZero`.
    #[inline]
    pub fn is_known_never_logical_negative_zero(self, mode: DenormalMode) -> bool {
        self.is_known_never_negative_zero()
            && (self.is_known_never_negative_subnormal()
                || input_denormal_is_ieee_or_positive_zero(mode))
    }

    /// Ports `isKnownNeverLogicalPosZero`.
    pub fn is_known_never_logical_positive_zero(self, mode: DenormalMode) -> bool {
        if !self.is_known_never_positive_zero() {
            return false;
        }
        // With no denormals there is nothing to flush to zero.
        if self.is_known_never_subnormal() {
            return true;
        }
        match mode.input() {
            DenormalModeKind::Ieee => true,
            // A negative subnormal will not flush to `+0`.
            DenormalModeKind::PreserveSign => self.is_known_never_positive_subnormal(),
            // Under `PositiveZero` — and under a dynamic mode, which upstream's
            // `default` arm covers — either sign of subnormal could reach `+0`.
            _ => false,
        }
    }

    /// Whether the value is provably NaN or never less than `-0.0`.
    ///
    /// Ports `cannotBeOrderedLessThanZero`. Upstream's table:
    /// NaN, `+0`, `-0` and `x > +0` answer `true`; `x < -0` answers `false`.
    #[inline]
    pub const fn cannot_be_ordered_less_than_zero(self) -> bool {
        self.is_known_never(Self::ORDERED_LESS_THAN_ZERO)
    }

    /// Whether the value is provably NaN or never greater than `-0.0`.
    ///
    /// Ports `cannotBeOrderedGreaterThanZero`.
    #[inline]
    pub const fn cannot_be_ordered_greater_than_zero(self) -> bool {
        self.is_known_never(Self::ORDERED_GREATER_THAN_ZERO)
    }

    /// Whether the value is provably never positive nor a logical zero.
    ///
    /// Ports `cannotBeOrderedGreaterEqZero`, whose `nsub` row depends on the
    /// denormal mode — hence the parameter.
    #[inline]
    pub fn cannot_be_ordered_greater_equal_zero(self, mode: DenormalMode) -> bool {
        self.is_known_never(FpClassTest::POSITIVE)
            && self.is_known_never_logical_negative_zero(mode)
    }

    /// Whether the sign bit must be clear, ignoring the sign of NaNs.
    ///
    /// Ports `signBitIsZeroOrNaN`.
    #[inline]
    pub const fn sign_bit_is_zero_or_nan(self) -> bool {
        self.is_known_never(FpClassTest::NEGATIVE)
    }

    /// What both facts agree on. Ports `intersectWith`.
    ///
    /// Note the direction: upstream writes `~(~A & ~B)`, which is the *union*
    /// of the two class masks — intersecting two facts weakens the claim, and
    /// a sign bit survives only when both agree on it.
    #[inline]
    pub fn intersect_with(self, other: Self) -> Self {
        Self {
            classes: self.classes.union(other.classes),
            sign_bit: if self.sign_bit == other.sign_bit {
                self.sign_bit
            } else {
                None
            },
        }
    }

    /// Widen this fact to admit everything `other` admits. Ports `operator|=`.
    #[inline]
    pub fn union_in_place(&mut self, other: Self) {
        self.classes |= other.classes;
        if self.sign_bit != other.sign_bit {
            self.sign_bit = None;
        }
    }

    /// [`Self::union_in_place`] as a value operation. Ports `operator|`.
    #[inline]
    #[must_use]
    pub fn union_with(mut self, other: Self) -> Self {
        self.union_in_place(other);
        self
    }

    /// Rule out every class in `mask`.
    ///
    /// Ports `knownNot`. The trailing reconciliation is the one place the mask
    /// teaches the sign bit anything: once NaN is ruled out, a mask confined to
    /// one side of zero pins the sign.
    pub fn known_not(&mut self, mask: FpClassTest) {
        self.classes = self.classes.difference(mask);
        if self.is_known_never_nan() && self.sign_bit.is_none() {
            if self.is_known_never(FpClassTest::NEGATIVE) {
                self.sign_bit = Some(false);
            } else if self.is_known_never(FpClassTest::POSITIVE) {
                self.sign_bit = Some(true);
            }
        }
    }

    /// Apply `fneg`. Ports `KnownFPClass::fneg`.
    pub fn negate(&mut self) {
        self.classes = self.classes.negated();
        if let Some(sign) = self.sign_bit {
            self.sign_bit = Some(!sign);
        }
    }

    /// Apply `fabs`. Ports `KnownFPClass::fabs`.
    ///
    /// Each negative class it admits gains its positive twin — `fabs` maps one
    /// onto the other — and then the sign is forced clear, which drops the
    /// negative classes again.
    pub fn absolute(&mut self) {
        for (from, to) in [
            (FpClassTest::NEGATIVE_ZERO, FpClassTest::POSITIVE_ZERO),
            (
                FpClassTest::NEGATIVE_INFINITY,
                FpClassTest::POSITIVE_INFINITY,
            ),
            (
                FpClassTest::NEGATIVE_SUBNORMAL,
                FpClassTest::POSITIVE_SUBNORMAL,
            ),
            (FpClassTest::NEGATIVE_NORMAL, FpClassTest::POSITIVE_NORMAL),
        ] {
            if self.classes.intersects(from) {
                self.classes |= to;
            }
        }
        self.sign_bit_must_be_zero();
    }

    /// Forget whatever was known about the sign bit.
    ///
    /// Ports `KnownFPClass::SignBit.reset()`. The reducing min/max intrinsics
    /// need it: they may return a NaN whose sign is not the one the elements
    /// agreed on, so a sign learned from the elements has to be dropped unless
    /// the result is known never to be a NaN.
    #[inline]
    pub fn reset_sign_bit(&mut self) {
        self.sign_bit = None;
    }

    /// Assume the sign bit is clear. Ports `signBitMustBeZero`.
    #[inline]
    pub fn sign_bit_must_be_zero(&mut self) {
        self.classes &= FpClassTest::POSITIVE.union(FpClassTest::NAN);
        self.sign_bit = Some(false);
    }

    /// Assume the sign bit is set. Ports `signBitMustBeOne`.
    #[inline]
    pub fn sign_bit_must_be_one(&mut self) {
        self.classes &= FpClassTest::NEGATIVE.union(FpClassTest::NAN);
        self.sign_bit = Some(true);
    }

    /// Take this value's magnitude and `sign`'s sign. Ports `copysign`.
    pub fn copy_sign(&mut self, sign: Self) {
        // The source's sign is discarded, so every class it admits widens to
        // its opposite-sign pair.
        for mask in [
            FpClassTest::ZERO,
            FpClassTest::SUBNORMAL,
            FpClassTest::NORMAL,
            FpClassTest::INFINITY,
        ] {
            if self.classes.intersects(mask) {
                self.classes |= mask;
            }
        }

        // The sign bit is preserved exactly, NaNs included.
        self.sign_bit = sign.sign_bit;

        if sign.is_known_never(FpClassTest::POSITIVE.union(FpClassTest::NAN))
            || self.sign_bit == Some(true)
        {
            self.classes &= FpClassTest::NEGATIVE.union(FpClassTest::NAN);
        }
        if sign.is_known_never(FpClassTest::NEGATIVE.union(FpClassTest::NAN))
            || self.sign_bit == Some(false)
        {
            self.classes &= FpClassTest::POSITIVE.union(FpClassTest::NAN);
        }
    }

    /// Carry "not a NaN" forward from a source value. Ports `propagateNaN`.
    ///
    /// Upstream's comment on why the two arms differ: an unconstrained
    /// operation is not guaranteed to *quieten* a signaling NaN, but it cannot
    /// introduce one either.
    pub fn propagate_nan(&mut self, source: Self, preserve_sign: bool) {
        if source.is_known_never_nan() {
            self.known_not(FpClassTest::NAN);
            if preserve_sign {
                self.sign_bit = source.sign_bit;
            }
        } else if source.is_known_never(FpClassTest::SIGNALING_NAN) {
            self.known_not(FpClassTest::SIGNALING_NAN);
        }
    }

    /// Forget everything. Ports `resetAll`.
    #[inline]
    pub fn reset_all(&mut self) {
        *self = Self::unknown();
    }

    // ----------------------------------------------------------------------
    // Operations, ported from `llvm/lib/Support/KnownFPClass.cpp`
    // ----------------------------------------------------------------------

    /// Replace what is known with `source`'s, allowing for a denormal that the
    /// mode may flush to zero.
    ///
    /// Ports `propagateDenormal`. Upstream's comment names the hazard: output
    /// flushing is not guaranteed, so a known-never-zero source may still yield
    /// a zero once denormals are flushed.
    pub fn propagate_denormal(&mut self, source: Self, mode: DenormalMode) {
        self.classes = source.classes;

        // With a zero already possible there is nothing a flush could add.
        if !source.is_known_never_positive_zero() && !source.is_known_never_negative_zero() {
            return;
        }
        // With no denormals possible, nothing can be flushed to zero.
        if source.is_known_never_subnormal() {
            return;
        }

        let ieee = DenormalMode::ieee();
        if !source.is_known_never_positive_subnormal() && mode != ieee {
            self.classes |= FpClassTest::POSITIVE_ZERO;
        }

        if !source.is_known_never_negative_subnormal() && mode != ieee {
            if mode != DenormalMode::positive_zero() {
                self.classes |= FpClassTest::NEGATIVE_ZERO;
            }
            if matches!(
                mode.input(),
                DenormalModeKind::PositiveZero | DenormalModeKind::Dynamic
            ) || matches!(
                mode.output(),
                DenormalModeKind::PositiveZero | DenormalModeKind::Dynamic
            ) {
                self.classes |= FpClassTest::POSITIVE_ZERO;
            }
        }
    }

    /// Replace what is known with `source`'s, as seen through a potentially
    /// canonicalizing operation.
    ///
    /// Ports `propagateCanonicalizingSrc`: signaling NaNs will not be
    /// introduced, but a denormal cannot be assumed flushed.
    pub fn propagate_canonicalizing_source(&mut self, source: Self, mode: DenormalMode) {
        self.propagate_denormal(source, mode);
        self.propagate_nan(source, true);
    }

    /// What is known of `llvm.canonicalize` of a value.
    ///
    /// Ports `KnownFPClass::canonicalize`. Upstream's comment calls it "a
    /// stronger form of `propagateCanonicalizingSrc`": canonicalize is the one
    /// operation guaranteed to quieten a signaling NaN.
    ///
    /// Upstream's `FIXME: Missing check of IEEE like types` is inherited.
    pub fn canonicalize(source: Self, mode: DenormalMode) -> Self {
        let mut known = Self::unknown();

        // Canonicalize may flush denormals to zero, so the denormal mode has to
        // be consulted before any known-not-zero fact survives.
        known.classes = source
            .classes
            .union(FpClassTest::ZERO)
            .union(FpClassTest::QUIET_NAN);

        if source.is_known_never_nan() {
            known.known_not(FpClassTest::NAN);
        } else {
            known.known_not(FpClassTest::SIGNALING_NAN);
        }

        // With the parent function flushing denormals, the canonical output
        // cannot be one.
        if mode == DenormalMode::ieee() {
            if source.is_known_never(FpClassTest::POSITIVE_ZERO) {
                known.known_not(FpClassTest::POSITIVE_ZERO);
            }
            if source.is_known_never(FpClassTest::NEGATIVE_ZERO) {
                known.known_not(FpClassTest::NEGATIVE_ZERO);
            }
            return known;
        }

        if mode.inputs_are_zero() || mode.outputs_are_zero() {
            known.known_not(FpClassTest::SUBNORMAL);
        }

        if mode == DenormalMode::preserve_sign() {
            if source
                .is_known_never(FpClassTest::POSITIVE_ZERO.union(FpClassTest::POSITIVE_SUBNORMAL))
            {
                known.known_not(FpClassTest::POSITIVE_ZERO);
            }
            if source
                .is_known_never(FpClassTest::NEGATIVE_ZERO.union(FpClassTest::NEGATIVE_SUBNORMAL))
            {
                known.known_not(FpClassTest::NEGATIVE_ZERO);
            }
            return known;
        }

        if mode.input() == DenormalModeKind::PositiveZero
            || (mode.output() == DenormalModeKind::PositiveZero
                && mode.input() == DenormalModeKind::Ieee)
        {
            known.known_not(FpClassTest::NEGATIVE_ZERO);
        }

        known
    }

    /// What is known of `fmul lhs, rhs`.
    ///
    /// Ports `KnownFPClass::fmul`.
    pub fn fmul(lhs: Self, rhs: Self, mode: DenormalMode) -> Self {
        let mut known = Self::unknown();

        // The sign bit is the xor of the two.
        if (lhs.is_known_never(FpClassTest::NEGATIVE) && rhs.is_known_never(FpClassTest::NEGATIVE))
            || (lhs.is_known_never(FpClassTest::POSITIVE)
                && rhs.is_known_never(FpClassTest::POSITIVE))
        {
            known.known_not(FpClassTest::NEGATIVE);
        }
        if (lhs.is_known_never(FpClassTest::POSITIVE) && rhs.is_known_never(FpClassTest::NEGATIVE))
            || (lhs.is_known_never(FpClassTest::NEGATIVE)
                && rhs.is_known_never(FpClassTest::POSITIVE))
        {
            known.known_not(FpClassTest::POSITIVE);
        }

        let infinity_or_nan = FpClassTest::INFINITY.union(FpClassTest::NAN);
        let zero_or_nan = FpClassTest::ZERO.union(FpClassTest::NAN);

        // inf * anything => inf or nan
        if lhs.is_known_always(infinity_or_nan) || rhs.is_known_always(infinity_or_nan) {
            known.known_not(
                FpClassTest::NORMAL
                    .union(FpClassTest::SUBNORMAL)
                    .union(FpClassTest::ZERO),
            );
        }
        // 0 * anything => 0 or nan
        if rhs.is_known_always(zero_or_nan) || lhs.is_known_always(zero_or_nan) {
            known.known_not(
                FpClassTest::NORMAL
                    .union(FpClassTest::SUBNORMAL)
                    .union(FpClassTest::INFINITY),
            );
        }
        // +/-0 * +/-inf = nan
        if (lhs.is_known_always(zero_or_nan) && rhs.is_known_always(infinity_or_nan))
            || (lhs.is_known_always(infinity_or_nan) && rhs.is_known_always(zero_or_nan))
        {
            known.known_not(FpClassTest::NAN.complement());
        }

        if !lhs.is_known_never_nan() || !rhs.is_known_never_nan() {
            return known;
        }

        if let (Some(lhs_sign), Some(rhs_sign)) = (lhs.sign_bit, rhs.sign_bit) {
            if lhs_sign == rhs_sign {
                known.sign_bit_must_be_zero();
            } else {
                known.sign_bit_must_be_one();
            }
        }

        // Only `0 * +/-inf` produces a NaN, and neither pairing can arise.
        if (rhs.is_known_never_infinity() || lhs.is_known_never_logical_zero(mode))
            && (lhs.is_known_never_infinity() || rhs.is_known_never_logical_zero(mode))
        {
            known.known_not(FpClassTest::NAN);
        }

        known
    }

    /// What is known of `fmul x, x`.
    ///
    /// Ports `KnownFPClass::square`, which is [`Self::fmul`] of a value with
    /// itself plus the one extra fact that gives: a square is never negative.
    pub fn square(source: Self, mode: DenormalMode) -> Self {
        let mut known = Self::fmul(source, source, mode);
        known.known_not(FpClassTest::NEGATIVE);
        known
    }

    /// What is known of `exp`, `exp2` or `exp10` of a value.
    ///
    /// Ports `KnownFPClass::exp`.
    pub fn exp(source: Self) -> Self {
        let mut known = Self::unknown();
        known.known_not(FpClassTest::NEGATIVE);
        known.propagate_nan(source, false);

        if source.cannot_be_ordered_less_than_zero() {
            // A positive source cannot underflow, so no zero and no denormal.
            known.known_not(FpClassTest::POSITIVE_ZERO);
            known.known_not(FpClassTest::POSITIVE_SUBNORMAL);
        }
        // A negative source cannot overflow to infinity.
        if source.cannot_be_ordered_greater_than_zero() {
            known.known_not(FpClassTest::POSITIVE_INFINITY);
        }
        known
    }

    /// What is known of `log`, `log2` or `log10` of a value.
    ///
    /// Ports `KnownFPClass::log`.
    pub fn log(source: Self, mode: DenormalMode) -> Self {
        let mut known = Self::unknown();
        known.known_not(FpClassTest::NEGATIVE_ZERO);

        if source.is_known_never_positive_infinity() {
            known.known_not(FpClassTest::POSITIVE_INFINITY);
        }
        if source.is_known_never_nan() && source.cannot_be_ordered_less_than_zero() {
            known.known_not(FpClassTest::NAN);
        }
        if source.is_known_never_logical_zero(mode) {
            known.known_not(FpClassTest::NEGATIVE_INFINITY);
        }
        known
    }

    /// What is known of `sqrt` of a value.
    ///
    /// Ports `KnownFPClass::sqrt`.
    pub fn sqrt(source: Self, mode: DenormalMode) -> Self {
        let mut known = Self::unknown();
        known.known_not(FpClassTest::POSITIVE_SUBNORMAL);

        if source.is_known_never_positive_infinity() {
            known.known_not(FpClassTest::POSITIVE_INFINITY);
        }
        if source.is_known_never(FpClassTest::SIGNALING_NAN) {
            known.known_not(FpClassTest::SIGNALING_NAN);
        }
        // Any negative value but `-0` gives a NaN.
        if source.is_known_never_nan() && source.cannot_be_ordered_less_than_zero() {
            known.known_not(FpClassTest::NAN);
        }
        // `-0` is the only negative value that can come back.
        known.known_not(
            FpClassTest::NEGATIVE_INFINITY
                .union(FpClassTest::NEGATIVE_SUBNORMAL)
                .union(FpClassTest::NEGATIVE_NORMAL),
        );
        // Under `PreserveSign` a negative subnormal input can produce `-0`.
        if source.is_known_never_logical_negative_zero(mode) {
            known.known_not(FpClassTest::NEGATIVE_ZERO);
        }
        known
    }

    /// What is known of an `fpext` from `source_semantics` to
    /// `destination_semantics`.
    ///
    /// Ports `KnownFPClass::fpext`. Infinity, NaN and zero pass straight
    /// through; a subnormal that the wider type can represent as a normal
    /// becomes one.
    pub fn fpext(
        source: Self,
        destination_semantics: ApFloatSemantics,
        source_semantics: ApFloatSemantics,
    ) -> Self {
        let mut known = source;

        if source_semantics.is_representable_as_normal_in(destination_semantics) {
            if known.classes.intersects(FpClassTest::POSITIVE_SUBNORMAL) {
                known.classes |= FpClassTest::POSITIVE_NORMAL;
            }
            if known.classes.intersects(FpClassTest::NEGATIVE_SUBNORMAL) {
                known.classes |= FpClassTest::NEGATIVE_NORMAL;
            }
            known.known_not(FpClassTest::SUBNORMAL);
        }

        // The sign bit of a NaN is not guaranteed to survive.
        if !known.is_known_never_nan() {
            known.sign_bit = None;
        }
        known
    }

    /// What is known of a rounding intrinsic — `trunc`, `floor`, `ceil`,
    /// `rint`, `nearbyint`, `round` or `roundeven`.
    ///
    /// Ports `KnownFPClass::roundToIntegral`. `is_trunc` selects `trunc`, and
    /// `is_multi_unit_float_type` marks `ppc_fp128`, which upstream's comment
    /// records as the special case where infinities do *not* pass through for
    /// anything but `trunc`.
    pub fn round_to_integral(source: Self, is_trunc: bool, is_multi_unit_float_type: bool) -> Self {
        let mut known = Self::unknown();

        // An integral result cannot be subnormal.
        known.known_not(FpClassTest::SUBNORMAL);
        known.propagate_nan(source, true);

        if is_trunc || !is_multi_unit_float_type {
            if source.is_known_never_positive_infinity() {
                known.known_not(FpClassTest::POSITIVE_INFINITY);
            }
            if source.is_known_never_negative_infinity() {
                known.known_not(FpClassTest::NEGATIVE_INFINITY);
            }
        }

        // Rounding a negative value up to zero produces `-0`.
        if source.is_known_never(FpClassTest::POSITIVE_FINITE) {
            known.known_not(FpClassTest::POSITIVE_FINITE);
        }
        if source.is_known_never(FpClassTest::NEGATIVE_FINITE) {
            known.known_not(FpClassTest::NEGATIVE_FINITE);
        }
        known
    }

    /// What is known of a min/max intrinsic applied to two values.
    ///
    /// Ports `KnownFPClass::minMaxLike`. Upstream's `llvm_unreachable("unhandled
    /// intrinsic")` guards a closed enum, which [`MinMaxKind`] is here, so the
    /// match is total and needs no counterpart.
    pub fn min_max_like(lhs: Self, rhs: Self, kind: MinMaxKind, mode: DenormalMode) -> Self {
        let mut known_lhs = lhs;
        let mut known_rhs = rhs;

        let never_nan = known_lhs.is_known_never_nan() || known_rhs.is_known_never_nan();
        let mut known = known_lhs.union_with(known_rhs);

        // The `*num` forms return the non-NaN operand, so one non-NaN operand
        // is enough to rule NaN out.
        if never_nan && kind.returns_the_non_nan_operand() {
            known.known_not(FpClassTest::NAN);
        }

        match kind {
            // One known-positive operand forces a positive maximum. The `*num`
            // forms need that operand to be non-NaN as well, because a NaN
            // operand is the one they discard.
            MinMaxKind::MaxNum | MinMaxKind::MaximumNum => {
                if (known_lhs.cannot_be_ordered_less_than_zero() && known_lhs.is_known_never_nan())
                    || (known_rhs.cannot_be_ordered_less_than_zero()
                        && known_rhs.is_known_never_nan())
                {
                    known.known_not(Self::ORDERED_LESS_THAN_ZERO);
                }
            }
            MinMaxKind::Maximum => {
                if known_lhs.cannot_be_ordered_less_than_zero()
                    || known_rhs.cannot_be_ordered_less_than_zero()
                {
                    known.known_not(Self::ORDERED_LESS_THAN_ZERO);
                }
            }
            MinMaxKind::MinNum | MinMaxKind::MinimumNum => {
                if (known_lhs.cannot_be_ordered_greater_than_zero()
                    && known_lhs.is_known_never_nan())
                    || (known_rhs.cannot_be_ordered_greater_than_zero()
                        && known_rhs.is_known_never_nan())
                {
                    known.known_not(Self::ORDERED_GREATER_THAN_ZERO);
                }
            }
            MinMaxKind::Minimum => {
                if known_lhs.cannot_be_ordered_greater_than_zero()
                    || known_rhs.cannot_be_ordered_greater_than_zero()
                {
                    known.known_not(Self::ORDERED_GREATER_THAN_ZERO);
                }
            }
        }

        // Denormals could come back as a zero. Upstream's comment: there is no
        // spec for denormal flushing, and on older AMDGPU subtargets min/max
        // returned the original value rather than flushing it, so this stays
        // conservative.
        if known.classes.intersects(FpClassTest::ZERO)
            && !known.is_known_never_subnormal()
            && mode != DenormalMode::ieee()
        {
            known.classes |= FpClassTest::ZERO;
        }

        if known.is_known_never_nan() {
            if let (Some(lhs_sign), Some(rhs_sign)) = (known_lhs.sign_bit, known_rhs.sign_bit)
                && lhs_sign == rhs_sign
            {
                if lhs_sign {
                    known.sign_bit_must_be_one();
                } else {
                    known.sign_bit_must_be_zero();
                }
                return known;
            }

            // Upstream's `FIXME: Should be using logical zero versions` on the
            // second disjunct is inherited.
            let zeros_cannot_disagree = (known_lhs.is_known_never_negative_zero()
                || known_rhs.is_known_never_positive_zero())
                && (known_lhs.is_known_never_positive_zero()
                    || known_rhs.is_known_never_negative_zero());

            if kind.is_ieee_754_2019_form() || zeros_cannot_disagree {
                // A NaN operand's sign bit says nothing about the result's.
                if !known_lhs.is_known_never_nan() {
                    known_lhs.sign_bit = None;
                }
                if !known_rhs.is_known_never_nan() {
                    known_rhs.sign_bit = None;
                }
                if kind.is_maximum()
                    && (known_lhs.sign_bit == Some(false) || known_rhs.sign_bit == Some(false))
                {
                    known.sign_bit_must_be_zero();
                } else if kind.is_minimum()
                    && (known_lhs.sign_bit == Some(true) || known_rhs.sign_bit == Some(true))
                {
                    known.sign_bit_must_be_one();
                }
            }
        }

        known
    }
}

/// Which min/max operation [`KnownFpClass::min_max_like`] is reasoning about.
///
/// Ports `KnownFPClass::MinMaxKind`, whose own comment gives the reason it
/// exists rather than reusing the intrinsic ids: "Enum of min/max intrinsics to
/// avoid dependency on IR."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MinMaxKind {
    /// `llvm.minimum` — IEEE-754-2019 `minimum`, which propagates NaN.
    Minimum,
    /// `llvm.maximum` — IEEE-754-2019 `maximum`, which propagates NaN.
    Maximum,
    /// `llvm.minimumnum` — IEEE-754-2019 `minimumNumber`.
    MinimumNum,
    /// `llvm.maximumnum` — IEEE-754-2019 `maximumNumber`.
    MaximumNum,
    /// `llvm.minnum` — IEEE-754-2008 `minNum`.
    MinNum,
    /// `llvm.maxnum` — IEEE-754-2008 `maxNum`.
    MaxNum,
}

impl MinMaxKind {
    /// Whether a NaN operand is discarded in favour of the other one, so that
    /// one non-NaN operand rules NaN out of the result.
    #[inline]
    pub const fn returns_the_non_nan_operand(self) -> bool {
        matches!(
            self,
            Self::MinNum | Self::MaxNum | Self::MinimumNum | Self::MaximumNum
        )
    }

    /// Whether this is one of the four IEEE-754-2019 forms, which order `-0`
    /// below `+0` and so give the result's sign bit without further reasoning.
    #[inline]
    pub const fn is_ieee_754_2019_form(self) -> bool {
        matches!(
            self,
            Self::Maximum | Self::Minimum | Self::MaximumNum | Self::MinimumNum
        )
    }

    /// Whether this takes the larger of its operands.
    #[inline]
    pub const fn is_maximum(self) -> bool {
        matches!(self, Self::Maximum | Self::MaximumNum | Self::MaxNum)
    }

    /// Whether this takes the smaller of its operands.
    #[inline]
    pub const fn is_minimum(self) -> bool {
        matches!(self, Self::Minimum | Self::MinimumNum | Self::MinNum)
    }

    /// The min/max computing the opposite extremum.
    ///
    /// Ports the six floating-point arms of `llvm::getInverseMinMaxIntrinsic`.
    /// Upstream warns at those arms that — unlike the integer four — inverting
    /// may produce the *same* result as the original even for distinct
    /// operands, because NaN is handled specially. Inverting is therefore an
    /// involution on the operation, not a statement about the value.
    #[inline]
    pub const fn inverse(self) -> Self {
        match self {
            Self::Minimum => Self::Maximum,
            Self::Maximum => Self::Minimum,
            Self::MinimumNum => Self::MaximumNum,
            Self::MaximumNum => Self::MinimumNum,
            Self::MinNum => Self::MaxNum,
            Self::MaxNum => Self::MinNum,
        }
    }

    /// The intrinsic's base name, as it appears in `.ll` text.
    ///
    /// The inverse of the mapping `computeKnownFPClass` reads its operand
    /// through — upstream's static `getMinMaxKind`.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Minimum => "llvm.minimum",
            Self::Maximum => "llvm.maximum",
            Self::MinimumNum => "llvm.minimumnum",
            Self::MaximumNum => "llvm.maximumnum",
            Self::MinNum => "llvm.minnum",
            Self::MaxNum => "llvm.maxnum",
        }
    }
}

/// Whether IEEE treatment of denormal inputs may be assumed.
///
/// Ports the static `inputDenormalIsIEEE` (`KnownFPClass.cpp`).
#[inline]
fn input_denormal_is_ieee(mode: DenormalMode) -> bool {
    mode.input() == DenormalModeKind::Ieee
}

/// Ports the static `inputDenormalIsIEEEOrPosZero`.
#[inline]
fn input_denormal_is_ieee_or_positive_zero(mode: DenormalMode) -> bool {
    matches!(
        mode.input(),
        DenormalModeKind::Ieee | DenormalModeKind::PositiveZero
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ap_float::{ApFloatSemantics, ApFloatSign, NanPayload, RoundingMode};

    /// A single-precision constant from its decimal text, at the default
    /// rounding mode. Every `APFloat(APFloat::IEEEsingle(), Str)` upstream
    /// writes is this.
    fn parse(text: &str) -> ApFloat {
        let (value, _) = ApFloat::from_string(
            ApFloatSemantics::IeeeSingle,
            text,
            RoundingMode::NearestTiesToEven,
        )
        .expect("fixture parses");
        value
    }

    /// The `classify()` assertions inside
    /// `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, isSignaling)` —
    /// upstream has no test named for `classify`, so its assertions are ported
    /// from the two tests that make them.
    #[test]
    fn classify_separates_the_two_nans() {
        let quiet = ApFloat::qnan(
            ApFloatSemantics::IeeeSingle,
            ApFloatSign::Positive,
            NanPayload::Absent,
        );
        assert!(!quiet.is_signaling());
        assert_eq!(FpClassTest::of(&quiet), FpClassTest::QUIET_NAN);

        let signaling = ApFloat::snan(
            ApFloatSemantics::IeeeSingle,
            ApFloatSign::Positive,
            NanPayload::Absent,
        );
        assert!(signaling.is_signaling());
        assert_eq!(FpClassTest::of(&signaling), FpClassTest::SIGNALING_NAN);
    }

    /// The `classify()` assertions inside the single-precision block of
    /// `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, isDenormal)`,
    /// including its construction: the smallest normal divided by two.
    #[test]
    fn classify_separates_the_two_subnormals() {
        let two = parse("2");

        let min_normal = parse("1.17549435082228750797e-38");
        assert!(!min_normal.is_denormal());
        let (subnormal, _) = min_normal.divide(&two, RoundingMode::NearestTiesToEven);
        assert!(subnormal.is_denormal());
        assert_eq!(FpClassTest::of(&subnormal), FpClassTest::POSITIVE_SUBNORMAL);

        let negative_min_normal = parse("-1.17549435082228750797e-38");
        assert!(!negative_min_normal.is_denormal());
        let (negative_subnormal, _) =
            negative_min_normal.divide(&two, RoundingMode::NearestTiesToEven);
        assert!(negative_subnormal.is_denormal());
        assert_eq!(
            FpClassTest::of(&negative_subnormal),
            FpClassTest::NEGATIVE_SUBNORMAL
        );
    }

    /// `classify()` over the remaining categories.
    ///
    /// No upstream counterpart: LLVM asserts these only incidentally, through
    /// folds. The expectations come from `APFloat::classify` itself, whose five
    /// arms this covers end to end.
    #[test]
    fn classify_covers_every_category() {
        let zero = ApFloat::zero(ApFloatSemantics::IeeeSingle, ApFloatSign::Positive);
        assert_eq!(FpClassTest::of(&zero), FpClassTest::POSITIVE_ZERO);
        let negative_zero = ApFloat::zero(ApFloatSemantics::IeeeSingle, ApFloatSign::Negative);
        assert_eq!(FpClassTest::of(&negative_zero), FpClassTest::NEGATIVE_ZERO);

        let one = parse("1");
        assert_eq!(FpClassTest::of(&one), FpClassTest::POSITIVE_NORMAL);
        let minus_one = parse("-1");
        assert_eq!(FpClassTest::of(&minus_one), FpClassTest::NEGATIVE_NORMAL);
    }

    /// `llvm::fneg`, `llvm::inverse_fabs` and `llvm::unknown_sign`
    /// (`llvm/lib/Support/FloatingPointMode.cpp`).
    ///
    /// No upstream unit test covers these three; the expectations are read off
    /// the functions themselves, one row per branch they contain.
    #[test]
    fn the_three_mask_transforms() {
        // Every signed class swaps; NaN passes through unchanged.
        assert_eq!(
            FpClassTest::NEGATIVE_INFINITY.negated(),
            FpClassTest::POSITIVE_INFINITY
        );
        assert_eq!(
            FpClassTest::POSITIVE_ZERO.negated(),
            FpClassTest::NEGATIVE_ZERO
        );
        assert_eq!(FpClassTest::NAN.negated(), FpClassTest::NAN);
        assert_eq!(FpClassTest::ALL.negated(), FpClassTest::ALL);
        assert_eq!(FpClassTest::NONE.negated(), FpClassTest::NONE);

        // `fabs` maps both signs onto the positive class, so a positive class
        // in the mask admits either sign of input and a negative one admits
        // nothing.
        assert_eq!(
            FpClassTest::POSITIVE_ZERO.inverse_absolute(),
            FpClassTest::ZERO
        );
        assert_eq!(
            FpClassTest::NEGATIVE_ZERO.inverse_absolute(),
            FpClassTest::NONE
        );
        assert_eq!(
            FpClassTest::POSITIVE_INFINITY.inverse_absolute(),
            FpClassTest::INFINITY
        );
        assert_eq!(FpClassTest::NAN.inverse_absolute(), FpClassTest::NAN);

        // Same magnitudes, sign forgotten.
        assert_eq!(
            FpClassTest::NEGATIVE_NORMAL.with_unknown_sign(),
            FpClassTest::NORMAL
        );
        assert_eq!(
            FpClassTest::POSITIVE_NORMAL.with_unknown_sign(),
            FpClassTest::NORMAL
        );
        assert_eq!(FpClassTest::ALL.with_unknown_sign(), FpClassTest::ALL);
    }

    /// `operator<<(raw_ostream &, FPClassTest)`
    /// (`llvm/lib/Support/FloatingPointMode.cpp`), whose comments state the two
    /// rules: widest name first, and each bit printed once.
    ///
    /// No upstream unit test; the expectations follow from `NoFPClassName`'s
    /// order, which the port reproduces.
    #[test]
    fn mask_printing_consumes_the_widest_name_first() {
        assert_eq!(FpClassTest::NONE.to_string(), "none");
        assert_eq!(FpClassTest::ALL.to_string(), "all");
        assert_eq!(FpClassTest::NAN.to_string(), "nan");
        assert_eq!(FpClassTest::SIGNALING_NAN.to_string(), "snan");
        assert_eq!(FpClassTest::ZERO.to_string(), "zero");
        // `nan` is consumed whole before `inf` is reached.
        assert_eq!(
            FpClassTest::NAN.union(FpClassTest::INFINITY).to_string(),
            "nan inf"
        );
        // A half of a pair prints its own name, not the pair's.
        assert_eq!(
            FpClassTest::QUIET_NAN
                .union(FpClassTest::NEGATIVE_ZERO)
                .to_string(),
            "qnan nzero"
        );
    }

    /// The predicates `llvm/Support/KnownFPClass.h` defines inline, each read
    /// off its own definition. No upstream unit test covers `KnownFPClass`
    /// directly — it is exercised through `computeKnownFPClass`.
    #[test]
    fn lattice_predicates() {
        assert!(KnownFpClass::unknown().is_unknown());
        assert!(!KnownFpClass::unknown().is_known_never_nan());

        let finite = KnownFpClass::from_classes(FpClassTest::FINITE);
        assert!(finite.is_known_never_nan());
        assert!(finite.is_known_never_infinity());
        assert!(finite.is_known_never_infinity_or_nan());
        assert!(!finite.is_known_never_zero());
        assert!(finite.is_known_always(FpClassTest::FINITE));

        let positive = KnownFpClass::from_classes(FpClassTest::POSITIVE);
        assert!(positive.cannot_be_ordered_less_than_zero());
        assert!(!positive.cannot_be_ordered_greater_than_zero());
        assert!(positive.sign_bit_is_zero_or_nan());
    }

    /// `knownNot` is the one place the class mask teaches the sign bit
    /// anything: once NaN is ruled out, a mask on one side of zero pins it.
    #[test]
    fn ruling_out_nan_and_one_sign_pins_the_sign_bit() {
        let mut known = KnownFpClass::unknown();
        assert_eq!(known.sign_bit(), None);

        known.known_not(FpClassTest::NAN);
        assert_eq!(known.sign_bit(), None, "both signs still possible");

        known.known_not(FpClassTest::NEGATIVE);
        assert_eq!(known.sign_bit(), Some(false));

        // The other direction.
        let mut negative = KnownFpClass::unknown();
        negative.known_not(FpClassTest::NAN.union(FpClassTest::POSITIVE));
        assert_eq!(negative.sign_bit(), Some(true));

        // With NaN still possible the mask says nothing about the sign.
        let mut with_nan = KnownFpClass::unknown();
        with_nan.known_not(FpClassTest::NEGATIVE);
        assert_eq!(with_nan.sign_bit(), None);
    }

    /// `KnownFPClass::fneg` and `KnownFPClass::fabs`, which move the mask and
    /// the sign bit together.
    #[test]
    fn negate_and_absolute_move_mask_and_sign_together() {
        let mut known = KnownFpClass::new(FpClassTest::POSITIVE_NORMAL, Some(false));
        known.negate();
        assert_eq!(known.classes(), FpClassTest::NEGATIVE_NORMAL);
        assert_eq!(known.sign_bit(), Some(true));

        known.absolute();
        assert_eq!(known.classes(), FpClassTest::POSITIVE_NORMAL);
        assert_eq!(known.sign_bit(), Some(false));
    }

    /// `intersectWith` weakens and `operator|=` widens; both drop a sign bit
    /// the two sides disagree on.
    #[test]
    fn intersect_weakens_and_union_widens() {
        let positive_zero = KnownFpClass::new(FpClassTest::POSITIVE_ZERO, Some(false));
        let negative_zero = KnownFpClass::new(FpClassTest::NEGATIVE_ZERO, Some(true));

        // Upstream's `intersectWith` unions the class masks — intersecting two
        // *facts* weakens the claim.
        let both = positive_zero.intersect_with(negative_zero);
        assert_eq!(both.classes(), FpClassTest::ZERO);
        assert_eq!(both.sign_bit(), None);

        let merged = positive_zero.union_with(negative_zero);
        assert_eq!(merged.classes(), FpClassTest::ZERO);
        assert_eq!(merged.sign_bit(), None);
    }

    /// `isKnownNeverLogicalZero` and its two siblings
    /// (`llvm/lib/Support/KnownFPClass.cpp`) — the denormal mode decides
    /// whether a subnormal counts as a zero.
    #[test]
    fn logical_zero_depends_on_the_denormal_mode() {
        // Never a literal zero, but subnormals are still possible.
        let subnormal = KnownFpClass::from_classes(FpClassTest::SUBNORMAL);
        assert!(subnormal.is_known_never_zero());

        // Under IEEE nothing is flushed, so it is never a logical zero either.
        assert!(subnormal.is_known_never_logical_zero(DenormalMode::ieee()));
        // Under a flushing mode a subnormal reads as zero.
        assert!(!subnormal.is_known_never_logical_zero(DenormalMode::new(
            DenormalModeKind::PreserveSign,
            DenormalModeKind::PreserveSign,
        )));

        // `PreserveSign` sends a negative subnormal to `-0`, never to `+0`.
        let negative_subnormal = KnownFpClass::from_classes(FpClassTest::NEGATIVE_SUBNORMAL);
        let preserve_sign = DenormalMode::new(
            DenormalModeKind::PreserveSign,
            DenormalModeKind::PreserveSign,
        );
        assert!(negative_subnormal.is_known_never_logical_positive_zero(preserve_sign));
        assert!(!negative_subnormal.is_known_never_logical_negative_zero(preserve_sign));

        // `PositiveZero` sends either sign to `+0`.
        let positive_zero_mode = DenormalMode::new(
            DenormalModeKind::PositiveZero,
            DenormalModeKind::PositiveZero,
        );
        assert!(!negative_subnormal.is_known_never_logical_positive_zero(positive_zero_mode));
        assert!(negative_subnormal.is_known_never_logical_negative_zero(positive_zero_mode));
    }

    /// `KnownFPClass::fmul` (`llvm/lib/Support/KnownFPClass.cpp`).
    ///
    /// No upstream unit test; the rows are its own branches — the sign xor, the
    /// two absorbing cases, and the `0 * inf` NaN.
    #[test]
    fn fmul_xors_the_sign_and_finds_the_absorbing_cases() {
        let ieee = DenormalMode::ieee();
        let positive = KnownFpClass::from_classes(FpClassTest::POSITIVE);
        let negative = KnownFpClass::from_classes(FpClassTest::NEGATIVE);

        // Two positives, or two negatives, give a non-negative result.
        assert!(KnownFpClass::fmul(positive, positive, ieee).is_known_never(FpClassTest::NEGATIVE));
        assert!(KnownFpClass::fmul(negative, negative, ieee).is_known_never(FpClassTest::NEGATIVE));
        // Mixed signs give a non-positive one.
        assert!(KnownFpClass::fmul(positive, negative, ieee).is_known_never(FpClassTest::POSITIVE));

        // `inf * anything` is an infinity or a NaN — never finite.
        let infinity = KnownFpClass::from_classes(FpClassTest::INFINITY);
        let anything = KnownFpClass::unknown();
        let product = KnownFpClass::fmul(infinity, anything, ieee);
        assert!(product.is_known_never(FpClassTest::NORMAL));
        assert!(product.is_known_never(FpClassTest::SUBNORMAL));
        assert!(product.is_known_never(FpClassTest::ZERO));

        // `0 * inf` is exactly a NaN.
        let zero = KnownFpClass::from_classes(FpClassTest::ZERO);
        let nan_only = KnownFpClass::fmul(zero, infinity, ieee);
        assert!(nan_only.is_known_always(FpClassTest::NAN));

        // A finite non-zero pair cannot make a NaN.
        let normal = KnownFpClass::from_classes(FpClassTest::NORMAL);
        assert!(KnownFpClass::fmul(normal, normal, ieee).is_known_never_nan());
    }

    /// `KnownFPClass::square` — [`KnownFpClass::fmul`] of a value with itself,
    /// plus the one extra fact that gives.
    #[test]
    fn a_square_is_never_negative() {
        let known = KnownFpClass::square(KnownFpClass::unknown(), DenormalMode::ieee());
        assert!(known.is_known_never(FpClassTest::NEGATIVE));
    }

    /// `KnownFPClass::sqrt`, `::log` and `::exp`.
    ///
    /// No upstream unit test; each row is a branch of the function named.
    #[test]
    fn sqrt_log_and_exp() {
        let ieee = DenormalMode::ieee();

        // `sqrt` returns no negative value but `-0`, and never a positive
        // subnormal.
        let root = KnownFpClass::sqrt(KnownFpClass::unknown(), ieee);
        assert!(root.is_known_never(FpClassTest::NEGATIVE_NORMAL));
        assert!(root.is_known_never(FpClassTest::NEGATIVE_INFINITY));
        assert!(root.is_known_never(FpClassTest::POSITIVE_SUBNORMAL));
        // A non-negative, non-NaN source cannot make a NaN.
        let non_negative = KnownFpClass::from_classes(FpClassTest::POSITIVE);
        assert!(KnownFpClass::sqrt(non_negative, ieee).is_known_never_nan());

        // `log` never returns `-0`; a source that cannot be zero rules out
        // `-inf`.
        let logarithm = KnownFpClass::log(KnownFpClass::unknown(), ieee);
        assert!(logarithm.is_known_never(FpClassTest::NEGATIVE_ZERO));
        let non_zero = KnownFpClass::from_classes(FpClassTest::NORMAL);
        assert!(KnownFpClass::log(non_zero, ieee).is_known_never(FpClassTest::NEGATIVE_INFINITY));

        // `exp` is never negative; a non-negative source cannot underflow.
        let exponential = KnownFpClass::exp(KnownFpClass::unknown());
        assert!(exponential.is_known_never(FpClassTest::NEGATIVE));
        let positive = KnownFpClass::from_classes(FpClassTest::POSITIVE);
        let from_positive = KnownFpClass::exp(positive);
        assert!(from_positive.is_known_never(FpClassTest::POSITIVE_ZERO));
        assert!(from_positive.is_known_never(FpClassTest::POSITIVE_SUBNORMAL));
    }

    /// `KnownFPClass::fpext` over `APFloatBase::isRepresentableAsNormalIn`.
    ///
    /// `float` → `double` widens the exponent range strictly at both ends, so
    /// every `float` subnormal is a `double` normal; `float` → `float` does
    /// not, so nothing moves.
    #[test]
    fn fpext_normalises_subnormals_only_when_the_range_grows() {
        let subnormal = KnownFpClass::from_classes(FpClassTest::POSITIVE_SUBNORMAL);

        let widened = KnownFpClass::fpext(
            subnormal,
            ApFloatSemantics::IeeeDouble,
            ApFloatSemantics::IeeeSingle,
        );
        assert!(widened.is_known_never(FpClassTest::SUBNORMAL));
        assert!(widened.classes().intersects(FpClassTest::POSITIVE_NORMAL));

        let unchanged = KnownFpClass::fpext(
            subnormal,
            ApFloatSemantics::IeeeSingle,
            ApFloatSemantics::IeeeSingle,
        );
        assert_eq!(unchanged.classes(), FpClassTest::POSITIVE_SUBNORMAL);
    }

    /// `KnownFPClass::roundToIntegral`. The `ppc_fp128` row is upstream's
    /// stated special case: infinities pass through only for `trunc`.
    #[test]
    fn round_to_integral_never_yields_a_subnormal() {
        let finite = KnownFpClass::from_classes(FpClassTest::FINITE);

        let rounded = KnownFpClass::round_to_integral(finite, false, false);
        assert!(rounded.is_known_never(FpClassTest::SUBNORMAL));
        assert!(rounded.is_known_never(FpClassTest::INFINITY));

        // On a multi-unit type, a non-`trunc` rounding does not pass
        // infinities through.
        let multi_unit = KnownFpClass::round_to_integral(finite, false, true);
        assert!(!multi_unit.is_known_never(FpClassTest::INFINITY));
        // ... but `trunc` does.
        let truncated = KnownFpClass::round_to_integral(finite, true, true);
        assert!(truncated.is_known_never(FpClassTest::INFINITY));
    }

    /// `KnownFPClass::minMaxLike`. The `*num` forms discard a NaN operand, so
    /// one non-NaN operand rules NaN out; the IEEE-754-2019 `minimum`/`maximum`
    /// propagate it.
    #[test]
    fn min_max_like_discards_nan_only_for_the_num_forms() {
        let ieee = DenormalMode::ieee();
        let no_nan = KnownFpClass::from_classes(FpClassTest::FINITE);
        let maybe_nan = KnownFpClass::unknown();

        for kind in [
            MinMaxKind::MinNum,
            MinMaxKind::MaxNum,
            MinMaxKind::MinimumNum,
            MinMaxKind::MaximumNum,
        ] {
            assert!(
                KnownFpClass::min_max_like(no_nan, maybe_nan, kind, ieee).is_known_never_nan(),
                "{kind:?} discards a NaN operand"
            );
        }
        for kind in [MinMaxKind::Minimum, MinMaxKind::Maximum] {
            assert!(
                !KnownFpClass::min_max_like(no_nan, maybe_nan, kind, ieee).is_known_never_nan(),
                "{kind:?} propagates a NaN operand"
            );
        }

        // One known-positive non-NaN operand forces a non-negative maximum.
        let positive = KnownFpClass::from_classes(FpClassTest::POSITIVE_NORMAL);
        let maximum = KnownFpClass::min_max_like(positive, no_nan, MinMaxKind::MaxNum, ieee);
        assert!(maximum.is_known_never(KnownFpClass::ORDERED_LESS_THAN_ZERO));
    }

    /// `KnownFPClass::canonicalize` — the one operation guaranteed to quieten a
    /// signaling NaN.
    #[test]
    fn canonicalize_quietens_signaling_nans() {
        let signaling = KnownFpClass::from_classes(FpClassTest::SIGNALING_NAN);
        let canonical = KnownFpClass::canonicalize(signaling, DenormalMode::ieee());
        assert!(canonical.is_known_never(FpClassTest::SIGNALING_NAN));
        assert!(!canonical.is_known_never_nan(), "a quiet NaN remains");

        // A non-NaN source stays non-NaN.
        let finite = KnownFpClass::from_classes(FpClassTest::FINITE);
        assert!(KnownFpClass::canonicalize(finite, DenormalMode::ieee()).is_known_never_nan());

        // A flushing mode rules out subnormals.
        let flushing = KnownFpClass::canonicalize(finite, DenormalMode::preserve_sign());
        assert!(flushing.is_known_never(FpClassTest::SUBNORMAL));
    }

    /// `KnownFPClass::propagateDenormal` — a source that could be a denormal
    /// gains a zero once the mode flushes.
    #[test]
    fn propagate_denormal_admits_a_flushed_zero() {
        // Never a literal zero, but subnormals are possible.
        let source = KnownFpClass::from_classes(FpClassTest::SUBNORMAL);

        let mut under_ieee = KnownFpClass::unknown();
        under_ieee.propagate_denormal(source, DenormalMode::ieee());
        assert!(under_ieee.is_known_never_zero(), "IEEE flushes nothing");

        let mut flushing = KnownFpClass::unknown();
        flushing.propagate_denormal(source, DenormalMode::preserve_sign());
        assert!(!flushing.is_known_never_zero());
    }

    /// `KnownFPClass::KnownFPClass(const APFloat &C)` — a constant pins both
    /// the class and the sign.
    #[test]
    fn a_constant_pins_class_and_sign() {
        let minus_one = parse("-1");
        let known = KnownFpClass::of(&minus_one);
        assert_eq!(known.classes(), FpClassTest::NEGATIVE_NORMAL);
        assert_eq!(known.sign_bit(), Some(true));
        assert!(known.is_known_never_nan());
        assert!(!known.cannot_be_ordered_less_than_zero());
    }
}
