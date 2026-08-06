//! `ConstantRange` bitwise operations and shifts — slice 3d-iii (see
//! `docs/future-work.md`).
//!
//! Enumeration oracle as in 3b–3d-ii. One case needs care: **`shl` by an
//! amount at or above the bit width is poison**, so those pairings are skipped
//! exactly as the div-by-zero pairings were in 3d-ii. `lshr` and `ashr` are
//! not poison in llvmkit's `ApInt` at large amounts — they saturate — but the
//! range analysis clamps the shift amount to the width, so the oracle clamps
//! the same way rather than inventing a different rule.

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

fn signed(v: u64) -> i64 {
    if v & (1 << (BITS - 1)) != 0 {
        (v as i64) - (DOMAIN as i64)
    } else {
        v as i64
    }
}

fn assert_covers(
    label: &str,
    first: &ConstantRange,
    second: &ConstantRange,
    got: &ConstantRange,
    op: impl Fn(u64, u64) -> Option<u64>,
) {
    let covered = members(got);
    for lhs in members(first) {
        for rhs in members(second) {
            let Some(result) = op(lhs, rhs) else { continue };
            assert!(
                covered.contains(&(result & MASK)),
                "{label}: {first:?} vs {second:?} dropped {lhs} {label} {rhs} = {result}"
            );
        }
    }
}

/// `binary_and` covers every AND. Mirrors `ConstantRange::binaryAnd`.
#[test]
fn binary_and_covers_every_result() {
    enumerate_pairs(|first, second| {
        assert_covers("and", first, second, &first.binary_and(second), |a, b| {
            Some(a & b)
        });
    });
}

/// `binary_or` covers every OR. Mirrors `ConstantRange::binaryOr`.
#[test]
fn binary_or_covers_every_result() {
    enumerate_pairs(|first, second| {
        assert_covers("or", first, second, &first.binary_or(second), |a, b| {
            Some(a | b)
        });
    });
}

/// `binary_xor` covers every XOR. Mirrors `ConstantRange::binaryXor`.
#[test]
fn binary_xor_covers_every_result() {
    enumerate_pairs(|first, second| {
        assert_covers("xor", first, second, &first.binary_xor(second), |a, b| {
            Some(a ^ b)
        });
    });
}

/// `binary_not` is exact — complement is a bijection, so no range precision is
/// lost. Mirrors `ConstantRange::binaryNot`.
#[test]
fn binary_not_is_exact() {
    enumerate(|range| {
        let expected: BTreeSet<u64> = members(range).iter().map(|v| (!v) & MASK).collect();
        assert_eq!(
            members(&range.binary_not()),
            expected,
            "binary_not of {range:?} is not the exact complement-of-each-member"
        );
    });
}

/// XOR against a single all-ones value is exactly the complement — one of the
/// shortcuts upstream special-cases.
#[test]
fn xor_with_all_ones_is_complement() {
    let all_ones = ConstantRange::new(ap(MASK), ap(0)).expect("range");
    enumerate(|range| {
        assert_eq!(
            members(&range.binary_xor(&all_ones)),
            members(&range.binary_not()),
            "{range:?} ^ -1 must equal ~{range:?}"
        );
    });
}

/// `shl` covers every defined left shift. Mirrors `ConstantRange::shl`.
///
/// A shift amount at or above the bit width is poison, so those pairings are
/// skipped.
#[test]
fn shl_covers_every_defined_result() {
    enumerate_pairs(|first, second| {
        assert_covers("shl", first, second, &first.shl(second), |a, b| {
            (b < u64::from(BITS)).then(|| a << b)
        });
    });
}

/// `lshr` covers every logical right shift. Mirrors `ConstantRange::lshr`.
#[test]
fn lshr_covers_every_result() {
    enumerate_pairs(|first, second| {
        assert_covers("lshr", first, second, &first.lshr(second), |a, b| {
            let amount = b.min(u64::from(BITS));
            Some(if amount >= u64::from(BITS) {
                0
            } else {
                a >> amount
            })
        });
    });
}

/// `ashr` covers every arithmetic right shift. Mirrors `ConstantRange::ashr`.
///
/// The three-case split in the implementation exists because a negative value
/// grows toward -1 under shifting while a non-negative one shrinks toward 0, so
/// which shift amount yields the extreme depends on the sign.
#[test]
fn ashr_covers_every_result() {
    enumerate_pairs(|first, second| {
        assert_covers("ashr", first, second, &first.ashr(second), |a, b| {
            let amount = b.min(u64::from(BITS));
            let s = signed(a);
            let shifted = if amount >= u64::from(BITS) {
                if s < 0 { -1 } else { 0 }
            } else {
                s >> amount
            };
            Some((shifted as u64) & MASK)
        });
    });
}

/// An empty operand yields empty throughout.
#[test]
fn an_empty_operand_yields_empty() {
    let empty = ConstantRange::empty(BITS);
    let some = ConstantRange::new(ap(3), ap(9)).expect("range");
    for (label, got) in [
        ("and", some.binary_and(&empty)),
        ("or", some.binary_or(&empty)),
        ("xor", some.binary_xor(&empty)),
        ("shl", some.shl(&empty)),
        ("lshr", some.lshr(&empty)),
        ("ashr", some.ashr(&empty)),
        ("not", empty.binary_not()),
    ] {
        assert!(got.is_empty_set(), "{label} with an empty operand");
    }
}

/// Shifting left by at least the bit width is poison, so a single such amount
/// gives the empty set. Mirrors the `RHS->uge(BW)` early return.
#[test]
fn shl_by_the_full_width_is_empty() {
    let some = ConstantRange::new(ap(3), ap(9)).expect("range");
    // At 4 bits the domain tops out at 15, so a single-element range holding
    // exactly the width is the smallest poison amount representable.
    let width_shift =
        ConstantRange::new(ap(u64::from(BITS)), ap(u64::from(BITS) + 1)).expect("range");
    assert!(some.shl(&width_shift).is_empty_set());
}
