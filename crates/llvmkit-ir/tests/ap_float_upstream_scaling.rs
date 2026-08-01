//! Ports of `TEST(APFloatTest, scalbn)`, `frexp`, and `getExactLog2` from
//! `llvm/unittests/ADT/APFloatTest.cpp` in the vendored `llvmorg-22.1.4` tree.
//!
//! Upstream's `getExactLog2` / `getExactLog2Abs` answer `INT_MIN` for "not an
//! exact power of two"; llvmkit spells the same answer `None`, so the ports
//! below read `None` wherever upstream reads `INT_MIN`. That is a spelling
//! difference in the return type, not in the logic.
//!
//! `frexp`'s exponent for the special categories is `ilogb`'s answer, not zero
//! — upstream's `frexp` opens with `Exp = ilogb(Val)` and returns before
//! overwriting it for NaN and infinity. Upstream reads those two as the
//! sentinel `int`s `IEK_NaN` / `IEK_Inf`; llvmkit returns a `BinaryExponent`,
//! so the ports read its variants, again a spelling difference only.

use llvmkit_ir::{
    ApFloat, ApFloatSemantics, ApFloatSign, ApInt, BinaryExponent, NanPayload, RoundingMode,
};

const HALF: ApFloatSemantics = ApFloatSemantics::IeeeHalf;
const BFLOAT: ApFloatSemantics = ApFloatSemantics::BFloat;
const SINGLE: ApFloatSemantics = ApFloatSemantics::IeeeSingle;
const DOUBLE: ApFloatSemantics = ApFloatSemantics::IeeeDouble;
const QUAD: ApFloatSemantics = ApFloatSemantics::IeeeQuad;
const X87: ApFloatSemantics = ApFloatSemantics::X87DoubleExtended;
const PPC: ApFloatSemantics = ApFloatSemantics::PpcDoubleDouble;

const MODELED: [ApFloatSemantics; 7] = [HALF, BFLOAT, SINGLE, DOUBLE, QUAD, X87, PPC];
const NEAREST: RoundingMode = RoundingMode::NearestTiesToEven;

fn parse(semantics: ApFloatSemantics, text: &str) -> ApFloat {
    ApFloat::from_string(semantics, text, NEAREST)
        .unwrap_or_else(|e| panic!("upstream spells {text:?}, which must parse: {e}"))
        .0
}

fn scalbn(value: &ApFloat, exponent: i32) -> ApFloat {
    value.scalbn(exponent, NEAREST).0
}

/// Port of `TEST(APFloatTest, scalbn)`.
#[test]
fn upstream_scalbn() {
    assert!(parse(SINGLE, "0x1p+0").bitwise_is_equal(&scalbn(&parse(SINGLE, "0x1p+0"), 0)));
    assert!(parse(SINGLE, "0x1p+42").bitwise_is_equal(&scalbn(&parse(SINGLE, "0x1p+0"), 42)));
    assert!(parse(SINGLE, "0x1p-42").bitwise_is_equal(&scalbn(&parse(SINGLE, "0x1p+0"), -42)));

    let pos_inf = ApFloat::inf(SINGLE, ApFloatSign::Positive);
    let neg_inf = ApFloat::inf(SINGLE, ApFloatSign::Negative);
    let pos_zero = ApFloat::zero(SINGLE, ApFloatSign::Positive);
    let neg_zero = ApFloat::zero(SINGLE, ApFloatSign::Negative);
    let pos_qnan = ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent);
    let neg_qnan = ApFloat::qnan(SINGLE, ApFloatSign::Negative, NanPayload::Absent);
    let snan = ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent);

    assert!(pos_inf.bitwise_is_equal(&scalbn(&pos_inf, 0)));
    assert!(neg_inf.bitwise_is_equal(&scalbn(&neg_inf, 0)));
    assert!(pos_zero.bitwise_is_equal(&scalbn(&pos_zero, 0)));
    assert!(neg_zero.bitwise_is_equal(&scalbn(&neg_zero, 0)));
    assert!(pos_qnan.bitwise_is_equal(&scalbn(&pos_qnan, 0)));
    assert!(neg_qnan.bitwise_is_equal(&scalbn(&neg_qnan, 0)));
    assert!(!scalbn(&snan, 0).is_signaling());

    let scalbn_snan = scalbn(&snan, 1);
    assert!(scalbn_snan.is_nan() && !scalbn_snan.is_signaling());

    // The highest bit of the payload must be preserved.
    let payload = ApInt::from_words(64, &[(1u64 << 50) | (1u64 << 49) | (1234u64 << 32) | 1]);
    let snan_with_payload =
        ApFloat::snan(DOUBLE, ApFloatSign::Positive, NanPayload::Bits(&payload));
    let quiet_payload = scalbn(&snan_with_payload, 1);
    assert!(quiet_payload.is_nan() && !quiet_payload.is_signaling());
    assert_eq!(
        quiet_payload
            .to_bits()
            .bitand(&ApInt::low_bits_set(64, 51))
            .try_zext_u64(),
        payload.try_zext_u64(),
        "scalbn must preserve the NaN payload"
    );

    assert!(pos_inf.bitwise_is_equal(&scalbn(&parse(SINGLE, "0x1p+0"), 128)));
    assert!(neg_inf.bitwise_is_equal(&scalbn(&parse(SINGLE, "-0x1p+0"), 128)));
    assert!(pos_inf.bitwise_is_equal(&scalbn(&parse(SINGLE, "0x1p+127"), 1)));
    assert!(pos_zero.bitwise_is_equal(&scalbn(&parse(SINGLE, "0x1p-127"), -127)));
    assert!(neg_zero.bitwise_is_equal(&scalbn(&parse(SINGLE, "-0x1p-127"), -127)));
    assert!(parse(SINGLE, "-0x1p-149").bitwise_is_equal(&scalbn(&parse(SINGLE, "-0x1p-127"), -22)));
    assert!(pos_zero.bitwise_is_equal(&scalbn(&parse(SINGLE, "0x1p-126"), -24)));
}

/// Port of `TEST(APFloatTest, frexp)`.
#[test]
fn upstream_frexp() {
    let frexp = |value: &ApFloat| {
        let (fraction, exponent, _status) = value.frexp(NEAREST);
        (fraction, exponent)
    };

    let (fraction, exponent) = frexp(&ApFloat::zero(DOUBLE, ApFloatSign::Positive));
    assert_eq!(exponent, BinaryExponent::Finite(0));
    assert!(fraction.is_pos_zero());

    let (fraction, exponent) = frexp(&ApFloat::zero(DOUBLE, ApFloatSign::Negative));
    assert_eq!(exponent, BinaryExponent::Finite(0));
    assert!(fraction.is_neg_zero());

    let (fraction, exponent) = frexp(&parse(DOUBLE, "1.0"));
    assert_eq!(exponent, BinaryExponent::Finite(1));
    assert!(parse(DOUBLE, "0x1p-1").bitwise_is_equal(&fraction));

    let (fraction, exponent) = frexp(&parse(DOUBLE, "-1.0"));
    assert_eq!(exponent, BinaryExponent::Finite(1));
    assert!(parse(DOUBLE, "-0x1p-1").bitwise_is_equal(&fraction));

    // Infinities and NaNs answer with `ilogb`'s category, not zero.
    let (fraction, exponent) = frexp(&ApFloat::inf(DOUBLE, ApFloatSign::Positive));
    assert_eq!(exponent, BinaryExponent::Infinity);
    assert!(fraction.is_infinity() && !fraction.is_negative());

    let (fraction, exponent) = frexp(&ApFloat::inf(DOUBLE, ApFloatSign::Negative));
    assert_eq!(exponent, BinaryExponent::Infinity);
    assert!(fraction.is_infinity() && fraction.is_negative());

    for sign in [ApFloatSign::Positive, ApFloatSign::Negative] {
        let (fraction, exponent) = frexp(&ApFloat::qnan(DOUBLE, sign, NanPayload::Absent));
        assert_eq!(exponent, BinaryExponent::Nan);
        assert!(fraction.is_nan());
    }

    let (fraction, exponent) = frexp(&ApFloat::snan(
        DOUBLE,
        ApFloatSign::Positive,
        NanPayload::Absent,
    ));
    assert_eq!(exponent, BinaryExponent::Nan);
    assert!(fraction.is_nan() && !fraction.is_signaling());

    // A signaling NaN is quieted and keeps its payload.
    let payload = ApInt::from_words(64, &[(1u64 << 50) | (1u64 << 49) | (1234u64 << 32) | 1]);
    let (fraction, exponent) = frexp(&ApFloat::snan(
        DOUBLE,
        ApFloatSign::Positive,
        NanPayload::Bits(&payload),
    ));
    assert_eq!(exponent, BinaryExponent::Nan);
    assert!(fraction.is_nan() && !fraction.is_signaling());
    assert_eq!(
        fraction
            .to_bits()
            .bitand(&ApInt::low_bits_set(64, 51))
            .try_zext_u64(),
        payload.try_zext_u64(),
        "frexp must preserve the NaN payload"
    );

    let (fraction, exponent) = frexp(&parse(DOUBLE, "0x0.ffffp-1"));
    assert_eq!(exponent, BinaryExponent::Finite(-1));
    assert!(parse(DOUBLE, "0x1.fffep-1").bitwise_is_equal(&fraction));
}

/// Port of `TEST(APFloatTest, getExactLog2)` over the modeled semantics.
///
/// `None` stands in for upstream's `INT_MIN`.
#[test]
fn upstream_get_exact_log2() {
    for semantics in MODELED {
        assert_eq!(
            parse(semantics, "1.0").exact_log2(),
            Some(0),
            "{semantics:?}"
        );
        assert_eq!(parse(semantics, "3.0").exact_log2(), None, "{semantics:?}");
        assert_eq!(parse(semantics, "-3.0").exact_log2(), None, "{semantics:?}");
        assert_eq!(
            parse(semantics, "3.0").exact_log2_abs(),
            None,
            "{semantics:?}"
        );
        assert_eq!(
            parse(semantics, "-3.0").exact_log2_abs(),
            None,
            "{semantics:?}"
        );

        assert_eq!(
            parse(semantics, "8.0").exact_log2(),
            Some(3),
            "{semantics:?}"
        );
        assert_eq!(parse(semantics, "-8.0").exact_log2(), None, "{semantics:?}");
        assert_eq!(
            parse(semantics, "0.25").exact_log2(),
            Some(-2),
            "{semantics:?}"
        );
        assert_eq!(
            parse(semantics, "0.25").exact_log2_abs(),
            Some(-2),
            "{semantics:?}"
        );
        assert_eq!(
            parse(semantics, "-0.25").exact_log2(),
            None,
            "{semantics:?}"
        );
        assert_eq!(
            parse(semantics, "-0.25").exact_log2_abs(),
            Some(-2),
            "{semantics:?}"
        );
        assert_eq!(
            parse(semantics, "8.0").exact_log2_abs(),
            Some(3),
            "{semantics:?}"
        );
        assert_eq!(
            parse(semantics, "-8.0").exact_log2_abs(),
            Some(3),
            "{semantics:?}"
        );

        for sign in [ApFloatSign::Positive, ApFloatSign::Negative] {
            assert_eq!(ApFloat::zero(semantics, sign).exact_log2(), None);
            assert_eq!(ApFloat::zero(semantics, sign).exact_log2_abs(), None);
            assert_eq!(ApFloat::inf(semantics, sign).exact_log2(), None);
            assert_eq!(ApFloat::inf(semantics, sign).exact_log2_abs(), None);
            assert_eq!(
                ApFloat::qnan(semantics, sign, NanPayload::Absent).exact_log2(),
                None
            );
            assert_eq!(
                ApFloat::qnan(semantics, sign, NanPayload::Absent).exact_log2_abs(),
                None
            );
        }
    }
}
