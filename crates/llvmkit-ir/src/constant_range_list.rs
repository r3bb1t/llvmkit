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
}

/// `APIntOps::smax`.
fn smax(lhs: &ApInt, rhs: &ApInt) -> ApInt {
    if lhs.sgt(rhs) {
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

/// `ConstantRange(Lower, Upper)` as upstream writes it inside `insert`, where
/// the bounds are strictly ordered by construction and the constructor's
/// assert is relied on never to fire.
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
