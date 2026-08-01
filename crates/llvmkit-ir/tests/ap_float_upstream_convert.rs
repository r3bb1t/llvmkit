//! Port of `TEST(APFloatTest, convert)` from
//! `llvm/unittests/ADT/APFloatTest.cpp` in the vendored `llvmorg-22.1.4` tree.
//!
//! Ported in upstream's order and with upstream's assertions: the converted
//! value (compared bitwise, or against the exact host value upstream compares
//! against), the `losesInfo` flag, and — where upstream checks it — the
//! returned status.
//!
//! This is where NaN payloads cross a semantics boundary in both directions:
//! widening keeps the payload and quiets a signaling NaN, narrowing drops a
//! payload that no longer fits while still returning a NaN.

use llvmkit_ir::{
    ApFloat, ApFloatSemantics, ApFloatSign, ApFloatStatus, ApInt, LosesInfo, NanPayload,
    RoundingMode,
};

const BFLOAT: ApFloatSemantics = ApFloatSemantics::BFloat;
const SINGLE: ApFloatSemantics = ApFloatSemantics::IeeeSingle;
const DOUBLE: ApFloatSemantics = ApFloatSemantics::IeeeDouble;
const QUAD: ApFloatSemantics = ApFloatSemantics::IeeeQuad;
const X87: ApFloatSemantics = ApFloatSemantics::X87DoubleExtended;

const NEAREST: RoundingMode = RoundingMode::NearestTiesToEven;

fn parse(semantics: ApFloatSemantics, text: &str) -> ApFloat {
    ApFloat::from_string(semantics, text, NEAREST)
        .unwrap_or_else(|e| panic!("upstream spells {text:?}, which must parse: {e}"))
        .0
}

fn u64_bits(value: &ApFloat) -> u64 {
    value
        .to_bits()
        .try_zext_u64()
        .expect("semantics under test fit in u64")
}

/// `convertToFloat` / `convertToDouble` compare against an exact host value;
/// llvmkit spells the same check as a bit comparison against the literal.
fn is_f32(value: &ApFloat, expected: f32) -> bool {
    u64_bits(value) == u64::from(expected.to_bits())
}

#[test]
fn upstream_convert() {
    // double 1.0 -> single, exact.
    let (test, _status, loses) = parse(DOUBLE, "1.0").convert(SINGLE, NEAREST);
    assert!(is_f32(&test, 1.0), "double 1.0 -> single");
    assert_eq!(loses, LosesInfo::No);

    // x87 (1.0 + 0x1p-53) -> double, rounds back to 1.0 and loses information.
    let (sum, _) = parse(X87, "0x1p-53").add(&parse(X87, "1.0"), NEAREST);
    let (test, _status, loses) = sum.convert(DOUBLE, NEAREST);
    assert!(test.is_exactly_value_f64(1.0), "x87 1+2^-53 -> double");
    assert_eq!(loses, LosesInfo::Yes);

    // Same shape from quad.
    let (sum, _) = parse(QUAD, "0x1p-53").add(&parse(QUAD, "1.0"), NEAREST);
    let (test, _status, loses) = sum.convert(DOUBLE, NEAREST);
    assert!(test.is_exactly_value_f64(1.0), "quad 1+2^-53 -> double");
    assert_eq!(loses, LosesInfo::Yes);

    // An x87 value that is exactly representable as a double.
    let (test, _status, loses) = parse(X87, "0xf.fffffffp+28").convert(DOUBLE, NEAREST);
    assert!(test.is_exactly_value_f64(4294967295.0));
    assert_eq!(loses, LosesInfo::No);

    // Widening a signaling NaN quiets it, and upstream notes that two bits of
    // the 64-bit x87 significand end up set.
    let top_two_bits = ApInt::from_words(64, &[0x6000_0000_0000_0000]);
    let (test, status, loses) =
        ApFloat::snan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).convert(X87, NEAREST);
    assert!(
        test.bitwise_is_equal(&ApFloat::qnan(
            X87,
            ApFloatSign::Positive,
            NanPayload::Bits(&top_two_bits)
        )),
        "single sNaN -> x87 should be the x87 qNaN with the top two significand bits set"
    );
    assert_eq!(loses, LosesInfo::No);
    assert_eq!(status, ApFloatStatus::INVALID_OP);

    // Widening a quiet NaN leaves the x87 quiet NaN.
    let x87_qnan = ApFloat::qnan(X87, ApFloatSign::Positive, NanPayload::Absent);
    let (test, _status, loses) =
        ApFloat::qnan(SINGLE, ApFloatSign::Positive, NanPayload::Absent).convert(X87, NEAREST);
    assert!(test.bitwise_is_equal(&x87_qnan));
    assert_eq!(loses, LosesInfo::No);

    // Converting to the same semantics is the identity, signaling included.
    let x87_snan = ApFloat::snan(X87, ApFloatSign::Positive, NanPayload::Absent);
    let (test, _status, loses) =
        ApFloat::snan(X87, ApFloatSign::Positive, NanPayload::Absent).convert(X87, NEAREST);
    assert!(test.bitwise_is_equal(&x87_snan));
    assert_eq!(loses, LosesInfo::No);

    let (test, _status, loses) =
        ApFloat::qnan(X87, ApFloatSign::Positive, NanPayload::Absent).convert(X87, NEAREST);
    assert!(test.bitwise_is_equal(&x87_qnan));
    assert_eq!(loses, LosesInfo::No);

    // The payload is lost in truncation, but NaN is retained by setting the
    // quiet bit.
    let payload = ApInt::from_words(52, &[1]);
    let (test, status, loses) =
        ApFloat::snan(DOUBLE, ApFloatSign::Positive, NanPayload::Bits(&payload))
            .convert(SINGLE, NEAREST);
    assert_eq!(u64_bits(&test), 0x7fc0_0000);
    assert_eq!(loses, LosesInfo::Yes);
    assert_eq!(status, ApFloatStatus::INVALID_OP);

    // The payload is lost in truncation. A quiet NaN remains quiet, and the
    // status stays OK.
    let (test, status, loses) =
        ApFloat::qnan(DOUBLE, ApFloatSign::Positive, NanPayload::Bits(&payload))
            .convert(SINGLE, NEAREST);
    assert_eq!(u64_bits(&test), 0x7fc0_0000);
    assert_eq!(loses, LosesInfo::Yes);
    assert_eq!(status, ApFloatStatus::OK);

    // Subnormals in double -> float conversion.
    for text in [
        "0x0.0000010000000p-1022",
        "0x0.0000010000001p-1022",
        "-0x0.0000010000001p-1022",
        "0x0.0000020000000p-1022",
        "0x0.0000020000001p-1022",
    ] {
        let (test, _status, loses) = parse(DOUBLE, text).convert(SINGLE, NEAREST);
        assert!(test.is_zero(), "{text} -> single should flush to zero");
        assert_eq!(loses, LosesInfo::Yes, "{text}");
    }

    // Subnormal conversion to bfloat.
    let (test, _status, loses) = parse(SINGLE, "0x0.01p-126").convert(BFLOAT, NEAREST);
    assert!(test.is_zero());
    assert_eq!(loses, LosesInfo::Yes);

    let (test, _status, loses) = parse(SINGLE, "0x0.02p-126").convert(BFLOAT, NEAREST);
    assert_eq!(u64_bits(&test), 0x01);
    assert_eq!(loses, LosesInfo::No);

    let (test, _status, loses) =
        parse(SINGLE, "0x0.01p-126").convert(BFLOAT, RoundingMode::NearestTiesToAway);
    assert_eq!(u64_bits(&test), 0x01);
    assert_eq!(loses, LosesInfo::Yes);
}
