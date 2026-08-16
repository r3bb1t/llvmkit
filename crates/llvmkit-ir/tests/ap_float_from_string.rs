//! `ApFloat::from_string` against the three grammars upstream accepts.
//!
//! **Boundary coverage with llvmkit-chosen inputs, not a verbatim port.** The
//! *rules* come from `IEEEFloat::convertFromString` and its two helpers
//! (`convertFromStringSpecials`, `convertFromHexadecimalString`), and the
//! closest upstream test family is `llvm/unittests/ADT/APFloatTest.cpp`'s
//! `fromHexadecimalString` / `fromStringSpecials` / `makeNaN`. The specific
//! spellings below were picked here to sit on the interesting boundaries —
//! the subnormal edge, the two tie-to-even directions, the payload radices —
//! so they are marked `llvmkit-specific subset` in `UPSTREAM.md` rather than
//! claiming to be those tests. Porting those three fixtures verbatim is
//! recorded as remaining work in `docs/future-work.md`.
//!
//! The verbatim port of the arithmetic tables lives next door in
//! `ap_float_upstream_arithmetic.rs`, and it exercises this parser hard: all
//! 784 of its rows construct their operands and expectations through
//! `from_string`, using upstream's own spellings.
//!
//! Before this, `from_string` accepted only `inf`/`nan`/`-nan` and decimal:
//! every hexadecimal literal and every NaN payload was rejected outright, even
//! though `llvm-as` accepts them and `APFloatTest.cpp` is written in them.

use llvmkit_ir::{ApFloat, ApFloatSemantics, RoundingMode};

const HALF: ApFloatSemantics = ApFloatSemantics::IeeeHalf;
const SINGLE: ApFloatSemantics = ApFloatSemantics::IeeeSingle;
const DOUBLE: ApFloatSemantics = ApFloatSemantics::IeeeDouble;

fn bits(semantics: ApFloatSemantics, text: &str) -> u64 {
    let (value, _status) = ApFloat::from_string(semantics, text, RoundingMode::NearestTiesToEven)
        .unwrap_or_else(|e| panic!("{text:?} should parse: {e}"));
    value
        .to_bits()
        .try_zext_u64()
        .expect("semantics under test are at most 64 bits wide")
}

/// The boundary values `APFloatTest.cpp`'s arithmetic tables are written in.
#[test]
fn hexadecimal_significands() {
    for (text, expected) in [
        ("0x1p+0", 0x3F80_0000),
        ("-0x1p+0", 0xBF80_0000),
        ("0x1p+1", 0x4000_0000),
        ("0x1.fffffep+127", 0x7F7F_FFFF),
        ("-0x1.fffffep+127", 0xFF7F_FFFF),
        ("0x1p-126", 0x0080_0000),
        // Subnormals: the exponent alone puts these below the normal range.
        ("0x1p-149", 0x0000_0001),
        ("0x1p-148", 0x0000_0002),
        ("0x1.fffffcp-127", 0x007F_FFFF),
        // The same subnormal written with the significand shifted instead.
        ("0x0.000002p-126", 0x0000_0001),
        ("-0x0.000002p-126", 0x8000_0001),
        ("0x0p+0", 0x0000_0000),
        ("-0x0p+0", 0x8000_0000),
    ] {
        assert_eq!(bits(SINGLE, text), expected, "for {text:?}");
    }
}

/// Rounding still happens in the destination semantics, so digits below the
/// available precision are rounded, not truncated.
#[test]
fn hexadecimal_rounds_to_nearest_even() {
    // The gap above 1.0 is 2^-23, so 0x1.000001p+0 (= 1 + 2^-24) sits exactly
    // halfway between 1.0 and its successor: ties go to the even significand,
    // which is 1.0 itself.
    assert_eq!(bits(SINGLE, "0x1.000001p+0"), 0x3F80_0000);
    // 1 + 3·2^-24 is halfway between the first and second successors; there
    // the even significand is the *upper* one.
    assert_eq!(bits(SINGLE, "0x1.000003p+0"), 0x3F80_0002);
    // Strictly above halfway, so it rounds up regardless of the tie rule.
    assert_eq!(bits(SINGLE, "0x1.0000018p+0"), 0x3F80_0001);
    // Wider than the destination by a long way: half keeps 11 bits.
    assert_eq!(bits(HALF, "0x1.ffcp+0"), 0x3FFF);
    assert_eq!(
        bits(DOUBLE, "0x1.fffffffffffffp+1023"),
        0x7FEF_FFFF_FFFF_FFFF
    );
}

/// A hexadecimal literal without the binary exponent is not a float —
/// upstream requires the `p`.
#[test]
fn hexadecimal_without_exponent_is_rejected() {
    for text in ["0x1", "0x1.8", "0x"] {
        assert!(
            ApFloat::from_string(SINGLE, text, RoundingMode::NearestTiesToEven).is_err(),
            "{text:?} must not parse as a float"
        );
    }
}

#[test]
fn infinities_and_plain_nans() {
    assert_eq!(bits(SINGLE, "inf"), 0x7F80_0000);
    assert_eq!(bits(SINGLE, "+inf"), 0x7F80_0000);
    assert_eq!(bits(SINGLE, "infinity"), 0x7F80_0000);
    assert_eq!(bits(SINGLE, "-inf"), 0xFF80_0000);
    assert_eq!(bits(SINGLE, "-infinity"), 0xFF80_0000);
    assert_eq!(bits(SINGLE, "nan"), 0x7FC0_0000);
    assert_eq!(bits(SINGLE, "-nan"), 0xFFC0_0000);
}

/// The spelling the arithmetic tables use for their signaling operand:
/// `APFloat(IEEEsingle, "snan123")` is a signaling NaN with payload 123, so
/// the quiet bit is clear and the payload sits in the low significand bits.
#[test]
fn signaling_nans_and_payloads() {
    assert_eq!(bits(SINGLE, "snan123"), 0x7F80_0000 | 123);
    assert_eq!(bits(SINGLE, "nan123"), 0x7FC0_0000 | 123);
    assert_eq!(bits(SINGLE, "-snan123"), 0xFF80_0000 | 123);

    let signaling = ApFloat::from_string(SINGLE, "snan123", RoundingMode::NearestTiesToEven)
        .expect("snan123 parses")
        .0;
    assert!(signaling.is_nan() && signaling.is_signaling());

    let quiet = ApFloat::from_string(SINGLE, "nan123", RoundingMode::NearestTiesToEven)
        .expect("nan123 parses")
        .0;
    assert!(!quiet.is_signaling());
}

/// A signaling NaN with an all-zero payload still needs *some* significand bit
/// set, or it would be an infinity; upstream sets the bit below the quiet one.
#[test]
fn signaling_nan_without_payload_gets_a_filler_bit() {
    assert_eq!(bits(SINGLE, "snan"), 0x7FA0_0000);
}

/// Upstream takes the payload radix from the usual C prefixes and allows the
/// payload in parentheses.
#[test]
fn nan_payload_radix_and_parentheses() {
    assert_eq!(bits(SINGLE, "nan0x7b"), 0x7FC0_0000 | 0x7B);
    assert_eq!(bits(SINGLE, "nan0173"), 0x7FC0_0000 | 123);
    assert_eq!(bits(SINGLE, "nan(123)"), 0x7FC0_0000 | 123);
    assert_eq!(bits(SINGLE, "nan(0x7b)"), 0x7FC0_0000 | 0x7B);
}

/// Quieting a signaling NaN keeps the payload — the defect the arithmetic
/// audit found, checked here through the parser rather than through addition.
#[test]
fn quieting_a_parsed_signaling_nan_keeps_its_payload() {
    let (signaling, _) = ApFloat::from_string(SINGLE, "snan123", RoundingMode::NearestTiesToEven)
        .expect("snan123 parses");
    assert_eq!(
        signaling
            .make_quiet()
            .to_bits()
            .try_zext_u64()
            .expect("single fits in u64"),
        0x7FC0_0000 | 123
    );
}
