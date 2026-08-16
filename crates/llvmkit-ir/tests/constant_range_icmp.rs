//! `ConstantRange` ICmp regions — slice 3c of tranche 3 (see
//! `docs/future-work.md`).
//!
//! Same discipline as 3a and 3b: every range at 4 bits is expanded to the set
//! it holds, the predicate is evaluated over concrete pairs, and the
//! range-level answer is checked against that.
//!
//! The three region builders answer three *different* questions, and the tests
//! keep them apart:
//!
//! - `make_allowed_icmp_region(p, R)` — values comparing true against **some**
//!   member of `R`. An over-approximation, so the law is that it loses nothing.
//! - `make_satisfying_icmp_region(p, R)` — values comparing true against
//!   **every** member. An under-approximation, so the law is that it admits
//!   nothing false.
//! - `make_exact_icmp_region(p, c)` — `c` is a single value, so both questions
//!   coincide and equality is asserted.
//!
//! Getting those three backwards is the bug this file exists to catch.

use std::collections::BTreeSet;

use llvmkit_ir::{ApInt, ApIntTruncation, ConstantRange, IntPredicate, Signedness};

const BITS: u32 = 4;
const DOMAIN: u64 = 1 << BITS;

fn ap(value: u64) -> ApInt {
    ApInt::new(BITS, value, Signedness::Unsigned, ApIntTruncation::Truncate)
        .expect("in-range constant")
}

/// Ports `EnumerateConstantRanges` at 4 bits.
fn enumerate(mut test: impl FnMut(&ConstantRange)) {
    for lo in 0..DOMAIN {
        for hi in 0..DOMAIN {
            if lo == hi && lo != 0 && lo != DOMAIN - 1 {
                continue;
            }
            test(&ConstantRange::new(ap(lo), ap(hi)).expect("legal range"));
        }
    }
}

fn members(range: &ConstantRange) -> BTreeSet<u64> {
    (0..DOMAIN).filter(|v| range.contains(&ap(*v))).collect()
}

const PREDICATES: [IntPredicate; 10] = [
    IntPredicate::Eq,
    IntPredicate::Ne,
    IntPredicate::Ult,
    IntPredicate::Ule,
    IntPredicate::Ugt,
    IntPredicate::Uge,
    IntPredicate::Slt,
    IntPredicate::Sle,
    IntPredicate::Sgt,
    IntPredicate::Sge,
];

/// Evaluate `lhs <pred> rhs` on two concrete 4-bit values, by hand.
///
/// This is the oracle. It reads nothing from `ConstantRange`, so the region
/// builders are never checked against themselves. Signed values are recovered
/// by sign-extending the 4-bit pattern into `i64`.
fn evaluate(predicate: IntPredicate, lhs: u64, rhs: u64) -> bool {
    let signed = |v: u64| -> i64 {
        if v & (1 << (BITS - 1)) != 0 {
            (v as i64) - (DOMAIN as i64)
        } else {
            v as i64
        }
    };
    match predicate {
        IntPredicate::Eq => lhs == rhs,
        IntPredicate::Ne => lhs != rhs,
        IntPredicate::Ult => lhs < rhs,
        IntPredicate::Ule => lhs <= rhs,
        IntPredicate::Ugt => lhs > rhs,
        IntPredicate::Uge => lhs >= rhs,
        IntPredicate::Slt => signed(lhs) < signed(rhs),
        IntPredicate::Sle => signed(lhs) <= signed(rhs),
        IntPredicate::Sgt => signed(lhs) > signed(rhs),
        IntPredicate::Sge => signed(lhs) >= signed(rhs),
    }
}

/// The allowed region loses nothing: every value that compares true against
/// *some* member of the range is in it.
///
/// Mirrors `ConstantRange::makeAllowedICmpRegion`.
#[test]
fn allowed_region_contains_every_value_that_can_compare_true() {
    enumerate(|range| {
        let rhs_values = members(range);
        for predicate in PREDICATES {
            let region = ConstantRange::make_allowed_icmp_region(predicate, range);
            let got = members(&region);
            for lhs in 0..DOMAIN {
                let can_be_true = rhs_values.iter().any(|rhs| evaluate(predicate, lhs, *rhs));
                if can_be_true {
                    assert!(
                        got.contains(&lhs),
                        "allowed({predicate:?}, {range:?}) dropped {lhs}, \
                         which compares true against some member"
                    );
                }
            }
        }
    });
}

/// The satisfying region admits nothing false: every value in it compares true
/// against *every* member of the range.
///
/// Mirrors `ConstantRange::makeSatisfyingICmpRegion`. This is the dual of the
/// test above — one checks nothing is lost, the other that nothing bogus is
/// gained, and a builder that confused the two would fail exactly one of them.
#[test]
fn satisfying_region_admits_only_values_that_always_compare_true() {
    enumerate(|range| {
        if range.is_empty_set() {
            return;
        }
        let rhs_values = members(range);
        for predicate in PREDICATES {
            let region = ConstantRange::make_satisfying_icmp_region(predicate, range);
            for lhs in members(&region) {
                let always_true = rhs_values.iter().all(|rhs| evaluate(predicate, lhs, *rhs));
                assert!(
                    always_true,
                    "satisfying({predicate:?}, {range:?}) admitted {lhs}, \
                     which is false against some member"
                );
            }
        }
    });
}

/// Against a single value the two questions coincide, so the exact region is
/// exactly the satisfying set. Mirrors `ConstantRange::makeExactICmpRegion`.
#[test]
fn exact_region_is_exact_for_a_single_value() {
    for rhs in 0..DOMAIN {
        for predicate in PREDICATES {
            let region = ConstantRange::make_exact_icmp_region(predicate, &ap(rhs));
            let expected: BTreeSet<u64> = (0..DOMAIN)
                .filter(|lhs| evaluate(predicate, *lhs, rhs))
                .collect();
            assert_eq!(
                members(&region),
                expected,
                "exact({predicate:?}, {rhs}) is not the exact region"
            );
        }
    }
}

/// `icmp` answers true only when every pairing really does compare true.
///
/// Mirrors `ConstantRange::icmp`. It is allowed to answer false when the
/// truth is true (it under-approximates), so the law is one-sided.
#[test]
fn icmp_is_true_only_when_every_pairing_is() {
    enumerate(|first| {
        enumerate(|second| {
            let lhs_values = members(first);
            let rhs_values = members(second);
            for predicate in PREDICATES {
                if !first.icmp(predicate, second) {
                    continue;
                }
                for lhs in &lhs_values {
                    for rhs in &rhs_values {
                        assert!(
                            evaluate(predicate, *lhs, *rhs),
                            "icmp({predicate:?}) claimed {first:?} vs {second:?} \
                             always holds, but {lhs} vs {rhs} is false"
                        );
                    }
                }
            }
        });
    });
}

/// `icmp` is vacuously true when either side is empty — there is no pairing to
/// falsify it. Mirrors the early return in `ConstantRange::icmp`.
#[test]
fn icmp_is_vacuously_true_on_an_empty_side() {
    let empty = ConstantRange::empty(BITS);
    let some = ConstantRange::new(ap(3), ap(9)).expect("range");
    for predicate in PREDICATES {
        assert!(empty.icmp(predicate, &some), "empty lhs is vacuous");
        assert!(some.icmp(predicate, &empty), "empty rhs is vacuous");
    }
}

/// `make_mask_not_equal_range(mask, c)` holds every value satisfying
/// `(v & mask) != c`. Mirrors `ConstantRange::makeMaskNotEqualRange`.
#[test]
fn mask_not_equal_range_contains_every_satisfying_value() {
    for mask in 0..DOMAIN {
        for c in 0..DOMAIN {
            let region = ConstantRange::make_mask_not_equal_range(&ap(mask), &ap(c));
            let got = members(&region);
            for v in 0..DOMAIN {
                if (v & mask) != c {
                    assert!(
                        got.contains(&v),
                        "mask_not_equal({mask:#06b}, {c:#06b}) dropped {v:#06b}, \
                         where v & mask = {:#06b}",
                        v & mask
                    );
                }
            }
        }
    }
}

/// The `icmp` a range is equivalent to really does describe that range.
///
/// Mirrors the two-argument `ConstantRange::getEquivalentICmp`, whose `bool`
/// return says the offset came back zero. When it does, the exact region for
/// that predicate must be the range itself — which is the assertion upstream
/// makes at the end of the three-argument form.
#[test]
fn equivalent_icmp_round_trips_when_no_offset_is_needed() {
    enumerate(|range| {
        let Some((predicate, rhs)) = range.equivalent_icmp() else {
            // An offset was needed; the round-trip law is stated in terms of
            // `add`, which lands in slice 3d.
            return;
        };
        let rebuilt = ConstantRange::make_exact_icmp_region(predicate, &rhs);
        assert_eq!(
            members(&rebuilt),
            members(range),
            "{range:?} claims to be `icmp {predicate:?} {rhs:?}`, which is a different set"
        );
    });
}

/// Every range yields *some* equivalent icmp, and the offset is zero exactly
/// when `equivalent_icmp` returns `Some`.
#[test]
fn equivalent_icmp_agrees_with_its_offset_form() {
    enumerate(|range| {
        let with_offset = range.equivalent_icmp_with_offset();
        let without = range.equivalent_icmp();
        assert_eq!(
            with_offset.offset.is_zero(),
            without.is_some(),
            "{range:?}: the offset-free form must be Some exactly when the offset is zero"
        );
        if let Some((predicate, rhs)) = without {
            assert_eq!(predicate, with_offset.predicate);
            assert_eq!(rhs, with_offset.rhs);
        }
    });
}
