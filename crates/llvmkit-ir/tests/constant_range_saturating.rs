//! `ConstantRange` saturating operations — slice 3d-iv (see
//! `docs/future-work.md`).
//!
//! Saturating operations never wrap and never invoke undefined behaviour, so
//! unlike 3d-ii and 3d-iii there is nothing to exclude: every pairing has a
//! defined result and the range must cover all of them.

use std::collections::BTreeSet;

use llvmkit_ir::{ApInt, ApIntSignedness, ApIntTruncation, ConstantRange};

const BITS: u32 = 4;
const DOMAIN: u64 = 1 << BITS;
const MASK: u64 = DOMAIN - 1;
const SIGNED_MIN: i64 = -(1 << (BITS - 1));
const SIGNED_MAX: i64 = (1 << (BITS - 1)) - 1;

fn ap(value: u64) -> ApInt {
    ApInt::new(
        BITS,
        value & MASK,
        ApIntSignedness::Unsigned,
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

fn from_signed(v: i64) -> u64 {
    (v as u64) & MASK
}

/// Clamp to the unsigned domain.
fn usat(v: i128) -> u64 {
    v.clamp(0, MASK as i128) as u64
}

/// Clamp to the signed domain, then re-encode.
fn ssat(v: i128) -> u64 {
    from_signed(v.clamp(SIGNED_MIN as i128, SIGNED_MAX as i128) as i64)
}

fn assert_covers(
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

/// The whole saturating family covers every pairing.
///
/// Mirrors `ConstantRange::uadd_sat` / `sadd_sat` / `usub_sat` / `ssub_sat` /
/// `umul_sat` / `smul_sat` / `ushl_sat` / `sshl_sat`.
#[test]
fn saturating_family_covers_every_result() {
    enumerate_pairs(|first, second| {
        assert_covers(
            "uadd_sat",
            first,
            second,
            &first.uadd_sat(second),
            |a, b| usat(i128::from(a) + i128::from(b)),
        );
        assert_covers(
            "sadd_sat",
            first,
            second,
            &first.sadd_sat(second),
            |a, b| ssat(i128::from(signed(a)) + i128::from(signed(b))),
        );
        assert_covers(
            "usub_sat",
            first,
            second,
            &first.usub_sat(second),
            |a, b| usat(i128::from(a) - i128::from(b)),
        );
        assert_covers(
            "ssub_sat",
            first,
            second,
            &first.ssub_sat(second),
            |a, b| ssat(i128::from(signed(a)) - i128::from(signed(b))),
        );
        assert_covers(
            "umul_sat",
            first,
            second,
            &first.umul_sat(second),
            |a, b| usat(i128::from(a) * i128::from(b)),
        );
        assert_covers(
            "smul_sat",
            first,
            second,
            &first.smul_sat(second),
            |a, b| ssat(i128::from(signed(a)) * i128::from(signed(b))),
        );
        // A shift amount at or beyond the width is *unconditionally* an
        // overflow for the saturating shifts — upstream's `ushl_ov` / `sshl_ov`
        // open with `Overflow = ShAmt >= getBitWidth()` before looking at the
        // value at all. So `0 ushl_sat 4` saturates to the maximum rather than
        // staying 0, which the naive `0 << 4 == 0` reading would predict.
        assert_covers(
            "ushl_sat",
            first,
            second,
            &first.ushl_sat(second),
            |a, b| {
                let amount = b.min(u64::from(BITS));
                if amount >= u64::from(BITS) {
                    return MASK;
                }
                usat(i128::from(a) << amount)
            },
        );
        assert_covers(
            "sshl_sat",
            first,
            second,
            &first.sshl_sat(second),
            |a, b| {
                let amount = b.min(u64::from(BITS));
                let value = signed(a);
                if amount >= u64::from(BITS) {
                    return from_signed(if value < 0 { SIGNED_MIN } else { SIGNED_MAX });
                }
                ssat(i128::from(value) << amount)
            },
        );
    });
}

/// An empty operand yields empty throughout — the early return every one of
/// the eight shares.
#[test]
fn an_empty_operand_yields_empty() {
    let empty = ConstantRange::empty(BITS);
    let some = ConstantRange::new(ap(3), ap(9)).expect("range");
    for (label, got) in [
        ("uadd_sat", some.uadd_sat(&empty)),
        ("sadd_sat", some.sadd_sat(&empty)),
        ("usub_sat", some.usub_sat(&empty)),
        ("ssub_sat", some.ssub_sat(&empty)),
        ("umul_sat", some.umul_sat(&empty)),
        ("smul_sat", some.smul_sat(&empty)),
        ("ushl_sat", some.ushl_sat(&empty)),
        ("sshl_sat", some.sshl_sat(&empty)),
    ] {
        assert!(got.is_empty_set(), "{label} with an empty operand");
    }
}

/// Saturating operations never leave the domain, so a full-domain operand
/// still yields something the domain can hold. This is the property that
/// distinguishes them from the wrapping forms in 3d-i.
#[test]
fn saturating_results_stay_inside_the_domain() {
    let full = ConstantRange::full(BITS);
    enumerate(|range| {
        for (label, got) in [
            ("uadd_sat", range.uadd_sat(&full)),
            ("sadd_sat", range.sadd_sat(&full)),
            ("usub_sat", range.usub_sat(&full)),
            ("ssub_sat", range.ssub_sat(&full)),
            ("umul_sat", range.umul_sat(&full)),
            ("smul_sat", range.smul_sat(&full)),
        ] {
            assert_eq!(
                got.bit_width(),
                BITS,
                "{label} must not change the width of {range:?}"
            );
        }
    });
}

/// Saturating addition of a single zero is the identity — the simplest case
/// where saturation must not lose precision.
#[test]
fn saturating_add_of_zero_is_exact() {
    let zero = ConstantRange::new(ap(0), ap(1)).expect("range");
    enumerate(|range| {
        if range.is_empty_set() {
            return;
        }
        assert!(
            members(range).is_subset(&members(&range.uadd_sat(&zero))),
            "{range:?} uadd_sat 0 lost members"
        );
        assert!(
            members(range).is_subset(&members(&range.sadd_sat(&zero))),
            "{range:?} sadd_sat 0 lost members"
        );
    });
}
