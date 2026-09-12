//! An ordered, non-overlapping list of signed [`ConstantRange`]s. Mirrors
//! `llvm/include/llvm/IR/ConstantRangeList.h`.
//!
//! Upstream's header comment states the invariant this type exists to hold:
//! the ranges are signed, do not wrap around the end of the numeric range,
//! are ordered and non-overlapping, share one bit width, and each has
//! `lower <s upper`.
//!
//! The one deliberate difference from upstream's shape is that the vector is
//! private. `ConstantRangeList` upstream hands out mutable `begin()`/`end()`
//! iterators, through which a caller can break every invariant above;
//! llvmkit's entry points are the checked constructor
//! ([`ConstantRangeList::new`], `getConstantRangeList`) and [`insert`], which
//! preserves them.
//!
//! [`insert`]: ConstantRangeList::insert

use core::fmt;

use crate::ApInt;
use crate::ap_int::Signedness;
use crate::constant_range::ConstantRange;

/// A list of signed constant ranges, ordered and non-overlapping.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ConstantRangeList {
    ranges: Vec<ConstantRange>,
}

impl ConstantRangeList {
    /// The list built from `ranges`, or `None` when they are not ordered.
    /// Mirrors `ConstantRangeList::getConstantRangeList`.
    ///
    /// Upstream's other constructor takes the same argument and merely
    /// *asserts* the invariant, so it is not reproduced: this is the whole of
    /// its checked entry point, and `Option` is how llvmkit spells
    /// `std::optional`.
    #[must_use]
    pub fn new(ranges: Vec<ConstantRange>) -> Option<Self> {
        if Self::is_ordered_ranges(&ranges) {
            Some(Self { ranges })
        } else {
            None
        }
    }

    /// `true` when `ranges` are non-overlapping and increasing. Mirrors the
    /// static `ConstantRangeList::isOrderedRanges`.
    ///
    /// Every comparison is **signed**, and the second one is `sle`, not
    /// `slt` — two ranges that merely *touch* (`(0, 4), (4, 8)`) are
    /// rejected, which `test/Verifier/initializes-attr.ll`'s `overlapping1`
    /// case pins.
    ///
    /// Kept callable without a list because upstream exposes it statically
    /// and its `Verifier` caller asks it about a raw `ArrayRef`.
    #[must_use]
    pub fn is_ordered_ranges(ranges: &[ConstantRange]) -> bool {
        let Some(first) = ranges.first() else {
            // An empty list is ordered. The parser can never build one — the
            // grammar is one-or-more — but `Verifier::verifyParameterAttrs`
            // rejects an empty list separately, so this is not the place.
            return true;
        };
        if first.lower().sge(first.upper()) {
            return false;
        }
        for window in ranges.windows(2) {
            let [previous, current] = window else {
                continue;
            };
            if current.lower().sge(current.upper()) || current.lower().sle(previous.upper()) {
                return false;
            }
        }
        true
    }

    /// The ranges, in order.
    #[must_use]
    pub fn ranges(&self) -> &[ConstantRange] {
        &self.ranges
    }

    /// `true` when the list holds no ranges.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// How many ranges the list holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// The shared bit width of the ranges, or `None` when the list is empty.
    ///
    /// Upstream's `getBitWidth` documents that calling it on an empty list is
    /// invalid and then reads `Ranges.front()` anyway; the emptiness is an
    /// ordinary state here, so it is an `Option`.
    #[must_use]
    pub fn bit_width(&self) -> Option<u32> {
        Some(self.ranges.first()?.bit_width())
    }

    /// Insert `range`, keeping the list ordered. Mirrors
    /// `ConstantRangeList::insert`.
    ///
    /// Touching ranges **merge**: the fast-append guard is `slt`, so
    /// inserting `(4, 8)` into a list ending at `(0, 4)` falls through to the
    /// merging path and yields `(0, 8)`. That is why a list built by `insert`
    /// always satisfies [`Self::is_ordered_ranges`], which rejects touching
    /// pairs.
    ///
    /// Upstream returns early on an empty `range` and *asserts* on a full or
    /// reversed one. All three have `lower >=s upper`, so one guard covers
    /// them, and llvmkit ignores rather than asserts — this crate raises no
    /// runtime panics on a production path.
    pub fn insert(&mut self, range: ConstantRange) {
        if range.lower().sge(range.upper()) {
            return;
        }

        // Handle common cases.
        let Some(last) = self.ranges.last() else {
            self.ranges.push(range);
            return;
        };
        if last.upper().slt(range.lower()) {
            self.ranges.push(range);
            return;
        }

        let Some(first) = self.ranges.first() else {
            unreachable!("a list with a last range has a first range")
        };
        if range.upper().slt(first.lower()) {
            self.ranges.insert(0, range);
            return;
        }

        // `lower_bound` by lower endpoint: the first range whose lower is not
        // less than the new one's.
        let lower_bound = self
            .ranges
            .partition_point(|existing| existing.lower().slt(range.lower()));
        if let Some(at_bound) = self.ranges.get(lower_bound)
            && at_bound.contains_range(&range)
        {
            // Upstream reaches for `ConstantRange::contains`, which compares
            // *unsigned* even though every other test here is signed. It is an
            // upstream inconsistency — `subtract` carries the comment saying
            // signed checking is what is wanted — and it is reproduced rather
            // than corrected.
            return;
        }

        // Slow insert.
        let existing_tail: Vec<ConstantRange> = self.ranges.split_off(lower_bound);
        match self.ranges.last() {
            Some(last) if range.lower().sle(last.upper()) => {
                let merged = merged_range(last.lower().clone(), smax(range.upper(), last.upper()));
                let Some(slot) = self.ranges.last_mut() else {
                    unreachable!("the list was just observed to have a last range")
                };
                *slot = merged;
            }
            _ => self.ranges.push(range),
        }
        for tail_range in existing_tail {
            let Some(last) = self.ranges.last() else {
                unreachable!("the merge above always leaves at least one range")
            };
            if last.upper().slt(tail_range.lower()) {
                self.ranges.push(tail_range);
                continue;
            }
            let merged = merged_range(last.lower().clone(), smax(tail_range.upper(), last.upper()));
            let Some(slot) = self.ranges.last_mut() else {
                unreachable!("the list was just observed to have a last range")
            };
            *slot = merged;
        }
    }

    /// Insert `[lower, upper)` at 64 bits. Mirrors upstream's
    /// `insert(int64_t, int64_t)` overload, whose width is likewise
    /// hard-coded.
    pub fn insert_signed(&mut self, lower: i64, upper: i64) {
        if let Ok(range) = ConstantRange::new(ap_int_from_i64(lower), ap_int_from_i64(upper)) {
            self.insert(range);
        }
    }

    /// Remove `sub_range` from every range in the list. Mirrors
    /// `ConstantRangeList::subtract`.
    ///
    /// Upstream returns early on an empty `SubRange` or an empty list and then
    /// *asserts* three things: `SubRange` is not the full set, its bounds are
    /// strictly ordered, and the widths agree. This crate raises no runtime
    /// panics on a production path, so each assert is a guard that returns the
    /// list unchanged — the same treatment [`Self::insert`] gives upstream's
    /// asserts.
    ///
    /// Every comparison here is **signed**, including the two containment
    /// tests. Upstream's own comment on the first of them says so explicitly
    /// ("Note that `ConstantRange::contains(ConstantRange)` checks unsigned,
    /// but we need signed checking here"), which is why this reads nothing from
    /// `insert`'s unsigned `contains` shortcut.
    pub fn subtract(&mut self, sub_range: &ConstantRange) {
        // `if (SubRange.isEmptySet() || empty()) return;`
        if sub_range.is_empty_set() || self.is_empty() {
            return;
        }
        // `assert(!SubRange.isFullSet() && "Do not support full set");`
        // `assert(SubRange.getLower().slt(SubRange.getUpper()));`
        if sub_range.is_full_set() || !sub_range.lower().slt(sub_range.upper()) {
            return;
        }
        // `assert(getBitWidth() == SubRange.getBitWidth());`
        if self.bit_width() != Some(sub_range.bit_width()) {
            return;
        }

        // Handle common cases.
        let (Some(first), Some(last)) = (self.ranges.first(), self.ranges.last()) else {
            unreachable!("a non-empty list has a first and a last range")
        };
        // `if (Ranges.back().getUpper().sle(SubRange.getLower())) return;`
        if last.upper().sle(sub_range.lower()) {
            return;
        }
        // `if (SubRange.getUpper().sle(Ranges.front().getLower())) return;`
        if sub_range.upper().sle(first.lower()) {
            return;
        }

        let mut result: Vec<ConstantRange> = Vec::new();
        // `auto AppendRangeIfNonEmpty = [&Result](APInt Start, APInt End) {
        //    if (Start.slt(End)) Result.push_back(ConstantRange(Start, End)); };`
        let append_if_non_empty = |result: &mut Vec<ConstantRange>, start: &ApInt, end: &ApInt| {
            if start.slt(end) {
                result.push(merged_range(start.clone(), end.clone()));
            }
        };

        for range in &self.ranges {
            if sub_range.upper().sle(range.lower()) || range.upper().sle(sub_range.lower()) {
                // "Range" and "SubRange" do not overlap.
                //       L---U        : Range
                // L---U              : SubRange (Case1)
                //             L---U  : SubRange (Case2)
                result.push(range.clone());
            } else if range.lower().sle(sub_range.lower()) && sub_range.upper().sle(range.upper()) {
                // "Range" contains "SubRange".
                //       L---U        : Range
                //        L-U         : SubRange
                append_if_non_empty(&mut result, range.lower(), sub_range.lower());
                append_if_non_empty(&mut result, sub_range.upper(), range.upper());
            } else if sub_range.lower().sle(range.lower()) && range.upper().sle(sub_range.upper()) {
                // "SubRange" contains "Range".
                //        L-U        : Range
                //       L---U       : SubRange
                continue;
            } else if range.lower().sge(sub_range.lower()) && range.lower().sle(sub_range.upper()) {
                // "Range" and "SubRange" overlap at the left.
                //       L---U        : Range
                //     L---U          : SubRange
                append_if_non_empty(&mut result, sub_range.upper(), range.upper());
            } else {
                // "Range" and "SubRange" overlap at the right.
                //       L---U        : Range
                //         L---U      : SubRange
                // Upstream asserts `SubRange.getLower()` lies inside `Range`
                // here; the four arms above leave no other shape.
                append_if_non_empty(&mut result, range.lower(), sub_range.lower());
            }
        }

        self.ranges = result;
    }

    /// The union of two lists. Mirrors `ConstantRangeList::unionWith`.
    ///
    /// Upstream asserts the widths agree; this returns `self` unchanged
    /// instead, for the reason [`Self::subtract`] gives.
    #[must_use]
    pub fn union_with(&self, other: &Self) -> Self {
        // `if (empty()) return CRL; if (CRL.empty()) return *this;`
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        // `assert(getBitWidth() == CRL.getBitWidth() && …);`
        if self.bit_width() != other.bit_width() {
            return self.clone();
        }

        let mut result: Vec<ConstantRange> = Vec::new();
        let mut i = 0usize;
        let mut j = 0usize;
        // "PreviousRange" tracks the lowest unioned range that is being
        // processed. Its lower is fixed and the upper may be updated over
        // iterations.
        let mut previous_range = if self.ranges[i].lower().slt(other.ranges[j].lower()) {
            let range = self.ranges[i].clone();
            i += 1;
            range
        } else {
            let range = other.ranges[j].clone();
            j += 1;
            range
        };

        // Try to union "PreviousRange" and "CR". If they are disjoint, push
        // "PreviousRange" to the result and assign it to "CR", a new union
        // range. Otherwise, update the upper of "PreviousRange" to cover "CR".
        // Note that, the lower of "PreviousRange" is always less or equal the
        // lower of "CR".
        let union_and_update_range =
            |result: &mut Vec<ConstantRange>, previous: &mut ConstantRange, cr: &ConstantRange| {
                if previous.upper().slt(cr.lower()) {
                    result.push(previous.clone());
                    *previous = cr.clone();
                } else {
                    *previous =
                        merged_range(previous.lower().clone(), smax(previous.upper(), cr.upper()));
                }
            };

        while i < self.len() || j < other.len() {
            if j == other.len()
                || (i < self.len() && self.ranges[i].lower().slt(other.ranges[j].lower()))
            {
                // Merge PreviousRange with this.
                let range = self.ranges[i].clone();
                i += 1;
                union_and_update_range(&mut result, &mut previous_range, &range);
            } else {
                // Merge PreviousRange with CRL.
                let range = other.ranges[j].clone();
                j += 1;
                union_and_update_range(&mut result, &mut previous_range, &range);
            }
        }
        result.push(previous_range);
        Self { ranges: result }
    }

    /// The intersection of two lists. Mirrors
    /// `ConstantRangeList::intersectWith`.
    ///
    /// Upstream's comment explains why it does not delegate to
    /// `ConstantRange::intersectWith`: that routine handles the wrapped-upper
    /// case and can yield *two* ranges, where this one wants the plain signed
    /// `(max(lowers), min(uppers))`.
    ///
    /// Upstream asserts the widths agree; this returns an empty list instead,
    /// for the reason [`Self::subtract`] gives.
    #[must_use]
    pub fn intersect_with(&self, other: &Self) -> Self {
        // `if (empty()) return *this; if (CRL.empty()) return CRL;`
        if self.is_empty() {
            return self.clone();
        }
        if other.is_empty() {
            return other.clone();
        }
        // `assert(getBitWidth() == CRL.getBitWidth() && …);`
        if self.bit_width() != other.bit_width() {
            return Self::default();
        }

        let mut result: Vec<ConstantRange> = Vec::new();
        let mut i = 0usize;
        let mut j = 0usize;
        while i < self.len() && j < other.len() {
            let range = &self.ranges[i];
            let other_range = &other.ranges[j];

            // The intersection of two Ranges is (max(lowers), min(uppers)), and
            // it's possible that max(lowers) > min(uppers) if they don't have
            // intersection. Add the intersection to result only if it's
            // non-empty.
            let start = smax(range.lower(), other_range.lower());
            let end = smin(range.upper(), other_range.upper());
            if start.slt(&end) {
                result.push(merged_range(start, end));
            }

            // Move to the next Range in one list determined by the uppers.
            // For example: A = {(0, 2), (4, 8)}; B = {(-2, 5), (6, 10)}
            // We need to intersect three pairs: A0 && B0; A1 && B0; A1 && B1.
            if range.upper().slt(other_range.upper()) {
                i += 1;
            } else {
                j += 1;
            }
        }
        Self { ranges: result }
    }
}

/// `APIntOps::smax`.
fn smax(lhs: &ApInt, rhs: &ApInt) -> ApInt {
    if lhs.sgt(rhs) {
        lhs.clone()
    } else {
        rhs.clone()
    }
}

/// `APIntOps::smin`.
fn smin(lhs: &ApInt, rhs: &ApInt) -> ApInt {
    if lhs.slt(rhs) {
        lhs.clone()
    } else {
        rhs.clone()
    }
}

/// The 64-bit signed `APInt(64, V, /*isSigned=*/true)` upstream's `int64_t`
/// `insert` overload builds.
fn ap_int_from_i64(value: i64) -> ApInt {
    ApInt::from_words(64, &[i64::cast_unsigned(value)])
}

/// `ConstantRange(Lower, Upper)` as upstream writes it inside `insert`,
/// `subtract`, `unionWith` and `intersectWith`, where the bounds are strictly
/// ordered by construction and the constructor's assert is relied on never to
/// fire. Every caller here either takes an `smax` of an upper already above the
/// lower, or sits behind an explicit `lower.slt(upper)` test.
fn merged_range(lower: ApInt, upper: ApInt) -> ConstantRange {
    match ConstantRange::new(lower, upper) {
        Ok(range) => range,
        // Both bounds are built at the same width, and `smax` never lowers an
        // upper bound below a lower bound that was already strictly less than
        // it, so neither of `ConstantRange::new`'s two rejections is
        // reachable.
        Err(_) => unreachable!("merging keeps range bounds strictly ordered"),
    }
}

impl fmt::Display for ConstantRangeList {
    /// Mirrors `ConstantRangeList::print`: `(lo, hi)` elements joined by
    /// `", "`, both bounds signed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (position, range) in self.ranges.iter().enumerate() {
            if position > 0 {
                f.write_str(", ")?;
            }
            write!(
                f,
                "({}, {})",
                range.lower().to_string_radix(10, Signedness::Signed),
                range.upper().to_string_radix(10, Signedness::Signed)
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(lower: i64, upper: i64) -> ConstantRange {
        ConstantRange::new(ap_int_from_i64(lower), ap_int_from_i64(upper))
            .expect("test ranges are well formed")
    }

    /// Ports `ConstantRangeListTest.Basics`
    /// (`unittests/IR/ConstantRangeListTest.cpp`).
    #[test]
    fn basics() {
        let mut crl1a = ConstantRangeList::default();
        crl1a.insert_signed(0, 12);
        assert!(!crl1a.is_empty());

        let mut crl1b = ConstantRangeList::default();
        crl1b.insert_signed(0, 4);
        crl1b.insert_signed(4, 8);
        crl1b.insert_signed(8, 12);
        assert!(crl1a == crl1b);

        let mut crl1c = ConstantRangeList::default();
        crl1c.insert_signed(0, 4);
        crl1c.insert_signed(8, 12);
        crl1c.insert_signed(4, 8);
        assert!(crl1a == crl1c);

        let mut crl2 = ConstantRangeList::default();
        crl2.insert_signed(-4, 0);
        crl2.insert_signed(8, 12);
        assert!(crl1a != crl2);
    }

    /// Ports `ConstantRangeListTest.getConstantRangeList`
    /// (`unittests/IR/ConstantRangeListTest.cpp`).
    #[test]
    fn get_constant_range_list() {
        assert!(ConstantRangeList::new(Vec::new()).is_some());

        let valid = vec![range(0, 4), range(8, 12)];
        assert!(ConstantRangeList::new(valid).is_some());

        let invalid1 = vec![range(4, 0)];
        assert!(ConstantRangeList::new(invalid1).is_none());

        let invalid2 = vec![range(0, 4), range(12, 8)];
        assert!(ConstantRangeList::new(invalid2).is_none());

        let invalid3 = vec![range(0, 4), range(4, 8)];
        assert!(ConstantRangeList::new(invalid3).is_none());

        let invalid4 = vec![range(0, 12), range(8, 16)];
        assert!(ConstantRangeList::new(invalid4).is_none());
    }

    /// Ports `ConstantRangeListTest.Insert`
    /// (`unittests/IR/ConstantRangeListTest.cpp`).
    #[test]
    fn insert() {
        let mut crl = ConstantRangeList::default();
        crl.insert_signed(0, 4);
        crl.insert_signed(8, 12);
        // No overlap, left
        crl.insert_signed(-8, -4);
        // No overlap, right
        crl.insert_signed(16, 20);
        // No overlap, middle
        crl.insert_signed(13, 15);
        // Overlap with left
        crl.insert_signed(-6, -2);
        // Overlap with right
        crl.insert_signed(5, 9);
        // Overlap with left and right
        crl.insert_signed(14, 18);
        // Overlap cross ranges
        crl.insert_signed(2, 14);
        // An existing range
        crl.insert_signed(0, 20);

        let mut expected = ConstantRangeList::default();
        expected.insert_signed(-8, -2);
        expected.insert_signed(0, 20);
        assert!(crl == expected);
    }

    /// Upstream's `GetCRL` test helper: the list built from `(lower, upper)`
    /// pairs at 64 bits.
    fn crl(pairs: &[(i64, i64)]) -> ConstantRangeList {
        ConstantRangeList::new(pairs.iter().map(|(lo, hi)| range(*lo, *hi)).collect())
            .expect("test range lists are ordered")
    }

    /// Ports `ConstantRangeListTest.Subtract`
    /// (`unittests/IR/ConstantRangeListTest.cpp`).
    #[test]
    fn subtract() {
        let base = crl(&[(0, 4), (8, 12)]);

        // Execute ConstantRangeList::subtract(ConstantRange) and check the
        // result is expected. Takes "crl" by value so that subtract() does not
        // affect the argument in caller.
        let subtract_and_check =
            |mut list: ConstantRangeList, sub: (i64, i64), expected: ConstantRangeList| {
                list.subtract(&range(sub.0, sub.1));
                assert_eq!(list, expected, "subtract {sub:?}");
            };

        // No overlap
        subtract_and_check(base.clone(), (-4, 0), base.clone());
        subtract_and_check(base.clone(), (4, 8), base.clone());
        subtract_and_check(base.clone(), (12, 16), base.clone());

        // Overlap (left, right, or both)
        subtract_and_check(base.clone(), (-4, 2), crl(&[(2, 4), (8, 12)]));
        subtract_and_check(base.clone(), (-4, 4), crl(&[(8, 12)]));
        subtract_and_check(base.clone(), (-4, 8), crl(&[(8, 12)]));
        subtract_and_check(base.clone(), (0, 2), crl(&[(2, 4), (8, 12)]));
        subtract_and_check(base.clone(), (0, 4), crl(&[(8, 12)]));
        subtract_and_check(base.clone(), (0, 8), crl(&[(8, 12)]));
        subtract_and_check(base.clone(), (10, 12), crl(&[(0, 4), (8, 10)]));
        subtract_and_check(base.clone(), (8, 12), crl(&[(0, 4)]));
        subtract_and_check(base.clone(), (6, 12), crl(&[(0, 4)]));
        subtract_and_check(base.clone(), (10, 16), crl(&[(0, 4), (8, 10)]));
        subtract_and_check(base.clone(), (8, 16), crl(&[(0, 4)]));
        subtract_and_check(base.clone(), (6, 16), crl(&[(0, 4)]));
        subtract_and_check(base.clone(), (2, 10), crl(&[(0, 2), (10, 12)]));

        // Subset
        subtract_and_check(base.clone(), (2, 3), crl(&[(0, 2), (3, 4), (8, 12)]));
        subtract_and_check(base.clone(), (10, 11), crl(&[(0, 4), (8, 10), (11, 12)]));

        // Superset
        subtract_and_check(base.clone(), (0, 12), crl(&[]));
        subtract_and_check(base, (-4, 16), crl(&[]));
    }

    /// Ports `ConstantRangeListTest.Union`
    /// (`unittests/IR/ConstantRangeListTest.cpp`).
    #[test]
    fn union() {
        let base = crl(&[(0, 4), (8, 12)]);

        // Union with a subset.
        let empty = ConstantRangeList::default();
        assert_eq!(base.union_with(&empty), base);
        assert_eq!(empty.union_with(&base), base);

        assert_eq!(base.union_with(&crl(&[(0, 2)])), base);
        assert_eq!(base.union_with(&crl(&[(10, 12)])), base);

        assert_eq!(base.union_with(&crl(&[(0, 2), (8, 10)])), base);
        assert_eq!(base.union_with(&crl(&[(0, 2), (10, 12)])), base);
        assert_eq!(base.union_with(&crl(&[(2, 4), (8, 10)])), base);
        assert_eq!(base.union_with(&crl(&[(2, 4), (10, 12)])), base);

        assert_eq!(base.union_with(&crl(&[(0, 4), (8, 10), (11, 12)])), base);

        assert_eq!(base.union_with(&base), base);

        // Union with new ranges.
        assert_eq!(
            base.union_with(&crl(&[(-4, -2)])),
            crl(&[(-4, -2), (0, 4), (8, 12)])
        );
        assert_eq!(
            base.union_with(&crl(&[(6, 7)])),
            crl(&[(0, 4), (6, 7), (8, 12)])
        );
        assert_eq!(
            base.union_with(&crl(&[(16, 18)])),
            crl(&[(0, 4), (8, 12), (16, 18)])
        );

        assert_eq!(base.union_with(&crl(&[(-2, 2)])), crl(&[(-2, 4), (8, 12)]));
        assert_eq!(base.union_with(&crl(&[(2, 6)])), crl(&[(0, 6), (8, 12)]));
        assert_eq!(base.union_with(&crl(&[(10, 16)])), crl(&[(0, 4), (8, 16)]));

        assert_eq!(base.union_with(&crl(&[(-2, 10)])), crl(&[(-2, 12)]));
        assert_eq!(base.union_with(&crl(&[(2, 10)])), crl(&[(0, 12)]));
        assert_eq!(base.union_with(&crl(&[(4, 16)])), crl(&[(0, 16)]));
        assert_eq!(base.union_with(&crl(&[(-2, 16)])), crl(&[(-2, 16)]));
    }

    /// Ports `ConstantRangeListTest.Intersect`
    /// (`unittests/IR/ConstantRangeListTest.cpp`).
    #[test]
    fn intersect() {
        let base = crl(&[(0, 4), (8, 12)]);

        // No intersection.
        let empty = ConstantRangeList::default();
        assert_eq!(base.intersect_with(&empty), empty);
        assert_eq!(empty.intersect_with(&base), empty);

        assert_eq!(base.intersect_with(&crl(&[(-2, 0)])), empty);
        assert_eq!(base.intersect_with(&crl(&[(6, 8)])), empty);
        assert_eq!(base.intersect_with(&crl(&[(12, 16)])), empty);

        // Single intersect range.
        assert_eq!(base.intersect_with(&crl(&[(-2, 2)])), crl(&[(0, 2)]));
        assert_eq!(base.intersect_with(&crl(&[(-2, 6)])), crl(&[(0, 4)]));
        assert_eq!(base.intersect_with(&crl(&[(2, 4)])), crl(&[(2, 4)]));
        assert_eq!(base.intersect_with(&crl(&[(2, 6)])), crl(&[(2, 4)]));
        assert_eq!(base.intersect_with(&crl(&[(6, 10)])), crl(&[(8, 10)]));
        assert_eq!(base.intersect_with(&crl(&[(6, 16)])), crl(&[(8, 12)]));
        assert_eq!(base.intersect_with(&crl(&[(10, 12)])), crl(&[(10, 12)]));
        assert_eq!(base.intersect_with(&crl(&[(10, 16)])), crl(&[(10, 12)]));

        // Multiple intersect ranges.
        assert_eq!(
            base.intersect_with(&crl(&[(-2, 10)])),
            crl(&[(0, 4), (8, 10)])
        );
        assert_eq!(base.intersect_with(&crl(&[(-2, 16)])), base);
        assert_eq!(
            base.intersect_with(&crl(&[(2, 10)])),
            crl(&[(2, 4), (8, 10)])
        );
        assert_eq!(
            base.intersect_with(&crl(&[(2, 16)])),
            crl(&[(2, 4), (8, 12)])
        );
        assert_eq!(
            base.intersect_with(&crl(&[(-2, 2), (6, 10)])),
            crl(&[(0, 2), (8, 10)])
        );
        assert_eq!(
            base.intersect_with(&crl(&[(2, 6), (10, 16)])),
            crl(&[(2, 4), (10, 12)])
        );
        assert_eq!(
            base.intersect_with(&crl(&[(-2, 2), (7, 10), (11, 16)])),
            crl(&[(0, 2), (8, 10), (11, 12)])
        );
        assert_eq!(base.intersect_with(&base), base);
    }

    /// llvmkit-specific: no upstream counterpart. `ConstantRangeList::print`
    /// has no unit test upstream; the shape it must produce is pinned by
    /// `test/Bitcode/attributes.ll`'s `@initializes` CHECK line, which this
    /// reproduces at the list level.
    #[test]
    fn display_matches_print() {
        let list = ConstantRangeList::new(vec![range(-4, 0), range(4, 8)])
            .expect("the ranges are ordered");
        assert_eq!(format!("{list}"), "(-4, 0), (4, 8)");
    }
}
