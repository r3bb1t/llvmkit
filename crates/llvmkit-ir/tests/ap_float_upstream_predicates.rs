//! Ports of the `APFloat` classification, factory, and small-operation tests
//! from `llvm/unittests/ADT/APFloatTest.cpp` in the vendored `llvmorg-22.1.4`
//! tree.
//!
//! Rows for semantics llvmkit does not model (`Float8*`, `Float6*`,
//! `Float4E2M1FN`, `FloatTF32`) are omitted; every row for the seven modeled
//! semantics is ported as written. Where upstream asserts through
//! `classify() -> FPClassTest` — a finer classification than llvmkit's
//! `ApFloatCategory` — the surrounding assertions are ported and the
//! `classify()` line is noted as unmodeled rather than approximated.

use llvmkit_ir::{
    ApFloat, ApFloatCmpResult, ApFloatNextDirection, ApFloatSemantics, ApFloatSign, ApFloatStatus,
    ApInt, BinaryExponent, NanPayload, RoundingMode,
};

const HALF: ApFloatSemantics = ApFloatSemantics::IeeeHalf;
const SINGLE: ApFloatSemantics = ApFloatSemantics::IeeeSingle;
const DOUBLE: ApFloatSemantics = ApFloatSemantics::IeeeDouble;
const QUAD: ApFloatSemantics = ApFloatSemantics::IeeeQuad;
const X87: ApFloatSemantics = ApFloatSemantics::X87DoubleExtended;
const PPC: ApFloatSemantics = ApFloatSemantics::PpcDoubleDouble;
const BFLOAT: ApFloatSemantics = ApFloatSemantics::Bfloat;

/// Every modeled semantics, in the order `APFloat::Semantics` enumerates the
/// ones llvmkit shares with it.
const MODELED: [ApFloatSemantics; 7] = [HALF, BFLOAT, SINGLE, DOUBLE, QUAD, X87, PPC];

fn parse(semantics: ApFloatSemantics, text: &str) -> ApFloat {
    ApFloat::from_string(semantics, text, RoundingMode::NearestTiesToEven)
        .unwrap_or_else(|e| panic!("upstream spells {text:?}, which must parse: {e}"))
        .0
}

fn bits_of(value: &ApFloat) -> ApInt {
    value.to_bits()
}

fn u64_bits(value: &ApFloat) -> u64 {
    value
        .to_bits()
        .try_zext_u64()
        .expect("semantics under test fit in u64")
}

// ── Factories ───────────────────────────────────────────────────────────────

/// Port of `TEST(APFloatTest, getOne)`.
#[test]
fn get_one() {
    assert!(ApFloat::one(SINGLE, ApFloatSign::Positive).is_exactly_value_f64(1.0));
    assert!(ApFloat::one(SINGLE, ApFloatSign::Negative).is_exactly_value_f64(-1.0));
}

/// Port of `TEST(APFloatTest, getZero)`, restricted to the modeled semantics.
///
/// The `PPCDoubleDouble` row is the one deliberate divergence: upstream's
/// pattern puts the sign in the *low* word because its low word holds the
/// leading `double`, and llvmkit stores that pair mirrored. See
/// `ap_float_ppc_word_order.rs`, which pins the mirroring itself.
#[test]
fn get_zero() {
    let cases: [(ApFloatSemantics, bool, [u64; 2]); 12] = [
        (HALF, false, [0, 0]),
        (HALF, true, [0x8000, 0]),
        (SINGLE, false, [0, 0]),
        (SINGLE, true, [0x8000_0000, 0]),
        (DOUBLE, false, [0, 0]),
        (DOUBLE, true, [0x8000_0000_0000_0000, 0]),
        (QUAD, false, [0, 0]),
        (QUAD, true, [0, 0x8000_0000_0000_0000]),
        // Upstream: {0x8000000000000000, 0} — mirrored here, see above.
        (PPC, false, [0, 0]),
        (PPC, true, [0, 0x8000_0000_0000_0000]),
        (X87, false, [0, 0]),
        (X87, true, [0, 0x8000]),
    ];
    for (semantics, negative, expected) in cases {
        let sign = if negative {
            ApFloatSign::Negative
        } else {
            ApFloatSign::Positive
        };
        let zero = ApFloat::zero(semantics, sign);
        assert!(zero.is_zero(), "{semantics:?} {negative} must be zero");
        assert_eq!(zero.is_negative(), negative, "{semantics:?} sign");
        assert_eq!(
            bits_of(&zero),
            ApInt::from_words(semantics.bit_width(), &expected),
            "{semantics:?} negative={negative} bit pattern"
        );
    }
}

/// Port of `TEST(APFloatTest, makeNaN)` — the `IEEEsingle` and `IEEEdouble`
/// rows, which are the modeled ones. Upstream drives them through
/// `nanbitsFromAPInt`, i.e. `getSNaN`/`getQNaN` with a 64-bit payload, then
/// `bitcastToAPInt`.
#[test]
fn make_nan() {
    let cases: [(u64, ApFloatSemantics, bool, bool, u64); 20] = [
        (0x7fc0_0000, SINGLE, false, false, 0x0000_0000),
        (0xffc0_0000, SINGLE, false, true, 0x0000_0000),
        (0x7fc0_ae72, SINGLE, false, false, 0x0000_ae72),
        (0x7fff_ae72, SINGLE, false, false, 0xffff_ae72),
        (0x7fda_ae72, SINGLE, false, false, 0x00da_ae72),
        (0x7fa0_0000, SINGLE, true, false, 0x0000_0000),
        (0xffa0_0000, SINGLE, true, true, 0x0000_0000),
        (0x7f80_ae72, SINGLE, true, false, 0x0000_ae72),
        (0x7fbf_ae72, SINGLE, true, false, 0xffff_ae72),
        (0x7f9a_ae72, SINGLE, true, false, 0x001a_ae72),
        (
            0x7ff8_0000_0000_0000,
            DOUBLE,
            false,
            false,
            0x0000_0000_0000_0000,
        ),
        (
            0xfff8_0000_0000_0000,
            DOUBLE,
            false,
            true,
            0x0000_0000_0000_0000,
        ),
        (
            0x7ff8_0000_0000_ae72,
            DOUBLE,
            false,
            false,
            0x0000_0000_0000_ae72,
        ),
        (
            0x7fff_ffff_ffff_ae72,
            DOUBLE,
            false,
            false,
            0xffff_ffff_ffff_ae72,
        ),
        (
            0x7ffd_aaaa_aaaa_ae72,
            DOUBLE,
            false,
            false,
            0x000d_aaaa_aaaa_ae72,
        ),
        (
            0x7ff4_0000_0000_0000,
            DOUBLE,
            true,
            false,
            0x0000_0000_0000_0000,
        ),
        (
            0xfff4_0000_0000_0000,
            DOUBLE,
            true,
            true,
            0x0000_0000_0000_0000,
        ),
        (
            0x7ff0_0000_0000_ae72,
            DOUBLE,
            true,
            false,
            0x0000_0000_0000_ae72,
        ),
        (
            0x7ff7_ffff_ffff_ae72,
            DOUBLE,
            true,
            false,
            0xffff_ffff_ffff_ae72,
        ),
        (
            0x7ff1_aaaa_aaaa_ae72,
            DOUBLE,
            true,
            false,
            0x0001_aaaa_aaaa_ae72,
        ),
    ];
    for (expected, semantics, signaling, negative, payload) in cases {
        let payload_bits = ApInt::from_words(64, &[payload]);
        let sign = if negative {
            ApFloatSign::Negative
        } else {
            ApFloatSign::Positive
        };
        let value = if signaling {
            ApFloat::snan(semantics, sign, NanPayload::Bits(&payload_bits))
        } else {
            ApFloat::qnan(semantics, sign, NanPayload::Bits(&payload_bits))
        };
        assert_eq!(
            u64_bits(&value),
            expected,
            "{semantics:?} signaling={signaling} negative={negative} payload={payload:#x}"
        );
    }
}

// ── Classification ──────────────────────────────────────────────────────────

/// Port of `TEST(APFloatTest, isSignaling)`. The `classify()` assertions are
/// upstream's finer `FPClassTest`, which llvmkit does not model.
#[test]
fn is_signaling() {
    let payload = ApInt::one_bit_set(4, 2);
    assert!(!ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_signaling());
    assert!(!ApFloat::qnan(SINGLE, ApFloatSign::Negative, NanPayload::Absent).is_signaling());
    assert!(
        !ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Bits(&payload)).is_signaling()
    );
    assert!(
        !ApFloat::qnan(SINGLE, ApFloatSign::Negative, NanPayload::Bits(&payload)).is_signaling()
    );

    assert!(ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_signaling());
    assert!(ApFloat::snan(SINGLE, ApFloatSign::Negative, NanPayload::Absent).is_signaling());
    assert!(
        ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Bits(&payload)).is_signaling()
    );
    assert!(
        ApFloat::snan(SINGLE, ApFloatSign::Negative, NanPayload::Bits(&payload)).is_signaling()
    );
}

/// Port of `TEST(APFloatTest, isNegative)`.
#[test]
fn is_negative() {
    assert!(!parse(SINGLE, "0x1p+0").is_negative());
    assert!(parse(SINGLE, "-0x1p+0").is_negative());
    assert!(!ApFloat::inf(SINGLE, ApFloatSign::Positive).is_negative());
    assert!(ApFloat::inf(SINGLE, ApFloatSign::Negative).is_negative());
    assert!(!ApFloat::zero(SINGLE, ApFloatSign::Positive).is_negative());
    assert!(ApFloat::zero(SINGLE, ApFloatSign::Negative).is_negative());
    assert!(!ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_negative());
    assert!(ApFloat::qnan(SINGLE, ApFloatSign::Negative, NanPayload::Absent).is_negative());
    assert!(!ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_negative());
    assert!(ApFloat::snan(SINGLE, ApFloatSign::Negative, NanPayload::Absent).is_negative());
}

/// Port of `TEST(APFloatTest, isNormal)`.
#[test]
fn is_normal() {
    assert!(parse(SINGLE, "0x1p+0").is_normal());
    assert!(!ApFloat::inf(SINGLE, ApFloatSign::Positive).is_normal());
    assert!(!ApFloat::zero(SINGLE, ApFloatSign::Positive).is_normal());
    assert!(!ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_normal());
    assert!(!ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_normal());
    assert!(!parse(SINGLE, "0x1p-149").is_normal());
}

/// Port of `TEST(APFloatTest, isFinite)`.
#[test]
fn is_finite() {
    assert!(parse(SINGLE, "0x1p+0").is_finite());
    assert!(!ApFloat::inf(SINGLE, ApFloatSign::Positive).is_finite());
    assert!(ApFloat::zero(SINGLE, ApFloatSign::Positive).is_finite());
    assert!(!ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_finite());
    assert!(!ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_finite());
    assert!(parse(SINGLE, "0x1p-149").is_finite());
}

/// Port of `TEST(APFloatTest, isNaN)`, including its sweep over every
/// semantics that has a NaN.
#[test]
fn is_nan() {
    assert!(!parse(SINGLE, "0x1p+0").is_nan());
    assert!(!ApFloat::inf(SINGLE, ApFloatSign::Positive).is_nan());
    assert!(!ApFloat::zero(SINGLE, ApFloatSign::Positive).is_nan());
    assert!(ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_nan());
    assert!(ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_nan());
    assert!(!parse(SINGLE, "0x1p-149").is_nan());

    for semantics in MODELED {
        assert!(
            ApFloat::qnan(semantics, ApFloatSign::Positive, NanPayload::Absent).is_nan(),
            "{semantics:?}"
        );
    }
}

/// Port of `TEST(APFloatTest, isInfinity)`, including its sweep over every
/// semantics that has an infinity.
#[test]
fn is_infinity() {
    assert!(!parse(SINGLE, "0x1p+0").is_infinity());

    let pos = ApFloat::inf(SINGLE, ApFloatSign::Positive);
    let neg = ApFloat::inf(SINGLE, ApFloatSign::Negative);
    assert!(pos.is_infinity() && pos.is_pos_infinity() && !pos.is_neg_infinity());
    assert!(neg.is_infinity() && !neg.is_pos_infinity() && neg.is_neg_infinity());

    assert!(!ApFloat::zero(SINGLE, ApFloatSign::Positive).is_infinity());
    assert!(!ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_infinity());
    assert!(!ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_infinity());
    assert!(!parse(SINGLE, "0x1p-149").is_infinity());

    for semantics in MODELED {
        assert!(
            ApFloat::inf(semantics, ApFloatSign::Positive).is_infinity(),
            "{semantics:?}"
        );
    }
}

/// Port of `TEST(APFloatTest, isFiniteNonZero)`.
#[test]
fn is_finite_non_zero() {
    assert!(parse(SINGLE, "0x1p+0").is_finite_non_zero());
    assert!(parse(SINGLE, "-0x1p+0").is_finite_non_zero());
    assert!(parse(SINGLE, "0x1p-149").is_finite_non_zero());
    assert!(parse(SINGLE, "-0x1p-149").is_finite_non_zero());
    assert!(!ApFloat::inf(SINGLE, ApFloatSign::Positive).is_finite_non_zero());
    assert!(!ApFloat::inf(SINGLE, ApFloatSign::Negative).is_finite_non_zero());
    assert!(!ApFloat::zero(SINGLE, ApFloatSign::Positive).is_finite_non_zero());
    assert!(!ApFloat::zero(SINGLE, ApFloatSign::Negative).is_finite_non_zero());
    assert!(!ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_finite_non_zero());
    assert!(!ApFloat::qnan(SINGLE, ApFloatSign::Negative, NanPayload::Absent).is_finite_non_zero());
    assert!(!ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).is_finite_non_zero());
    assert!(!ApFloat::snan(SINGLE, ApFloatSign::Negative, NanPayload::Absent).is_finite_non_zero());
}

/// Port of `TEST(APFloatTest, isInteger)`.
#[test]
fn is_integer() {
    assert!(ApFloat::zero(DOUBLE, ApFloatSign::Negative).is_integer());
    assert!(!parse(DOUBLE, "3.14159").is_integer());
    assert!(!ApFloat::qnan(DOUBLE, ApFloatSign::Positive, NanPayload::Absent).is_integer());
    assert!(!ApFloat::inf(DOUBLE, ApFloatSign::Positive).is_integer());
    assert!(!ApFloat::inf(DOUBLE, ApFloatSign::Negative).is_integer());
    assert!(ApFloat::largest(DOUBLE, ApFloatSign::Positive).is_integer());
}

/// Port of `TEST(APFloatTest, Denormal)` for the four modeled semantics it
/// covers. The `classify()` assertions are upstream's finer `FPClassTest`.
#[test]
fn denormal() {
    let cases: [(ApFloatSemantics, &str); 4] = [
        (SINGLE, "1.17549435082228750797e-38"),
        (DOUBLE, "2.22507385850720138309e-308"),
        (X87, "3.36210314311209350626e-4932"),
        (QUAD, "3.36210314311209350626267781732175260e-4932"),
    ];
    for (semantics, min_normal) in cases {
        assert!(
            !parse(semantics, min_normal).is_denormal(),
            "{semantics:?} min-normal must not be denormal"
        );
        assert!(
            !ApFloat::zero(semantics, ApFloatSign::Positive).is_denormal(),
            "{semantics:?} zero must not be denormal"
        );
        let two = parse(semantics, "2.0");
        let (halved, _) =
            parse(semantics, min_normal).divide(&two, RoundingMode::NearestTiesToEven);
        assert!(
            halved.is_denormal(),
            "{semantics:?} min-normal / 2 must be denormal"
        );
    }

    // Upstream repeats the whole block with the negated single-precision
    // constant; ported here rather than only for TF32 as upstream happens to.
    let neg_min_normal = "-1.17549435082228750797e-38";
    assert!(!parse(SINGLE, neg_min_normal).is_denormal());
    let two = parse(SINGLE, "2.0");
    let (halved, _) = parse(SINGLE, neg_min_normal).divide(&two, RoundingMode::NearestTiesToEven);
    assert!(halved.is_denormal());
    assert!(halved.is_negative());
}

/// Port of `TEST(APFloatTest, IsSmallestNormalized)` over the modeled
/// semantics. `getAllOnesValue` has no llvmkit counterpart, so that one row is
/// omitted; every other assertion is ported, including the four-step
/// `next` walk around the boundary.
#[test]
fn is_smallest_normalized() {
    for semantics in MODELED {
        assert!(!ApFloat::zero(semantics, ApFloatSign::Positive).is_smallest_normalized());
        assert!(!ApFloat::zero(semantics, ApFloatSign::Negative).is_smallest_normalized());
        assert!(!ApFloat::inf(semantics, ApFloatSign::Positive).is_smallest_normalized());
        assert!(!ApFloat::inf(semantics, ApFloatSign::Negative).is_smallest_normalized());
        assert!(
            !ApFloat::qnan(semantics, ApFloatSign::Positive, NanPayload::Absent)
                .is_smallest_normalized()
        );
        assert!(
            !ApFloat::snan(semantics, ApFloatSign::Positive, NanPayload::Absent)
                .is_smallest_normalized()
        );
        assert!(!ApFloat::largest(semantics, ApFloatSign::Positive).is_smallest_normalized());
        assert!(!ApFloat::largest(semantics, ApFloatSign::Negative).is_smallest_normalized());
        assert!(!ApFloat::smallest(semantics, ApFloatSign::Positive).is_smallest_normalized());
        assert!(!ApFloat::smallest(semantics, ApFloatSign::Negative).is_smallest_normalized());

        for sign in [ApFloatSign::Positive, ApFloatSign::Negative] {
            let value = ApFloat::smallest_normalized(semantics, sign);
            assert!(value.is_smallest_normalized(), "{semantics:?} {sign:?}");
            let old_sign = value.is_negative();

            let (stepped, status) = value.next(ApFloatNextDirection::TowardPositive);
            assert_eq!(status, ApFloatStatus::OK, "{semantics:?} {sign:?}");
            assert_eq!(stepped.is_negative(), old_sign);
            assert!(!stepped.is_smallest_normalized());

            let (back, status) = stepped.next(ApFloatNextDirection::TowardNegative);
            assert_eq!(status, ApFloatStatus::OK, "{semantics:?} {sign:?}");
            assert!(
                back.is_smallest_normalized(),
                "{semantics:?} {sign:?}: stepping up from smallest-normalized                  {:?} gave {:?}, and stepping back down gave {:?}",
                value.to_bits().words(),
                stepped.to_bits().words(),
                back.to_bits().words()
            );
            assert_eq!(back.is_negative(), old_sign);

            let (beyond, status) = back.next(ApFloatNextDirection::TowardNegative);
            assert_eq!(status, ApFloatStatus::OK, "{semantics:?} {sign:?}");
            assert!(!beyond.is_smallest_normalized());
            assert_eq!(beyond.is_negative(), old_sign);
        }
    }
}

// ── Small operations ────────────────────────────────────────────────────────

/// Port of `TEST(APFloatTest, abs)`.
#[test]
fn abs() {
    let pos_inf = ApFloat::inf(SINGLE, ApFloatSign::Positive);
    let pos_zero = ApFloat::zero(SINGLE, ApFloatSign::Positive);
    let pos_qnan = ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent);
    let pos_snan = ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent);
    let pos_normal = parse(SINGLE, "0x1p+0");
    let pos_largest = ApFloat::largest(SINGLE, ApFloatSign::Positive);
    let pos_smallest = ApFloat::smallest(SINGLE, ApFloatSign::Positive);
    let pos_smallest_normalized = ApFloat::smallest_normalized(SINGLE, ApFloatSign::Positive);

    let pairs: [(&ApFloat, ApFloat); 16] = [
        (&pos_inf, ApFloat::inf(SINGLE, ApFloatSign::Positive).abs()),
        (&pos_inf, ApFloat::inf(SINGLE, ApFloatSign::Negative).abs()),
        (
            &pos_zero,
            ApFloat::zero(SINGLE, ApFloatSign::Positive).abs(),
        ),
        (
            &pos_zero,
            ApFloat::zero(SINGLE, ApFloatSign::Negative).abs(),
        ),
        (
            &pos_qnan,
            ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).abs(),
        ),
        (
            &pos_qnan,
            ApFloat::qnan(SINGLE, ApFloatSign::Negative, NanPayload::Absent).abs(),
        ),
        (
            &pos_snan,
            ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).abs(),
        ),
        (
            &pos_snan,
            ApFloat::snan(SINGLE, ApFloatSign::Negative, NanPayload::Absent).abs(),
        ),
        (&pos_normal, parse(SINGLE, "0x1p+0").abs()),
        (&pos_normal, parse(SINGLE, "-0x1p+0").abs()),
        (
            &pos_largest,
            ApFloat::largest(SINGLE, ApFloatSign::Positive).abs(),
        ),
        (
            &pos_largest,
            ApFloat::largest(SINGLE, ApFloatSign::Negative).abs(),
        ),
        (
            &pos_smallest,
            ApFloat::smallest(SINGLE, ApFloatSign::Positive).abs(),
        ),
        (
            &pos_smallest,
            ApFloat::smallest(SINGLE, ApFloatSign::Negative).abs(),
        ),
        (
            &pos_smallest_normalized,
            ApFloat::smallest_normalized(SINGLE, ApFloatSign::Positive).abs(),
        ),
        (
            &pos_smallest_normalized,
            ApFloat::smallest_normalized(SINGLE, ApFloatSign::Negative).abs(),
        ),
    ];
    for (index, (expected, got)) in pairs.iter().enumerate() {
        assert!(expected.bitwise_is_equal(got), "abs case {index}");
    }
}

/// Port of `TEST(APFloatTest, neg)`.
#[test]
fn neg() {
    let one = parse(SINGLE, "1.0");
    let neg_one = parse(SINGLE, "-1.0");
    let zero = ApFloat::zero(SINGLE, ApFloatSign::Positive);
    let neg_zero = ApFloat::zero(SINGLE, ApFloatSign::Negative);
    let inf = ApFloat::inf(SINGLE, ApFloatSign::Positive);
    let neg_inf = ApFloat::inf(SINGLE, ApFloatSign::Negative);
    let qnan = ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent);
    let neg_qnan = ApFloat::qnan(SINGLE, ApFloatSign::Negative, NanPayload::Absent);

    assert!(neg_one.bitwise_is_equal(&one.neg()));
    assert!(one.bitwise_is_equal(&neg_one.neg()));
    assert!(neg_zero.bitwise_is_equal(&zero.neg()));
    assert!(zero.bitwise_is_equal(&neg_zero.neg()));
    assert!(neg_inf.bitwise_is_equal(&inf.neg()));
    assert!(inf.bitwise_is_equal(&neg_inf.neg()));
    assert!(neg_qnan.bitwise_is_equal(&qnan.neg()));
    assert!(qnan.bitwise_is_equal(&neg_qnan.neg()));
}

/// Port of `TEST(APFloatTest, ilogb)`.
///
/// Upstream compares against the sentinel `int`s `IEK_NaN = INT_MIN`,
/// `IEK_Zero = INT_MIN + 1`, `IEK_Inf = INT_MAX` (`APFloat.h`); llvmkit returns
/// a `BinaryExponent`, so the same comparisons are spelled against its
/// variants.
#[test]
fn ilogb() {
    let finite = BinaryExponent::Finite;

    assert_eq!(
        ApFloat::smallest(DOUBLE, ApFloatSign::Positive).ilogb(),
        finite(-1074)
    );
    assert_eq!(
        ApFloat::smallest(DOUBLE, ApFloatSign::Negative).ilogb(),
        finite(-1074)
    );
    assert_eq!(
        parse(DOUBLE, "0x1.ffffffffffffep-1024").ilogb(),
        finite(-1023)
    );
    assert_eq!(
        parse(DOUBLE, "0x1.ffffffffffffep-1023").ilogb(),
        finite(-1023)
    );
    assert_eq!(
        parse(DOUBLE, "-0x1.ffffffffffffep-1023").ilogb(),
        finite(-1023)
    );
    assert_eq!(parse(DOUBLE, "0x1p-51").ilogb(), finite(-51));
    assert_eq!(
        parse(DOUBLE, "0x1.c60f120d9f87cp-1023").ilogb(),
        finite(-1023)
    );
    assert_eq!(parse(DOUBLE, "0x0.ffffp-1").ilogb(), finite(-2));
    assert_eq!(parse(DOUBLE, "0x1.fffep-1023").ilogb(), finite(-1023));
    assert_eq!(
        ApFloat::largest(DOUBLE, ApFloatSign::Positive).ilogb(),
        finite(1023)
    );
    assert_eq!(
        ApFloat::largest(DOUBLE, ApFloatSign::Negative).ilogb(),
        finite(1023)
    );

    assert_eq!(parse(SINGLE, "0x1p+0").ilogb(), finite(0));
    assert_eq!(parse(SINGLE, "-0x1p+0").ilogb(), finite(0));
    assert_eq!(parse(SINGLE, "0x1p+42").ilogb(), finite(42));
    assert_eq!(parse(SINGLE, "0x1p-42").ilogb(), finite(-42));

    assert_eq!(
        ApFloat::inf(SINGLE, ApFloatSign::Positive).ilogb(),
        BinaryExponent::Infinity
    );
    assert_eq!(
        ApFloat::inf(SINGLE, ApFloatSign::Negative).ilogb(),
        BinaryExponent::Infinity
    );
    assert_eq!(
        ApFloat::zero(SINGLE, ApFloatSign::Positive).ilogb(),
        BinaryExponent::Zero
    );
    assert_eq!(
        ApFloat::zero(SINGLE, ApFloatSign::Negative).ilogb(),
        BinaryExponent::Zero
    );
    assert_eq!(
        ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).ilogb(),
        BinaryExponent::Nan
    );
    assert_eq!(
        ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).ilogb(),
        BinaryExponent::Nan
    );

    assert_eq!(
        ApFloat::largest(SINGLE, ApFloatSign::Positive).ilogb(),
        finite(127)
    );
    assert_eq!(
        ApFloat::largest(SINGLE, ApFloatSign::Negative).ilogb(),
        finite(127)
    );
    assert_eq!(
        ApFloat::smallest(SINGLE, ApFloatSign::Positive).ilogb(),
        finite(-149)
    );
    assert_eq!(
        ApFloat::smallest(SINGLE, ApFloatSign::Negative).ilogb(),
        finite(-149)
    );
    assert_eq!(
        ApFloat::smallest_normalized(SINGLE, ApFloatSign::Positive).ilogb(),
        finite(-126)
    );
    assert_eq!(
        ApFloat::smallest_normalized(SINGLE, ApFloatSign::Negative).ilogb(),
        finite(-126)
    );
}

/// Port of `TEST(APFloatTest, x87Largest)`.
#[test]
fn x87_largest() {
    assert!(ApFloat::largest(X87, ApFloatSign::Positive).is_largest());
}

/// Port of `TEST(APFloatTest, x87Next)`.
#[test]
fn x87_next() {
    let (stepped, _) = parse(X87, "-1.0").next(ApFloatNextDirection::TowardPositive);
    assert_eq!(stepped.ilogb(), BinaryExponent::Finite(-1));
}

/// Port of `TEST(APFloatTest, exactInverse)`, restricted to the modeled
/// semantics. Upstream's host-`float`/`double` constructors are spelled here
/// as the equivalent literals in the same semantics.
#[test]
fn exact_inverse() {
    let cases: [(ApFloatSemantics, &str, &str); 6] = [
        (DOUBLE, "2.0", "0.5"),
        (SINGLE, "2.0", "0.5"),
        (QUAD, "2.0", "0.5"),
        (PPC, "2.0", "0.5"),
        (X87, "2.0", "0.5"),
        (DOUBLE, "0x1p1022", "0x1p-1022"),
    ];
    for (semantics, value, expected) in cases {
        let inverse = parse(semantics, value)
            .exact_inverse()
            .unwrap_or_else(|| panic!("{semantics:?} {value} has an exact inverse"));
        assert!(
            inverse.bitwise_is_equal(&parse(semantics, expected)),
            "{semantics:?} 1/{value} should be {expected}"
        );
    }

    // Denormals and non-powers-of-two have no exact inverse.
    assert!(parse(DOUBLE, "0x1p-1074").exact_inverse().is_none());
    assert!(parse(DOUBLE, "1.40129846e-45").exact_inverse().is_none());
    assert!(parse(DOUBLE, "3.0").exact_inverse().is_none());
}

/// Port of `TEST(APFloatTest, Comparisons)`: the ten ordered values, each
/// compared against every other.
#[test]
fn comparisons() {
    let values = [
        ApFloat::qnan(SINGLE, ApFloatSign::Negative, NanPayload::Absent),
        ApFloat::inf(SINGLE, ApFloatSign::Negative),
        ApFloat::largest(SINGLE, ApFloatSign::Negative),
        parse(SINGLE, "-0x1p+0"),
        ApFloat::zero(SINGLE, ApFloatSign::Negative),
        ApFloat::zero(SINGLE, ApFloatSign::Positive),
        parse(SINGLE, "0x1p+0"),
        ApFloat::largest(SINGLE, ApFloatSign::Positive),
        ApFloat::inf(SINGLE, ApFloatSign::Positive),
        ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent),
    ];
    // Index 0 and 9 are the NaNs; -0.0 and +0.0 (indices 4 and 5) compare
    // equal. Everything else is strictly ordered by index.
    for (i, lhs) in values.iter().enumerate() {
        for (j, rhs) in values.iter().enumerate() {
            let expected = if i == 0 || i == 9 || j == 0 || j == 9 {
                ApFloatCmpResult::Unordered
            } else if (i == 4 || i == 5) && (j == 4 || j == 5) {
                ApFloatCmpResult::Equal
            } else if i < j {
                ApFloatCmpResult::LessThan
            } else if i > j {
                ApFloatCmpResult::GreaterThan
            } else {
                ApFloatCmpResult::Equal
            };
            assert_eq!(lhs.compare(rhs), expected, "compare({i}, {j})");
        }
    }
}
