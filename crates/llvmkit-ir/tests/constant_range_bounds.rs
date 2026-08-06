//! `ConstantRange` bounds and predicates — slice 3a of tranche 3 (see
//! `docs/future-work.md`).
//!
//! Ports from `llvm/unittests/IR/ConstantRangeTest.cpp`, including its
//! `EnumerateConstantRanges` harness: every legal `[lower, upper)` pair at 1
//! and 4 bits, which is small enough to be exhaustive and wide enough to catch
//! the wrap-around cases. Upstream calls that pairing
//! `EnumerateInterestingConstantRanges` and uses it throughout the file.

use llvmkit_ir::{ApInt, ApIntTruncation, ConstantRange, Signedness};

/// An `n`-bit value from a `u64`.
fn ap(bits: u32, value: u64) -> ApInt {
    ApInt::new(bits, value, Signedness::Unsigned, ApIntTruncation::Truncate)
        .expect("in-range constant")
}

/// Ports `EnumerateConstantRanges`: every legal range at `bits` wide.
///
/// The `lo == hi` pairs other than all-zero and all-ones are skipped because
/// they are not distinct ranges — upstream calls this "enforce ConstantRange
/// invariant".
fn enumerate_constant_ranges(bits: u32, mut test: impl FnMut(&ConstantRange)) {
    let max = 1_u64 << bits;
    for lo in 0..max {
        for hi in 0..max {
            if lo == hi && lo != 0 && lo != max - 1 {
                continue;
            }
            let range = ConstantRange::new(ap(bits, lo), ap(bits, hi)).expect("same width");
            test(&range);
        }
    }
}

/// Ports `EnumerateInterestingConstantRanges`: 1-bit ranges for their special
/// cases, then 4-bit for coverage without being slow.
fn enumerate_interesting(mut test: impl FnMut(&ConstantRange)) {
    enumerate_constant_ranges(1, &mut test);
    enumerate_constant_ranges(4, &mut test);
}

/// Ports `ForeachNumInConstantRange`: every member of the range, in order.
fn foreach_member(range: &ConstantRange, mut test: impl FnMut(&ApInt)) {
    if range.is_empty_set() {
        return;
    }
    let bits = range.bit_width();
    let one = ap(bits, 1);
    let mut n = range.lower().clone();
    loop {
        test(&n);
        n = n.wrapping_add(&one);
        if n.eq_ap_int(range.upper()) {
            break;
        }
    }
}

/// The fixture ranges upstream's `ConstantRangeTest` sets up.
fn full() -> ConstantRange {
    ConstantRange::full(16)
}
fn empty() -> ConstantRange {
    ConstantRange::empty(16)
}
fn one() -> ConstantRange {
    ConstantRange::new(ap(16, 0xa), ap(16, 0xb)).expect("range")
}
fn some() -> ConstantRange {
    ConstantRange::new(ap(16, 0xa), ap(16, 0xaaa)).expect("range")
}
fn wrap() -> ConstantRange {
    ConstantRange::new(ap(16, 0xaaa), ap(16, 0xa)).expect("range")
}

/// Port of `ConstantRangeTest.cpp::TEST_F(ConstantRangeTest, GetMinsAndMaxes)`,
/// the signed half — the unsigned half was already modeled.
#[test]
fn get_mins_and_maxes() {
    assert_eq!(full().unsigned_max(), ap(16, u64::from(u16::MAX)));
    assert_eq!(one().unsigned_max(), ap(16, 0xa));
    assert_eq!(some().unsigned_max(), ap(16, 0xaa9));
    assert_eq!(wrap().unsigned_max(), ap(16, u64::from(u16::MAX)));

    assert_eq!(full().unsigned_min(), ap(16, 0));
    assert_eq!(one().unsigned_min(), ap(16, 0xa));
    assert_eq!(some().unsigned_min(), ap(16, 0xa));
    assert_eq!(wrap().unsigned_min(), ap(16, 0));

    assert_eq!(full().signed_max(), ap(16, 0x7fff));
    assert_eq!(one().signed_max(), ap(16, 0xa));
    assert_eq!(some().signed_max(), ap(16, 0xaa9));
    assert_eq!(wrap().signed_max(), ap(16, 0x7fff));

    assert_eq!(full().signed_min(), ap(16, 0x8000));
    assert_eq!(one().signed_min(), ap(16, 0xa));
    assert_eq!(some().signed_min(), ap(16, 0xa));
    assert_eq!(wrap().signed_min(), ap(16, 0x8000));

    // Upstream's comment: "Found by Klee".
    assert_eq!(
        ConstantRange::new(ap(4, 7), ap(4, 0))
            .expect("range")
            .signed_max(),
        ap(4, 7)
    );
}

/// Port of `ConstantRangeTest.cpp::TEST_F(ConstantRangeTest, SignWrapped)`.
#[test]
fn sign_wrapped() {
    assert!(!full().is_sign_wrapped_set());
    assert!(!empty().is_sign_wrapped_set());
    assert!(!one().is_sign_wrapped_set());
    assert!(!some().is_sign_wrapped_set());
    assert!(wrap().is_sign_wrapped_set());

    let case = |lo: u64, hi: u64| {
        ConstantRange::new(ap(8, lo), ap(8, hi))
            .expect("range")
            .is_sign_wrapped_set()
    };
    assert!(!case(127, 128));
    assert!(case(127, 129));
    assert!(!case(128, 129));
    assert!(case(10, 9));
    assert!(case(10, 250));
    assert!(!case(250, 10));
    assert!(!case(250, 251));
}

/// Port of `ConstantRangeTest.cpp::TEST_F(ConstantRangeTest, SingleElement)`,
/// minus the `inverse()` half, which lands in slice 3b.
#[test]
fn single_element() {
    assert!(full().single_element().is_none());
    assert!(empty().single_element().is_none());
    assert!(full().single_missing_element().is_none());
    assert!(empty().single_missing_element().is_none());

    assert_eq!(one().single_element(), Some(&ap(16, 0xa)));
    assert!(some().single_element().is_none());
    assert!(wrap().single_element().is_none());

    assert!(one().single_missing_element().is_none());
    assert!(some().single_missing_element().is_none());

    assert!(!full().is_single_element());
    assert!(!empty().is_single_element());
    assert!(one().is_single_element());
    assert!(!some().is_single_element());
    assert!(!wrap().is_single_element());
}

/// The bounds agree with the members, over every 1- and 4-bit range.
///
/// This is the shape upstream uses throughout `ConstantRangeTest.cpp`: walk
/// the range's members and check the summary accessors against them. The
/// oracle is the enumeration, not a second implementation.
#[test]
fn bounds_agree_with_membership_exhaustively() {
    enumerate_interesting(|range| {
        if range.is_empty_set() {
            assert_eq!(range.active_bits(), 0, "empty: active_bits");
            assert_eq!(range.min_signed_bits(), 0, "empty: min_signed_bits");
            // Vacuous truth on an empty set, as upstream documents.
            assert!(range.is_all_negative(), "empty: is_all_negative");
            assert!(range.is_all_positive(), "empty: is_all_positive");
            return;
        }

        let unsigned_min = range.unsigned_min();
        let unsigned_max = range.unsigned_max();
        let signed_min = range.signed_min();
        let signed_max = range.signed_max();

        let mut count = 0_u64;
        let mut saw_non_negative = false;
        let mut saw_non_positive = false;
        foreach_member(range, |n| {
            count += 1;
            assert!(range.contains(n), "{range:?} must contain its own member");
            assert!(
                unsigned_min.ule(n) && n.ule(&unsigned_max),
                "unsigned bounds"
            );
            assert!(signed_min.sle(n) && n.sle(&signed_max), "signed bounds");
            assert!(n.active_bits() <= range.active_bits(), "active_bits bound");
            assert!(
                n.significant_bits() <= range.min_signed_bits(),
                "min_signed_bits bound"
            );
            if n.is_non_negative() {
                saw_non_negative = true;
            } else {
                saw_non_positive = true;
            }
        });

        assert_eq!(
            range.is_all_negative(),
            !saw_non_negative,
            "is_all_negative for {range:?}"
        );
        assert_eq!(
            range.is_all_non_negative(),
            !saw_non_positive,
            "is_all_non_negative for {range:?}"
        );
        assert_eq!(
            range.is_single_element(),
            count == 1,
            "is_single_element for {range:?}"
        );
        assert!(
            !range.is_size_larger_than(count),
            "size must not exceed its own member count"
        );
        if count > 0 {
            assert!(
                range.is_size_larger_than(count - 1),
                "size must exceed one less than its member count"
            );
        }
    });
}

/// Port of the `fromKnownBits` / `toKnownBits` round-trip law in
/// `ConstantRangeTest.cpp`: every value the known-bits set admits must be
/// inside the range built from it, and every member of a range must be
/// admitted by the range's own known bits.
#[test]
fn known_bits_conversions_are_sound_exhaustively() {
    enumerate_constant_ranges(4, |range| {
        // toKnownBits: every member satisfies the derived known bits.
        let known = range.to_known_bits();
        foreach_member(range, |n| {
            assert!(
                known.zero_mask().bitand(n).is_zero(),
                "{range:?}: member {n:?} sets a known-zero bit"
            );
            assert!(
                known.one_mask().bitand(n).eq_ap_int(known.one_mask()),
                "{range:?}: member {n:?} clears a known-one bit"
            );
        });

        // fromKnownBits: the range built from those bits still holds every
        // member, in both the signed and unsigned domain.
        for signedness in [
            llvmkit_ir::Signedness::Unsigned,
            llvmkit_ir::Signedness::Signed,
        ] {
            let rebuilt = ConstantRange::from_known_bits(&known, signedness);
            foreach_member(range, |n| {
                assert!(
                    rebuilt.contains(n),
                    "{range:?}: from_known_bits({signedness:?}) dropped member {n:?}"
                );
            });
        }
    });
}

/// `getNonEmpty` reads equal endpoints as the full set. Mirrors
/// `ConstantRange::getNonEmpty`.
#[test]
fn non_empty_reads_equal_endpoints_as_full() {
    let equal = ConstantRange::non_empty(ap(8, 5), ap(8, 5)).expect("same width");
    assert!(equal.is_full_set());

    let distinct = ConstantRange::non_empty(ap(8, 3), ap(8, 9)).expect("same width");
    assert_eq!(distinct.lower(), &ap(8, 3));
    assert_eq!(distinct.upper(), &ap(8, 9));
}

/// The plain constructor rejects an equal pair that is neither the minimum nor
/// the maximum, where upstream asserts.
///
/// Mirrors the second assertion in
/// `ConstantRange::ConstantRange(APInt L, APInt U)`:
/// `Lower != Upper || (Lower.isMaxValue() || Lower.isMinValue())`. Such a range
/// contains nothing yet answers `false` to both `is_empty_set` and
/// `is_full_set`, so every predicate downstream reads it wrongly. This is the
/// llvmkit spelling of that assert — an error return, since the crate has no
/// runtime asserts in production paths.
#[test]
fn degenerate_equal_endpoints_are_rejected() {
    assert!(ConstantRange::new(ap(8, 5), ap(8, 5)).is_err());
    assert!(ConstantRange::new(ap(8, 1), ap(8, 1)).is_err());
    assert!(ConstantRange::new(ap(8, 254), ap(8, 254)).is_err());

    // The two legal equal pairs stay legal.
    assert!(
        ConstantRange::new(ap(8, 0), ap(8, 0))
            .expect("empty is lower == upper == 0")
            .is_empty_set()
    );
    assert!(
        ConstantRange::new(ap(8, 255), ap(8, 255))
            .expect("full is lower == upper == max")
            .is_full_set()
    );
}
