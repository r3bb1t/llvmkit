//! Ports of `TEST(APFloatTest, FMA)`, `roundToIntegral`, and `toInteger` from
//! `llvm/unittests/ADT/APFloatTest.cpp` in the vendored `llvmorg-22.1.4` tree.
//!
//! Upstream builds many of these operands from host `float` / `double`
//! literals (`APFloat f1(14.5f)`). llvmkit has no host-float constructor, so
//! the literal is carried through its exact bit pattern — the same value, and
//! `f32`/`f64` round-trip through their own bits losslessly.

use llvmkit_ir::{
    ApFloat, ApFloatSemantics, ApFloatSign, ApFloatStatus as S, ApInt, Exactness, NanPayload,
    RoundingMode, Signedness,
};

const HALF: ApFloatSemantics = ApFloatSemantics::IeeeHalf;
const SINGLE: ApFloatSemantics = ApFloatSemantics::IeeeSingle;
const DOUBLE: ApFloatSemantics = ApFloatSemantics::IeeeDouble;
const X87: ApFloatSemantics = ApFloatSemantics::X87DoubleExtended;
const NEAREST: RoundingMode = RoundingMode::NearestTiesToEven;

/// An `IEEEsingle` from its exact bit pattern.
///
/// Upstream writes these operands as host `float` literals
/// (`APFloat f1(14.5f)`). Carrying the bit pattern instead is exact by
/// construction and keeps decimal literals — several of which clippy rejects
/// for excessive precision — out of a bit-exactness test. The upstream
/// spelling is kept in a comment beside each value.
fn f32v(pattern: u32) -> ApFloat {
    ApFloat::from_bits(SINGLE, &ApInt::from_words(32, &[u64::from(pattern)]))
        .expect("32-bit pattern is valid for IEEEsingle")
}

/// An `IEEEdouble` from its exact bit pattern.
fn f64v(pattern: u64) -> ApFloat {
    ApFloat::from_bits(DOUBLE, &ApInt::from_words(64, &[pattern]))
        .expect("64-bit pattern is valid for IEEEdouble")
}

fn parse(semantics: ApFloatSemantics, text: &str) -> ApFloat {
    ApFloat::from_string(semantics, text, NEAREST)
        .unwrap_or_else(|e| panic!("upstream spells {text:?}, which must parse: {e}"))
        .0
}

fn bits(value: &ApFloat) -> u64 {
    value
        .to_bits()
        .try_zext_u64()
        .expect("semantics under test fit in u64")
}

fn fma(a: &ApFloat, b: &ApFloat, c: &ApFloat, rounding: RoundingMode) -> ApFloat {
    a.fused_multiply_add(b, c, rounding).0
}

/// Port of `TEST(APFloatTest, FMA)`.
#[test]
fn upstream_fma() {
    // fma(14.5, -14.5, 225.0) == 14.75
    assert_eq!(
        bits(&fma(
            &f32v(0x41680000 /* 14.5 */),
            &f32v(0xc1680000 /* -14.5 */),
            &f32v(0x43610000 /* 225.0 */),
            NEAREST
        )),
        bits(&f32v(0x416c0000 /* 14.75 */))
    );

    // Denormal operands: (min_normal/2) * (min_normal/2) + 12.0 == 12.0
    let two = f32v(0x40000000 /* 2.0 */);
    let half_min_normal = f32v(0x00800000 /* 1.175_494_35e-38 */)
        .divide(&two, NEAREST)
        .0;
    assert_eq!(
        bits(&fma(
            &half_min_normal,
            &half_min_normal,
            &f32v(0x41400000 /* 12.0 */),
            NEAREST
        )),
        bits(&f32v(0x41400000 /* 12.0 */))
    );

    // Correct zero sign when the answer is exactly zero: fma(1, -1, 1) -> +0.
    let result = fma(
        &f64v(0x3ff0000000000000 /* 1.0 */),
        &f64v(0xbff0000000000000 /* -1.0 */),
        &f64v(0x3ff0000000000000 /* 1.0 */),
        NEAREST,
    );
    assert!(!result.is_negative() && result.is_zero());

    // Same, rounding toward negative -> -0.
    let result = fma(
        &f64v(0x3ff0000000000000 /* 1.0 */),
        &f64v(0xbff0000000000000 /* -1.0 */),
        &f64v(0x3ff0000000000000 /* 1.0 */),
        RoundingMode::TowardNegative,
    );
    assert!(result.is_negative() && result.is_zero());

    // Adding like-signed zeros: fma(0.0, -0.0, -0.0) -> -0.
    let result = fma(
        &f64v(0x0000000000000000 /* 0.0 */),
        &f64v(0x8000000000000000 /* -0.0 */),
        &f64v(0x8000000000000000 /* -0.0 */),
        NEAREST,
    );
    assert!(result.is_negative() && result.is_zero());

    // Negative sign preserved when a small negative result underflows.
    let result = fma(
        &parse(DOUBLE, "-0x1p-1074"),
        &parse(DOUBLE, "+0x1p-1074"),
        &f64v(0x0000000000000000 /* 0.0 */),
        NEAREST,
    );
    assert!(result.is_negative() && result.is_zero());

    // x87 extended-precision case from llvm.org/PR20728.
    let one = parse(X87, "1.0");
    let three = parse(X87, "3.0");
    let squared = fma(&one, &one, &three, NEAREST);
    let (narrowed, _status, loses) = squared.convert(SINGLE, NEAREST);
    assert_eq!(loses, llvmkit_ir::LosesInfo::No);
    assert_eq!(bits(&narrowed), bits(&f32v(0x40800000 /* 4.0 */)));

    // Regression test that failed an assertion.
    assert_eq!(
        bits(&fma(
            &f32v(0x8000f6c5 /* -8.852_422_8e-41 */),
            &f32v(0x40000000 /* 2.0 */),
            &f32v(0x0000f6c5 /* 8.852_422_8e-41 */),
            NEAREST
        )),
        bits(&f32v(0x8000f6c5 /* -8.852_422_8e-41 */))
    );

    // The nine `addOrSubtractSignificand` subtraction cases upstream enumerates.
    let cases: [(u32, u32, u32, u32); 7] = [
        // cmpEqual, loss from lhs
        (0x80a06150, 0x36790227, 0x00000027, 0x80000000), // -1.472_858_9e-38, 3.710_514_4e-6, 5.5e-44, -0.0
        // cmpGreaterThan, no loss
        (0x40000000, 0x40000000, 0xc0600000, 0x3f000000), // 2.0, 2.0, -3.5, 0.5
        // cmpLessThan, no loss
        (0x40000000, 0x40000000, 0xc0900000, 0xbf000000), // 2.0, 2.0, -4.5, -0.5
        // cmpEqual, no loss
        (0x40000000, 0x40000000, 0xc0800000, 0x00000000), // 2.0, 2.0, -4.0, 0.0
        // cmpLessThan, loss from lhs
        (0x40000001, 0x40000001, 0xc2000000, 0xc1dfffff), // 2.000_000_2, 2.000_000_2, -32.0, -27.999_998
        // cmpGreaterThan, loss from rhs
        (0x501502f9, 0x501502f9, 0xc0000001, 0x60ad78ec), // 1e10, 1e10, -2.000_000_2, 1e20
        // cmpGreaterThan, loss from lhs
        (0x03aa2425, 0x3b000001, 0x80000001, 0x00154484), // 1e-36, 0.001_953_125_2, -1e-45, 1.953_124e-39
    ];
    for (index, (a, b, c, expected)) in cases.into_iter().enumerate() {
        assert_eq!(
            bits(&fma(&f32v(a), &f32v(b), &f32v(c), NEAREST)),
            bits(&f32v(expected)),
            "addOrSubtractSignificand case {index}"
        );
    }

    // Cases from llvm/llvm-project#104984.
    assert_eq!(
        bits(&fma(
            &f32v(0x3e7fffff /* 0.249_999_98 */),
            &f32v(0x00ffffff /* 2.350_988_5e-38 */),
            &f32v(0x80000001 /* -1e-45 */),
            NEAREST
        )),
        bits(&f32v(0x003fffff /* 5.877_47e-39 */))
    );
    assert_eq!(
        bits(&fma(
            &f64v(0x001fffffffffffff /* 4.450_147_717_014_402_3e-308 */),
            &f64v(0x3fcfffffffffffff /* 0.249_999_999_999_999_97 */),
            &f64v(0x80061846cae05e3e /* -8.475_904_604_373_977e-309 */),
            NEAREST
        )),
        bits(&f64v(
            0x0001e7b9351fa1c2 /* 2.649_464_688_162_03e-309 */
        ))
    );

    let half_bits = |pattern: u64| {
        ApFloat::from_bits(HALF, &ApInt::from_words(16, &[pattern]))
            .expect("16-bit pattern is valid for IEEEhalf")
    };
    assert_eq!(
        bits(&fma(
            &half_bits(0x8fff),
            &half_bits(0x2bff),
            &half_bits(0x0172),
            NEAREST
        )),
        0x808e
    );

    // A single instance used for all three operands: fma(1.5, 1.5, 1.5).
    let f = f64v(0x3ff8000000000000 /* 1.5 */);
    assert_eq!(
        bits(&fma(&f, &f, &f, NEAREST)),
        bits(&f64v(0x400e000000000000 /* 3.75 */))
    );
}

/// Port of `TEST(APFloatTest, roundToIntegral)`.
#[test]
fn upstream_round_to_integral() {
    let round = |value: &ApFloat, rounding: RoundingMode| value.round_to_integral(rounding);

    let t = f64v(0xbfe0000000000000 /* -0.5 */);
    let s = f64v(0x40091eb851eb851f /* 3.14 */);
    let r = ApFloat::largest(DOUBLE, ApFloatSign::Positive);

    assert_eq!(
        bits(&round(&t, RoundingMode::TowardZero).0),
        bits(&f64v(0x8000000000000000 /* -0.0 */))
    );
    assert_eq!(
        bits(&round(&t, RoundingMode::TowardNegative).0),
        bits(&f64v(0xbff0000000000000 /* -1.0 */))
    );
    assert_eq!(
        bits(&round(&t, RoundingMode::TowardPositive).0),
        bits(&f64v(0x8000000000000000 /* -0.0 */))
    );
    assert_eq!(
        bits(&round(&t, NEAREST).0),
        bits(&f64v(0x8000000000000000 /* -0.0 */))
    );

    assert_eq!(
        bits(&round(&s, RoundingMode::TowardZero).0),
        bits(&f64v(0x4008000000000000 /* 3.0 */))
    );
    assert_eq!(
        bits(&round(&s, RoundingMode::TowardNegative).0),
        bits(&f64v(0x4008000000000000 /* 3.0 */))
    );
    assert_eq!(
        bits(&round(&s, RoundingMode::TowardPositive).0),
        bits(&f64v(0x4010000000000000 /* 4.0 */))
    );
    assert_eq!(
        bits(&round(&s, NEAREST).0),
        bits(&f64v(0x4008000000000000 /* 3.0 */))
    );

    for rounding in [
        RoundingMode::TowardZero,
        RoundingMode::TowardNegative,
        RoundingMode::TowardPositive,
        NEAREST,
    ] {
        assert_eq!(
            bits(&round(&r, rounding).0),
            bits(&r),
            "largest is integral"
        );
    }

    assert_eq!(
        bits(
            &round(
                &ApFloat::zero(DOUBLE, ApFloatSign::Positive),
                RoundingMode::TowardZero
            )
            .0
        ),
        bits(&f64v(0x0000000000000000 /* 0.0 */))
    );
    assert_eq!(
        bits(
            &round(
                &ApFloat::zero(DOUBLE, ApFloatSign::Negative),
                RoundingMode::TowardZero
            )
            .0
        ),
        bits(&f64v(0x8000000000000000 /* -0.0 */))
    );

    // Quiet NaNs pass through unchanged with an OK status; signaling NaNs are
    // quieted and raise invalid-op. The sign is preserved either way.
    for negative in [false, true] {
        let sign = if negative {
            ApFloatSign::Negative
        } else {
            ApFloatSign::Positive
        };

        let (value, status) = round(
            &ApFloat::qnan(DOUBLE, sign, NanPayload::Absent),
            RoundingMode::TowardZero,
        );
        assert!(value.is_nan());
        assert_eq!(value.is_negative(), negative);
        assert_eq!(status, S::OK);

        let (value, status) = round(
            &ApFloat::snan(DOUBLE, sign, NanPayload::Absent),
            RoundingMode::TowardZero,
        );
        assert!(value.is_nan() && !value.is_signaling());
        assert_eq!(value.is_negative(), negative);
        assert_eq!(status, S::INVALID_OP);

        let (value, status) = round(&ApFloat::inf(DOUBLE, sign), RoundingMode::TowardZero);
        assert!(value.is_infinity());
        assert_eq!(value.is_negative(), negative);
        assert_eq!(status, S::OK);
    }

    for rounding in [RoundingMode::TowardZero, RoundingMode::TowardNegative] {
        let (value, status) = round(&ApFloat::zero(DOUBLE, ApFloatSign::Positive), rounding);
        assert!(value.is_zero() && !value.is_negative());
        assert_eq!(status, S::OK);
    }
}

/// Port of `TEST(APFloatTest, toInteger)`.
#[test]
fn upstream_to_integer() {
    let to_integer = |text: &str, signedness: Signedness| {
        parse(DOUBLE, text).convert_to_integer(5, signedness, RoundingMode::TowardZero)
    };

    let (value, status, exact) = to_integer("10", Signedness::Unsigned);
    assert_eq!(status, S::OK);
    assert_eq!(exact, Exactness::Exact);
    assert_eq!(value.try_zext_u64(), Some(10));

    let (value, status, exact) = to_integer("-10", Signedness::Unsigned);
    assert_eq!(status, S::INVALID_OP);
    assert_eq!(exact, Exactness::Inexact);
    assert_eq!(value.try_zext_u64(), Some(0), "unsigned minimum");

    let (value, status, exact) = to_integer("32", Signedness::Unsigned);
    assert_eq!(status, S::INVALID_OP);
    assert_eq!(exact, Exactness::Inexact);
    assert_eq!(value.try_zext_u64(), Some(31), "unsigned maximum");

    let (value, status, exact) = to_integer("7.9", Signedness::Unsigned);
    assert_eq!(status, S::INEXACT);
    assert_eq!(exact, Exactness::Inexact);
    assert_eq!(value.try_zext_u64(), Some(7));

    let (value, status, exact) = to_integer("-10", Signedness::Signed);
    assert_eq!(status, S::OK);
    assert_eq!(exact, Exactness::Exact);
    assert_eq!(value.try_zext_u64(), Some(0b10110), "-10 in five bits");

    let (value, status, exact) = to_integer("-17", Signedness::Signed);
    assert_eq!(status, S::INVALID_OP);
    assert_eq!(exact, Exactness::Inexact);
    assert_eq!(value.try_zext_u64(), Some(0b10000), "signed minimum, -16");

    let (value, status, exact) = to_integer("16", Signedness::Signed);
    assert_eq!(status, S::INVALID_OP);
    assert_eq!(exact, Exactness::Inexact);
    assert_eq!(value.try_zext_u64(), Some(15), "signed maximum");
}
