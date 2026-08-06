//! `ConstantRange` bit counting — slice 3d-v (see `docs/future-work.md`).
//!
//! `ctlz`, `cttz` and `ctpop`. `abs` landed in 3d-ii, which needed it for
//! `srem`.
//!
//! `ctlz` and `cttz` take a `zero_is_poison` flag, mirroring the `llvm.ctlz` /
//! `llvm.cttz` intrinsics: counting on zero yields the full bit width, which
//! the intrinsic may instead declare poison. When it does, zero is excluded
//! from the input before counting, so the oracle excludes it too.

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

fn members(range: &ConstantRange) -> BTreeSet<u64> {
    (0..DOMAIN).filter(|v| range.contains(&ap(*v))).collect()
}

/// Leading zeros of a 4-bit value.
fn ctlz_of(v: u64) -> u64 {
    let v = v & MASK;
    if v == 0 {
        return u64::from(BITS);
    }
    u64::from(BITS) - (64 - u64::from(v.leading_zeros()))
}

/// Trailing zeros of a 4-bit value.
fn cttz_of(v: u64) -> u64 {
    let v = v & MASK;
    if v == 0 {
        u64::from(BITS)
    } else {
        u64::from(v.trailing_zeros())
    }
}

/// `ctlz` covers the leading-zero count of every member. Mirrors
/// `ConstantRange::ctlz`.
#[test]
fn ctlz_covers_every_count() {
    enumerate(|range| {
        for zero_is_poison in [false, true] {
            let got = members(&range.ctlz(zero_is_poison));
            for v in members(range) {
                if zero_is_poison && v == 0 {
                    continue;
                }
                assert!(
                    got.contains(&ctlz_of(v)),
                    "ctlz({range:?}, poison={zero_is_poison}) dropped ctlz({v}) = {}",
                    ctlz_of(v)
                );
            }
        }
    });
}

/// `cttz` covers the trailing-zero count of every member. Mirrors
/// `ConstantRange::cttz`.
#[test]
fn cttz_covers_every_count() {
    enumerate(|range| {
        for zero_is_poison in [false, true] {
            let got = members(&range.cttz(zero_is_poison));
            for v in members(range) {
                if zero_is_poison && v == 0 {
                    continue;
                }
                assert!(
                    got.contains(&cttz_of(v)),
                    "cttz({range:?}, poison={zero_is_poison}) dropped cttz({v}) = {}",
                    cttz_of(v)
                );
            }
        }
    });
}

/// `ctpop` covers the population count of every member. Mirrors
/// `ConstantRange::ctpop`.
#[test]
fn ctpop_covers_every_count() {
    enumerate(|range| {
        let got = members(&range.ctpop());
        for v in members(range) {
            let popcount = u64::from((v & MASK).count_ones());
            assert!(
                got.contains(&popcount),
                "ctpop({range:?}) dropped popcount({v}) = {popcount}"
            );
        }
    });
}

/// A range holding only zero becomes empty under `zero_is_poison`, since its
/// one member is excluded. Mirrors the explicit `[0, 1)` early returns in both
/// `ctlz` and `cttz`.
#[test]
fn only_zero_is_empty_when_zero_is_poison() {
    let only_zero = ConstantRange::new(ap(0), ap(1)).expect("range");
    assert!(only_zero.ctlz(true).is_empty_set(), "ctlz of only-zero");
    assert!(only_zero.cttz(true).is_empty_set(), "cttz of only-zero");

    // Without the flag, zero counts as the full width.
    assert_eq!(
        members(&only_zero.ctlz(false)),
        BTreeSet::from([u64::from(BITS)])
    );
    assert_eq!(
        members(&only_zero.cttz(false)),
        BTreeSet::from([u64::from(BITS)])
    );
    // `ctpop` has no poison flag; zero has population zero.
    assert_eq!(members(&only_zero.ctpop()), BTreeSet::from([0]));
}

/// An empty input yields empty throughout.
#[test]
fn an_empty_range_yields_empty() {
    let empty = ConstantRange::empty(BITS);
    assert!(empty.ctlz(false).is_empty_set());
    assert!(empty.ctlz(true).is_empty_set());
    assert!(empty.cttz(false).is_empty_set());
    assert!(empty.cttz(true).is_empty_set());
    assert!(empty.ctpop().is_empty_set());
}

/// Every count is bounded by the bit width — the invariant that makes these
/// safe to feed back into a range of the same width.
#[test]
fn every_count_fits_the_width() {
    let limit = u64::from(BITS);
    enumerate(|range| {
        for count in members(&range.ctlz(false)) {
            assert!(count <= limit, "ctlz of {range:?} produced {count}");
        }
        for count in members(&range.cttz(false)) {
            assert!(count <= limit, "cttz of {range:?} produced {count}");
        }
        for count in members(&range.ctpop()) {
            assert!(count <= limit, "ctpop of {range:?} produced {count}");
        }
    });
}
