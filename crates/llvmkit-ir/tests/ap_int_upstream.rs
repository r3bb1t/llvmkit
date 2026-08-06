//! Ports of `llvm/unittests/ADT/APIntTest.cpp` from the vendored
//! `llvmorg-22.1.4` tree, restricted to the operations llvmkit's `ApInt`
//! models.
//!
//! This file is the first sweep of that fixture — before it, `ApInt` had only
//! llvmkit-written tests. Upstream tests covering APIs llvmkit does not have
//! (`GCD`, `SolveQuadraticEquationWrap`, `clmul`, the rotate family, the
//! `tc*` word-level primitives) are not ported; what is here is ported as
//! written.
//!
//! Upstream constructs `APInt(width, value, isSigned)`; llvmkit spells that
//! `ApInt::new(width, value, signedness)`, sign-extending the `u64` when the
//! signedness says so.

use llvmkit_ir::{ApInt, ApIntTruncation, Signedness};

/// Upstream's `APInt(width, value, isSigned)` truncates implicitly, so the
/// ports below use `ApIntTruncation::Truncate` to match.
fn signed(bit_width: u32, value: i64) -> ApInt {
    ApInt::new(
        bit_width,
        u64::from_ne_bytes(value.to_ne_bytes()),
        Signedness::Signed,
        ApIntTruncation::Truncate,
    )
    .expect("truncating construction cannot overflow")
}

fn unsigned(bit_width: u32, value: u64) -> ApInt {
    ApInt::new(
        bit_width,
        value,
        Signedness::Unsigned,
        ApIntTruncation::Truncate,
    )
    .expect("truncating construction cannot overflow")
}

/// Port of `TEST(APIntTest, i33_Count)`.
#[test]
fn i33_count() {
    let i33minus2 = signed(33, -2);
    assert_eq!(i33minus2.count_leading_zeros(), 0);
    assert_eq!(i33minus2.count_leading_ones(), 32);
    assert_eq!(i33minus2.active_bits(), 33);
    assert_eq!(i33minus2.count_trailing_zeros(), 1);
    assert_eq!(i33minus2.popcount(), 32);
    assert_eq!(i33minus2.try_sext_i64(), Some(-2));
    assert_eq!(
        i33minus2.try_zext_u64(),
        Some((-2i64 as u64) & ((1u64 << 33) - 1))
    );
}

/// Port of `TEST(APIntTest, i65_Count)`.
#[test]
fn i65_count() {
    let i65 = signed(65, 0);
    assert_eq!(i65.count_leading_zeros(), 65);
    assert_eq!(i65.count_leading_ones(), 0);
    assert_eq!(i65.active_bits(), 0);
    assert_eq!(i65.active_words(), 1);
    assert_eq!(i65.count_trailing_zeros(), 65);
    assert_eq!(i65.popcount(), 0);

    let mut i65minus = signed(65, 0);
    i65minus.set_bit(64);
    assert_eq!(i65minus.count_leading_zeros(), 0);
    assert_eq!(i65minus.count_leading_ones(), 1);
    assert_eq!(i65minus.active_bits(), 65);
    assert_eq!(i65minus.count_trailing_zeros(), 64);
    assert_eq!(i65minus.popcount(), 1);
}

/// Port of `TEST(APIntTest, i128_PositiveCount)`.
#[test]
fn i128_positive_count() {
    let u128max = ApInt::all_ones(128);
    assert_eq!(u128max.count_leading_ones(), 128);
    assert_eq!(u128max.count_leading_zeros(), 0);
    assert_eq!(u128max.active_bits(), 128);
    assert_eq!(u128max.count_trailing_zeros(), 0);
    assert_eq!(u128max.count_trailing_ones(), 128);
    assert_eq!(u128max.popcount(), 128);

    let u64max = unsigned(128, u64::MAX);
    assert_eq!(u64max.count_leading_zeros(), 64);
    assert_eq!(u64max.count_leading_ones(), 0);
    assert_eq!(u64max.active_bits(), 64);
    assert_eq!(u64max.count_trailing_zeros(), 0);
    assert_eq!(u64max.count_trailing_ones(), 64);
    assert_eq!(u64max.popcount(), 64);
    assert_eq!(u64max.try_zext_u64(), Some(u64::MAX));

    let zero = signed(128, 0);
    assert_eq!(zero.count_leading_zeros(), 128);
    assert_eq!(zero.count_leading_ones(), 0);
    assert_eq!(zero.active_bits(), 0);
    assert_eq!(zero.count_trailing_zeros(), 128);
    assert_eq!(zero.count_trailing_ones(), 0);
    assert_eq!(zero.popcount(), 0);
    assert_eq!(zero.try_sext_i64(), Some(0));
    assert_eq!(zero.try_zext_u64(), Some(0));

    let one = signed(128, 1);
    assert_eq!(one.count_leading_zeros(), 127);
    assert_eq!(one.count_leading_ones(), 0);
    assert_eq!(one.active_bits(), 1);
    assert_eq!(one.count_trailing_zeros(), 0);
    assert_eq!(one.count_trailing_ones(), 1);
    assert_eq!(one.popcount(), 1);
    assert_eq!(one.try_sext_i64(), Some(1));
    assert_eq!(one.try_zext_u64(), Some(1));
}

/// Port of `TEST(APIntTest, i128_NegativeCount)`.
#[test]
fn i128_negative_count() {
    let minus3 = signed(128, -3);
    assert_eq!(minus3.count_leading_ones(), 126);
    assert_eq!(minus3.try_sext_i64(), Some(-3));

    let minus1 = signed(128, -1);
    assert_eq!(minus1.count_leading_zeros(), 0);
    assert_eq!(minus1.count_leading_ones(), 128);
    assert_eq!(minus1.active_bits(), 128);
    assert_eq!(minus1.count_trailing_zeros(), 0);
    assert_eq!(minus1.count_trailing_ones(), 128);
    assert_eq!(minus1.popcount(), 128);
    assert_eq!(minus1.try_sext_i64(), Some(-1));
}

/// Port of the value and comparison half of `TEST(APIntTest, i1)`.
#[test]
fn i1_values_and_equalities() {
    let neg_one = signed(1, -1);
    let zero = unsigned(1, 0);
    let one = unsigned(1, 1);

    assert_eq!(neg_one.try_sext_i64(), Some(-1));
    assert_eq!(neg_one.try_zext_u64(), Some(1));
    assert_eq!(zero.try_zext_u64(), Some(0));
    assert_eq!(one.try_sext_i64(), Some(-1));
    assert_eq!(one.try_zext_u64(), Some(1));

    assert!(ApInt::same_value(&one, &neg_one));

    assert!(zero.is_max_signed_value());
    assert!(!one.is_max_signed_value());
    assert!(!zero.is_min_signed_value());
    assert!(one.is_min_signed_value());

    // Additions wrap in one bit.
    assert!(ApInt::same_value(&zero, &one.wrapping_add(&one)));
    assert!(ApInt::same_value(&zero, &neg_one.wrapping_add(&one)));
}

/// Port of `TEST(APIntTest, isMask)`.
#[test]
fn is_mask() {
    assert!(!unsigned(32, 0x0101_0101).is_mask());
    assert!(!unsigned(32, 0xf000_0000).is_mask());
    assert!(!unsigned(32, 0xffff_0000).is_mask());
    assert!(!unsigned(32, 0xff << 1).is_mask());

    for n in [1u32, 2, 3, 4, 7, 8, 16, 32, 64, 127, 128, 129, 256] {
        assert!(!ApInt::zero(n).is_mask(), "width {n}");
        let one = unsigned(n, 1);
        for i in 1..=n {
            let mask = one
                .checked_shl(i)
                .unwrap_or_else(|| ApInt::zero(n))
                .wrapping_sub(&unsigned(n, 1));
            assert!(mask.is_mask(), "width {n}, {i} bits");
        }
    }
}

/// Port of `TEST(APIntTest, isShiftedMask)`.
#[test]
fn is_shifted_mask() {
    assert!(!unsigned(32, 0x0101_0101).is_shifted_mask());
    assert!(unsigned(32, 0xf000_0000).is_shifted_mask());
    assert!(unsigned(32, 0xffff_0000).is_shifted_mask());
    assert!(unsigned(32, 0xff << 1).is_shifted_mask());

    for n in [1u32, 2, 3, 4, 7, 8, 16, 32, 64, 127, 128, 129, 256] {
        assert!(!ApInt::zero(n).is_shifted_mask(), "width {n}");
        let one = unsigned(n, 1);
        for i in 1..n {
            let mask = one
                .checked_shl(i)
                .unwrap_or_else(|| ApInt::zero(n))
                .wrapping_sub(&unsigned(n, 1));
            assert!(mask.is_shifted_mask(), "width {n}, {i} bits");
        }
    }
}

/// Port of `TEST(APIntTest, isPowerOf2)` and `isNegatedPowerOf2`'s
/// power-of-two half.
#[test]
fn is_power_of_2() {
    assert!(!unsigned(5, 0x00).is_power_of_2());
    assert!(!unsigned(32, 0x11).is_power_of_2());
    assert!(unsigned(17, 0x01).is_power_of_2());

    for n in [1u32, 2, 8, 16, 32, 64, 127, 128, 129, 256] {
        assert!(!ApInt::zero(n).is_power_of_2(), "width {n}");
        assert!(ApInt::one_bit_set(n, 0).is_power_of_2(), "width {n}");
        for i in 1..n {
            assert!(
                ApInt::one_bit_set(n, i).is_power_of_2(),
                "width {n} bit {i}"
            );
            assert!(
                !ApInt::one_bit_set(n, i)
                    .bitor(&ApInt::one_bit_set(n, 0))
                    .is_power_of_2(),
                "width {n} bit {i} plus bit 0"
            );
        }
    }
}

/// Port of `TEST(APIntTest, byteSwap)`.
#[test]
fn byte_swap() {
    assert_eq!(
        unsigned(16, 0x0102).byte_swap().try_zext_u64(),
        Some(0x0201)
    );
    assert_eq!(
        unsigned(32, 0x0102_0304).byte_swap().try_zext_u64(),
        Some(0x0403_0201)
    );
    assert_eq!(
        unsigned(48, 0x0001_0203_0405).byte_swap().try_zext_u64(),
        Some(0x0504_0302_0100)
    );
    assert_eq!(
        unsigned(64, 0x0102_0304_0506_0708)
            .byte_swap()
            .try_zext_u64(),
        Some(0x0807_0605_0403_0201)
    );
}

/// Port of `TEST(APIntTest, LogicalRightShift)`, `ArithmeticRightShift`, and
/// `LeftShift`, using the same operands upstream does.
#[test]
fn shifts() {
    let i256 = ApInt::from_words(
        256,
        &[
            0x0000_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x0000_0000_0000_0000,
            0x8000_0000_0000_0000,
        ],
    );

    // A logical shift right by one clears the top bit.
    let shifted = i256.lshr(1);
    assert_eq!(shifted.count_leading_zeros(), 1);
    assert_eq!(shifted.popcount(), 1);

    // An arithmetic shift right by one keeps the sign.
    let shifted = i256.ashr(1);
    assert_eq!(shifted.count_leading_ones(), 2);
    assert!(shifted.is_negative());

    // Shifting a negative value all the way right leaves all ones.
    assert!(i256.ashr(255).is_all_ones());
    // Shifting left past the width leaves zero.
    assert!(i256.shl(256).is_zero());
    assert!(i256.lshr(256).is_zero());
}

/// Port of `TEST(APIntTest, insertBits)` and `extractBits`, restricted to the
/// widths llvmkit models.
#[test]
fn insert_and_extract_bits() {
    let mut value = signed(31, -1);
    value.insert_bits(&signed(5, 0), 0);
    assert_eq!(value.active_bits(), 31);
    assert_eq!(value.try_sext_i64(), Some(-32));

    value.insert_bits(&unsigned(5, 31), 0);
    assert_eq!(value.try_sext_i64(), Some(-1));

    let extracted = signed(32, -1).extract_bits(5, 0);
    assert_eq!(extracted.bit_width(), 5);
    assert!(extracted.is_all_ones());

    let value = unsigned(32, 0xdead_beef);
    assert_eq!(value.extract_bits(16, 0).try_zext_u64(), Some(0xbeef));
    assert_eq!(value.extract_bits(16, 16).try_zext_u64(), Some(0xdead));
    assert_eq!(value.extract_bits(8, 8).try_zext_u64(), Some(0xbe));
}

/// Port of `TEST(APIntTest, SignbitZeroChecks)`.
#[test]
fn signbit_zero_checks() {
    assert!(signed(8, -1).is_negative());
    assert!(!signed(8, -1).is_non_negative());
    assert!(!signed(8, -1).is_strictly_positive());
    assert!(signed(8, -1).is_non_positive());

    assert!(!unsigned(8, 0).is_negative());
    assert!(unsigned(8, 0).is_non_negative());
    assert!(!unsigned(8, 0).is_strictly_positive());
    assert!(unsigned(8, 0).is_non_positive());

    assert!(!unsigned(8, 1).is_negative());
    assert!(unsigned(8, 1).is_non_negative());
    assert!(unsigned(8, 1).is_strictly_positive());
    assert!(!unsigned(8, 1).is_non_positive());
}

/// Port of `TEST(APIntTest, ZeroWidth)` — the operations llvmkit models on a
/// zero-width value.
#[test]
fn zero_width() {
    let zero_width = ApInt::zero_width();
    assert_eq!(zero_width.bit_width(), 0);
    assert_eq!(zero_width.popcount(), 0);
    assert_eq!(zero_width.count_leading_zeros(), 0);
    assert_eq!(zero_width.count_trailing_zeros(), 0);
    assert_eq!(zero_width.active_bits(), 0);
    assert!(zero_width.is_zero());
    assert!(zero_width.is_all_ones());
    assert!(!zero_width.is_negative());

    // Widening from zero width gives zero.
    assert!(zero_width.zext(8).expect("widening succeeds").is_zero());
    assert!(zero_width.sext(8).expect("widening succeeds").is_zero());
}

/// Port of `TEST(APIntTest, Splat)`.
#[test]
fn splat() {
    let value = unsigned(8, 0xff);
    assert!(ApInt::splat(16, &value).is_all_ones());

    let value = signed(8, 0);
    assert!(ApInt::splat(16, &value).is_zero());

    let value = unsigned(3, 5);
    assert_eq!(ApInt::splat(9, &value).try_zext_u64(), Some(0b101_101_101));
}

/// Port of `TEST(APIntTest, toString)`, restricted to the plain form.
///
/// Upstream's `toString` also takes `formatAsCLiteral`, `UpperCase`, and
/// `InsertSeparators` flags; llvmkit's `to_string_radix` has neither the C
/// literal prefix nor separators, so only the rows with
/// `formatAsCLiteral = false` are portable. Those are upstream's radix-36
/// rows plus the digits of the C-literal rows with the prefix removed —
/// the digit sequence is the same either way. Case follows upstream's
/// `UpperCase = true` default.
#[test]
fn to_string() {
    let unsigned_ = Signedness::Unsigned;
    let signed_ = Signedness::Signed;

    assert_eq!(unsigned(8, 0).to_string_radix(2, unsigned_), "0");
    assert_eq!(unsigned(8, 0).to_string_radix(8, unsigned_), "0");
    assert_eq!(unsigned(8, 0).to_string_radix(10, unsigned_), "0");
    assert_eq!(unsigned(8, 0).to_string_radix(16, unsigned_), "0");
    assert_eq!(unsigned(8, 0).to_string_radix(36, unsigned_), "0");

    assert_eq!(unsigned(8, 255).to_string_radix(2, unsigned_), "11111111");
    assert_eq!(unsigned(8, 255).to_string_radix(8, unsigned_), "377");
    assert_eq!(unsigned(8, 255).to_string_radix(10, unsigned_), "255");
    // Upstream: `toString(S, 16, isSigned, true)` gives "0xFF" — upper case is
    // the default, and the "0x" is the C-literal prefix llvmkit does not add.
    assert_eq!(unsigned(8, 255).to_string_radix(16, unsigned_), "FF");
    assert_eq!(unsigned(8, 255).to_string_radix(36, unsigned_), "73");

    // Read as signed, the same 8-bit pattern is -1.
    assert_eq!(unsigned(8, 255).to_string_radix(2, signed_), "-1");
    assert_eq!(unsigned(8, 255).to_string_radix(8, signed_), "-1");
    assert_eq!(unsigned(8, 255).to_string_radix(10, signed_), "-1");
    assert_eq!(unsigned(8, 255).to_string_radix(16, signed_), "-1");
    assert_eq!(unsigned(8, 255).to_string_radix(36, signed_), "-1");
}

/// Round-trip check across every radix `to_string_radix` accepts.
///
/// **llvmkit-specific**: upstream's `TEST(APIntTest, fromString)` drives
/// `APInt(width, str, radix)` with a large hand-written table including
/// negative and overflowing spellings, and its closest llvmkit counterpart
/// (`ApInt::from_string`) does not model the signed spellings. This checks the
/// property that matters here — that the two directions agree — rather than
/// restating a table llvmkit cannot express.
#[test]
fn from_string_round_trips_to_string() {
    for radix in [2u8, 8, 10, 16, 36] {
        for value in [0u64, 1, 2, 7, 8, 15, 16, 100, 254, 255] {
            let original = unsigned(8, value);
            let text = original.to_string_radix(radix, Signedness::Unsigned);
            let parsed = ApInt::from_string(8, &text, radix)
                .unwrap_or_else(|e| panic!("radix {radix} {text:?} must parse: {e}"));
            assert_eq!(
                parsed.try_zext_u64(),
                Some(value),
                "radix {radix} round trip of {value}"
            );
        }
    }
}
