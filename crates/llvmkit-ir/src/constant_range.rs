//! Half-open integer ranges. Mirrors `llvm/include/llvm/IR/ConstantRange.h`.

use core::cmp::Ordering;

use crate::ApInt;
use crate::constant::ConstantData;
use crate::error::{IrError, IrResult};
use crate::known_bits::KnownBits;
use crate::metadata::{MetadataKind, MetadataSlot, MetadataStore};
use crate::module::ModuleCore;
use crate::r#type::{TypeData, TypeSlot};
use crate::value::{ValueKindData, ValueSlot};

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
