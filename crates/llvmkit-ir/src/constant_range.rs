//! Half-open integer ranges. Mirrors `llvm/include/llvm/IR/ConstantRange.h`.

use core::cmp::Ordering;

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
