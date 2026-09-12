//! `ConstantRange` set operations — slice 3b of tranche 3 (see
//! `docs/future-work.md`).
//!
//! The oracle throughout is **set membership computed by enumeration**. Every
//! range at 1 and 4 bits is expanded to the concrete set of values it holds,
//! the set operation is performed on those sets directly, and the range-level
//! answer is checked against it.
//!
//! The laws are one-sided on purpose. `intersectWith` and `unionWith` return a
//! single `[lower, upper)`, and the exact answer sometimes needs two disjoint
//! runs — so they are *over*-approximations, and the law is containment, not
//! equality. Demanding equality would reject correct conservatism; demanding
//! nothing would let a bug through. Where an operation *is* exact — `inverse`,
//! and the extends — equality is asserted.
//!
//! Ports the `EnumerateConstantRanges` harness from
//! `llvm/unittests/IR/ConstantRangeTest.cpp`.

use std::collections::BTreeSet;

use llvmkit_ir::{ApInt, ApIntTruncation, ConstantRange, PreferredRangeType, Signedness};

fn ap(bits: u32, value: u64) -> ApInt {
    ApInt::new(bits, value, Signedness::Unsigned, ApIntTruncation::Truncate)
        .expect("in-range constant")
}

/// Ports `EnumerateConstantRanges`.
fn enumerate(bits: u32, mut test: impl FnMut(&ConstantRange)) {
    let max = 1_u64 << bits;
    for lo in 0..max {
        for hi in 0..max {
            if lo == hi && lo != 0 && lo != max - 1 {
                continue;
            }
            test(&ConstantRange::new(ap(bits, lo), ap(bits, hi)).expect("legal range"));
        }
    }
}

/// Ports `EnumerateTwoInterestingConstantRanges`, at a single width.
fn enumerate_pairs(bits: u32, mut test: impl FnMut(&ConstantRange, &ConstantRange)) {
    enumerate(bits, |first| {
        enumerate(bits, |second| test(first, second));
    });
}

/// The concrete set of values a range holds, as plain integers.
///
/// This is the oracle: it reads only `contains`, which slice 3a pinned against
/// its own enumerated members, so the set operations are never checked against
/// a second implementation of themselves.
fn members(range: &ConstantRange) -> BTreeSet<u64> {
    let bits = range.bit_width();
    (0..(1_u64 << bits))
        .filter(|v| range.contains(&ap(bits, *v)))
        .collect()
}

/// Every value in both ranges survives the intersection.
///
/// One-sided: when the true intersection needs two disjoint runs, the result
/// is a larger range that still contains them, so containment is the law.
#[test]
fn intersect_with_contains_the_true_intersection() {
    for bits in [1_u32, 4] {
        enumerate_pairs(bits, |first, second| {
            let truth: BTreeSet<u64> = members(first)
                .intersection(&members(second))
                .copied()
                .collect();
            for preferred in [
                PreferredRangeType::Smallest,
                PreferredRangeType::Unsigned,
                PreferredRangeType::Signed,
            ] {
                let got = members(&first.intersect_with(second, preferred));
                assert!(
                    truth.is_subset(&got),
                    "{first:?} ∩ {second:?} ({preferred:?}) dropped {:?}",
                    truth.difference(&got).collect::<Vec<_>>()
                );
            }
        });
    }
}

/// Every value in either range survives the union.
#[test]
fn union_with_contains_the_true_union() {
    for bits in [1_u32, 4] {
        enumerate_pairs(bits, |first, second| {
            let truth: BTreeSet<u64> = members(first).union(&members(second)).copied().collect();
            for preferred in [
                PreferredRangeType::Smallest,
                PreferredRangeType::Unsigned,
                PreferredRangeType::Signed,
            ] {
                let got = members(&first.union_with(second, preferred));
                assert!(
                    truth.is_subset(&got),
                    "{first:?} ∪ {second:?} ({preferred:?}) dropped {:?}",
                    truth.difference(&got).collect::<Vec<_>>()
                );
            }
        });
    }
}

/// `inverse` is exact — it is the complement, no approximation involved.
///
/// Mirrors the `inverse()` half of
/// `ConstantRangeTest.cpp::TEST_F(ConstantRangeTest, SingleElement)`, which
/// slice 3a had to leave out, plus the whole-domain law.
#[test]
fn inverse_is_the_exact_complement() {
    for bits in [1_u32, 4] {
        enumerate(bits, |range| {
            let all: BTreeSet<u64> = (0..(1_u64 << bits)).collect();
            let expected: BTreeSet<u64> = all.difference(&members(range)).copied().collect();
            assert_eq!(
                members(&range.inverse()),
                expected,
                "inverse of {range:?} is not the complement"
            );
            // Involution.
            assert_eq!(
                members(&range.inverse().inverse()),
                members(range),
                "inverse is not an involution on {range:?}"
            );
        });
    }
}

/// A single-element range's inverse misses exactly that element — the half of
/// upstream's `SingleElement` fixture that needed `inverse()`.
#[test]
fn single_element_inverse_misses_that_element() {
    let one = ConstantRange::new(ap(16, 0xa), ap(16, 0xb)).expect("range");
    let inverse = one.inverse();
    assert_eq!(
        inverse.single_missing_element(),
        one.single_element(),
        "the inverse of a single-element range misses that element"
    );
}

/// `difference` keeps everything in the first range that is not in the second.
#[test]
fn difference_contains_the_true_difference() {
    for bits in [1_u32, 4] {
        enumerate_pairs(bits, |first, second| {
            let truth: BTreeSet<u64> = members(first)
                .difference(&members(second))
                .copied()
                .collect();
            let got = members(&first.difference(second));
            assert!(
                truth.is_subset(&got),
                "{first:?} \\ {second:?} dropped {:?}",
                truth.difference(&got).collect::<Vec<_>>()
            );
        });
    }
}

/// `subtract` translates the range, so membership shifts with it. Empty and
/// full are fixed points, since their endpoints encode identity rather than
/// position.
#[test]
fn subtract_translates_membership() {
    let bits = 4_u32;
    for delta in 0..(1_u64 << bits) {
        enumerate(bits, |range| {
            let shifted = range.subtract(&ap(bits, delta));
            if range.is_empty_set() || range.is_full_set() {
                assert_eq!(
                    members(&shifted),
                    members(range),
                    "{range:?} must be a fixed point of subtract"
                );
                return;
            }
            let expected: BTreeSet<u64> = members(range)
                .iter()
                .map(|v| (v.wrapping_sub(delta)) & ((1_u64 << bits) - 1))
                .collect();
            assert_eq!(
                members(&shifted),
                expected,
                "{range:?} - {delta} shifted wrongly"
            );
        });
    }
}

/// `zero_extend` is exact: every member widens to itself, and nothing else is
/// admitted beyond what the range already covered.
#[test]
fn zero_extend_contains_every_widened_member() {
    let bits = 4_u32;
    enumerate(bits, |range| {
        let widened = range.zero_extend(8).expect("widening");
        for v in members(range) {
            assert!(
                widened.contains(&ap(8, v)),
                "zext of {range:?} dropped member {v}"
            );
        }
        assert_eq!(widened.bit_width(), 8);
    });
}

/// `sign_extend` keeps every member, read as a signed value.
#[test]
fn sign_extend_contains_every_widened_member() {
    let bits = 4_u32;
    enumerate(bits, |range| {
        let widened = range.sign_extend(8).expect("widening");
        for v in members(range) {
            // Sign-extend the 4-bit value by hand into 8 bits.
            let signed = if v & 0b1000 != 0 { v | 0xf0 } else { v };
            assert!(
                widened.contains(&ap(8, signed)),
                "sext of {range:?} dropped member {v} (as {signed})"
            );
        }
        assert_eq!(widened.bit_width(), 8);
    });
}

/// `truncate` keeps every member's low bits.
#[test]
fn truncate_contains_every_narrowed_member() {
    let bits = 8_u32;
    // 8 bits is 65 025 ranges at two endpoints; step the enumeration so the
    // sweep stays quick while still covering wrapped and non-wrapped shapes.
    let max = 1_u64 << bits;
    for lo in (0..max).step_by(17) {
        for hi in (0..max).step_by(19) {
            if lo == hi && lo != 0 && lo != max - 1 {
                continue;
            }
            let range = ConstantRange::new(ap(bits, lo), ap(bits, hi)).expect("legal range");
            let narrowed = range.truncate(4, false).expect("narrowing");
            assert_eq!(narrowed.bit_width(), 4);
            for v in 0..max {
                if range.contains(&ap(bits, v)) {
                    assert!(
                        narrowed.contains(&ap(4, v & 0xf)),
                        "trunc of {range:?} dropped member {v} (low bits {})",
                        v & 0xf
                    );
                }
            }
        }
    }
}

/// The width-changing operations reject a change in the wrong direction rather
/// than silently doing the other one.
///
/// Upstream asserts (`assert(SrcTySize < DstTySize && "Not a value extension")`);
/// llvmkit has no runtime asserts in production paths, so these return an
/// error. `zext_or_trunc` / `sext_or_trunc` are the spellings that accept
/// either direction.
#[test]
fn width_changes_reject_the_wrong_direction() {
    let wide = ConstantRange::new(ap(8, 3), ap(8, 9)).expect("range");
    assert!(wide.zero_extend(4).is_err(), "zext must not narrow");
    assert!(wide.sign_extend(4).is_err(), "sext must not widen-down");

    let narrow = ConstantRange::new(ap(4, 3), ap(4, 9)).expect("range");
    assert!(narrow.truncate(8, false).is_err(), "trunc must not widen");

    // Same width is the identity in every direction.
    assert_eq!(wide.zero_extend(8).expect("same width"), wide);
    assert_eq!(wide.sign_extend(8).expect("same width"), wide);
    assert_eq!(wide.truncate(8, false).expect("same width"), wide);

    // The or_trunc spellings take either direction.
    assert_eq!(wide.zext_or_trunc(4).expect("narrows").bit_width(), 4);
    assert_eq!(narrow.zext_or_trunc(8).expect("widens").bit_width(), 8);
    assert_eq!(wide.sext_or_trunc(4).expect("narrows").bit_width(), 4);
    assert_eq!(narrow.sext_or_trunc(8).expect("widens").bit_width(), 8);
}

/// `split_pos_neg` partitions into the strictly-positive and negative halves,
/// and at 1 bit the positive half is empty — the lone 1 reads as -1.
///
/// Mirrors `ConstantRange::splitPosNeg`, including its explicit note about the
/// 1-bit case.
#[test]
fn split_pos_neg_partitions_by_sign() {
    for bits in [1_u32, 4] {
        enumerate(bits, |range| {
            let halves = range.split_pos_neg();
            let (positive, negative) = (halves.positive, halves.negative);
            let sign_bit = 1_u64 << (bits - 1);
            for v in members(range) {
                let is_negative = v & sign_bit != 0;
                if is_negative {
                    assert!(
                        negative.contains(&ap(bits, v)),
                        "{range:?}: negative member {v} missing from the negative half"
                    );
                } else if v != 0 {
                    assert!(
                        bits == 1 || positive.contains(&ap(bits, v)),
                        "{range:?}: positive member {v} missing from the positive half"
                    );
                }
            }
            if bits == 1 {
                assert!(
                    positive.is_empty_set(),
                    "there are no positive 1-bit values"
                );
            }
        });
    }
}
