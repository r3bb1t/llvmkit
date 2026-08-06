//! `ConstantRange` division and remainder — slice 3d-ii (see
//! `docs/future-work.md`). Includes `abs`, pulled forward from 3d-v because
//! `srem` needs it.
//!
//! The enumeration oracle carries over from 3b–3d-i, with one addition that
//! matters here: **division and remainder by zero are undefined behaviour**,
//! and `SignedMin / -1` is UB at the IR level too. A range analysis is only
//! obliged to cover the *defined* pairings, so the oracle skips exactly those
//! and no others. Skipping too much would let a wrong answer pass; skipping
//! too little would fail a correct implementation for not covering UB.

use std::collections::BTreeSet;

use llvmkit_ir::{ApInt, ApIntTruncation, ConstantRange, Signedness};

const BITS: u32 = 4;
const DOMAIN: u64 = 1 << BITS;
const MASK: u64 = DOMAIN - 1;
const SIGNED_MIN: i64 = -(1 << (BITS - 1));

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

fn from_signed(v: i64) -> u64 {
    (v as u64) & MASK
}

/// Cover every *defined* pairing. `op` returns `None` for a pairing that is
/// undefined behaviour, which the analysis is not obliged to represent.
fn assert_covers_defined_pairings(
    label: &str,
    first: &ConstantRange,
    second: &ConstantRange,
    got: &ConstantRange,
    op: impl Fn(u64, u64) -> Option<u64>,
) {
    let covered = members(got);
    for lhs in members(first) {
        for rhs in members(second) {
            let Some(result) = op(lhs, rhs) else {
                continue;
            };
            assert!(
                covered.contains(&(result & MASK)),
                "{label}: {first:?} vs {second:?} dropped {lhs} {label} {rhs} = {result}"
            );
        }
    }
}

/// `udiv` covers every quotient with a non-zero divisor. Mirrors
/// `ConstantRange::udiv`.
#[test]
fn udiv_covers_every_defined_quotient() {
    enumerate_pairs(|first, second| {
        let got = first.udiv(second);
        assert_covers_defined_pairings("udiv", first, second, &got, |a, b| (b != 0).then(|| a / b));
    });
}

/// `sdiv` covers every quotient with a non-zero divisor, excluding
/// `SignedMin / -1`. Mirrors `ConstantRange::sdiv`.
///
/// That one pairing is UB at the IR level even though `APInt` defines it, and
/// upstream goes to real trouble to exclude it — the `neg / neg` arm computes
/// its bound twice, once dropping `-1` from the divisor and once dropping
/// `SignedMin` from the dividend.
#[test]
fn sdiv_covers_every_defined_quotient() {
    enumerate_pairs(|first, second| {
        let got = first.sdiv(second);
        assert_covers_defined_pairings("sdiv", first, second, &got, |a, b| {
            let (sa, sb) = (signed(a), signed(b));
            if sb == 0 || (sa == SIGNED_MIN && sb == -1) {
                return None;
            }
            Some(from_signed(sa / sb))
        });
    });
}

/// `urem` covers every remainder with a non-zero divisor. Mirrors
/// `ConstantRange::urem`.
#[test]
fn urem_covers_every_defined_remainder() {
    enumerate_pairs(|first, second| {
        let got = first.urem(second);
        assert_covers_defined_pairings("urem", first, second, &got, |a, b| (b != 0).then(|| a % b));
    });
}

/// `srem` covers every remainder with a non-zero divisor, excluding
/// `SignedMin % -1`. Mirrors `ConstantRange::srem`.
#[test]
fn srem_covers_every_defined_remainder() {
    enumerate_pairs(|first, second| {
        let got = first.srem(second);
        assert_covers_defined_pairings("srem", first, second, &got, |a, b| {
            let (sa, sb) = (signed(a), signed(b));
            if sb == 0 || (sa == SIGNED_MIN && sb == -1) {
                return None;
            }
            Some(from_signed(sa % sb))
        });
    });
}

/// A divisor range that can only be zero yields the empty set, since every
/// pairing is undefined. Mirrors the `getUnsignedMax().isZero()` early returns.
#[test]
fn a_divisor_that_is_only_zero_yields_empty() {
    let zero = ConstantRange::new(ap(0), ap(1)).expect("range");
    let some = ConstantRange::new(ap(3), ap(9)).expect("range");
    assert!(some.udiv(&zero).is_empty_set(), "udiv by only-zero");
    assert!(some.urem(&zero).is_empty_set(), "urem by only-zero");
    assert!(some.srem(&zero).is_empty_set(), "srem by only-zero");
}

/// An empty operand yields empty throughout.
#[test]
fn an_empty_operand_yields_empty() {
    let empty = ConstantRange::empty(BITS);
    let some = ConstantRange::new(ap(3), ap(9)).expect("range");
    for (label, got) in [
        ("udiv", some.udiv(&empty)),
        ("sdiv", some.sdiv(&empty)),
        ("urem", some.urem(&empty)),
        ("srem", some.srem(&empty)),
        ("abs", empty.abs(false)),
    ] {
        assert!(got.is_empty_set(), "{label} with an empty operand");
    }
}

/// Single-element operands divide and remainder exactly — the shortcuts
/// upstream takes before the general reasoning.
#[test]
fn single_element_div_rem_is_exact() {
    for a in 0..DOMAIN {
        for b in 1..DOMAIN {
            let lhs = ConstantRange::new(ap(a), ap(a + 1)).expect("range");
            let rhs = ConstantRange::new(ap(b), ap(b + 1)).expect("range");

            assert_eq!(
                members(&lhs.urem(&rhs)),
                BTreeSet::from([a % b]),
                "{a} urem {b}"
            );

            let (sa, sb) = (signed(a), signed(b));
            if sa == SIGNED_MIN && sb == -1 {
                continue;
            }
            assert_eq!(
                members(&lhs.srem(&rhs)),
                BTreeSet::from([from_signed(sa % sb)]),
                "{sa} srem {sb}"
            );
        }
    }
}

/// `abs` covers the absolute value of every member. Mirrors
/// `ConstantRange::abs`.
///
/// With `int_min_is_poison`, the signed minimum is excluded from the input —
/// it has no positive counterpart at this width.
#[test]
fn abs_covers_every_absolute_value() {
    enumerate(|range| {
        for int_min_is_poison in [false, true] {
            let got = members(&range.abs(int_min_is_poison));
            for v in members(range) {
                let s = signed(v);
                if int_min_is_poison && s == SIGNED_MIN {
                    continue;
                }
                let expected = from_signed(s.wrapping_abs());
                assert!(
                    got.contains(&expected),
                    "abs({range:?}, poison={int_min_is_poison}) dropped |{s}| = {expected}"
                );
            }
        }
    });
}

/// A range holding only the signed minimum becomes empty under
/// `int_min_is_poison`, because its one member is excluded. Mirrors the
/// explicit early return upstream.
#[test]
fn abs_of_only_signed_min_is_empty_when_poison() {
    let only_signed_min =
        ConstantRange::new(ap(from_signed(SIGNED_MIN)), ap(from_signed(SIGNED_MIN) + 1))
            .expect("range");
    assert!(only_signed_min.abs(true).is_empty_set());
    // Without the poison flag it wraps to itself and stays.
    assert!(!only_signed_min.abs(false).is_empty_set());
}
