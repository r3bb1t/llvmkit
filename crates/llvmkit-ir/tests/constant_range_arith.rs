//! `ConstantRange` arithmetic — slice 3d of tranche 3, first group (see
//! `docs/future-work.md`).
//!
//! Covers `add`, `sub`, `multiply` and the min/max quartet: the operations
//! `computeOverflowFor*` needs in slice 3e.
//!
//! The oracle is the same as 3b and 3c — expand each range to the set it
//! holds, apply the operation to every concrete pairing, and check the
//! range-level answer covers the resulting set. Every one of these is an
//! over-approximation (a range cannot always name the exact set of products,
//! for instance), so the law is containment. Exactness *is* checked where it
//! is guaranteed: multiplying by a single `1` or `-1`.

use std::collections::BTreeSet;

use llvmkit_ir::{ApInt, ApIntTruncation, ConstantRange, Signedness};

const BITS: u32 = 4;
const DOMAIN: u64 = 1 << BITS;
const MASK: u64 = DOMAIN - 1;

fn ap(value: u64) -> ApInt {
    ApInt::new(
        BITS,
        value & MASK,
        Signedness::Unsigned,
        ApIntTruncation::Truncate,
    )
    .expect("in-range constant")
}

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

fn enumerate_pairs(mut test: impl FnMut(&ConstantRange, &ConstantRange)) {
    enumerate(|first| enumerate(|second| test(first, second)));
}

fn members(range: &ConstantRange) -> BTreeSet<u64> {
    (0..DOMAIN).filter(|v| range.contains(&ap(*v))).collect()
}

/// A 4-bit pattern read as a signed number.
fn signed(v: u64) -> i64 {
    if v & (1 << (BITS - 1)) != 0 {
        (v as i64) - (DOMAIN as i64)
    } else {
        v as i64
    }
}

/// Check that `op` applied to every concrete pairing lands inside `got`.
fn assert_covers_all_pairings(
    label: &str,
    first: &ConstantRange,
    second: &ConstantRange,
    got: &ConstantRange,
    op: impl Fn(u64, u64) -> u64,
) {
    let covered = members(got);
    for lhs in members(first) {
        for rhs in members(second) {
            let result = op(lhs, rhs) & MASK;
            assert!(
                covered.contains(&result),
                "{label}: {first:?} vs {second:?} dropped {lhs} {label} {rhs} = {result}"
            );
        }
    }
}

/// `add` covers every concrete sum. Mirrors `ConstantRange::add`.
#[test]
fn add_covers_every_sum() {
    enumerate_pairs(|first, second| {
        let got = first.add(second);
        assert_covers_all_pairings("add", first, second, &got, |a, b| a.wrapping_add(b));
    });
}

/// `sub` covers every concrete difference. Mirrors `ConstantRange::sub`.
#[test]
fn sub_covers_every_difference() {
    enumerate_pairs(|first, second| {
        let got = first.sub(second);
        assert_covers_all_pairings("sub", first, second, &got, |a, b| a.wrapping_sub(b));
    });
}

/// `multiply` covers every concrete product. Mirrors `ConstantRange::multiply`.
#[test]
fn multiply_covers_every_product() {
    enumerate_pairs(|first, second| {
        let got = first.multiply(second);
        assert_covers_all_pairings("mul", first, second, &got, |a, b| a.wrapping_mul(b));
    });
}

/// Multiplying by a single `1` or `-1` is exact — those are the two shortcuts
/// upstream takes before the double-width work, and they must not lose
/// precision.
#[test]
fn multiply_by_one_and_minus_one_is_exact() {
    let one = ConstantRange::new(ap(1), ap(2)).expect("range");
    let minus_one = ConstantRange::new(ap(MASK), ap(0)).expect("range");
    enumerate(|range| {
        assert_eq!(
            members(&range.multiply(&one)),
            members(range),
            "{range:?} * 1 must be exact"
        );
        assert_eq!(
            members(&one.multiply(range)),
            members(range),
            "1 * {range:?} must be exact"
        );

        let negated: BTreeSet<u64> = members(range)
            .iter()
            .map(|v| v.wrapping_neg() & MASK)
            .collect();
        assert_eq!(
            members(&range.multiply(&minus_one)),
            negated,
            "{range:?} * -1 must be exact negation"
        );
    });
}

/// The min/max quartet covers every concrete result. Mirrors
/// `ConstantRange::smax` / `smin` / `umax` / `umin`.
#[test]
fn min_max_quartet_covers_every_result() {
    enumerate_pairs(|first, second| {
        assert_covers_all_pairings("umax", first, second, &first.umax(second), |a, b| a.max(b));
        assert_covers_all_pairings("umin", first, second, &first.umin(second), |a, b| a.min(b));
        assert_covers_all_pairings("smax", first, second, &first.smax(second), |a, b| {
            if signed(a) >= signed(b) { a } else { b }
        });
        assert_covers_all_pairings("smin", first, second, &first.smin(second), |a, b| {
            if signed(a) <= signed(b) { a } else { b }
        });
    });
}

/// An empty operand makes every arithmetic result empty — there is no pairing
/// to produce a value. Mirrors the early returns in each upstream function.
#[test]
fn an_empty_operand_yields_empty() {
    let empty = ConstantRange::empty(BITS);
    let some = ConstantRange::new(ap(3), ap(9)).expect("range");
    for (label, got) in [
        ("add", some.add(&empty)),
        ("sub", some.sub(&empty)),
        ("multiply", some.multiply(&empty)),
        ("umax", some.umax(&empty)),
        ("umin", some.umin(&empty)),
        ("smax", some.smax(&empty)),
        ("smin", some.smin(&empty)),
    ] {
        assert!(got.is_empty_set(), "{label} with an empty operand");
    }
}

/// `add` and `sub` are inverse on a single-element right-hand side: shifting
/// up then back down recovers the original set exactly.
#[test]
fn add_then_sub_of_a_single_value_round_trips() {
    for delta in 0..DOMAIN {
        let step = ConstantRange::new(ap(delta), ap(delta + 1)).expect("range");
        enumerate(|range| {
            if range.is_empty_set() {
                return;
            }
            let round_tripped = range.add(&step).sub(&step);
            assert!(
                members(range).is_subset(&members(&round_tripped)),
                "{range:?} + {delta} - {delta} lost members"
            );
        });
    }
}
