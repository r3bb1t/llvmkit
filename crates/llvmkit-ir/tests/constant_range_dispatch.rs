//! `ConstantRange` dispatchers and no-wrap variants — slice 3d-vi, the last of
//! slice 3d (see `docs/future-work.md`).
//!
//! Two kinds of claim here.
//!
//! The **dispatchers** (`binary_op`, `overflowing_binary_op`, `intrinsic`) own
//! no reasoning of their own — every arm forwards. So the tests assert routing:
//! each opcode reaches the operation it names, checked by comparing against a
//! direct call. A dispatcher bug is a mis-wired arm, and that is exactly what
//! this catches.
//!
//! The **no-wrap variants** do carry reasoning, so they get the enumeration
//! oracle: constrained to pairings that genuinely do not wrap, the result must
//! still cover all of them.

use std::collections::BTreeSet;

use llvmkit_ir::{
    ApInt, ApIntTruncation, BinaryOpcode, ConstantRange, NoWrapKind, PreferredRangeType,
    RangeIntrinsic, Signedness,
};

const BITS: u32 = 4;
const DOMAIN: u64 = 1 << BITS;
const MASK: u64 = DOMAIN - 1;
const SIGNED_MIN: i64 = -(1 << (BITS - 1));
const SIGNED_MAX: i64 = (1 << (BITS - 1)) - 1;

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

/// One no-wrap operation under test: its label, the range the implementation
/// produced, and the exact arithmetic to check it against.
type NoWrapCheck = (&'static str, ConstantRange, fn(i128, i128) -> i128);

/// Every `binary_op` arm routes to the operation it names.
///
/// Mirrors `ConstantRange::binaryOp`, including its choice to send the float
/// opcodes to their integer counterparts.
#[test]
fn binary_op_routes_every_opcode() {
    let lhs = ConstantRange::new(ap(3), ap(9)).expect("range");
    let rhs = ConstantRange::new(ap(2), ap(6)).expect("range");

    let cases: &[(BinaryOpcode, ConstantRange)] = &[
        (BinaryOpcode::Add, lhs.add(&rhs)),
        (BinaryOpcode::Sub, lhs.sub(&rhs)),
        (BinaryOpcode::Mul, lhs.multiply(&rhs)),
        (BinaryOpcode::Udiv, lhs.udiv(&rhs)),
        (BinaryOpcode::Sdiv, lhs.sdiv(&rhs)),
        (BinaryOpcode::Urem, lhs.urem(&rhs)),
        (BinaryOpcode::Srem, lhs.srem(&rhs)),
        (BinaryOpcode::Shl, lhs.shl(&rhs)),
        (BinaryOpcode::Lshr, lhs.lshr(&rhs)),
        (BinaryOpcode::Ashr, lhs.ashr(&rhs)),
        (BinaryOpcode::And, lhs.binary_and(&rhs)),
        (BinaryOpcode::Or, lhs.binary_or(&rhs)),
        (BinaryOpcode::Xor, lhs.binary_xor(&rhs)),
        // Upstream forwards the float opcodes to the integer operations: a
        // range over floats is "an ideal integer operation with a lossy
        // representation", in its comment.
        (BinaryOpcode::Fadd, lhs.add(&rhs)),
        (BinaryOpcode::Fsub, lhs.sub(&rhs)),
        (BinaryOpcode::Fmul, lhs.multiply(&rhs)),
    ];

    for (opcode, expected) in cases {
        assert_eq!(
            lhs.binary_op(*opcode, &rhs),
            *expected,
            "binary_op({opcode:?}) routed to the wrong operation"
        );
    }

    // `fdiv` and `frem` have no integer counterpart, so they answer full.
    for opcode in [BinaryOpcode::Fdiv, BinaryOpcode::Frem] {
        assert!(
            lhs.binary_op(opcode, &rhs).is_full_set(),
            "binary_op({opcode:?}) must be conservative"
        );
    }
}

/// Every `intrinsic` arm routes to the operation it names. Mirrors
/// `ConstantRange::intrinsic`.
#[test]
fn intrinsic_routes_every_supported_id() {
    let lhs = ConstantRange::new(ap(3), ap(9)).expect("range");
    let rhs = ConstantRange::new(ap(2), ap(6)).expect("range");
    let pair = [lhs.clone(), rhs.clone()];
    let single = [lhs.clone()];

    let binary: &[(RangeIntrinsic, ConstantRange)] = &[
        (RangeIntrinsic::UaddSat, lhs.uadd_sat(&rhs)),
        (RangeIntrinsic::UsubSat, lhs.usub_sat(&rhs)),
        (RangeIntrinsic::SaddSat, lhs.sadd_sat(&rhs)),
        (RangeIntrinsic::SsubSat, lhs.ssub_sat(&rhs)),
        (RangeIntrinsic::Umin, lhs.umin(&rhs)),
        (RangeIntrinsic::Umax, lhs.umax(&rhs)),
        (RangeIntrinsic::Smin, lhs.smin(&rhs)),
        (RangeIntrinsic::Smax, lhs.smax(&rhs)),
    ];
    for (intrinsic, expected) in binary {
        assert_eq!(
            ConstantRange::intrinsic(*intrinsic, &pair),
            Some(expected.clone()),
            "intrinsic({intrinsic:?}) routed wrongly"
        );
    }

    // The flag-carrying ones take their `immarg` in the enum rather than as a
    // second operand range, so a single operand suffices.
    for flag in [false, true] {
        assert_eq!(
            ConstantRange::intrinsic(
                RangeIntrinsic::Abs {
                    int_min_is_poison: flag
                },
                &single
            ),
            Some(lhs.abs(flag))
        );
        assert_eq!(
            ConstantRange::intrinsic(
                RangeIntrinsic::Ctlz {
                    zero_is_poison: flag
                },
                &single
            ),
            Some(lhs.ctlz(flag))
        );
        assert_eq!(
            ConstantRange::intrinsic(
                RangeIntrinsic::Cttz {
                    zero_is_poison: flag
                },
                &single
            ),
            Some(lhs.cttz(flag))
        );
    }
    assert_eq!(
        ConstantRange::intrinsic(RangeIntrinsic::Ctpop, &single),
        Some(lhs.ctpop())
    );

    // A binary intrinsic with no second operand declines rather than panicking
    // — upstream asserts the arity instead.
    assert_eq!(
        ConstantRange::intrinsic(RangeIntrinsic::Umin, &single),
        None
    );
    assert_eq!(ConstantRange::intrinsic(RangeIntrinsic::Umin, &[]), None);
}

/// `overflowing_binary_op` routes the three wrap-carrying opcodes to their
/// no-wrap forms and everything else to the plain dispatcher.
///
/// Mirrors `ConstantRange::overflowingBinaryOp`, including its `default`
/// fallback.
#[test]
fn overflowing_binary_op_routes_by_opcode() {
    let lhs = ConstantRange::new(ap(3), ap(9)).expect("range");
    let rhs = ConstantRange::new(ap(2), ap(6)).expect("range");
    let no_wrap = NoWrapKind::BOTH;
    let preferred = PreferredRangeType::Smallest;

    assert_eq!(
        lhs.overflowing_binary_op(BinaryOpcode::Add, &rhs, no_wrap, preferred),
        lhs.add_with_no_wrap(&rhs, no_wrap, preferred)
    );
    assert_eq!(
        lhs.overflowing_binary_op(BinaryOpcode::Sub, &rhs, no_wrap, preferred),
        lhs.sub_with_no_wrap(&rhs, no_wrap, preferred)
    );
    assert_eq!(
        lhs.overflowing_binary_op(BinaryOpcode::Mul, &rhs, no_wrap, preferred),
        lhs.multiply_with_no_wrap(&rhs, no_wrap, preferred)
    );
    // An opcode with no wrap flags falls back to the plain dispatcher, even
    // when promises are passed.
    assert_eq!(
        lhs.overflowing_binary_op(BinaryOpcode::And, &rhs, no_wrap, preferred),
        lhs.binary_op(BinaryOpcode::And, &rhs)
    );
}

/// The no-wrap variants cover every pairing that genuinely does not wrap.
///
/// Mirrors `ConstantRange::addWithNoWrap` / `subWithNoWrap` /
/// `multiplyWithNoWrap`. A pairing that *would* wrap is excluded, because the
/// promise makes it poison — which is exactly what the intersection with the
/// saturating form is enforcing.
#[test]
fn no_wrap_variants_cover_every_non_wrapping_pairing() {
    let preferred = PreferredRangeType::Smallest;
    let fits_unsigned = |v: i128| (0..=i128::from(MASK)).contains(&v);
    let fits_signed = |v: i128| (i128::from(SIGNED_MIN)..=i128::from(SIGNED_MAX)).contains(&v);

    enumerate_pairs(|first, second| {
        for (no_wrap, label) in [
            (NoWrapKind::SIGNED, "nsw"),
            (NoWrapKind::UNSIGNED, "nuw"),
            (NoWrapKind::BOTH, "nsw nuw"),
        ] {
            let checks: [NoWrapCheck; 3] = [
                (
                    "add",
                    first.add_with_no_wrap(second, no_wrap, preferred),
                    |a, b| a + b,
                ),
                (
                    "sub",
                    first.sub_with_no_wrap(second, no_wrap, preferred),
                    |a, b| a - b,
                ),
                (
                    "mul",
                    first.multiply_with_no_wrap(second, no_wrap, preferred),
                    |a, b| a * b,
                ),
            ];

            for (op_label, got, op) in checks {
                let covered = members(&got);
                for lhs in members(first) {
                    for rhs in members(second) {
                        // Does this pairing keep every promise made?
                        let unsigned_result = op(i128::from(lhs), i128::from(rhs));
                        let signed_result = op(i128::from(signed(lhs)), i128::from(signed(rhs)));
                        if no_wrap.unsigned && !fits_unsigned(unsigned_result) {
                            continue;
                        }
                        if no_wrap.signed && !fits_signed(signed_result) {
                            continue;
                        }
                        let wrapped = (unsigned_result as u64) & MASK;
                        assert!(
                            covered.contains(&wrapped),
                            "{op_label} {label}: {first:?} vs {second:?} dropped \
                             {lhs} {op_label} {rhs} = {wrapped}"
                        );
                    }
                }
            }
        }
    });
}

/// `smul_fast` covers every product it claims to know, and gives up to the
/// full set rather than approximating. Mirrors `ConstantRange::smul_fast`.
#[test]
fn smul_fast_covers_every_product_it_claims() {
    enumerate_pairs(|first, second| {
        let got = first.smul_fast(second);
        if got.is_full_set() {
            // Giving up is always sound.
            return;
        }
        let covered = members(&got);
        for lhs in members(first) {
            for rhs in members(second) {
                let product = (signed(lhs).wrapping_mul(signed(rhs)) as u64) & MASK;
                assert!(
                    covered.contains(&product),
                    "smul_fast: {first:?} vs {second:?} dropped {lhs} * {rhs} = {product}"
                );
            }
        }
    });
}

/// Every `RangeIntrinsic` is supported by construction — the enum *is* the
/// supported set. Mirrors `ConstantRange::isIntrinsicSupported`, whose
/// run-time list becomes a type here.
#[test]
fn every_range_intrinsic_is_supported() {
    for intrinsic in [
        RangeIntrinsic::UaddSat,
        RangeIntrinsic::UsubSat,
        RangeIntrinsic::SaddSat,
        RangeIntrinsic::SsubSat,
        RangeIntrinsic::Umin,
        RangeIntrinsic::Umax,
        RangeIntrinsic::Smin,
        RangeIntrinsic::Smax,
        RangeIntrinsic::Abs {
            int_min_is_poison: false,
        },
        RangeIntrinsic::Ctlz {
            zero_is_poison: false,
        },
        RangeIntrinsic::Cttz {
            zero_is_poison: false,
        },
        RangeIntrinsic::Ctpop,
    ] {
        assert!(ConstantRange::is_intrinsic_supported(intrinsic));
    }
}
