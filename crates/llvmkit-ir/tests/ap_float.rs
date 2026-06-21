//! APFloat tests copied from upstream where exact assertions exist, plus
//! source-derived llvmkit-specific subsets for fp128/non-host-float gaps.
//!
//! These assert the Rust `ApFloat` semantic wrapper preserves LLVM's category,
//! sign, status, and bitcast behavior for the IR float kinds llvmkit models.

use llvmkit_ir::{
    ApFloat, ApFloatCategory, ApFloatCmpResult, ApFloatNextDirection, ApFloatSemantics,
    ApFloatSign, ApFloatStatus, ApInt, ApIntSignedness, ApIntTruncation, Exactness, IrError,
    LosesInfo, NanPayload, RoundingMode,
};

fn ap_f32(value: f32) -> Result<ApFloat, IrError> {
    ApFloat::from_bits(
        ApFloatSemantics::IeeeSingle,
        &ApInt::from_words(32, &[u64::from(value.to_bits())]),
    )
}

fn ap_f64(value: f64) -> Result<ApFloat, IrError> {
    ApFloat::from_bits(
        ApFloatSemantics::IeeeDouble,
        &ApInt::from_words(64, &[value.to_bits()]),
    )
}

fn ap_ppc_double_double(high_bits: u64, low_bits: u64) -> Result<ApFloat, IrError> {
    ApFloat::from_bits(
        ApFloatSemantics::PpcDoubleDouble,
        &ApInt::from_words(128, &[low_bits, high_bits]),
    )
}

fn assert_convert_from_apint_matches(
    input: &ApInt,
    signedness: ApIntSignedness,
    rounding: RoundingMode,
    expected: &ApInt,
) {
    let (float, _) =
        ApFloat::convert_from_ap_int(ApFloatSemantics::IeeeQuad, input, signedness, rounding);
    let (result, _, exact) =
        float.convert_to_integer(input.bit_width(), signedness, RoundingMode::TowardZero);
    assert_eq!(exact, Exactness::Exact);
    assert_eq!(result, *expected);
}

/// Port of `llvm/unittests/ADT/APFloatTest.cpp` signed-zero checks.
#[test]
fn signed_zero_categories_and_bits_round_trip() -> Result<(), IrError> {
    let neg_zero = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Negative);
    assert!(neg_zero.is_zero());
    assert!(neg_zero.is_neg_zero());
    assert_eq!(neg_zero.category(), ApFloatCategory::Zero);
    assert_eq!(
        neg_zero.to_bits().try_zext_u64(),
        Some(0x8000_0000_0000_0000)
    );

    let round_trip = ApFloat::from_bits(ApFloatSemantics::IeeeDouble, &neg_zero.to_bits())?;
    assert!(round_trip.bitwise_is_equal(&neg_zero));
    Ok(())
}

/// Port of `llvm/unittests/ADT/APFloatTest.cpp` infinity and NaN construction checks.
#[test]
fn infinity_and_nan_classification() {
    let inf = ApFloat::inf(ApFloatSemantics::IeeeSingle, ApFloatSign::Positive);
    assert!(inf.is_pos_infinity());
    assert_eq!(inf.category(), ApFloatCategory::Infinity);

    let nan = ApFloat::qnan(
        ApFloatSemantics::IeeeSingle,
        ApFloatSign::Negative,
        NanPayload::Absent,
    );
    assert!(nan.is_nan());
    assert!(!nan.is_signaling());
    assert!(nan.is_negative());
}

/// Port of `llvm/unittests/ADT/APFloatTest.cpp` arithmetic status checks.
#[test]
fn double_add_and_divide_status_bits() -> Result<(), IrError> {
    let one = ApFloat::one(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive);
    let two = one.add(&one, RoundingMode::NearestTiesToEven).0;
    assert!(two.is_exactly_value_f64(2.0));

    let zero = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive);
    let (_quotient, status) = one.divide(&zero, RoundingMode::NearestTiesToEven);
    assert!(status.contains(ApFloatStatus::DIV_BY_ZERO));
    Ok(())
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, add)`:
/// source-derived fp128 exact significand arithmetic; mirrors `IEEEFloat::add`.
#[test]
fn quad_add_preserves_low_significand_bit() -> Result<(), IrError> {
    let one = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0, 0x3fff_0000_0000_0000]),
    )?;
    let ulp = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0, 0x3f8f_0000_0000_0000]),
    )?;

    let (sum, status) = one.add(&ulp, RoundingMode::NearestTiesToEven);

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(
        sum.to_bits(),
        ApInt::from_words(128, &[1, 0x3fff_0000_0000_0000])
    );
    Ok(())
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, subtract)`:
/// source-derived fp128 exact significand arithmetic; mirrors `IEEEFloat::subtract`.
#[test]
fn quad_sub_preserves_low_significand_bit() -> Result<(), IrError> {
    let sum = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[1, 0x3fff_0000_0000_0000]),
    )?;
    let one = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0, 0x3fff_0000_0000_0000]),
    )?;

    let (difference, status) = sum.subtract(&one, RoundingMode::NearestTiesToEven);

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(
        difference.to_bits(),
        ApInt::from_words(128, &[0, 0x3f8f_0000_0000_0000])
    );
    Ok(())
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, multiply)`:
/// source-derived fp128 exact significand arithmetic; mirrors `IEEEFloat::multiply`.
#[test]
fn quad_multiply_preserves_low_significand_bit() -> Result<(), IrError> {
    let sum = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[1, 0x3fff_0000_0000_0000]),
    )?;
    let two = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0, 0x4000_0000_0000_0000]),
    )?;

    let (product, status) = sum.multiply(&two, RoundingMode::NearestTiesToEven);

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(
        product.to_bits(),
        ApInt::from_words(128, &[1, 0x4000_0000_0000_0000])
    );
    Ok(())
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, divide)`:
/// source-derived fp128 wide-range arithmetic; mirrors `IEEEFloat::divide`.
#[test]
fn quad_divide_large_finite_value_without_double_overflow() -> Result<(), IrError> {
    let largest = ApFloat::largest(ApFloatSemantics::IeeeQuad, ApFloatSign::Positive);
    let two = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0, 0x4000_0000_0000_0000]),
    )?;

    let (quotient, status) = largest.divide(&two, RoundingMode::NearestTiesToEven);

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(
        quotient.to_bits(),
        ApInt::from_words(128, &[u64::MAX, 0x7ffd_ffff_ffff_ffff])
    );
    Ok(())
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, remainder)`:
/// source-derived fp128 IEEE remainder arithmetic; mirrors `IEEEFloat::remainder`.
#[test]
fn quad_remainder_preserves_fraction_below_double_epsilon() -> Result<(), IrError> {
    let sum = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[1, 0x3fff_0000_0000_0000]),
    )?;
    let one = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0, 0x3fff_0000_0000_0000]),
    )?;

    let (remainder, status) = sum.remainder(&one);

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(
        remainder.to_bits(),
        ApInt::from_words(128, &[0, 0x3f8f_0000_0000_0000])
    );
    Ok(())
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, mod)`:
/// `frem` follows C `fmod` semantics, so the result keeps the dividend sign.
#[test]
fn modulo_keeps_negative_dividend_sign() -> Result<(), IrError> {
    let (lhs, _) = ApFloat::from_string(
        ApFloatSemantics::IeeeDouble,
        "-5.0",
        RoundingMode::TowardZero,
    )?;
    let (rhs, _) = ApFloat::from_string(
        ApFloatSemantics::IeeeDouble,
        "2.0",
        RoundingMode::TowardZero,
    )?;
    let (result, status) = lhs.modulo(&rhs);
    assert!(result.is_exactly_value_f64(-1.0));
    assert!(!status.contains(ApFloatStatus::INVALID_OP));
    Ok(())
}

/// llvmkit-specific subset of
/// `llvm/unittests/ADT/APFloatTest.cpp::APFloatConvertFromAPIntParamTest.HalfwayRounding` for IeeeQuad.
#[test]
fn convert_from_apint_halfway_rounding_matches_upstream() {
    let precision = ApFloatSemantics::IeeeQuad.precision();
    for signedness in [ApIntSignedness::Unsigned, ApIntSignedness::Signed] {
        let bit_width = precision
            + 1
            + if matches!(signedness, ApIntSignedness::Signed) {
                1
            } else {
                0
            };
        let one = ApInt::from_words(bit_width, &[1]);
        let rounded_down = ApInt::one_bit_set(bit_width, precision);
        let halfway = rounded_down.wrapping_add(&one);
        let rounded_up = halfway.wrapping_add(&one);

        assert_convert_from_apint_matches(
            &halfway,
            signedness,
            RoundingMode::NearestTiesToEven,
            &rounded_down,
        );
        assert_convert_from_apint_matches(
            &halfway,
            signedness,
            RoundingMode::NearestTiesToAway,
            &rounded_up,
        );
        assert_convert_from_apint_matches(
            &halfway,
            signedness,
            RoundingMode::TowardPositive,
            &rounded_up,
        );
        assert_convert_from_apint_matches(
            &halfway,
            signedness,
            RoundingMode::TowardNegative,
            &rounded_down,
        );
        assert_convert_from_apint_matches(
            &halfway,
            signedness,
            RoundingMode::TowardZero,
            &rounded_down,
        );
    }
}

/// Port of `llvm/unittests/ADT/APFloatTest.cpp` compare and bitcast checks.
#[test]
fn bitcast_round_trips_and_unordered_compare() -> Result<(), IrError> {
    let bits = ApInt::new(
        32,
        0x3f80_0000,
        ApIntSignedness::Unsigned,
        ApIntTruncation::RejectOverflow,
    )?;
    let one = ApFloat::from_bits(ApFloatSemantics::IeeeSingle, &bits)?;
    assert_eq!(one.to_bits(), bits);
    assert!(one.is_exactly_value_f64(1.0));

    let nan = ApFloat::qnan(
        ApFloatSemantics::IeeeSingle,
        ApFloatSign::Positive,
        NanPayload::Absent,
    );
    assert_eq!(nan.compare(&one), ApFloatCmpResult::Unordered);
    Ok(())
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, compare)`:
/// fp128 comparison observes significand bits below host `double` precision.
#[test]
fn quad_compare_preserves_low_significand_bits() -> Result<(), IrError> {
    let one = ApFloat::one(ApFloatSemantics::IeeeQuad, ApFloatSign::Positive);
    let one_plus_low = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[1, 0x3fff_0000_0000_0000]),
    )?;

    assert_eq!(one_plus_low.compare(&one), ApFloatCmpResult::GreaterThan);
    assert_eq!(one.compare(&one_plus_low), ApFloatCmpResult::LessThan);
    Ok(())
}

/// Port of `llvm/unittests/ADT/APFloatTest.cpp` PPC double-double layout distinction.
#[test]
fn ppc_double_double_preserves_two_component_bits() -> Result<(), IrError> {
    let bits = ApInt::from_words(128, &[0, 0x3ff0_0000_0000_0000]);
    let value = ApFloat::from_bits(ApFloatSemantics::PpcDoubleDouble, &bits)?;
    assert_eq!(value.to_bits(), bits);
    assert_eq!(value.semantics(), ApFloatSemantics::PpcDoubleDouble);
    Ok(())
}

/// llvmkit-specific regression backed by `llvm/lib/Support/APFloat.cpp`:
/// `IEEEFloat::convertFromDecimalString` handles IEEE formats and
/// `DoubleAPFloat::convertFromString` handles PPC double-double.
#[test]
fn decimal_non_double_values_preserve_magnitude() -> Result<(), IrError> {
    let cases: &[(ApFloatSemantics, &[u64])] = &[
        (ApFloatSemantics::IeeeHalf, &[0x4000]),
        (ApFloatSemantics::BFloat, &[0x4000]),
        (ApFloatSemantics::IeeeQuad, &[0, 0x4000_0000_0000_0000]),
        (
            ApFloatSemantics::X87DoubleExtended,
            &[0x8000_0000_0000_0000, 0x4000],
        ),
        (
            ApFloatSemantics::PpcDoubleDouble,
            &[0, 0x4000_0000_0000_0000],
        ),
    ];
    for (semantics, words) in cases.iter().copied() {
        let (value, status) =
            ApFloat::from_string(semantics, "2.0", RoundingMode::NearestTiesToEven)?;
        assert_eq!(
            value.to_bits(),
            ApInt::from_words(semantics.bit_width(), words),
            "{semantics:?}"
        );
        assert!(value.is_exactly_value_f64(2.0), "{semantics:?}");
        assert!(!status.contains(ApFloatStatus::INEXACT), "{semantics:?}");
    }
    Ok(())
}

/// Port of the `LDBL_MAX` assertions from
/// `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, PPCDoubleDouble)`.
#[test]
fn ppc_decimal_ldbl_max_preserves_low_word() -> Result<(), IrError> {
    let (value, _status) = ApFloat::from_string(
        ApFloatSemantics::PpcDoubleDouble,
        "1.79769313486231580793728971405301e+308",
        RoundingMode::NearestTiesToEven,
    )?;

    assert_eq!(
        value.to_bits(),
        ApInt::from_words(128, &[0x7c8f_ffff_ffff_fffe, 0x7fef_ffff_ffff_ffff])
    );
    Ok(())
}

/// llvmkit-specific regression backed by
/// `llvm/lib/Support/APFloat.cpp::IEEEFloat::convertFromDecimalString`:
/// decimal parsing happens in fp128 semantics, not through host `double`.
#[test]
fn decimal_quad_literal_keeps_bits_beyond_double_precision() -> Result<(), IrError> {
    let (value, status) = ApFloat::from_string(
        ApFloatSemantics::IeeeQuad,
        "1.0000000000000001",
        RoundingMode::NearestTiesToEven,
    )?;
    assert_eq!(
        value.to_bits(),
        ApInt::from_words(128, &[0x0734_aca5_f622_6f0b, 0x3fff_0000_0000_0000])
    );
    assert!(status.contains(ApFloatStatus::INEXACT));
    Ok(())
}

/// llvmkit-specific regression backed by `IEEEFloat::convertFromDecimalString`:
/// fp128 can represent normal values beyond host `double` range.
#[test]
fn decimal_quad_literal_beyond_double_range_stays_finite() -> Result<(), IrError> {
    let (value, status) = ApFloat::from_string(
        ApFloatSemantics::IeeeQuad,
        "1e400",
        RoundingMode::NearestTiesToEven,
    )?;
    assert!(value.is_finite_non_zero());
    assert!(!value.is_infinity());
    assert!(status.contains(ApFloatStatus::INEXACT));
    Ok(())
}

/// llvmkit-specific regression backed by `APFloatTest.cpp::TEST(APFloatTest, isDenormal)`:
/// fp128 subnormal decimal values below host `double` range stay nonzero.
#[test]
fn decimal_quad_subnormal_below_double_range_stays_nonzero() -> Result<(), IrError> {
    let (value, status) = ApFloat::from_string(
        ApFloatSemantics::IeeeQuad,
        "1e-4932",
        RoundingMode::NearestTiesToEven,
    )?;
    assert!(value.is_finite_non_zero());
    assert!(value.is_denormal());
    assert!(status.contains(ApFloatStatus::INEXACT));
    Ok(())
}

/// llvmkit-specific subset of `llvm/lib/Support/APFloat.cpp::IEEEFloat::convertFromDecimalString`:
/// decimal overflow is handled by APInt-backed semantic conversion, not host `double` parsing.
#[test]
fn decimal_double_overflow_reports_overflow_without_host_parse() -> Result<(), IrError> {
    let (value, status) = ApFloat::from_string(
        ApFloatSemantics::IeeeDouble,
        "1e400",
        RoundingMode::NearestTiesToEven,
    )?;

    assert!(value.is_pos_infinity());
    assert!(status.contains(ApFloatStatus::OVERFLOW));
    assert!(status.contains(ApFloatStatus::INEXACT));
    Ok(())
}

/// llvmkit-specific subset of `llvm/lib/Support/APFloat.cpp::IEEEFloat::convertFromDecimalString`:
/// malformed decimal exponents do not fall back to host `double` parsing for wide semantics.
#[test]
fn decimal_quad_malformed_exponent_rejects_without_host_parse() {
    let result = ApFloat::from_string(
        ApFloatSemantics::IeeeQuad,
        "1e2147483648",
        RoundingMode::NearestTiesToEven,
    );

    assert!(matches!(result, Err(IrError::InvalidOperation { .. })));
}

/// Port of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, convert)`.
#[test]
fn quad_convert_to_double_reports_loses_info() -> Result<(), IrError> {
    let one = ApFloat::one(ApFloatSemantics::IeeeQuad, ApFloatSign::Positive);
    let low = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0, 0x3fca_0000_0000_0000]),
    )?;
    let test = low.add(&one, RoundingMode::NearestTiesToEven).0;

    let (converted, _, loses) = test.convert(
        ApFloatSemantics::IeeeDouble,
        RoundingMode::NearestTiesToEven,
    );

    assert!(converted.is_exactly_value_f64(1.0));
    assert_eq!(loses, LosesInfo::Yes);
    Ok(())
}

/// llvmkit-specific subset of
/// `llvm/unittests/ADT/APFloatTest.cpp::APFloatConvertFromAPIntParamTest.HalfwayRounding`:
/// an `i257` power-of-two exercises the APInt encoder beyond host `double` precision.
#[test]
fn sitofp_i257_to_quad_uses_apint_encoder() {
    let input = ApInt::one_bit_set(257, 200);

    let (converted, status) = ApFloat::convert_from_ap_int(
        ApFloatSemantics::IeeeQuad,
        &input,
        ApIntSignedness::Signed,
        RoundingMode::NearestTiesToEven,
    );

    assert_eq!(status, ApFloatStatus::OK);
    assert!(converted.is_finite_non_zero());
    assert_eq!(
        converted.to_bits(),
        ApInt::from_words(128, &[0, 0x40c7_0000_0000_0000])
    );
}

/// llvmkit-specific subset of
/// `llvm/unittests/ADT/APFloatTest.cpp::APFloatConvertFromAPIntParamTest`
/// for PPC double-double: the double-rounded high word keeps the residual low component.
#[test]
fn ppc_convert_from_ap_int_keeps_residual_low_component() {
    let input = ApInt::one_bit_set(61, 60).wrapping_add(&ApInt::from_words(61, &[1]));

    let (converted, status) = ApFloat::convert_from_ap_int(
        ApFloatSemantics::PpcDoubleDouble,
        &input,
        ApIntSignedness::Unsigned,
        RoundingMode::NearestTiesToEven,
    );

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(
        converted.to_bits(),
        ApInt::from_words(128, &[0x3ff0_0000_0000_0000, 0x43b0_0000_0000_0000])
    );
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::DoubleAPFloat::convertToInteger`:
/// PPC double-double integer conversion observes the low component beyond host `double` precision.
#[test]
fn ppc_convert_to_integer_uses_low_component() -> Result<(), IrError> {
    let value = ap_ppc_double_double(0x3ff0_0000_0000_0000, 0x3c30_0000_0000_0000)?;

    let (rounded_up, status, exact) =
        value.convert_to_integer(2, ApIntSignedness::Unsigned, RoundingMode::TowardPositive);
    assert_eq!(rounded_up, ApInt::from_words(2, &[2]));
    assert_eq!(status, ApFloatStatus::INEXACT);
    assert_eq!(exact, Exactness::Inexact);

    let (truncated, status, exact) =
        value.convert_to_integer(2, ApIntSignedness::Unsigned, RoundingMode::TowardZero);
    assert_eq!(truncated, ApInt::from_words(2, &[1]));
    assert_eq!(status, ApFloatStatus::INEXACT);
    assert_eq!(exact, Exactness::Inexact);
    Ok(())
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::IEEEFloat::convertToInteger`:
/// fp128 integer conversion preserves significand bits that host `double` cannot represent.
#[test]
fn quad_convert_to_integer_keeps_wide_significand_bits() -> Result<(), IrError> {
    let value = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[1, 0x406f_0000_0000_0000]),
    )?;
    let expected = ApInt::bitor(&ApInt::one_bit_set(257, 112), &ApInt::from_words(257, &[1]));

    let (converted, status, exact) =
        value.convert_to_integer(257, ApIntSignedness::Unsigned, RoundingMode::TowardZero);

    assert_eq!(converted, expected);
    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(exact, Exactness::Exact);
    Ok(())
}

/// llvmkit-specific subset of `llvm/lib/Support/APFloat.cpp::DoubleAPFloat::convert`:
/// converting an exactly-representable quad value to PPC double-double preserves the residual low word.
#[test]
fn ppc_convert_from_quad_keeps_residual_low_component() -> Result<(), IrError> {
    let value = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0x0010_0000_0000_0000, 0x403b_0000_0000_0000]),
    )?;

    let (converted, status, loses) = value.convert(
        ApFloatSemantics::PpcDoubleDouble,
        RoundingMode::NearestTiesToEven,
    );

    assert_eq!(converted.semantics(), ApFloatSemantics::PpcDoubleDouble);
    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(loses, LosesInfo::No);
    assert_eq!(
        converted.to_bits(),
        ApInt::from_words(128, &[0x3ff0_0000_0000_0000, 0x43b0_0000_0000_0000])
    );
    Ok(())
}

/// Port of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, roundToIntegral)`.
#[test]
fn round_to_integral_matches_upstream() -> Result<(), IrError> {
    let t = ap_f64(-0.5)?;
    let s = ap_f64(f64::from_bits(0x4009_1eb8_51eb_851f))?;
    let r = ApFloat::largest(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive);

    let (p, _) = t.round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_exactly_value_f64(-0.0));
    let (p, _) = t.round_to_integral(RoundingMode::TowardNegative);
    assert!(p.is_exactly_value_f64(-1.0));
    let (p, _) = t.round_to_integral(RoundingMode::TowardPositive);
    assert!(p.is_exactly_value_f64(-0.0));
    let (p, _) = t.round_to_integral(RoundingMode::NearestTiesToEven);
    assert!(p.is_exactly_value_f64(-0.0));

    let (p, _) = s.round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_exactly_value_f64(3.0));
    let (p, _) = s.round_to_integral(RoundingMode::TowardNegative);
    assert!(p.is_exactly_value_f64(3.0));
    let (p, _) = s.round_to_integral(RoundingMode::TowardPositive);
    assert!(p.is_exactly_value_f64(4.0));
    let (p, _) = s.round_to_integral(RoundingMode::NearestTiesToEven);
    assert!(p.is_exactly_value_f64(3.0));

    let (p, _) = r.round_to_integral(RoundingMode::TowardZero);
    assert_eq!(p.to_bits(), r.to_bits());
    let (p, _) = r.round_to_integral(RoundingMode::TowardNegative);
    assert_eq!(p.to_bits(), r.to_bits());
    let (p, _) = r.round_to_integral(RoundingMode::TowardPositive);
    assert_eq!(p.to_bits(), r.to_bits());
    let (p, _) = r.round_to_integral(RoundingMode::NearestTiesToEven);
    assert_eq!(p.to_bits(), r.to_bits());

    let (p, _) = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive)
        .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_exactly_value_f64(0.0));
    let (p, _) = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Negative)
        .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_exactly_value_f64(-0.0));
    let (p, _) = ApFloat::qnan(
        ApFloatSemantics::IeeeDouble,
        ApFloatSign::Positive,
        NanPayload::Absent,
    )
    .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_nan());
    let (p, _) = ApFloat::inf(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive)
        .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_infinity() && !p.is_negative());
    let (p, _) = ApFloat::inf(ApFloatSemantics::IeeeDouble, ApFloatSign::Negative)
        .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_infinity() && p.is_negative());

    let (p, status) = ApFloat::qnan(
        ApFloatSemantics::IeeeDouble,
        ApFloatSign::Positive,
        NanPayload::Absent,
    )
    .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_nan());
    assert!(!p.is_negative());
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ApFloat::qnan(
        ApFloatSemantics::IeeeDouble,
        ApFloatSign::Negative,
        NanPayload::Absent,
    )
    .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_nan());
    assert!(p.is_negative());
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ApFloat::snan(
        ApFloatSemantics::IeeeDouble,
        ApFloatSign::Positive,
        NanPayload::Absent,
    )
    .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_nan());
    assert!(!p.is_signaling());
    assert!(!p.is_negative());
    assert_eq!(status, ApFloatStatus::INVALID_OP);

    let (p, status) = ApFloat::snan(
        ApFloatSemantics::IeeeDouble,
        ApFloatSign::Negative,
        NanPayload::Absent,
    )
    .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_nan());
    assert!(!p.is_signaling());
    assert!(p.is_negative());
    assert_eq!(status, ApFloatStatus::INVALID_OP);

    let (p, status) = ApFloat::inf(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive)
        .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_infinity());
    assert!(!p.is_negative());
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ApFloat::inf(ApFloatSemantics::IeeeDouble, ApFloatSign::Negative)
        .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_infinity());
    assert!(p.is_negative());
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive)
        .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_zero());
    assert!(!p.is_negative());
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive)
        .round_to_integral(RoundingMode::TowardNegative);
    assert!(p.is_zero());
    assert!(!p.is_negative());
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Negative)
        .round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_zero());
    assert!(p.is_negative());
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Negative)
        .round_to_integral(RoundingMode::TowardNegative);
    assert!(p.is_zero());
    assert!(p.is_negative());
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ap_f64(1E-100)?.round_to_integral(RoundingMode::TowardNegative);
    assert!(p.is_zero());
    assert!(!p.is_negative());
    assert_eq!(status, ApFloatStatus::INEXACT);
    let (p, status) = ap_f64(1E-100)?.round_to_integral(RoundingMode::TowardPositive);
    assert!(p.is_exactly_value_f64(1.0));
    assert!(!p.is_negative());
    assert_eq!(status, ApFloatStatus::INEXACT);
    let (p, status) = ap_f64(-1E-100)?.round_to_integral(RoundingMode::TowardNegative);
    assert!(p.is_negative());
    assert!(p.is_exactly_value_f64(-1.0));
    assert_eq!(status, ApFloatStatus::INEXACT);
    let (p, status) = ap_f64(-1E-100)?.round_to_integral(RoundingMode::TowardPositive);
    assert!(p.is_zero());
    assert!(p.is_negative());
    assert_eq!(status, ApFloatStatus::INEXACT);

    let (p, status) = ap_f64(10.0)?.round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_exactly_value_f64(10.0));
    assert_eq!(status, ApFloatStatus::OK);

    let (p, status) = ap_f64(10.5)?.round_to_integral(RoundingMode::TowardZero);
    assert!(p.is_exactly_value_f64(10.0));
    assert_eq!(status, ApFloatStatus::INEXACT);
    let (p, status) = ap_f64(10.5)?.round_to_integral(RoundingMode::TowardPositive);
    assert!(p.is_exactly_value_f64(11.0));
    assert_eq!(status, ApFloatStatus::INEXACT);
    let (p, status) = ap_f64(10.5)?.round_to_integral(RoundingMode::TowardNegative);
    assert!(p.is_exactly_value_f64(10.0));
    assert_eq!(status, ApFloatStatus::INEXACT);
    let (p, status) = ap_f64(10.5)?.round_to_integral(RoundingMode::NearestTiesToAway);
    assert!(p.is_exactly_value_f64(11.0));
    assert_eq!(status, ApFloatStatus::INEXACT);
    let (p, status) = ap_f64(10.5)?.round_to_integral(RoundingMode::NearestTiesToEven);
    assert!(p.is_exactly_value_f64(10.0));
    assert_eq!(status, ApFloatStatus::INEXACT);
    Ok(())
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, roundToIntegral)`:
/// fp128 fractional bits below host `double` precision still drive status and directed rounding.
#[test]
fn quad_round_to_integral_preserves_low_bits() -> Result<(), IrError> {
    let value = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0x0008_0000_0000_0000, 0x403b_0000_0000_0000]),
    )?;

    let (nearest, nearest_status) = value.round_to_integral(RoundingMode::NearestTiesToEven);
    assert_eq!(
        nearest.to_bits(),
        ApInt::from_words(128, &[0, 0x403b_0000_0000_0000])
    );
    assert_eq!(nearest_status, ApFloatStatus::INEXACT);

    let (toward_zero, toward_zero_status) = value.round_to_integral(RoundingMode::TowardZero);
    assert_eq!(
        toward_zero.to_bits(),
        ApInt::from_words(128, &[0, 0x403b_0000_0000_0000])
    );
    assert_eq!(toward_zero_status, ApFloatStatus::INEXACT);

    let (toward_positive, toward_positive_status) =
        value.round_to_integral(RoundingMode::TowardPositive);
    assert_eq!(
        toward_positive.to_bits(),
        ApInt::from_words(128, &[0x0010_0000_0000_0000, 0x403b_0000_0000_0000])
    );
    assert_eq!(toward_positive_status, ApFloatStatus::INEXACT);
    Ok(())
}

/// llvmkit-specific subset of
/// `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, roundToIntegral)`:
/// PPC double-double rounding observes the residual low component.
#[test]
fn ppc_round_to_integral_uses_low_component() -> Result<(), IrError> {
    let value = ap_ppc_double_double(0x3ff0_0000_0000_0000, 0x3c30_0000_0000_0000)?;

    let (toward_zero, toward_zero_status) = value.round_to_integral(RoundingMode::TowardZero);
    assert_eq!(toward_zero_status, ApFloatStatus::INEXACT);
    assert_eq!(
        toward_zero.to_bits(),
        ApInt::from_words(128, &[0, 0x3ff0_0000_0000_0000])
    );

    let (toward_positive, toward_positive_status) =
        value.round_to_integral(RoundingMode::TowardPositive);
    assert_eq!(toward_positive_status, ApFloatStatus::INEXACT);
    assert_eq!(
        toward_positive.to_bits(),
        ApInt::from_words(128, &[0, 0x4000_0000_0000_0000])
    );
    Ok(())
}

/// Port of selected exact assertions from
/// `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, FMA)`.
#[test]
fn fma_matches_upstream() -> Result<(), IrError> {
    let f1 = ap_f32(14.5)?;
    let f2 = ap_f32(-14.5)?;
    let f3 = ap_f32(225.0)?;
    let (f1, _) = f1.fused_multiply_add(&f2, &f3, RoundingMode::NearestTiesToEven);
    assert!(f1.is_exactly_value_f64(f64::from(14.75f32)));

    let f1 = ap_f64(1.0)?;
    let f2 = ap_f64(-1.0)?;
    let f3 = ap_f64(1.0)?;
    let (f1, _) = f1.fused_multiply_add(&f2, &f3, RoundingMode::NearestTiesToEven);
    assert!(!f1.is_negative() && f1.is_zero());

    let f1 = ap_f64(1.0)?;
    let f2 = ap_f64(-1.0)?;
    let f3 = ap_f64(1.0)?;
    let (f1, _) = f1.fused_multiply_add(&f2, &f3, RoundingMode::TowardNegative);
    assert!(f1.is_negative() && f1.is_zero());

    let f1 = ap_f32(2.0)?;
    let f2 = ap_f32(2.0)?;
    let f3 = ap_f32(-3.5)?;
    let (f1, _) = f1.fused_multiply_add(&f2, &f3, RoundingMode::NearestTiesToEven);
    assert!(f1.is_exactly_value_f64(f64::from(0.5f32)));

    let f1 = ap_f32(2.0)?;
    let f2 = ap_f32(2.0)?;
    let f3 = ap_f32(-4.5)?;
    let (f1, _) = f1.fused_multiply_add(&f2, &f3, RoundingMode::NearestTiesToEven);
    assert!(f1.is_exactly_value_f64(f64::from(-0.5f32)));

    let f = ap_f64(1.5)?;
    let (f, _) = f.fused_multiply_add(&f, &f, RoundingMode::NearestTiesToEven);
    assert!(f.is_exactly_value_f64(3.75));
    Ok(())
}

/// llvmkit-specific subset of
/// `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, FMA)`:
/// fp128 cancellation proves `fusedMultiplyAdd` rounds once after the add.
#[test]
fn quad_fma_rounds_once() -> Result<(), IrError> {
    let one_plus = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[1, 0x3fff_0000_0000_0000]),
    )?;
    let one_minus = ApFloat::from_bits(
        ApFloatSemantics::IeeeQuad,
        &ApInt::from_words(128, &[0xffff_ffff_ffff_fffe, 0x3ffe_ffff_ffff_ffff]),
    )?;
    let minus_one = ApFloat::one(ApFloatSemantics::IeeeQuad, ApFloatSign::Negative);

    let (result, status) =
        one_plus.fused_multiply_add(&one_minus, &minus_one, RoundingMode::NearestTiesToEven);

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(
        result.to_bits(),
        ApInt::from_words(128, &[0, 0xbf1f_0000_0000_0000])
    );
    Ok(())
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::DoubleAPFloat::fusedMultiplyAdd`:
/// PPC double-double FMA consumes both high and low components of each operand.
#[test]
fn ppc_fma_uses_component_sums() -> Result<(), IrError> {
    let two = ap_ppc_double_double(0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0000)?;
    let three = ap_ppc_double_double(0x4000_0000_0000_0000, 0x3ff0_0000_0000_0000)?;
    let four = ap_ppc_double_double(0x4008_0000_0000_0000, 0x3ff0_0000_0000_0000)?;

    let (result, status) = two.fused_multiply_add(&three, &four, RoundingMode::NearestTiesToEven);

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(
        result.to_bits(),
        ApInt::from_words(128, &[0, 0x4024_0000_0000_0000])
    );
    Ok(())
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::IEEEFloat::fusedMultiplyAdd`:
/// `multiplySpecials` reports zero-times-infinity before addend NaN propagation.
#[test]
fn fma_zero_times_infinity_precedes_addend_nan() {
    let zero = ApFloat::zero(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive);
    let infinity = ApFloat::inf(ApFloatSemantics::IeeeDouble, ApFloatSign::Positive);
    let addend = ApFloat::qnan(
        ApFloatSemantics::IeeeDouble,
        ApFloatSign::Negative,
        NanPayload::Absent,
    );

    let (result, status) =
        zero.fused_multiply_add(&infinity, &addend, RoundingMode::NearestTiesToEven);

    assert!(result.is_nan());
    assert!(!result.is_negative());
    assert_eq!(status, ApFloatStatus::INVALID_OP);
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::DoubleAPFloat::fusedMultiplyAdd`:
/// PPC zero-times-infinity invalid operation takes precedence over quiet-NaN addend propagation.
#[test]
fn ppc_fma_zero_times_infinity_precedes_addend_nan() {
    let zero = ApFloat::zero(ApFloatSemantics::PpcDoubleDouble, ApFloatSign::Positive);
    let infinity = ApFloat::inf(ApFloatSemantics::PpcDoubleDouble, ApFloatSign::Positive);
    let addend = ApFloat::qnan(
        ApFloatSemantics::PpcDoubleDouble,
        ApFloatSign::Negative,
        NanPayload::Absent,
    );

    let (result, status) =
        zero.fused_multiply_add(&infinity, &addend, RoundingMode::NearestTiesToEven);

    assert!(result.is_nan());
    assert_eq!(result.semantics(), ApFloatSemantics::PpcDoubleDouble);
    assert!(!result.is_negative());
    assert_eq!(status, ApFloatStatus::INVALID_OP);
}

/// Port of the `nextUp(+0) = +getSmallest()` assertion from
/// `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, next)`.
#[test]
fn quad_next_uses_explicit_direction() {
    let zero = ApFloat::zero(ApFloatSemantics::IeeeQuad, ApFloatSign::Positive);

    let (next, status) = zero.next(ApFloatNextDirection::TowardPositive);

    assert_eq!(status, ApFloatStatus::OK);
    assert_eq!(next.to_bits(), ApInt::from_words(128, &[1, 0]));
}

/// llvmkit-specific subset of `llvm/unittests/ADT/APFloatTest.cpp::TEST(APFloatTest, next)`:
/// PPC double-double `next` advances from the full high-plus-low value, not the high word alone.
#[test]
fn ppc_next_uses_low_component() -> Result<(), IrError> {
    let high_only = ap_ppc_double_double(0x3ff0_0000_0000_0000, 0)?;
    let with_low = ap_ppc_double_double(0x3ff0_0000_0000_0000, 0x3c30_0000_0000_0000)?;

    let (next_high_only, high_status) = high_only.next(ApFloatNextDirection::TowardPositive);
    let (next_with_low, low_status) = with_low.next(ApFloatNextDirection::TowardPositive);

    assert_eq!(high_status, ApFloatStatus::OK);
    assert_eq!(low_status, ApFloatStatus::OK);
    assert_eq!(
        next_high_only.compare(&high_only),
        ApFloatCmpResult::GreaterThan
    );
    assert_eq!(
        next_with_low.compare(&with_low),
        ApFloatCmpResult::GreaterThan
    );
    assert_eq!(
        next_with_low.compare(&next_high_only),
        ApFloatCmpResult::GreaterThan
    );
    Ok(())
}
