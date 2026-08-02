//! Half-open integer ranges. Mirrors `llvm/include/llvm/IR/ConstantRange.h`.

use core::cmp::Ordering;
use core::ops::Not;

use crate::ApInt;
use crate::cmp_predicate::IntPredicate;
use crate::constant::ConstantData;
use crate::error::{IrError, IrResult};
use crate::known_bits::KnownBits;
use crate::metadata::{MetadataKind, MetadataSlot, MetadataStore};
use crate::module::ModuleCore;
use crate::r#type::{TypeData, TypeSlot};
use crate::value::{ValueKindData, ValueSlot};

/// Which over-approximation to prefer when a set operation's exact answer
/// needs two disjoint runs and only one `[lower, upper)` can be returned.
///
/// Mirrors `ConstantRange::PreferredRangeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreferredRangeType {
    /// Whichever candidate holds fewer values.
    Smallest,
    /// Prefer a candidate that does not wrap in the unsigned domain, falling
    /// back to the smaller one when both or neither wrap.
    Unsigned,
    /// Prefer a candidate that does not wrap in the signed domain, falling
    /// back to the smaller one when both or neither wrap.
    Signed,
}

/// Ports the file-local `getPreferredRange` helper in `ConstantRange.cpp`.
fn preferred_range(
    first: &ConstantRange,
    second: &ConstantRange,
    preferred: PreferredRangeType,
) -> ConstantRange {
    match preferred {
        PreferredRangeType::Unsigned => {
            if !first.is_wrapped_set() && second.is_wrapped_set() {
                return first.clone();
            }
            if first.is_wrapped_set() && !second.is_wrapped_set() {
                return second.clone();
            }
        }
        PreferredRangeType::Signed => {
            if !first.is_sign_wrapped_set() && second.is_sign_wrapped_set() {
                return first.clone();
            }
            if first.is_sign_wrapped_set() && !second.is_sign_wrapped_set() {
                return second.clone();
            }
        }
        PreferredRangeType::Smallest => {}
    }

    if first.is_size_strictly_smaller_than(second) {
        first.clone()
    } else {
        second.clone()
    }
}

/// The `icmp` a [`ConstantRange`] is equivalent to, as returned by
/// [`ConstantRange::equivalent_icmp_with_offset`].
///
/// Upstream's `getEquivalentICmp` fills three out-parameters; a Rust caller
/// wants all three together, so they are returned as one value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EquivalentICmp {
    /// The comparison predicate.
    pub predicate: IntPredicate,
    /// The value to compare against.
    pub rhs: ApInt,
    /// Added to the compared value before the comparison. Zero for every range
    /// whose shape maps onto a predicate directly; non-zero only when the
    /// range had to be shifted down to start at zero.
    pub offset: ApInt,
}

/// Estimate a lower bound for the bit-masked AND of two ranges.
///
/// Ports the file-local `estimateBitMaskedAndLowerBound` in
/// `ConstantRange.cpp`. The idea is that the high bits both ranges hold
/// constant across all their members survive the AND, so a prefix of the
/// smaller endpoint is a sound floor.
fn estimate_bit_masked_and_lower_bound(lhs: &ConstantRange, rhs: &ConstantRange) -> ApInt {
    let bit_width = lhs.bit_width();
    // A full or unsigned-wrapped range contains zero, and `x & 0` is zero, so
    // nothing above zero can be guaranteed.
    if lhs.is_full_set() || rhs.is_full_set() || lhs.is_wrapped_set() || rhs.is_wrapped_set() {
        return ApInt::zero(bit_width);
    }

    let one_v = one(bit_width);
    let lhs_lo = lhs.lower().clone();
    let lhs_hi = lhs.upper().wrapping_sub(&one_v);
    let rhs_lo = rhs.lower().clone();
    let rhs_hi = rhs.upper().wrapping_sub(&one_v);

    // Bits that are equal within each range *and* equal across the two.
    let mut mask = lhs_lo
        .bitxor(&lhs_hi)
        .bitor(&rhs_lo.bitxor(&rhs_hi))
        .bitor(&lhs_lo.bitxor(&rhs_lo))
        .not();
    let leading_ones = mask.count_leading_ones();
    mask.clear_low_bits(bit_width - leading_ones);

    let estimate_bound = |mut a_lo: ApInt, b_lo: &ApInt, b_hi: &ApInt| -> ApInt {
        let leading_ones = b_lo.bitand(b_hi).bitor(&mask).count_leading_ones();
        a_lo.clear_low_bits(bit_width - leading_ones);
        a_lo
    };

    let by_lhs = estimate_bound(lhs_lo.clone(), &rhs_lo, &rhs_hi);
    let by_rhs = estimate_bound(rhs_lo, &lhs_lo, &lhs_hi);
    if by_lhs.ugt(&by_rhs) { by_lhs } else { by_rhs }
}

/// Trailing-zero counts over a non-wrapped, non-empty `[lower, upper)`.
///
/// Ports the file-local `getUnsignedCountTrailingZerosRange`. Upstream asserts
/// both preconditions; every caller here establishes them, and a violation
/// falls back to the full set rather than misreporting.
fn unsigned_count_trailing_zeros_range(lower: &ApInt, upper: &ApInt) -> ConstantRange {
    let bit_width = lower.bit_width();
    let one_v = one(bit_width);
    let count = |v: u32| ApInt::from_words(bit_width, &[u64::from(v)]);

    if lower.eq_ap_int(upper) {
        return ConstantRange::full(bit_width);
    }
    if lower.wrapping_add(&one_v).eq_ap_int(upper) {
        // One member, so the count is exact.
        return ConstantRange::single(count(lower.count_trailing_zeros()));
    }
    if lower.is_zero() {
        // Zero contributes the full width, and everything below it is
        // reachable.
        return ConstantRange::new(ApInt::zero(bit_width), count(bit_width + 1))
            .unwrap_or_else(|_| ConstantRange::full(bit_width));
    }

    // The bits both endpoints agree on cannot be where the lowest set bit
    // moves, so the longest common prefix bounds the count.
    let common_prefix_length = lower
        .bitxor(&upper.wrapping_sub(&one_v))
        .count_leading_zeros();
    let highest = (bit_width - common_prefix_length - 1).max(lower.count_trailing_zeros());
    ConstantRange::new(ApInt::zero(bit_width), count(highest + 1))
        .unwrap_or_else(|_| ConstantRange::full(bit_width))
}

/// Population counts over a non-wrapped, non-empty `[lower, upper)`.
///
/// Ports the file-local `getUnsignedPopCountRange`.
fn unsigned_pop_count_range(lower: &ApInt, upper: &ApInt) -> ConstantRange {
    let bit_width = lower.bit_width();
    let one_v = one(bit_width);
    let count = |v: u32| ApInt::from_words(bit_width, &[u64::from(v)]);

    if lower.eq_ap_int(upper) {
        return ConstantRange::full(bit_width);
    }
    if lower.wrapping_add(&one_v).eq_ap_int(upper) {
        return ConstantRange::single(count(lower.popcount()));
    }

    let max = upper.wrapping_sub(&one_v);
    let common_prefix_length = lower.bitxor(&max).count_leading_zeros();
    let prefix_pop_count = lower.hi_bits(common_prefix_length).popcount();
    // If the lower endpoint is the prefix followed by zeros, the prefix's own
    // population is attainable; otherwise at least one more bit is set.
    let min_bits = prefix_pop_count
        + u32::from(lower.count_trailing_zeros() < bit_width - common_prefix_length);
    // Symmetrically at the top: a max of prefix-then-ones attains the full
    // remaining width, otherwise one less.
    let max_bits = prefix_pop_count + (bit_width - common_prefix_length)
        - u32::from(max.count_trailing_ones() < bit_width - common_prefix_length);
    ConstantRange::new(count(min_bits), count(max_bits + 1))
        .unwrap_or_else(|_| ConstantRange::full(bit_width))
}

/// The eight bounds every saturating operation picks its endpoints from.
///
/// Named rather than passed as a tuple so the pairings in each operation read
/// as what they are — `self_umin` with `other_umax` for a subtraction, for
/// instance, because subtraction decreases in its right operand.
struct SaturatingBounds {
    self_umin: ApInt,
    self_umax: ApInt,
    self_smin: ApInt,
    self_smax: ApInt,
    other_umin: ApInt,
    other_umax: ApInt,
    other_smin: ApInt,
    other_smax: ApInt,
}

/// Which of the four min/max operations `ConstantRange::min_max` is running.
///
/// Upstream writes `smax`, `smin`, `umax` and `umin` as four near-identical
/// functions; this names the two axes they differ on so the body can be
/// written once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MinMaxKind {
    SignedMax,
    SignedMin,
    UnsignedMax,
    UnsignedMin,
}

/// Half-open range `[lower, upper)` over a fixed-width integer domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstantRange {
    lower: ApInt,
    upper: ApInt,
}

impl ConstantRange {
    /// A range over `[lower, upper)`.
    ///
    /// Equal endpoints are the encoding for the two degenerate sets — all-zero
    /// is empty, all-ones is full — so any *other* equal pair is rejected. It
    /// would describe a range containing nothing while answering `false` to
    /// both `is_empty_set` and `is_full_set`, and every predicate downstream
    /// reads those two. Upstream asserts the same invariant in
    /// `ConstantRange::ConstantRange`; this crate has no runtime asserts in
    /// production paths, so it is an error return instead. Use
    /// [`Self::non_empty`] to read an equal pair as the full set.
    pub fn new(lower: ApInt, upper: ApInt) -> IrResult<Self> {
        if lower.bit_width() != upper.bit_width() {
            return Err(IrError::OperandWidthMismatch {
                lhs: lower.bit_width(),
                rhs: upper.bit_width(),
            });
        }
        if lower.eq_ap_int(&upper) && !lower.is_min_value() && !lower.is_max_value() {
            return Err(IrError::DegenerateConstantRange {
                value: lower.to_string_radix(10, crate::ap_int::ApIntSignedness::Unsigned),
                bit_width: lower.bit_width(),
            });
        }
        Ok(Self { lower, upper })
    }

    #[inline]
    pub fn full(bit_width: u32) -> Self {
        let max = ApInt::max_value(bit_width);
        Self {
            lower: max.clone(),
            upper: max,
        }
    }

    #[inline]
    pub fn empty(bit_width: u32) -> Self {
        Self {
            lower: ApInt::zero(bit_width),
            upper: ApInt::zero(bit_width),
        }
    }

    #[inline]
    pub fn bit_width(&self) -> u32 {
        self.lower.bit_width()
    }

    #[inline]
    pub fn lower(&self) -> &ApInt {
        &self.lower
    }

    #[inline]
    pub fn upper(&self) -> &ApInt {
        &self.upper
    }

    #[inline]
    pub fn is_full_set(&self) -> bool {
        self.lower.eq_ap_int(&self.upper) && self.lower.is_max_value()
    }

    #[inline]
    pub fn is_empty_set(&self) -> bool {
        self.lower.eq_ap_int(&self.upper) && self.lower.is_min_value()
    }

    #[inline]
    pub fn is_wrapped_set(&self) -> bool {
        !self.is_full_set()
            && !self.is_empty_set()
            && self.lower.ugt(&self.upper)
            && !self.upper.is_min_value()
    }

    #[inline]
    pub fn is_upper_wrapped(&self) -> bool {
        !self.is_full_set() && !self.is_empty_set() && self.lower.ugt(&self.upper)
    }

    pub fn contains(&self, value: &ApInt) -> bool {
        if value.bit_width() != self.bit_width() || self.is_empty_set() {
            return false;
        }
        if self.is_full_set() {
            return true;
        }
        if self.is_upper_wrapped() {
            value.uge(&self.lower) || value.ult(&self.upper)
        } else {
            value.uge(&self.lower) && value.ult(&self.upper)
        }
    }

    /// Smallest unsigned value contained by this range.
    pub fn unsigned_min(&self) -> ApInt {
        if self.is_empty_set() || self.is_full_set() || self.is_wrapped_set() {
            ApInt::zero(self.bit_width())
        } else {
            self.lower.clone()
        }
    }

    /// Largest unsigned value contained by this range.
    pub fn unsigned_max(&self) -> ApInt {
        if self.is_empty_set() || self.is_full_set() || self.is_upper_wrapped() {
            ApInt::max_value(self.bit_width())
        } else {
            self.upper.wrapping_sub(&one(self.bit_width()))
        }
    }

    /// A range over `[lower, upper)` that reads an equal pair as *full* rather
    /// than empty. Mirrors `ConstantRange::getNonEmpty`.
    ///
    /// Infallible where [`Self::new`] is fallible: `getNonEmpty` is only ever
    /// called with two endpoints of the same width, and equal endpoints — the
    /// one case that would be ambiguous — are resolved to the full set.
    pub fn non_empty(lower: ApInt, upper: ApInt) -> IrResult<Self> {
        if lower.eq_ap_int(&upper) {
            return Ok(Self::full(lower.bit_width()));
        }
        Self::new(lower, upper)
    }

    /// Smallest signed value contained by this range. Mirrors
    /// `ConstantRange::getSignedMin`.
    pub fn signed_min(&self) -> ApInt {
        if self.is_full_set() || self.is_sign_wrapped_set() {
            ApInt::signed_min_value(self.bit_width())
        } else {
            self.lower.clone()
        }
    }

    /// Largest signed value contained by this range. Mirrors
    /// `ConstantRange::getSignedMax`.
    pub fn signed_max(&self) -> ApInt {
        if self.is_full_set() || self.is_upper_sign_wrapped() {
            ApInt::signed_max_value(self.bit_width())
        } else {
            self.upper.wrapping_sub(&one(self.bit_width()))
        }
    }

    /// True when the range wraps around the *signed* domain's edge. Mirrors
    /// `ConstantRange::isSignWrappedSet`.
    #[inline]
    pub fn is_sign_wrapped_set(&self) -> bool {
        self.lower.sgt(&self.upper) && !self.upper.is_min_signed_value()
    }

    /// As [`Self::is_sign_wrapped_set`], but counts the range whose upper
    /// endpoint is exactly the signed minimum. Mirrors
    /// `ConstantRange::isUpperSignWrapped`.
    #[inline]
    pub fn is_upper_sign_wrapped(&self) -> bool {
        self.lower.sgt(&self.upper)
    }

    /// The sole member, when the range holds exactly one. Mirrors
    /// `ConstantRange::getSingleElement`.
    pub fn single_element(&self) -> Option<&ApInt> {
        self.lower
            .wrapping_add(&one(self.bit_width()))
            .eq_ap_int(&self.upper)
            .then_some(&self.lower)
    }

    /// The sole *non*-member, when the range holds all but one. Mirrors
    /// `ConstantRange::getSingleMissingElement`.
    pub fn single_missing_element(&self) -> Option<&ApInt> {
        self.upper
            .wrapping_add(&one(self.bit_width()))
            .eq_ap_int(&self.lower)
            .then_some(&self.upper)
    }

    /// Mirrors `ConstantRange::isSingleElement`.
    #[inline]
    pub fn is_single_element(&self) -> bool {
        self.single_element().is_some()
    }

    /// Mirrors `ConstantRange::isSizeStrictlySmallerThan`. Answers `false` on a
    /// width mismatch, where upstream asserts.
    pub fn is_size_strictly_smaller_than(&self, other: &Self) -> bool {
        if self.bit_width() != other.bit_width() {
            return false;
        }
        if self.is_full_set() {
            return false;
        }
        if other.is_full_set() {
            return true;
        }
        self.upper
            .wrapping_sub(&self.lower)
            .ult(&other.upper.wrapping_sub(&other.lower))
    }

    /// Mirrors `ConstantRange::isSizeLargerThan`.
    ///
    /// The full set needs the special case upstream gives it: its size is
    /// `2^bit_width`, one more than the domain can name, so comparing the
    /// stored endpoints would answer zero.
    pub fn is_size_larger_than(&self, max_size: u64) -> bool {
        if self.is_full_set() {
            return max_size == 0
                || ApInt::max_value(self.bit_width()).unsigned_cmp_u64(max_size - 1)
                    == Ordering::Greater;
        }
        self.upper
            .wrapping_sub(&self.lower)
            .unsigned_cmp_u64(max_size)
            == Ordering::Greater
    }

    /// Mirrors `ConstantRange::isAllNegative`. The empty set is vacuously all
    /// negative; the full set is not.
    pub fn is_all_negative(&self) -> bool {
        if self.is_empty_set() {
            return true;
        }
        if self.is_full_set() {
            return false;
        }
        !self.is_upper_sign_wrapped() && !self.upper.is_strictly_positive()
    }

    /// Mirrors `ConstantRange::isAllNonNegative`. Empty and full are both
    /// handled by the two conditions without a special case.
    #[inline]
    pub fn is_all_non_negative(&self) -> bool {
        !self.is_sign_wrapped_set() && self.lower.is_non_negative()
    }

    /// Mirrors `ConstantRange::isAllPositive`.
    pub fn is_all_positive(&self) -> bool {
        if self.is_empty_set() {
            return true;
        }
        if self.is_full_set() {
            return false;
        }
        !self.is_sign_wrapped_set() && self.lower.is_strictly_positive()
    }

    /// Bits needed to represent every member unsigned. Mirrors
    /// `ConstantRange::getActiveBits`.
    pub fn active_bits(&self) -> u32 {
        if self.is_empty_set() {
            return 0;
        }
        self.unsigned_max().active_bits()
    }

    /// Bits needed to represent every member signed. Mirrors
    /// `ConstantRange::getMinSignedBits`.
    pub fn min_signed_bits(&self) -> u32 {
        if self.is_empty_set() {
            return 0;
        }
        self.signed_min()
            .significant_bits()
            .max(self.signed_max().significant_bits())
    }

    /// The range a known-bits constraint describes. Mirrors
    /// `ConstantRange::fromKnownBits`.
    ///
    /// `is_signed` picks which domain the result must not wrap in.
    pub fn from_known_bits(known: &KnownBits, is_signed: bool) -> Self {
        let bit_width = known.bit_width();
        if known.has_conflict() {
            return Self::empty(bit_width);
        }
        if known.is_unknown() {
            return Self::full(bit_width);
        }

        let min = known.min_value();
        let max = known.max_value();
        // Unsigned, or signed with the sign bit already known: a plain range
        // between the extremes.
        if !is_signed || known.is_negative() || known.is_non_negative() {
            return Self::new(min, max.wrapping_add(&one(bit_width)))
                .unwrap_or_else(|_| Self::full(bit_width));
        }

        // Sign unknown: take the lower bound negative and the upper bound
        // non-negative, so the range does not wrap in the signed domain.
        let mut lower = min;
        let mut upper = max;
        lower.set_sign_bit();
        upper.clear_sign_bit();
        Self::new(lower, upper.wrapping_add(&one(bit_width)))
            .unwrap_or_else(|_| Self::full(bit_width))
    }

    /// The known bits every member shares. Mirrors
    /// `ConstantRange::toKnownBits`.
    ///
    /// Only the leading bits that agree between the unsigned minimum and
    /// maximum can be retained. Upstream notes it could return conflicting
    /// known bits for the empty set but does not, because consumers are not
    /// prepared for that; this keeps that choice.
    pub fn to_known_bits(&self) -> KnownBits {
        if self.is_empty_set() {
            return KnownBits::unknown(self.bit_width());
        }
        let min = self.unsigned_min();
        let max = self.unsigned_max();
        let known = KnownBits::make_constant(min.clone());
        let Some(different_bit) = ApInt::most_significant_different_bit(&min, &max) else {
            return known;
        };
        let mut zero = known.zero_mask().clone();
        let mut one_mask = known.one_mask().clone();
        zero.clear_low_bits(different_bit + 1);
        one_mask.clear_low_bits(different_bit + 1);
        KnownBits::from_zero_one(zero, one_mask).unwrap_or_else(|_| {
            // Unreachable by construction: both masks came from a well-formed
            // KnownBits of this width and only had bits cleared.
            KnownBits::unknown(self.bit_width())
        })
    }

    /// The complement of this range. Mirrors `ConstantRange::inverse`.
    pub fn inverse(&self) -> Self {
        if self.is_full_set() {
            return Self::empty(self.bit_width());
        }
        if self.is_empty_set() {
            return Self::full(self.bit_width());
        }
        // Swapping the endpoints of a non-degenerate range is exactly its
        // complement, and cannot itself be degenerate.
        Self {
            lower: self.upper.clone(),
            upper: self.lower.clone(),
        }
    }

    /// This range shifted down by `value`. Mirrors `ConstantRange::subtract`.
    ///
    /// Empty and full are left alone: their endpoints are an encoding rather
    /// than a position, so translating them would change which set they name.
    pub fn subtract(&self, value: &ApInt) -> Self {
        if value.bit_width() != self.bit_width() || self.lower.eq_ap_int(&self.upper) {
            return self.clone();
        }
        Self {
            lower: self.lower.wrapping_sub(value),
            upper: self.upper.wrapping_sub(value),
        }
    }

    /// The part of this range not in `other`. Mirrors
    /// `ConstantRange::difference`.
    pub fn difference(&self, other: &Self) -> Self {
        self.intersect_with(&other.inverse(), PreferredRangeType::Smallest)
    }

    /// This range split into its strictly-positive and negative halves.
    /// Mirrors `ConstantRange::splitPosNeg`.
    ///
    /// There are no positive 1-bit values — the lone 1 reads as -1 — so the
    /// positive half is empty at that width, exactly as upstream notes.
    pub fn split_pos_neg(&self) -> (Self, Self) {
        let bit_width = self.bit_width();
        let zero = ApInt::zero(bit_width);
        let signed_min = ApInt::signed_min_value(bit_width);
        let positive_filter = if bit_width == 1 {
            Self::empty(bit_width)
        } else {
            Self::new(one(bit_width), signed_min.clone()).unwrap_or_else(|_| Self::full(bit_width))
        };
        let negative_filter = Self::new(signed_min, zero).unwrap_or_else(|_| Self::full(bit_width));
        (
            self.intersect_with(&positive_filter, PreferredRangeType::Smallest),
            self.intersect_with(&negative_filter, PreferredRangeType::Smallest),
        )
    }

    /// The intersection of two ranges.
    ///
    /// Mirrors `ConstantRange::intersectWith`. When the true intersection is
    /// disjoint — two runs that cannot both be named by one `[lower, upper)` —
    /// `preferred` picks which over-approximation to return. Answers the empty
    /// set on a width mismatch, where upstream asserts.
    pub fn intersect_with(&self, other: &Self, preferred: PreferredRangeType) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::empty(self.bit_width());
        }

        // Common cases.
        if self.is_empty_set() || other.is_full_set() {
            return self.clone();
        }
        if other.is_empty_set() || self.is_full_set() {
            return other.clone();
        }

        // Normalise so that a wrapped range is never on the right alone.
        if !self.is_upper_wrapped() && other.is_upper_wrapped() {
            return other.intersect_with(self, preferred);
        }

        let bit_width = self.bit_width();
        let range = |lower: ApInt, upper: ApInt| {
            Self::new(lower, upper).unwrap_or_else(|_| Self::full(bit_width))
        };

        if !self.is_upper_wrapped() && !other.is_upper_wrapped() {
            if self.lower.ult(&other.lower) {
                // L---U       : self
                //       L---U : other
                if self.upper.ule(&other.lower) {
                    return Self::empty(bit_width);
                }
                // L---U       : self
                //   L---U     : other
                if self.upper.ult(&other.upper) {
                    return range(other.lower.clone(), self.upper.clone());
                }
                // L-------U   : self
                //   L---U     : other
                return other.clone();
            }
            //   L---U     : self
            // L-------U   : other
            if self.upper.ult(&other.upper) {
                return self.clone();
            }
            //   L-----U   : self
            // L-----U     : other
            if self.lower.ult(&other.upper) {
                return range(self.lower.clone(), other.upper.clone());
            }
            //       L---U : self
            // L---U       : other
            return Self::empty(bit_width);
        }

        if self.is_upper_wrapped() && !other.is_upper_wrapped() {
            if other.lower.ult(&self.upper) {
                // ------U   L--- : self
                //  L--U          : other
                if other.upper.ult(&self.upper) {
                    return other.clone();
                }
                // ------U   L--- : self
                //  L------U      : other
                if other.upper.ule(&self.lower) {
                    return range(other.lower.clone(), self.upper.clone());
                }
                // ------U   L--- : self
                //  L----------U  : other
                return preferred_range(self, other, preferred);
            }
            if other.lower.ult(&self.lower) {
                // --U      L---- : self
                //     L--U       : other
                if other.upper.ule(&self.lower) {
                    return Self::empty(bit_width);
                }
                // --U      L---- : self
                //     L------U   : other
                return range(self.lower.clone(), other.upper.clone());
            }
            // --U  L------ : self
            //        L--U  : other
            return other.clone();
        }

        // Both wrapped.
        if other.upper.ult(&self.upper) {
            // ------U L-- : self
            // --U L------ : other
            if other.lower.ult(&self.upper) {
                return preferred_range(self, other, preferred);
            }
            // ----U   L-- : self
            // --U   L---- : other
            if other.lower.ult(&self.lower) {
                return range(self.lower.clone(), other.upper.clone());
            }
            // ----U L---- : self
            // --U     L-- : other
            return other.clone();
        }
        if other.upper.ule(&self.lower) {
            // --U     L-- : self
            // ----U L---- : other
            if other.lower.ult(&self.lower) {
                return self.clone();
            }
            // --U   L---- : self
            // ----U   L-- : other
            return range(other.lower.clone(), self.upper.clone());
        }
        // --U L------ : self
        // ------U L-- : other
        preferred_range(self, other, preferred)
    }

    /// The smallest range containing both. Mirrors `ConstantRange::unionWith`.
    ///
    /// Answers the full set on a width mismatch, where upstream asserts —
    /// a union can only ever grow, so full is the sound answer.
    pub fn union_with(&self, other: &Self, preferred: PreferredRangeType) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }

        if self.is_full_set() || other.is_empty_set() {
            return self.clone();
        }
        if other.is_full_set() || self.is_empty_set() {
            return other.clone();
        }

        if !self.is_upper_wrapped() && other.is_upper_wrapped() {
            return other.union_with(self, preferred);
        }

        let bit_width = self.bit_width();
        let one_v = one(bit_width);
        let range = |lower: ApInt, upper: ApInt| {
            Self::new(lower, upper).unwrap_or_else(|_| Self::full(bit_width))
        };

        if !self.is_upper_wrapped() && !other.is_upper_wrapped() {
            //        L---U  and  L---U        : self
            //  L---U                   L---U  : other
            if other.upper.ult(&self.lower) || self.upper.ult(&other.lower) {
                return preferred_range(
                    &range(self.lower.clone(), other.upper.clone()),
                    &range(other.lower.clone(), self.upper.clone()),
                    preferred,
                );
            }

            let lower = if other.lower.ult(&self.lower) {
                other.lower.clone()
            } else {
                self.lower.clone()
            };
            let upper = if other
                .upper
                .wrapping_sub(&one_v)
                .ugt(&self.upper.wrapping_sub(&one_v))
            {
                other.upper.clone()
            } else {
                self.upper.clone()
            };

            // Both endpoints landing on zero means the union wrapped all the
            // way round, which is the full set rather than the empty one.
            if lower.is_zero() && upper.is_zero() {
                return Self::full(bit_width);
            }
            return range(lower, upper);
        }

        if !other.is_upper_wrapped() {
            // ------U   L-----  and  ------U   L----- : self
            //   L--U                            L--U  : other
            if other.upper.ule(&self.upper) || other.lower.uge(&self.lower) {
                return self.clone();
            }
            // ------U   L----- : self
            //    L---------U   : other
            if other.lower.ule(&self.upper) && self.lower.ule(&other.upper) {
                return Self::full(bit_width);
            }
            // ----U       L---- : self
            //       L---U       : other
            if self.upper.ult(&other.lower) && other.upper.ult(&self.lower) {
                return preferred_range(
                    &range(self.lower.clone(), other.upper.clone()),
                    &range(other.lower.clone(), self.upper.clone()),
                    preferred,
                );
            }
            // ----U     L----- : self
            //        L----U    : other
            if self.upper.ult(&other.lower) && self.lower.ule(&other.upper) {
                return range(other.lower.clone(), self.upper.clone());
            }
            // ------U    L---- : self
            //    L-----U       : other
            //
            // Upstream asserts `other.lower <= self.upper && other.upper <
            // self.lower` here — the only case left once the four above are
            // ruled out.
            return range(self.lower.clone(), other.upper.clone());
        }

        // Both wrapped.
        // ------U    L----  and  ------U    L---- : self
        // -U  L-----------  and  ------------U  L : other
        if other.lower.ule(&self.upper) || self.lower.ule(&other.upper) {
            return Self::full(bit_width);
        }

        let lower = if other.lower.ult(&self.lower) {
            other.lower.clone()
        } else {
            self.lower.clone()
        };
        let upper = if other.upper.ugt(&self.upper) {
            other.upper.clone()
        } else {
            self.upper.clone()
        };
        range(lower, upper)
    }

    /// Zero-extend every member to `dst_bit_width`. Mirrors
    /// `ConstantRange::zeroExtend`.
    ///
    /// Narrowing is rejected rather than asserted: upstream's
    /// `assert(SrcTySize < DstTySize)` has no run-time counterpart here, and
    /// silently truncating would be worse than declining.
    pub fn zero_extend(&self, dst_bit_width: u32) -> IrResult<Self> {
        let src_bit_width = self.bit_width();
        if self.is_empty_set() {
            return Ok(Self::empty(dst_bit_width));
        }
        if dst_bit_width == src_bit_width {
            return Ok(self.clone());
        }
        if dst_bit_width < src_bit_width {
            return Err(IrError::OperandWidthMismatch {
                lhs: dst_bit_width,
                rhs: src_bit_width,
            });
        }

        if self.is_full_set() || self.is_upper_wrapped() {
            // Becomes [0, 1 << src_bit_width), except that `[X, 0)` is not
            // really wrapping and keeps its lower endpoint.
            let lower = if self.upper.is_zero() {
                self.lower
                    .zext(dst_bit_width)
                    .unwrap_or_else(|| ApInt::zero(dst_bit_width))
            } else {
                ApInt::zero(dst_bit_width)
            };
            return Self::new(lower, ApInt::one_bit_set(dst_bit_width, src_bit_width));
        }

        Self::new(
            self.lower
                .zext(dst_bit_width)
                .unwrap_or_else(|| ApInt::zero(dst_bit_width)),
            self.upper
                .zext(dst_bit_width)
                .unwrap_or_else(|| ApInt::zero(dst_bit_width)),
        )
    }

    /// Sign-extend every member to `dst_bit_width`. Mirrors
    /// `ConstantRange::signExtend`.
    pub fn sign_extend(&self, dst_bit_width: u32) -> IrResult<Self> {
        let src_bit_width = self.bit_width();
        if self.is_empty_set() {
            return Ok(Self::empty(dst_bit_width));
        }
        if dst_bit_width == src_bit_width {
            return Ok(self.clone());
        }
        if dst_bit_width < src_bit_width {
            return Err(IrError::OperandWidthMismatch {
                lhs: dst_bit_width,
                rhs: src_bit_width,
            });
        }
        let widen_s = |v: &ApInt| {
            v.sext(dst_bit_width)
                .unwrap_or_else(|| ApInt::zero(dst_bit_width))
        };
        let widen_z = |v: &ApInt| {
            v.zext(dst_bit_width)
                .unwrap_or_else(|| ApInt::zero(dst_bit_width))
        };

        // `[X, INT_MIN)` is not really wrapping around.
        if self.upper.is_min_signed_value() {
            return Self::new(widen_s(&self.lower), widen_z(&self.upper));
        }

        if self.is_full_set() || self.is_sign_wrapped_set() {
            return Self::new(
                ApInt::high_bits_set(dst_bit_width, dst_bit_width - src_bit_width + 1),
                ApInt::low_bits_set(dst_bit_width, src_bit_width - 1)
                    .wrapping_add(&one(dst_bit_width)),
            );
        }

        Self::new(widen_s(&self.lower), widen_s(&self.upper))
    }

    /// Truncate every member to `dst_bit_width`. Mirrors
    /// `ConstantRange::truncate`.
    ///
    /// `no_unsigned_wrap` is upstream's `NoWrapKind & TruncInst::NoUnsignedWrap`
    /// — the `trunc nuw` promise that no member's high bits are set. llvmkit
    /// spells the single flag as a bool because that is the only kind
    /// `truncate` reads.
    pub fn truncate(&self, dst_bit_width: u32, no_unsigned_wrap: bool) -> IrResult<Self> {
        let src_bit_width = self.bit_width();
        if dst_bit_width == src_bit_width {
            return Ok(self.clone());
        }
        if dst_bit_width > src_bit_width {
            return Err(IrError::OperandWidthMismatch {
                lhs: dst_bit_width,
                rhs: src_bit_width,
            });
        }
        if self.is_empty_set() {
            return Ok(Self::empty(dst_bit_width));
        }
        if self.is_full_set() {
            return Ok(Self::full(dst_bit_width));
        }

        let narrow = |v: &ApInt| {
            v.trunc(dst_bit_width)
                .unwrap_or_else(|| ApInt::zero(dst_bit_width))
        };
        let mut lower_div = self.lower.clone();
        let mut upper_div = self.upper.clone();
        let mut union = Self::empty(dst_bit_width);

        // A wrapped set is analysed as its two parts: [0, upper) and
        // [lower, max]. The non-wrapped path handles the second, then the
        // first is unioned back in.
        if self.is_upper_wrapped() {
            // An upper past the destination's range covers everything.
            if self.upper.active_bits() > dst_bit_width {
                return Ok(Self::full(dst_bit_width));
            }

            if no_unsigned_wrap {
                union = Self::new(ApInt::zero(dst_bit_width), narrow(&self.upper))
                    .unwrap_or_else(|_| Self::full(dst_bit_width));
                upper_div = ApInt::one_bit_set(src_bit_width, dst_bit_width);
            } else {
                // An upper exactly at the destination's maximum likewise
                // covers everything.
                if self.upper.count_trailing_ones() == dst_bit_width {
                    return Ok(Self::full(dst_bit_width));
                }
                union = Self::new(ApInt::max_value(dst_bit_width), narrow(&self.upper))
                    .unwrap_or_else(|_| Self::full(dst_bit_width));
                upper_div = ApInt::max_value(src_bit_width);
                // The union already covers the maximum, so if nothing else is
                // left there is nothing to add.
                if lower_div.eq_ap_int(&upper_div) {
                    return Ok(union);
                }
            }
        }

        // Chop the bits above the destination width off both endpoints.
        if lower_div.active_bits() > dst_bit_width {
            // Under `nuw` a lower above the destination's maximum puts the
            // whole range outside it.
            if no_unsigned_wrap {
                return Ok(union);
            }
            let adjust = lower_div.bitand(&ApInt::bits_set_from(src_bit_width, dst_bit_width));
            lower_div = lower_div.wrapping_sub(&adjust);
            upper_div = upper_div.wrapping_sub(&adjust);
        }

        let upper_div_width = upper_div.active_bits();
        if upper_div_width <= dst_bit_width {
            return Ok(Self::new(narrow(&lower_div), narrow(&upper_div))
                .unwrap_or_else(|_| Self::full(dst_bit_width))
                .union_with(&union, PreferredRangeType::Smallest));
        }

        if !lower_div.is_zero() && no_unsigned_wrap {
            return Ok(Self::new(narrow(&lower_div), ApInt::zero(dst_bit_width))
                .unwrap_or_else(|_| Self::full(dst_bit_width))
                .union_with(&union, PreferredRangeType::Smallest));
        }

        // The truncated value wraps. One more chance to beat the full set:
        // clearing the bit just above the destination width may bring the
        // upper endpoint below the lower one, which is a legal wrapped range.
        if upper_div_width == dst_bit_width + 1 {
            upper_div.clear_bit(dst_bit_width);
            if upper_div.ult(&lower_div) {
                return Ok(Self::new(narrow(&lower_div), narrow(&upper_div))
                    .unwrap_or_else(|_| Self::full(dst_bit_width))
                    .union_with(&union, PreferredRangeType::Smallest));
            }
        }

        Ok(Self::full(dst_bit_width))
    }

    /// Zero-extend or truncate to `dst_bit_width`, whichever the widths call
    /// for. Mirrors `ConstantRange::zextOrTrunc`.
    pub fn zext_or_trunc(&self, dst_bit_width: u32) -> IrResult<Self> {
        match dst_bit_width.cmp(&self.bit_width()) {
            Ordering::Less => self.truncate(dst_bit_width, false),
            Ordering::Greater => self.zero_extend(dst_bit_width),
            Ordering::Equal => Ok(self.clone()),
        }
    }

    /// Sign-extend or truncate to `dst_bit_width`. Mirrors
    /// `ConstantRange::sextOrTrunc`.
    pub fn sext_or_trunc(&self, dst_bit_width: u32) -> IrResult<Self> {
        match dst_bit_width.cmp(&self.bit_width()) {
            Ordering::Less => self.truncate(dst_bit_width, false),
            Ordering::Greater => self.sign_extend(dst_bit_width),
            Ordering::Equal => Ok(self.clone()),
        }
    }

    /// The one-element range `[value, value + 1)`. Mirrors
    /// `ConstantRange::ConstantRange(APInt V)`.
    pub fn single(value: ApInt) -> Self {
        let upper = value.wrapping_add(&one(value.bit_width()));
        Self {
            lower: value,
            upper,
        }
    }

    /// True when every member of `other` is a member of this range. Mirrors
    /// the `ConstantRange` overload of `ConstantRange::contains`.
    pub fn contains_range(&self, other: &Self) -> bool {
        if self.bit_width() != other.bit_width() {
            return false;
        }
        if self.is_full_set() || other.is_empty_set() {
            return true;
        }
        if self.is_empty_set() || other.is_full_set() {
            return false;
        }

        if !self.is_upper_wrapped() {
            if other.is_upper_wrapped() {
                return false;
            }
            return self.lower.ule(&other.lower) && other.upper.ule(&self.upper);
        }

        if !other.is_upper_wrapped() {
            return other.upper.ule(&self.upper) || self.lower.ule(&other.lower);
        }

        other.upper.ule(&self.upper) && self.lower.ule(&other.lower)
    }

    /// The largest range of values that *may* satisfy `predicate` against some
    /// member of `other`. Mirrors `ConstantRange::makeAllowedICmpRegion`.
    ///
    /// "Allowed" is the weaker of the two questions: a value in the result
    /// compares true against *at least one* member of `other`. Compare
    /// [`Self::make_satisfying_icmp_region`], which demands *every* member.
    pub fn make_allowed_icmp_region(predicate: IntPredicate, other: &Self) -> Self {
        if other.is_empty_set() {
            return other.clone();
        }
        let width = other.bit_width();
        let one_v = one(width);
        let range = |lower: ApInt, upper: ApInt| {
            Self::new(lower, upper).unwrap_or_else(|_| Self::full(width))
        };
        let non_empty = |lower: ApInt, upper: ApInt| {
            Self::non_empty(lower, upper).unwrap_or_else(|_| Self::full(width))
        };

        match predicate {
            IntPredicate::Eq => other.clone(),
            IntPredicate::Ne => {
                if other.is_single_element() {
                    return range(other.upper.clone(), other.lower.clone());
                }
                Self::full(width)
            }
            IntPredicate::Ult => {
                let unsigned_max = other.unsigned_max();
                if unsigned_max.is_min_value() {
                    return Self::empty(width);
                }
                range(ApInt::min_value(width), unsigned_max)
            }
            IntPredicate::Slt => {
                let signed_max = other.signed_max();
                if signed_max.is_min_signed_value() {
                    return Self::empty(width);
                }
                range(ApInt::signed_min_value(width), signed_max)
            }
            IntPredicate::Ule => non_empty(
                ApInt::min_value(width),
                other.unsigned_max().wrapping_add(&one_v),
            ),
            IntPredicate::Sle => non_empty(
                ApInt::signed_min_value(width),
                other.signed_max().wrapping_add(&one_v),
            ),
            IntPredicate::Ugt => {
                let unsigned_min = other.unsigned_min();
                if unsigned_min.is_max_value() {
                    return Self::empty(width);
                }
                range(unsigned_min.wrapping_add(&one_v), ApInt::zero(width))
            }
            IntPredicate::Sgt => {
                let signed_min = other.signed_min();
                if signed_min.is_max_signed_value() {
                    return Self::empty(width);
                }
                range(
                    signed_min.wrapping_add(&one_v),
                    ApInt::signed_min_value(width),
                )
            }
            IntPredicate::Uge => non_empty(other.unsigned_min(), ApInt::zero(width)),
            IntPredicate::Sge => non_empty(other.signed_min(), ApInt::signed_min_value(width)),
        }
    }

    /// The largest range of values that satisfy `predicate` against *every*
    /// member of `other`. Mirrors `ConstantRange::makeSatisfyingICmpRegion`.
    ///
    /// Upstream derives it from the allowed region by De Morgan:
    /// `~(~A ∪ ~B) == A ∩ B`, which here is the inverse of the allowed region
    /// for the inverse predicate.
    pub fn make_satisfying_icmp_region(predicate: IntPredicate, other: &Self) -> Self {
        Self::make_allowed_icmp_region(predicate.inverse(), other).inverse()
    }

    /// The exact range of values satisfying `predicate` against the single
    /// value `value`. Mirrors `ConstantRange::makeExactICmpRegion`.
    ///
    /// Allowed and satisfying coincide when the right-hand side is a single
    /// value; they diverge only for a multi-element range — upstream's example
    /// is `ult [2,5)`, where allowed is `[0,4)` but satisfying is `[0,2)`.
    pub fn make_exact_icmp_region(predicate: IntPredicate, value: &ApInt) -> Self {
        Self::make_allowed_icmp_region(predicate, &Self::single(value.clone()))
    }

    /// The range of values `v` for which `(v & mask) != c` is satisfiable.
    /// Mirrors `ConstantRange::makeMaskNotEqualRange`.
    pub fn make_mask_not_equal_range(mask: &ApInt, c: &ApInt) -> Self {
        let bit_width = mask.bit_width();
        if !mask.bitand(c).eq_ap_int(c) {
            // `c` has a bit set outside the mask, so the equality can never
            // hold and every value satisfies the inequality.
            return Self::full(bit_width);
        }
        if mask.is_zero() {
            // `v & 0` is always 0, which by the check above equals `c`, so
            // nothing satisfies the inequality.
            return Self::empty(bit_width);
        }
        // Otherwise the value must exceed the mask's lowest set bit, offset
        // by `c`.
        Self::non_empty(
            ApInt::one_bit_set(bit_width, mask.count_trailing_zeros()).wrapping_add(c),
            c.clone(),
        )
        .unwrap_or_else(|_| Self::full(bit_width))
    }

    /// The `icmp` that describes this range, together with the offset that has
    /// to be added to the compared value first. Mirrors the three-argument
    /// `ConstantRange::getEquivalentICmp`.
    ///
    /// Upstream fills three out-parameters; llvmkit returns them, since a
    /// caller always wants all three together.
    pub fn equivalent_icmp_with_offset(&self) -> EquivalentICmp {
        let bit_width = self.bit_width();
        let zero = ApInt::zero(bit_width);

        if self.is_full_set() || self.is_empty_set() {
            return EquivalentICmp {
                predicate: if self.is_empty_set() {
                    // Nothing is unsigned-less-than zero.
                    IntPredicate::Ult
                } else {
                    // Everything is unsigned-greater-or-equal to zero.
                    IntPredicate::Uge
                },
                rhs: zero.clone(),
                offset: zero,
            };
        }
        if let Some(only) = self.single_element() {
            return EquivalentICmp {
                predicate: IntPredicate::Eq,
                rhs: only.clone(),
                offset: zero,
            };
        }
        if let Some(missing) = self.single_missing_element() {
            return EquivalentICmp {
                predicate: IntPredicate::Ne,
                rhs: missing.clone(),
                offset: zero,
            };
        }
        if self.lower.is_min_signed_value() || self.lower.is_min_value() {
            return EquivalentICmp {
                predicate: if self.lower.is_min_signed_value() {
                    IntPredicate::Slt
                } else {
                    IntPredicate::Ult
                },
                rhs: self.upper.clone(),
                offset: zero,
            };
        }
        if self.upper.is_min_signed_value() || self.upper.is_min_value() {
            return EquivalentICmp {
                predicate: if self.upper.is_min_signed_value() {
                    IntPredicate::Sge
                } else {
                    IntPredicate::Uge
                },
                rhs: self.lower.clone(),
                offset: zero,
            };
        }
        // A range with neither endpoint at a domain edge becomes an unsigned
        // compare against its width, once the value is shifted down to zero.
        EquivalentICmp {
            predicate: IntPredicate::Ult,
            rhs: self.upper.wrapping_sub(&self.lower),
            offset: zero.wrapping_sub(&self.lower),
        }
    }

    /// The `icmp` that describes this range exactly, when no offset is needed.
    /// Mirrors the two-argument `ConstantRange::getEquivalentICmp`, whose
    /// `bool` return says whether the offset came back zero.
    pub fn equivalent_icmp(&self) -> Option<(IntPredicate, ApInt)> {
        let equivalent = self.equivalent_icmp_with_offset();
        equivalent
            .offset
            .is_zero()
            .then_some((equivalent.predicate, equivalent.rhs))
    }

    /// True when `predicate` holds for *every* pairing of a member of this
    /// range with a member of `other`. Mirrors `ConstantRange::icmp`.
    ///
    /// Vacuously true when either range is empty, since there is no pairing to
    /// falsify it.
    pub fn icmp(&self, predicate: IntPredicate, other: &Self) -> bool {
        if self.is_empty_set() || other.is_empty_set() {
            return true;
        }
        match predicate {
            IntPredicate::Eq => match (self.single_element(), other.single_element()) {
                (Some(lhs), Some(rhs)) => lhs.eq_ap_int(rhs),
                _ => false,
            },
            IntPredicate::Ne => self.inverse().contains_range(other),
            IntPredicate::Ult => self.unsigned_max().ult(&other.unsigned_min()),
            IntPredicate::Ule => self.unsigned_max().ule(&other.unsigned_min()),
            IntPredicate::Ugt => self.unsigned_min().ugt(&other.unsigned_max()),
            IntPredicate::Uge => self.unsigned_min().uge(&other.unsigned_max()),
            IntPredicate::Slt => self.signed_max().slt(&other.signed_min()),
            IntPredicate::Sle => self.signed_max().sle(&other.signed_min()),
            IntPredicate::Sgt => self.signed_min().sgt(&other.signed_max()),
            IntPredicate::Sge => self.signed_min().sge(&other.signed_max()),
        }
    }

    /// Sum of every pairing. Mirrors `ConstantRange::add`.
    ///
    /// Endpoints add, then the result is checked for having wrapped past a
    /// full domain: a sum smaller than either input can only mean the true
    /// answer covers everything.
    pub fn add(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(self.bit_width());
        }
        if self.is_full_set() || other.is_full_set() {
            return Self::full(self.bit_width());
        }

        let bit_width = self.bit_width();
        let one_v = one(bit_width);
        let new_lower = self.lower.wrapping_add(&other.lower);
        let new_upper = self.upper.wrapping_add(&other.upper).wrapping_sub(&one_v);
        if new_lower.eq_ap_int(&new_upper) {
            return Self::full(bit_width);
        }
        let candidate = Self::new(new_lower, new_upper).unwrap_or_else(|_| Self::full(bit_width));
        if candidate.is_size_strictly_smaller_than(self)
            || candidate.is_size_strictly_smaller_than(other)
        {
            // Shrinking means we wrapped, so nothing narrower than full is
            // sound.
            return Self::full(bit_width);
        }
        candidate
    }

    /// Difference of every pairing. Mirrors `ConstantRange::sub`.
    pub fn sub(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(self.bit_width());
        }
        if self.is_full_set() || other.is_full_set() {
            return Self::full(self.bit_width());
        }

        let bit_width = self.bit_width();
        let one_v = one(bit_width);
        let new_lower = self.lower.wrapping_sub(&other.upper).wrapping_add(&one_v);
        let new_upper = self.upper.wrapping_sub(&other.lower);
        if new_lower.eq_ap_int(&new_upper) {
            return Self::full(bit_width);
        }
        let candidate = Self::new(new_lower, new_upper).unwrap_or_else(|_| Self::full(bit_width));
        if candidate.is_size_strictly_smaller_than(self)
            || candidate.is_size_strictly_smaller_than(other)
        {
            return Self::full(bit_width);
        }
        candidate
    }

    /// Product of every pairing. Mirrors `ConstantRange::multiply`.
    ///
    /// Multiplication is signedness-independent, but the *range* you get is
    /// not: reading the inputs as unsigned and as signed gives two different,
    /// both-correct answers. Upstream computes both at double width and
    /// returns the smaller; so does this.
    pub fn multiply(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(self.bit_width());
        }

        let bit_width = self.bit_width();
        let zero_range = Self::single(ApInt::zero(bit_width));

        // Multiplying by a single 1 or -1 is exact, so take it before the
        // double-width work.
        if let Some(c) = self.single_element() {
            if c.is_one() {
                return other.clone();
            }
            if c.is_all_ones() {
                return zero_range.sub(other);
            }
        }
        if let Some(c) = other.single_element() {
            if c.is_one() {
                return self.clone();
            }
            if c.is_all_ones() {
                return zero_range.sub(self);
            }
        }

        let Some(wide) = bit_width.checked_mul(2) else {
            return Self::full(bit_width);
        };
        let one_wide = one(wide);
        let widen_z = |v: ApInt| v.zext(wide).unwrap_or_else(|| ApInt::zero(wide));
        let widen_s = |v: ApInt| v.sext(wide).unwrap_or_else(|| ApInt::zero(wide));

        // Unsigned reading first.
        let unsigned = Self::new(
            widen_z(self.unsigned_min()).wrapping_mul(&widen_z(other.unsigned_min())),
            widen_z(self.unsigned_max())
                .wrapping_mul(&widen_z(other.unsigned_max()))
                .wrapping_add(&one_wide),
        )
        .unwrap_or_else(|_| Self::full(wide))
        .truncate(bit_width, false)
        .unwrap_or_else(|_| Self::full(bit_width));

        // A non-wrapping, non-negative unsigned answer is already as tight as
        // this can get, so skip the signed work.
        if !unsigned.is_upper_wrapped()
            && (unsigned.upper.is_non_negative() || unsigned.upper.is_min_signed_value())
        {
            return unsigned;
        }

        // Signed reading. With negatives in play the extremes are the min and
        // max over all four corner products, not just min×min and max×max —
        // upstream's example is [-1,4) * [-2,3), whose lowest product is
        // 3 * -2 = -6.
        let this_min = widen_s(self.signed_min());
        let this_max = widen_s(self.signed_max());
        let other_min = widen_s(other.signed_min());
        let other_max = widen_s(other.signed_max());
        let corners = [
            this_min.wrapping_mul(&other_min),
            this_min.wrapping_mul(&other_max),
            this_max.wrapping_mul(&other_min),
            this_max.wrapping_mul(&other_max),
        ];
        let mut lowest = corners[0].clone();
        let mut highest = corners[0].clone();
        for corner in &corners[1..] {
            if corner.slt(&lowest) {
                lowest = corner.clone();
            }
            if highest.slt(corner) {
                highest = corner.clone();
            }
        }
        let signed = Self::new(lowest, highest.wrapping_add(&one_wide))
            .unwrap_or_else(|_| Self::full(wide))
            .truncate(bit_width, false)
            .unwrap_or_else(|_| Self::full(bit_width));

        if unsigned.is_size_strictly_smaller_than(&signed) {
            unsigned
        } else {
            signed
        }
    }

    /// Signed maximum of every pairing. Mirrors `ConstantRange::smax`.
    pub fn smax(&self, other: &Self) -> Self {
        self.min_max(other, MinMaxKind::SignedMax)
    }

    /// Signed minimum of every pairing. Mirrors `ConstantRange::smin`.
    pub fn smin(&self, other: &Self) -> Self {
        self.min_max(other, MinMaxKind::SignedMin)
    }

    /// Unsigned maximum of every pairing. Mirrors `ConstantRange::umax`.
    pub fn umax(&self, other: &Self) -> Self {
        self.min_max(other, MinMaxKind::UnsignedMax)
    }

    /// Unsigned minimum of every pairing. Mirrors `ConstantRange::umin`.
    pub fn umin(&self, other: &Self) -> Self {
        self.min_max(other, MinMaxKind::UnsignedMin)
    }

    /// The shared body of `smax` / `smin` / `umax` / `umin`.
    ///
    /// All four upstream functions have the same shape — take the operation
    /// pointwise on the two bounds, then, if either input wraps in the
    /// relevant domain, intersect with the union to stay sound. Writing it
    /// once keeps the four from drifting apart.
    fn min_max(&self, other: &Self, kind: MinMaxKind) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(self.bit_width());
        }
        let bit_width = self.bit_width();
        let one_v = one(bit_width);
        let signed = matches!(kind, MinMaxKind::SignedMax | MinMaxKind::SignedMin);
        let take_max = matches!(kind, MinMaxKind::SignedMax | MinMaxKind::UnsignedMax);

        let (self_min, self_max, other_min, other_max) = if signed {
            (
                self.signed_min(),
                self.signed_max(),
                other.signed_min(),
                other.signed_max(),
            )
        } else {
            (
                self.unsigned_min(),
                self.unsigned_max(),
                other.unsigned_min(),
                other.unsigned_max(),
            )
        };
        let pick = |lhs: ApInt, rhs: ApInt| -> ApInt {
            let lhs_first = if signed { lhs.slt(&rhs) } else { lhs.ult(&rhs) };
            // `lhs_first` is true when lhs is the smaller of the two.
            if lhs_first == take_max { rhs } else { lhs }
        };

        let new_lower = pick(self_min, other_min);
        let new_upper = pick(self_max, other_max).wrapping_add(&one_v);
        let result =
            Self::non_empty(new_lower, new_upper).unwrap_or_else(|_| Self::full(bit_width));

        let wraps = if signed {
            self.is_sign_wrapped_set() || other.is_sign_wrapped_set()
        } else {
            self.is_wrapped_set() || other.is_wrapped_set()
        };
        if wraps {
            let preferred = if signed {
                PreferredRangeType::Signed
            } else {
                PreferredRangeType::Unsigned
            };
            return result.intersect_with(&self.union_with(other, preferred), preferred);
        }
        result
    }

    /// Absolute value of every member. Mirrors `ConstantRange::abs`.
    ///
    /// `int_min_is_poison` reflects the `llvm.abs` intrinsic's flag: the
    /// signed minimum has no positive counterpart, so the caller says whether
    /// it is poison (excluded) or wraps to itself (included).
    ///
    /// Pulled forward from the bit-counting group because [`Self::srem`] needs
    /// it.
    pub fn abs(&self, int_min_is_poison: bool) -> Self {
        if self.is_empty_set() {
            return Self::empty(self.bit_width());
        }
        let bit_width = self.bit_width();
        let one_v = one(bit_width);
        let signed_min_value = ApInt::signed_min_value(bit_width);
        let range = |lower: ApInt, upper: ApInt| {
            Self::new(lower, upper).unwrap_or_else(|_| Self::full(bit_width))
        };
        let umin = |lhs: ApInt, rhs: ApInt| if lhs.ult(&rhs) { lhs } else { rhs };
        let umax = |lhs: ApInt, rhs: ApInt| if lhs.ugt(&rhs) { lhs } else { rhs };

        if self.is_sign_wrapped_set() {
            let lo = if self.upper.is_strictly_positive() || !self.lower.is_strictly_positive() {
                // The range crosses zero, so zero is attainable.
                ApInt::zero(bit_width)
            } else {
                umin(self.lower.clone(), self.upper.negate().wrapping_add(&one_v))
            };
            return if int_min_is_poison {
                range(lo, signed_min_value)
            } else {
                range(lo, signed_min_value.wrapping_add(&one_v))
            };
        }

        let mut signed_min = self.signed_min();
        let signed_max = self.signed_max();

        if int_min_is_poison && signed_min.is_min_signed_value() {
            // Dropping the signed minimum can empty the range, if that was
            // all it held.
            if signed_max.is_min_signed_value() {
                return Self::empty(bit_width);
            }
            signed_min = signed_min.wrapping_add(&one_v);
        }

        if signed_min.is_non_negative() {
            return range(signed_min, signed_max.wrapping_add(&one_v));
        }
        if signed_max.is_negative() {
            return range(
                signed_max.negate(),
                signed_min.negate().wrapping_add(&one_v),
            );
        }
        // Crosses zero: the largest magnitude is on whichever side reaches
        // further from it.
        Self::non_empty(
            ApInt::zero(bit_width),
            umax(signed_min.negate(), signed_max).wrapping_add(&one_v),
        )
        .unwrap_or_else(|_| Self::full(bit_width))
    }

    /// Unsigned quotient of every pairing. Mirrors `ConstantRange::udiv`.
    ///
    /// Division by zero is undefined behaviour, so a divisor range that can
    /// *only* be zero yields the empty set, and a divisor range that merely
    /// contains zero has the zero skipped when picking the smallest divisor.
    pub fn udiv(&self, rhs: &Self) -> Self {
        if self.bit_width() != rhs.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || rhs.is_empty_set() || rhs.unsigned_max().is_zero() {
            return Self::empty(bit_width);
        }
        let one_v = one(bit_width);

        let lower = self
            .unsigned_min()
            .checked_udiv(&rhs.unsigned_max())
            .unwrap_or_else(|| ApInt::zero(bit_width));

        // The smallest *non-zero* divisor. Normally 1, except for a range of
        // the form `[X, 1)`, where the only member is X.
        let mut rhs_umin = rhs.unsigned_min();
        if rhs_umin.is_zero() {
            rhs_umin = if rhs.upper.is_one() {
                rhs.lower.clone()
            } else {
                one_v.clone()
            };
        }

        let upper = self
            .unsigned_max()
            .checked_udiv(&rhs_umin)
            .unwrap_or_else(|| ApInt::max_value(bit_width))
            .wrapping_add(&one_v);
        Self::non_empty(lower, upper).unwrap_or_else(|_| Self::full(bit_width))
    }

    /// Signed quotient of every pairing. Mirrors `ConstantRange::sdiv`.
    ///
    /// Both sides are split by sign and the four sign combinations are
    /// computed separately, because the quotient's sign is determined by the
    /// operands' and mixing them loses precision.
    pub fn sdiv(&self, rhs: &Self) -> Self {
        if self.bit_width() != rhs.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        let zero = ApInt::zero(bit_width);
        let one_v = one(bit_width);
        let signed_min_value = ApInt::signed_min_value(bit_width);
        let range = |lower: ApInt, upper: ApInt| {
            Self::new(lower, upper).unwrap_or_else(|_| Self::full(bit_width))
        };
        let sdiv = |lhs: &ApInt, r: &ApInt| {
            lhs.checked_sdiv(r)
                .unwrap_or_else(|| ApInt::zero(bit_width))
        };

        let (positive_lhs, negative_lhs) = self.split_pos_neg();
        let (positive_rhs, negative_rhs) = rhs.split_pos_neg();

        let mut positive_result = Self::empty(bit_width);
        if !positive_lhs.is_empty_set() && !positive_rhs.is_empty_set() {
            // pos / pos = pos.
            positive_result = range(
                sdiv(
                    &positive_lhs.lower,
                    &positive_rhs.upper.wrapping_sub(&one_v),
                ),
                sdiv(
                    &positive_lhs.upper.wrapping_sub(&one_v),
                    &positive_rhs.lower,
                )
                .wrapping_add(&one_v),
            );
        }

        if !negative_lhs.is_empty_set() && !negative_rhs.is_empty_set() {
            // neg / neg = pos, with one trap: `SignedMin / -1` is UB at the IR
            // level even though `ApInt` defines it (yielding SignedMin). When
            // both are attainable, upstream computes the bound twice — once
            // with -1 dropped from the divisor, once with SignedMin dropped
            // from the dividend — and unions the two.
            let lo = sdiv(
                &negative_lhs.upper.wrapping_sub(&one_v),
                &negative_rhs.lower,
            );
            if negative_lhs.lower.is_min_signed_value() && negative_rhs.upper.is_zero() {
                // Drop -1 from the divisor, unless that would empty it.
                if !negative_rhs.lower.is_all_ones() {
                    let adjusted_upper = if rhs.lower.is_all_ones() {
                        // The negative part of `[-1, X]` without -1 is
                        // `[SignedMin, X]`.
                        rhs.upper.clone()
                    } else {
                        // `[X, -1]` without -1 is `[X, -2]`.
                        negative_rhs.upper.wrapping_sub(&one_v)
                    };
                    positive_result = positive_result.union_with(
                        &range(
                            lo.clone(),
                            sdiv(&negative_lhs.lower, &adjusted_upper.wrapping_sub(&one_v))
                                .wrapping_add(&one_v),
                        ),
                        PreferredRangeType::Smallest,
                    );
                }

                // Drop SignedMin from the dividend, unless that would empty it.
                if !negative_lhs
                    .upper
                    .eq_ap_int(&signed_min_value.wrapping_add(&one_v))
                {
                    let adjusted_lower =
                        if self.upper.eq_ap_int(&signed_min_value.wrapping_add(&one_v)) {
                            // The negative part of `[X, SignedMin]` without
                            // SignedMin is `[X, -1]`.
                            self.lower.clone()
                        } else {
                            // `[SignedMin, X]` without SignedMin is
                            // `[SignedMin + 1, X]`.
                            negative_lhs.lower.wrapping_add(&one_v)
                        };
                    positive_result = positive_result.union_with(
                        &range(
                            lo,
                            sdiv(&adjusted_lower, &negative_rhs.upper.wrapping_sub(&one_v))
                                .wrapping_add(&one_v),
                        ),
                        PreferredRangeType::Smallest,
                    );
                }
            } else {
                positive_result = positive_result.union_with(
                    &range(
                        lo,
                        sdiv(
                            &negative_lhs.lower,
                            &negative_rhs.upper.wrapping_sub(&one_v),
                        )
                        .wrapping_add(&one_v),
                    ),
                    PreferredRangeType::Smallest,
                );
            }
        }

        let mut negative_result = Self::empty(bit_width);
        if !positive_lhs.is_empty_set() && !negative_rhs.is_empty_set() {
            // pos / neg = neg.
            negative_result = range(
                sdiv(
                    &positive_lhs.upper.wrapping_sub(&one_v),
                    &negative_rhs.upper.wrapping_sub(&one_v),
                ),
                sdiv(&positive_lhs.lower, &negative_rhs.lower).wrapping_add(&one_v),
            );
        }
        if !negative_lhs.is_empty_set() && !positive_rhs.is_empty_set() {
            // neg / pos = neg.
            negative_result = negative_result.union_with(
                &range(
                    sdiv(&negative_lhs.lower, &positive_rhs.lower),
                    sdiv(
                        &negative_lhs.upper.wrapping_sub(&one_v),
                        &positive_rhs.upper.wrapping_sub(&one_v),
                    )
                    .wrapping_add(&one_v),
                ),
                PreferredRangeType::Smallest,
            );
        }

        // A non-wrapping signed range reads better here than a smaller
        // wrapping one.
        let mut result = negative_result.union_with(&positive_result, PreferredRangeType::Signed);

        // Splitting the dividend by sign dropped zero; put it back if it was
        // there and any divisor remains.
        if self.contains(&zero) && (!positive_rhs.is_empty_set() || !negative_rhs.is_empty_set()) {
            result = result.union_with(&Self::single(zero), PreferredRangeType::Smallest);
        }
        result
    }

    /// Unsigned remainder of every pairing. Mirrors `ConstantRange::urem`.
    pub fn urem(&self, rhs: &Self) -> Self {
        if self.bit_width() != rhs.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || rhs.is_empty_set() || rhs.unsigned_max().is_zero() {
            return Self::empty(bit_width);
        }
        let one_v = one(bit_width);

        if let Some(divisor) = rhs.single_element() {
            // Remainder by zero is UB.
            if divisor.is_zero() {
                return Self::empty(bit_width);
            }
            if let Some(dividend) = self.single_element()
                && let Some(exact) = dividend.checked_urem(divisor)
            {
                return Self::single(exact);
            }
        }

        // `L % R` is `L` when `L < R`.
        if self.unsigned_max().ult(&rhs.unsigned_min()) {
            return self.clone();
        }

        // Otherwise the result is at most `L` and strictly below `R`.
        let self_max = self.unsigned_max();
        let rhs_bound = rhs.unsigned_max().wrapping_sub(&one_v);
        let upper = if self_max.ult(&rhs_bound) {
            self_max
        } else {
            rhs_bound
        }
        .wrapping_add(&one_v);
        Self::non_empty(ApInt::zero(bit_width), upper).unwrap_or_else(|_| Self::full(bit_width))
    }

    /// Signed remainder of every pairing. Mirrors `ConstantRange::srem`.
    ///
    /// In LLVM the remainder takes the *dividend's* sign, so the three cases
    /// below are all-non-negative, all-negative, and crossing zero.
    pub fn srem(&self, rhs: &Self) -> Self {
        if self.bit_width() != rhs.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || rhs.is_empty_set() {
            return Self::empty(bit_width);
        }
        let one_v = one(bit_width);
        let range = |lower: ApInt, upper: ApInt| {
            Self::new(lower, upper).unwrap_or_else(|_| Self::full(bit_width))
        };
        let umin = |lhs: ApInt, rhs: ApInt| if lhs.ult(&rhs) { lhs } else { rhs };
        let umax = |lhs: ApInt, rhs: ApInt| if lhs.ugt(&rhs) { lhs } else { rhs };

        if let Some(divisor) = rhs.single_element() {
            // Remainder by zero is UB.
            if divisor.is_zero() {
                return Self::empty(bit_width);
            }
            if let Some(dividend) = self.single_element()
                && let Some(exact) = dividend.checked_srem(divisor)
            {
                return Self::single(exact);
            }
        }

        // Only the divisor's magnitude matters.
        let absolute_rhs = rhs.abs(false);
        let mut min_absolute_rhs = absolute_rhs.unsigned_min();
        let max_absolute_rhs = absolute_rhs.unsigned_max();

        if max_absolute_rhs.is_zero() {
            return Self::empty(bit_width);
        }
        if min_absolute_rhs.is_zero() {
            min_absolute_rhs = one_v.clone();
        }

        let min_lhs = self.signed_min();
        let max_lhs = self.signed_max();

        if min_lhs.is_non_negative() {
            // `L % R` is `L` when `L < R`.
            if max_lhs.ult(&min_absolute_rhs) {
                return self.clone();
            }
            let upper = umin(max_lhs, max_absolute_rhs.wrapping_sub(&one_v)).wrapping_add(&one_v);
            return range(ApInt::zero(bit_width), upper);
        }

        if max_lhs.is_negative() {
            // The same reasoning, with a negative result.
            if min_lhs.ugt(&min_absolute_rhs.negate()) {
                return self.clone();
            }
            let lower = umax(min_lhs, max_absolute_rhs.negate().wrapping_add(&one_v));
            return range(lower, one_v);
        }

        // The dividend crosses zero, so the remainder can take either sign.
        let lower = umax(min_lhs, max_absolute_rhs.negate().wrapping_add(&one_v));
        let upper = umin(max_lhs, max_absolute_rhs.wrapping_sub(&one_v)).wrapping_add(&one_v);
        range(lower, upper)
    }

    /// Bitwise complement of every member. Mirrors `ConstantRange::binaryNot`.
    ///
    /// `~x` is `-1 - x`, so this is a subtraction from the all-ones range.
    pub fn binary_not(&self) -> Self {
        Self::single(ApInt::all_ones(self.bit_width())).sub(self)
    }

    /// Bitwise AND of every pairing. Mirrors `ConstantRange::binaryAnd`.
    ///
    /// Two independent approximations are intersected: what the known bits
    /// say, and the `[estimated lower bound, min(umax) + 1)` interval. Neither
    /// subsumes the other.
    pub fn binary_and(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(bit_width);
        }
        let one_v = one(bit_width);

        let known_bits_range = Self::from_known_bits(
            &KnownBits::bitand(&self.to_known_bits(), &other.to_known_bits()),
            false,
        );
        let lower_bound = estimate_bit_masked_and_lower_bound(self, other);
        let self_max = self.unsigned_max();
        let other_max = other.unsigned_max();
        let upper = if other_max.ult(&self_max) {
            other_max
        } else {
            self_max
        }
        .wrapping_add(&one_v);
        let umin_umax_range =
            Self::non_empty(lower_bound, upper).unwrap_or_else(|_| Self::full(bit_width));
        known_bits_range.intersect_with(&umin_umax_range, PreferredRangeType::Smallest)
    }

    /// Bitwise OR of every pairing. Mirrors `ConstantRange::binaryOr`.
    pub fn binary_or(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(bit_width);
        }

        let known_bits_range = Self::from_known_bits(
            &KnownBits::bitor(&self.to_known_bits(), &other.to_known_bits()),
            false,
        );

        // De Morgan turns the OR's upper bound into the AND's lower bound:
        //   ~a & ~b >= x  <=>  a | b < -x
        // so the estimator can be reused on the complemented operands.
        let upper_bound =
            estimate_bit_masked_and_lower_bound(&self.binary_not(), &other.binary_not()).negate();
        let self_min = self.unsigned_min();
        let other_min = other.unsigned_min();
        let lower = if self_min.ugt(&other_min) {
            self_min
        } else {
            other_min
        };
        let umax_umin_range =
            Self::non_empty(lower, upper_bound).unwrap_or_else(|_| Self::full(bit_width));
        known_bits_range.intersect_with(&umax_umin_range, PreferredRangeType::Smallest)
    }

    /// Bitwise XOR of every pairing. Mirrors `ConstantRange::binaryXor`.
    pub fn binary_xor(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(bit_width);
        }

        // Two single values XOR exactly.
        if let (Some(lhs), Some(rhs)) = (self.single_element(), other.single_element()) {
            return Self::single(lhs.bitxor(rhs));
        }
        // XOR with all-ones is complement, which is exact.
        if other.single_element().is_some_and(ApInt::is_all_ones) {
            return self.binary_not();
        }
        if self.single_element().is_some_and(ApInt::is_all_ones) {
            return other.binary_not();
        }

        let lhs_known = self.to_known_bits();
        let rhs_known = other.to_known_bits();
        let mut result = Self::from_known_bits(&KnownBits::bitxor(&lhs_known, &rhs_known), false);
        // At one bit the refinement below does not improve on the known bits.
        if bit_width == 1 {
            return result;
        }

        // When one side's possible-one bits are a subset of the other's
        // known-one bits, the XOR is a borrow-free subtraction, which is a
        // tighter answer than the known bits alone.
        if lhs_known
            .zero_mask()
            .not()
            .is_subset_of(rhs_known.one_mask())
        {
            result = result.intersect_with(&other.sub(self), PreferredRangeType::Unsigned);
        } else if rhs_known
            .zero_mask()
            .not()
            .is_subset_of(lhs_known.one_mask())
        {
            result = result.intersect_with(&self.sub(other), PreferredRangeType::Unsigned);
        }
        result
    }

    /// Left shift of every pairing. Mirrors `ConstantRange::shl`.
    pub fn shl(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(bit_width);
        }
        let one_v = one(bit_width);
        let mut min = self.unsigned_min();
        let mut max = self.unsigned_max();
        let limit = u64::from(bit_width);

        if let Some(amount) = other.single_element() {
            // Shifting by at least the width is poison.
            if amount.limited_value(limit) >= limit {
                return Self::empty(bit_width);
            }
            let shift = u32::try_from(amount.limited_value(limit)).unwrap_or(bit_width);
            let equal_leading_bits = min.bitxor(&max).count_leading_zeros();
            if shift <= equal_leading_bits {
                // No member's significant bits fall off the top, so the
                // endpoints shift cleanly.
                return Self::non_empty(min.shl(shift), max.shl(shift).wrapping_add(&one_v))
                    .unwrap_or_else(|_| Self::full(bit_width));
            }
            return Self::non_empty(
                ApInt::zero(bit_width),
                ApInt::bits_set_from(bit_width, shift).wrapping_add(&one_v),
            )
            .unwrap_or_else(|_| Self::full(bit_width));
        }

        let other_max = other.unsigned_max();
        let other_max_amount = u32::try_from(other_max.limited_value(limit)).unwrap_or(bit_width);
        let other_min_amount =
            u32::try_from(other.unsigned_min().limited_value(limit)).unwrap_or(bit_width);

        if self.is_all_negative() && other_max_amount <= min.count_leading_ones() {
            // All-negative and no signed overflow: a bigger shift makes the
            // value smaller, so the roles of min and max swap.
            max = max.shl(other_min_amount);
            min = min.shl(other_max_amount);
            return Self::non_empty(min, max.wrapping_add(&one_v))
                .unwrap_or_else(|_| Self::full(bit_width));
        }

        // Overflow is possible, and upstream does not narrow further here.
        if other_max_amount > max.count_leading_zeros() {
            return Self::full(bit_width);
        }

        min = min.shl(other_min_amount);
        max = max.shl(other_max_amount);
        Self::non_empty(min, max.wrapping_add(&one_v)).unwrap_or_else(|_| Self::full(bit_width))
    }

    /// Logical right shift of every pairing. Mirrors `ConstantRange::lshr`.
    pub fn lshr(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(bit_width);
        }
        let one_v = one(bit_width);
        let limit = u64::from(bit_width);
        let amount = |v: &ApInt| u32::try_from(v.limited_value(limit)).unwrap_or(bit_width);

        // Shifting right shrinks, so the largest result comes from the
        // smallest shift and vice versa.
        let max = self
            .unsigned_max()
            .lshr(amount(&other.unsigned_min()))
            .wrapping_add(&one_v);
        let min = self.unsigned_min().lshr(amount(&other.unsigned_max()));
        Self::non_empty(min, max).unwrap_or_else(|_| Self::full(bit_width))
    }

    /// Arithmetic right shift of every pairing. Mirrors `ConstantRange::ashr`.
    ///
    /// A negative value grows toward -1 as it is shifted while a non-negative
    /// one shrinks toward 0, so which shift amount produces the extreme
    /// depends on the sign — hence the three cases.
    pub fn ashr(&self, other: &Self) -> Self {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(bit_width);
        }
        let one_v = one(bit_width);
        let limit = u64::from(bit_width);
        let amount = |v: &ApInt| u32::try_from(v.limited_value(limit)).unwrap_or(bit_width);
        let other_min = amount(&other.unsigned_min());
        let other_max = amount(&other.unsigned_max());

        let signed_min = self.signed_min();
        let signed_max = self.signed_max();

        // Bounds assuming a non-negative operand: shifting shrinks it.
        let positive_max = signed_max.ashr(other_min).wrapping_add(&one_v);
        let positive_min = signed_min.ashr(other_max);
        // Bounds assuming a negative operand: shifting grows it toward -1.
        let negative_max = signed_max.ashr(other_max).wrapping_add(&one_v);
        let negative_min = signed_min.ashr(other_min);

        let (min, max) = if signed_min.is_non_negative() {
            (positive_min, positive_max)
        } else if signed_max.is_negative() {
            (negative_min, negative_max)
        } else {
            // Straddles zero, so take the outer bound from each side.
            (negative_min, positive_max)
        };
        Self::non_empty(min, max).unwrap_or_else(|_| Self::full(bit_width))
    }

    /// Saturating unsigned addition of every pairing. Mirrors
    /// `ConstantRange::uadd_sat`.
    pub fn uadd_sat(&self, other: &Self) -> Self {
        self.saturating_pairwise(other, |bounds| {
            (
                bounds.self_umin.uadd_sat(&bounds.other_umin),
                bounds.self_umax.uadd_sat(&bounds.other_umax),
            )
        })
    }

    /// Saturating signed addition of every pairing. Mirrors
    /// `ConstantRange::sadd_sat`.
    pub fn sadd_sat(&self, other: &Self) -> Self {
        self.saturating_pairwise(other, |bounds| {
            (
                bounds.self_smin.sadd_sat(&bounds.other_smin),
                bounds.self_smax.sadd_sat(&bounds.other_smax),
            )
        })
    }

    /// Saturating unsigned subtraction of every pairing. Mirrors
    /// `ConstantRange::usub_sat`.
    ///
    /// Subtraction is *decreasing* in its right operand, so the smallest
    /// result pairs this range's minimum with the other's maximum.
    pub fn usub_sat(&self, other: &Self) -> Self {
        self.saturating_pairwise(other, |bounds| {
            (
                bounds.self_umin.usub_sat(&bounds.other_umax),
                bounds.self_umax.usub_sat(&bounds.other_umin),
            )
        })
    }

    /// Saturating signed subtraction of every pairing. Mirrors
    /// `ConstantRange::ssub_sat`.
    pub fn ssub_sat(&self, other: &Self) -> Self {
        self.saturating_pairwise(other, |bounds| {
            (
                bounds.self_smin.ssub_sat(&bounds.other_smax),
                bounds.self_smax.ssub_sat(&bounds.other_smin),
            )
        })
    }

    /// Saturating unsigned multiplication of every pairing. Mirrors
    /// `ConstantRange::umul_sat`.
    pub fn umul_sat(&self, other: &Self) -> Self {
        self.saturating_pairwise(other, |bounds| {
            (
                bounds.self_umin.umul_sat(&bounds.other_umin),
                bounds.self_umax.umul_sat(&bounds.other_umax),
            )
        })
    }

    /// Saturating signed multiplication of every pairing. Mirrors
    /// `ConstantRange::smul_sat`.
    ///
    /// With negatives in play the extremes can come from a mixed corner —
    /// upstream's example is `[-1,4) * [-2,3)`, whose lowest product is
    /// `3 * -2` — so all four corners are considered, as in
    /// [`Self::multiply`].
    pub fn smul_sat(&self, other: &Self) -> Self {
        self.saturating_pairwise(other, |bounds| {
            let corners = [
                bounds.self_smin.smul_sat(&bounds.other_smin),
                bounds.self_smin.smul_sat(&bounds.other_smax),
                bounds.self_smax.smul_sat(&bounds.other_smin),
                bounds.self_smax.smul_sat(&bounds.other_smax),
            ];
            let mut lowest = corners[0].clone();
            let mut highest = corners[0].clone();
            for corner in &corners[1..] {
                if corner.slt(&lowest) {
                    lowest = corner.clone();
                }
                if highest.slt(corner) {
                    highest = corner.clone();
                }
            }
            (lowest, highest)
        })
    }

    /// Saturating unsigned left shift of every pairing. Mirrors
    /// `ConstantRange::ushl_sat`.
    pub fn ushl_sat(&self, other: &Self) -> Self {
        let bit_width = self.bit_width();
        self.saturating_pairwise(other, move |bounds| {
            let amount = |v: &ApInt| {
                u32::try_from(v.limited_value(u64::from(bit_width))).unwrap_or(bit_width)
            };
            (
                bounds.self_umin.ushl_sat(amount(&bounds.other_umin)),
                bounds.self_umax.ushl_sat(amount(&bounds.other_umax)),
            )
        })
    }

    /// Saturating signed left shift of every pairing. Mirrors
    /// `ConstantRange::sshl_sat`.
    ///
    /// The shift amount that produces each extreme depends on that endpoint's
    /// sign: shifting a negative value left drives it *down*, so its minimum
    /// comes from the largest shift, while a non-negative value's minimum
    /// comes from the smallest.
    pub fn sshl_sat(&self, other: &Self) -> Self {
        let bit_width = self.bit_width();
        self.saturating_pairwise(other, move |bounds| {
            let amount = |v: &ApInt| {
                u32::try_from(v.limited_value(u64::from(bit_width))).unwrap_or(bit_width)
            };
            let shift_min = amount(&bounds.other_umin);
            let shift_max = amount(&bounds.other_umax);
            (
                bounds
                    .self_smin
                    .sshl_sat(if bounds.self_smin.is_non_negative() {
                        shift_min
                    } else {
                        shift_max
                    }),
                bounds
                    .self_smax
                    .sshl_sat(if bounds.self_smax.is_negative() {
                        shift_min
                    } else {
                        shift_max
                    }),
            )
        })
    }

    /// The shared frame of the saturating operations: both empty checks, the
    /// six bounds each of them reads, and the closing `[lower, upper + 1)`.
    ///
    /// Every upstream saturating function has exactly this shape — only the
    /// choice of which bounds pair up differs — so writing the frame once
    /// keeps the eight from drifting.
    fn saturating_pairwise<F>(&self, other: &Self, bounds_to_endpoints: F) -> Self
    where
        F: FnOnce(SaturatingBounds) -> (ApInt, ApInt),
    {
        if self.bit_width() != other.bit_width() {
            return Self::full(self.bit_width());
        }
        let bit_width = self.bit_width();
        if self.is_empty_set() || other.is_empty_set() {
            return Self::empty(bit_width);
        }
        let (lower, upper) = bounds_to_endpoints(SaturatingBounds {
            self_umin: self.unsigned_min(),
            self_umax: self.unsigned_max(),
            self_smin: self.signed_min(),
            self_smax: self.signed_max(),
            other_umin: other.unsigned_min(),
            other_umax: other.unsigned_max(),
            other_smin: other.signed_min(),
            other_smax: other.signed_max(),
        });
        Self::non_empty(lower, upper.wrapping_add(&one(bit_width)))
            .unwrap_or_else(|_| Self::full(bit_width))
    }

    /// Count of leading zeros over every member. Mirrors
    /// `ConstantRange::ctlz`.
    ///
    /// `zero_is_poison` reflects the `llvm.ctlz` flag: `ctlz(0)` is the full
    /// width, which the intrinsic may declare poison instead, in which case
    /// zero is excluded from the input before counting.
    pub fn ctlz(&self, zero_is_poison: bool) -> Self {
        if self.is_empty_set() {
            return Self::empty(self.bit_width());
        }
        let bit_width = self.bit_width();
        let zero = ApInt::zero(bit_width);
        let one_v = one(bit_width);
        let count = |v: u32| ApInt::from_words(bit_width, &[u64::from(v)]);
        let range = |lower: ApInt, upper: ApInt| {
            Self::new(lower, upper).unwrap_or_else(|_| Self::full(bit_width))
        };

        if zero_is_poison && self.contains(&zero) {
            // Zero is in the range but must be excluded. It can enter three
            // ways: as the lower endpoint, as the (wrapped) upper endpoint, or
            // in the interior of a wrapped range.
            let upper_minus_one = self.upper.wrapping_sub(&one_v);
            if self.lower.is_zero() {
                if upper_minus_one.is_zero() {
                    // `[0, 1)` holds only zero, so excluding it leaves nothing.
                    return Self::empty(bit_width);
                }
                return range(
                    count(upper_minus_one.count_leading_zeros()),
                    count(self.lower.wrapping_add(&one_v).count_leading_zeros() + 1),
                );
            }
            if upper_minus_one.is_zero() {
                return range(zero, count(self.lower.count_leading_zeros() + 1));
            }
            // Zero sits inside a wrapped range, so every count is possible.
            return range(zero, count(bit_width));
        }

        // Zero is either absent or harmless. More leading zeros means a
        // smaller value, so the extremes swap.
        Self::non_empty(
            count(self.unsigned_max().count_leading_zeros()),
            count(self.unsigned_min().count_leading_zeros()).wrapping_add(&one_v),
        )
        .unwrap_or_else(|_| Self::full(bit_width))
    }

    /// Count of trailing zeros over every member. Mirrors
    /// `ConstantRange::cttz`.
    pub fn cttz(&self, zero_is_poison: bool) -> Self {
        if self.is_empty_set() {
            return Self::empty(self.bit_width());
        }
        let bit_width = self.bit_width();
        let zero = ApInt::zero(bit_width);
        let one_v = one(bit_width);
        let count = |v: u32| ApInt::from_words(bit_width, &[u64::from(v)]);

        if zero_is_poison && self.contains(&zero) {
            if self.lower.is_zero() {
                if self.upper.is_one() {
                    return Self::empty(bit_width);
                }
                // Exclude zero by starting the sub-range at one.
                return unsigned_count_trailing_zeros_range(&one_v, &self.upper);
            }
            if self.upper.is_one() {
                return unsigned_count_trailing_zeros_range(&self.lower, &zero);
            }
            // Zero is interior to a wrapped range: handle the two halves.
            return unsigned_count_trailing_zeros_range(&self.lower, &zero).union_with(
                &unsigned_count_trailing_zeros_range(&one_v, &self.upper),
                PreferredRangeType::Smallest,
            );
        }

        if self.is_full_set() {
            return Self::non_empty(zero, count(bit_width).wrapping_add(&one_v))
                .unwrap_or_else(|_| Self::full(bit_width));
        }
        if !self.is_wrapped_set() {
            return unsigned_count_trailing_zeros_range(&self.lower, &self.upper);
        }
        // Decompose the wrapped range into `[lower, 0)` and `[0, upper)`.
        unsigned_count_trailing_zeros_range(&self.lower, &zero).union_with(
            &unsigned_count_trailing_zeros_range(&zero, &self.upper),
            PreferredRangeType::Smallest,
        )
    }

    /// Population count over every member. Mirrors `ConstantRange::ctpop`.
    pub fn ctpop(&self) -> Self {
        if self.is_empty_set() {
            return Self::empty(self.bit_width());
        }
        let bit_width = self.bit_width();
        let zero = ApInt::zero(bit_width);
        let one_v = one(bit_width);
        let count = |v: u32| ApInt::from_words(bit_width, &[u64::from(v)]);

        if self.is_full_set() {
            return Self::non_empty(zero, count(bit_width).wrapping_add(&one_v))
                .unwrap_or_else(|_| Self::full(bit_width));
        }
        if !self.is_wrapped_set() {
            return unsigned_pop_count_range(&self.lower, &self.upper);
        }
        // `[lower, 0)` is `[lower, max]`, whose smallest population count is
        // the run of leading ones the lower endpoint already has.
        let upper_half = Self::new(
            count(self.lower.count_leading_ones()),
            count(bit_width).wrapping_add(&one_v),
        )
        .unwrap_or_else(|_| Self::full(bit_width));
        upper_half.union_with(
            &unsigned_pop_count_range(&zero, &self.upper),
            PreferredRangeType::Smallest,
        )
    }

    pub fn intersects_with(&self, rhs: &Self) -> bool {
        if self.bit_width() != rhs.bit_width() {
            return false;
        }
        for (lhs_lo, lhs_hi) in self.segments() {
            for (rhs_lo, rhs_hi) in rhs.segments() {
                if lhs_lo.ule(&rhs_hi) && rhs_lo.ule(&lhs_hi) {
                    return true;
                }
            }
        }
        false
    }

    pub fn is_contiguous_with(&self, rhs: &Self) -> bool {
        if self.bit_width() != rhs.bit_width() {
            return false;
        }
        for (lhs_lo, lhs_hi) in self.segments() {
            for (rhs_lo, rhs_hi) in rhs.segments() {
                if next_value(&lhs_hi).eq_ap_int(&rhs_lo) || next_value(&rhs_hi).eq_ap_int(&lhs_lo)
                {
                    return true;
                }
            }
        }
        false
    }

    fn segments(&self) -> Vec<(ApInt, ApInt)> {
        let bit_width = self.bit_width();
        if self.is_empty_set() {
            return Vec::new();
        }
        if self.is_full_set() {
            return vec![(ApInt::zero(bit_width), ApInt::max_value(bit_width))];
        }
        if self.is_upper_wrapped() {
            let mut segments = vec![(self.lower.clone(), ApInt::max_value(bit_width))];
            if !self.upper.is_min_value() {
                segments.push((
                    ApInt::zero(bit_width),
                    self.upper.wrapping_sub(&one(bit_width)),
                ));
            }
            return segments;
        }
        vec![(self.lower.clone(), self.upper.wrapping_sub(&one(bit_width)))]
    }
}

pub(crate) fn constant_ranges_from_metadata(
    module: &ModuleCore,
    store: &MetadataStore,
    id: MetadataSlot,
    expected_scalar_ty: TypeSlot,
) -> Option<Vec<ConstantRange>> {
    let MetadataKind::Tuple { operands, .. } = store.get(id)? else {
        return None;
    };
    if operands.is_empty() || operands.len() % 2 != 0 {
        return None;
    }
    let mut ranges = Vec::with_capacity(operands.len() / 2);
    for pair in operands.chunks_exact(2) {
        let (low_ty, low) = metadata_constant_int(module, store, pair[0].slot())?;
        let (high_ty, high) = metadata_constant_int(module, store, pair[1].slot())?;
        if low_ty != high_ty || high_ty != expected_scalar_ty {
            return None;
        }
        let range = ConstantRange::new(low, high).ok()?;
        if range.is_empty_set() || range.is_full_set() {
            return None;
        }
        ranges.push(range);
    }
    Some(ranges)
}

pub(crate) fn metadata_constant_int(
    module: &ModuleCore,
    store: &MetadataStore,
    id: MetadataSlot,
) -> Option<(TypeSlot, ApInt)> {
    let MetadataKind::Constant(value_id) = store.get(id)? else {
        return None;
    };
    constant_int_from_value(module, value_id.slot())
}

fn constant_int_from_value(module: &ModuleCore, id: ValueSlot) -> Option<(TypeSlot, ApInt)> {
    let data = module.context().value_data(id);
    let ValueKindData::Constant(ConstantData::Int(words)) = &data.kind else {
        return None;
    };
    let TypeData::Integer { bits } = module.context().type_data(data.ty) else {
        return None;
    };
    Some((data.ty, ApInt::from_words(*bits, words)))
}

fn one(bit_width: u32) -> ApInt {
    ApInt::from_words(bit_width, &[1])
}

fn next_value(value: &ApInt) -> ApInt {
    value.wrapping_add(&one(value.bit_width()))
}
