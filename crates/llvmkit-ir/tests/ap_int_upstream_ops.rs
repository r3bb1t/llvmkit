//! Ports of the `APInt` operation tests from
//! `llvm/unittests/ADT/APIntTest.cpp` in the vendored `llvmorg-22.1.4` tree,
//! covering the families llvmkit gained alongside this sweep: rotates, the
//! `log2` family, `getLoBits`/`getHiBits`, `clearLowBits`/`clearHighBits`,
//! `abds`/`abdu`, `RoundingUDiv`/`RoundingSDiv`, the averages, the
//! multiplicative inverse, `ScaleBitMask`, and `GreatestCommonDivisor`.
//!
//! Spelling differences, none of which change the logic:
//!
//! - Upstream's `logBase2` and `nearestLogBase2` return `unsigned` and reserve
//!   an out-of-range value for zero; `exactLogBase2` reserves `-1` for "not a
//!   power of two". llvmkit returns `Option<u32>`, so the ports read `None`
//!   where upstream reads the marker.
//! - Upstream's `APIntOps::` free functions are inherent methods here, and the
//!   ones that require equal widths return `Option` rather than asserting.
//! - Upstream's `EXPECT_EQ(<integer>, <APInt>)` leans on
//!   `operator==(APInt, uint64_t)`; the ports compare the extracted value
//!   through `try_zext_u64` / `try_sext_i64`, or compare two `ApInt`s outright.

use llvmkit_ir::{ApInt, ApIntRounding, ApIntSignedness, ApIntTruncation};

const DOWN: ApIntRounding = ApIntRounding::Down;
const TOWARD_ZERO: ApIntRounding = ApIntRounding::TowardZero;
const UP: ApIntRounding = ApIntRounding::Up;

fn unsigned(bit_width: u32, value: u64) -> ApInt {
    ApInt::new(
        bit_width,
        value,
        ApIntSignedness::Unsigned,
        ApIntTruncation::Truncate,
    )
    .expect("truncating construction cannot overflow")
}

fn signed(bit_width: u32, value: i64) -> ApInt {
    ApInt::new(
        bit_width,
        u64::from_ne_bytes(value.to_ne_bytes()),
        ApIntSignedness::Signed,
        ApIntTruncation::Truncate,
    )
    .expect("truncating construction cannot overflow")
}

fn zext(value: &ApInt) -> u64 {
    value
        .try_zext_u64()
        .expect("the ported values all fit in 64 bits")
}

fn sext(value: &ApInt) -> i64 {
    value
        .try_sext_i64()
        .expect("the ported values all fit in 64 bits")
}

fn rounding_udiv(lhs: &ApInt, rhs: &ApInt, rounding: ApIntRounding) -> ApInt {
    lhs.rounding_udiv(rhs, rounding)
        .expect("equal widths, non-zero divisor")
}

fn rounding_sdiv(lhs: &ApInt, rhs: &ApInt, rounding: ApIntRounding) -> ApInt {
    lhs.rounding_sdiv(rhs, rounding)
        .expect("equal widths, non-zero divisor")
}

/// Port of `TEST(APIntTest, Log2)`.
#[test]
fn log2() {
    assert_eq!(unsigned(15, 7).log_base2(), Some(2));
    assert_eq!(unsigned(15, 7).ceil_log_base2(), 3);
    assert_eq!(unsigned(15, 7).exact_log_base2(), None);
    assert_eq!(unsigned(15, 8).log_base2(), Some(3));
    assert_eq!(unsigned(15, 8).ceil_log_base2(), 3);
    assert_eq!(unsigned(15, 8).exact_log_base2(), Some(3));
    assert_eq!(unsigned(15, 9).log_base2(), Some(3));
    assert_eq!(unsigned(15, 9).ceil_log_base2(), 4);
    assert_eq!(unsigned(15, 9).exact_log_base2(), None);
}

/// Port of `TEST(APIntTest, nearestLogBase2)`.
///
/// The final row builds `APInt(UINT32_MAX, 0)` — a four-billion-bit zero,
/// which allocates half a gigabyte purely to re-check the zero answer the
/// preceding row already checks. It is deliberately not ported; the zero and
/// one-bit cases it guards are covered by `A8` above it.
#[test]
fn nearest_log_base2() {
    // Single word check.

    // Test round up.
    let a1 = unsigned(64, 0x0180_0001);
    assert_eq!(a1.nearest_log_base2(), Some(a1.ceil_log_base2()));

    // Test round down.
    let a2 = unsigned(64, 0x0100_0011);
    assert_eq!(a2.nearest_log_base2(), a2.log_base2());

    // Test ties round up.
    let a3 = unsigned(64, 0x0180_0000);
    assert_eq!(a3.nearest_log_base2(), Some(a3.ceil_log_base2()));

    // Multiple word check.

    // Test round up.
    let a4 = ApInt::from_words(64 * 4, &[0x0, 0xF, 0x18, 0x0]);
    assert_eq!(a4.nearest_log_base2(), Some(a4.ceil_log_base2()));

    // Test round down.
    let a5 = ApInt::from_words(64 * 4, &[0x0, 0xF, 0x10, 0x0]);
    assert_eq!(a5.nearest_log_base2(), a5.log_base2());

    // Test ties round up.
    let a6 = ApInt::from_words(64 * 4, &[0x0, 0x0, 0x0, 0x18]);
    assert_eq!(a6.nearest_log_base2(), Some(a6.ceil_log_base2()));

    // Test BitWidth == 1 special cases.
    assert_eq!(unsigned(1, 1).nearest_log_base2(), Some(0));
    assert_eq!(unsigned(1, 0).nearest_log_base2(), None);
}

/// Port of `TEST(APIntTest, Rotate)`.
#[test]
fn rotate() {
    assert_eq!(unsigned(8, 1), unsigned(8, 1).rotl(0));
    assert_eq!(unsigned(8, 2), unsigned(8, 1).rotl(1));
    assert_eq!(unsigned(8, 4), unsigned(8, 1).rotl(2));
    assert_eq!(unsigned(8, 16), unsigned(8, 1).rotl(4));
    assert_eq!(unsigned(8, 1), unsigned(8, 1).rotl(8));

    assert_eq!(unsigned(8, 16), unsigned(8, 16).rotl(0));
    assert_eq!(unsigned(8, 32), unsigned(8, 16).rotl(1));
    assert_eq!(unsigned(8, 64), unsigned(8, 16).rotl(2));
    assert_eq!(unsigned(8, 1), unsigned(8, 16).rotl(4));
    assert_eq!(unsigned(8, 16), unsigned(8, 16).rotl(8));

    assert_eq!(unsigned(32, 2), unsigned(32, 1).rotl(33));
    assert_eq!(unsigned(32, 2), unsigned(32, 1).rotl_by(&unsigned(32, 33)));

    assert_eq!(unsigned(32, 2), unsigned(32, 1).rotl_by(&unsigned(33, 33)));
    assert_eq!(
        unsigned(32, 1 << 8),
        unsigned(32, 1).rotl_by(&unsigned(32, 40))
    );
    assert_eq!(
        unsigned(32, 1 << 30),
        unsigned(32, 1).rotl_by(&unsigned(31, 30))
    );
    assert_eq!(
        unsigned(32, 1 << 31),
        unsigned(32, 1).rotl_by(&unsigned(31, 31))
    );

    assert_eq!(unsigned(32, 1), unsigned(32, 1).rotl_by(&unsigned(1, 0)));
    assert_eq!(unsigned(32, 2), unsigned(32, 1).rotl_by(&unsigned(1, 1)));

    assert_eq!(unsigned(32, 16), unsigned(32, 1).rotl_by(&unsigned(3, 4)));

    assert_eq!(unsigned(32, 1), unsigned(32, 1).rotl_by(&unsigned(64, 64)));
    assert_eq!(unsigned(32, 2), unsigned(32, 1).rotl_by(&unsigned(64, 65)));

    assert_eq!(unsigned(7, 24), unsigned(7, 3).rotl_by(&unsigned(7, 3)));
    assert_eq!(unsigned(7, 24), unsigned(7, 3).rotl_by(&unsigned(7, 10)));
    assert_eq!(unsigned(7, 24), unsigned(7, 3).rotl_by(&unsigned(5, 10)));
    assert_eq!(unsigned(7, 6), unsigned(7, 3).rotl_by(&unsigned(12, 120)));

    assert_eq!(unsigned(8, 16), unsigned(8, 16).rotr(0));
    assert_eq!(unsigned(8, 8), unsigned(8, 16).rotr(1));
    assert_eq!(unsigned(8, 4), unsigned(8, 16).rotr(2));
    assert_eq!(unsigned(8, 1), unsigned(8, 16).rotr(4));
    assert_eq!(unsigned(8, 16), unsigned(8, 16).rotr(8));

    assert_eq!(unsigned(8, 1), unsigned(8, 1).rotr(0));
    assert_eq!(unsigned(8, 128), unsigned(8, 1).rotr(1));
    assert_eq!(unsigned(8, 64), unsigned(8, 1).rotr(2));
    assert_eq!(unsigned(8, 16), unsigned(8, 1).rotr(4));
    assert_eq!(unsigned(8, 1), unsigned(8, 1).rotr(8));

    assert_eq!(unsigned(32, 1 << 31), unsigned(32, 1).rotr(33));
    assert_eq!(
        unsigned(32, 1 << 31),
        unsigned(32, 1).rotr_by(&unsigned(32, 33))
    );

    assert_eq!(
        unsigned(32, 1 << 31),
        unsigned(32, 1).rotr_by(&unsigned(33, 33))
    );
    assert_eq!(
        unsigned(32, 1 << 24),
        unsigned(32, 1).rotr_by(&unsigned(32, 40))
    );

    assert_eq!(
        unsigned(32, 1 << 2),
        unsigned(32, 1).rotr_by(&unsigned(31, 30))
    );
    assert_eq!(
        unsigned(32, 1 << 1),
        unsigned(32, 1).rotr_by(&unsigned(31, 31))
    );

    assert_eq!(unsigned(32, 1), unsigned(32, 1).rotr_by(&unsigned(1, 0)));
    assert_eq!(
        unsigned(32, 1 << 31),
        unsigned(32, 1).rotr_by(&unsigned(1, 1))
    );

    assert_eq!(
        unsigned(32, 1 << 28),
        unsigned(32, 1).rotr_by(&unsigned(3, 4))
    );

    assert_eq!(unsigned(32, 1), unsigned(32, 1).rotr_by(&unsigned(64, 64)));
    assert_eq!(
        unsigned(32, 1 << 31),
        unsigned(32, 1).rotr_by(&unsigned(64, 65))
    );

    assert_eq!(unsigned(7, 48), unsigned(7, 3).rotr_by(&unsigned(7, 3)));
    assert_eq!(unsigned(7, 48), unsigned(7, 3).rotr_by(&unsigned(7, 10)));
    assert_eq!(unsigned(7, 48), unsigned(7, 3).rotr_by(&unsigned(5, 10)));
    assert_eq!(unsigned(7, 65), unsigned(7, 3).rotr_by(&unsigned(12, 120)));

    let big = ApInt::from_string(256, "00004000800000000000000000003fff8000000000000003", 16)
        .expect("upstream spells this literal");
    let rotated = ApInt::from_string(256, "3fff80000000000000030000000000000000000040008000", 16)
        .expect("upstream spells this literal");
    assert_eq!(rotated, big.rotr(144));

    assert_eq!(unsigned(32, 8), unsigned(32, 1).rotl_by(&big));
    assert_eq!(unsigned(32, 1 << 29), unsigned(32, 1).rotr_by(&big));
}

/// Port of `TEST(APIntTest, getLoBits)`.
#[test]
fn get_lo_bits() {
    let mut i32_value = unsigned(32, 0xfa);
    i32_value.set_high_bits(1);
    assert_eq!(0xa, zext(&i32_value.lo_bits(4)));
    let mut i128_value = unsigned(128, 0xfa);
    i128_value.set_high_bits(1);
    assert_eq!(0xa, zext(&i128_value.lo_bits(4)));
}

/// Port of `TEST(APIntTest, getHiBits)`.
#[test]
fn get_hi_bits() {
    let mut i32_value = unsigned(32, 0xfa);
    i32_value.set_high_bits(2);
    assert_eq!(0xc, zext(&i32_value.hi_bits(4)));
    let mut i128_value = unsigned(128, 0xfa);
    i128_value.set_high_bits(2);
    assert_eq!(0xc, zext(&i128_value.hi_bits(4)));
}

/// The shape every `clearLowBits` / `clearHighBits` / `clearBits` row checks.
fn assert_counts(
    value: &ApInt,
    leading_ones: u32,
    leading_zeros: u32,
    active_bits: u32,
    trailing_zeros: u32,
    trailing_ones: u32,
    popcount: u32,
) {
    assert_eq!(leading_ones, value.count_leading_ones());
    assert_eq!(leading_zeros, value.count_leading_zeros());
    assert_eq!(active_bits, value.active_bits());
    assert_eq!(trailing_zeros, value.count_trailing_zeros());
    assert_eq!(trailing_ones, value.count_trailing_ones());
    assert_eq!(popcount, value.popcount());
}

/// Port of `TEST(APIntTest, clearLowBits)`.
#[test]
fn clear_low_bits() {
    let mut i64hi32 = ApInt::all_ones(64);
    i64hi32.clear_low_bits(32);
    assert_counts(&i64hi32, 32, 0, 64, 32, 0, 32);

    let mut i128hi64 = ApInt::all_ones(128);
    i128hi64.clear_low_bits(64);
    assert_counts(&i128hi64, 64, 0, 128, 64, 0, 64);

    let mut i128hi24 = ApInt::all_ones(128);
    i128hi24.clear_low_bits(104);
    assert_counts(&i128hi24, 24, 0, 128, 104, 0, 24);

    let mut i128hi104 = ApInt::all_ones(128);
    i128hi104.clear_low_bits(24);
    assert_counts(&i128hi104, 104, 0, 128, 24, 0, 104);

    let mut i128hi0 = ApInt::all_ones(128);
    i128hi0.clear_low_bits(128);
    assert_counts(&i128hi0, 0, 128, 0, 128, 0, 0);

    let mut i80hi1 = ApInt::all_ones(80);
    i80hi1.clear_low_bits(79);
    assert_counts(&i80hi1, 1, 0, 80, 79, 0, 1);

    let mut i32hi16 = ApInt::all_ones(32);
    i32hi16.clear_low_bits(16);
    assert_counts(&i32hi16, 16, 0, 32, 16, 0, 16);
}

/// Port of `TEST(APIntTest, clearHighBits)`.
#[test]
fn clear_high_bits() {
    let mut i64hi32 = ApInt::all_ones(64);
    i64hi32.clear_high_bits(32);
    assert_counts(&i64hi32, 0, 32, 32, 0, 32, 32);

    let mut i128hi64 = ApInt::all_ones(128);
    i128hi64.clear_high_bits(64);
    assert_counts(&i128hi64, 0, 64, 64, 0, 64, 64);

    let mut i128hi24 = ApInt::all_ones(128);
    i128hi24.clear_high_bits(104);
    assert_counts(&i128hi24, 0, 104, 24, 0, 24, 24);

    let mut i128hi104 = ApInt::all_ones(128);
    i128hi104.clear_high_bits(24);
    assert_counts(&i128hi104, 0, 24, 104, 0, 104, 104);

    let mut i128hi0 = ApInt::all_ones(128);
    i128hi0.clear_high_bits(128);
    assert_counts(&i128hi0, 0, 128, 0, 128, 0, 0);

    let mut i80hi1 = ApInt::all_ones(80);
    i80hi1.clear_high_bits(79);
    assert_counts(&i80hi1, 0, 79, 1, 0, 1, 1);

    let mut i32hi16 = ApInt::all_ones(32);
    i32hi16.clear_high_bits(16);
    assert_counts(&i32hi16, 0, 16, 16, 0, 16, 16);
}

/// Port of `TEST(APIntTest, clearBits)`.
#[test]
fn clear_bits() {
    let mut i32_value = ApInt::all_ones(32);
    i32_value.clear_bits(1, 3);
    assert_counts(&i32_value, 29, 0, 32, 0, 1, 30);

    i32_value.clear_bits(15, 15);
    assert_counts(&i32_value, 29, 0, 32, 0, 1, 30);

    i32_value.clear_bits(28, 31);
    assert_counts(&i32_value, 1, 0, 32, 0, 1, 27);
    assert_eq!(
        ApInt::from_string(32, "8FFFFFF9", 16).expect("upstream spells this literal"),
        i32_value
    );

    let mut i256 = ApInt::all_ones(256);
    i256.clear_bits(10, 250);
    assert_counts(&i256, 6, 0, 256, 0, 10, 16);

    let mut i299 = ApInt::all_ones(299);
    i299.clear_bits(240, 250);
    assert_counts(&i299, 49, 0, 299, 0, 240, 289);

    let mut i311 = ApInt::all_ones(311);
    i311.clear_bits(33, 99);
    assert_counts(&i311, 212, 0, 311, 0, 33, 245);

    let mut i64hi32 = ApInt::all_ones(64);
    i64hi32.clear_bits(0, 32);
    assert_counts(&i64hi32, 32, 0, 64, 32, 0, 32);

    let mut i64hi32 = ApInt::all_ones(64);
    i64hi32.clear_bits(32, 64);
    assert_counts(&i64hi32, 0, 32, 32, 0, 32, 32);
}

/// Port of `TEST(APIntTest, setAllBits)`.
#[test]
fn set_all_bits() {
    for bit_width in [32, 64, 96, 128] {
        let mut value = unsigned(bit_width, 0);
        value.set_all_bits();
        assert_counts(&value, bit_width, 0, bit_width, 0, bit_width, bit_width);
    }
}

/// Port of `TEST(APIntTest, abds)`.
#[test]
fn abs_diff_signed() {
    let abds = |a: &ApInt, b: &ApInt| a.abs_diff_signed(b).expect("equal widths");

    let max_u1 = unsigned(1, 1);
    let min_u1 = unsigned(1, 0);
    assert_eq!(1, zext(&abds(&max_u1, &min_u1)));
    assert_eq!(1, zext(&abds(&min_u1, &max_u1)));

    let max_u4 = unsigned(4, 15);
    let min_u4 = unsigned(4, 0);
    assert_eq!(1, sext(&abds(&max_u4, &min_u4)));
    assert_eq!(1, sext(&abds(&min_u4, &max_u4)));

    let max_s8 = signed(8, 127);
    let min_s8 = signed(8, -128);
    assert_eq!(-1, sext(&abds(&max_s8, &min_s8)));
    assert_eq!(-1, sext(&abds(&min_s8, &max_s8)));

    let max_u16 = unsigned(16, 65535);
    let min_u16 = unsigned(16, 0);
    assert_eq!(1, sext(&abds(&max_u16, &min_u16)));
    assert_eq!(1, sext(&abds(&min_u16, &max_u16)));

    let max_s16 = signed(16, 32767);
    let min_s16 = signed(16, -32768);
    let zero_s16 = signed(16, 0);
    assert_eq!(-1, sext(&abds(&max_s16, &min_s16)));
    assert_eq!(-1, sext(&abds(&min_s16, &max_s16)));
    assert_eq!(32768, zext(&abds(&zero_s16, &min_s16)));
    assert_eq!(32768, zext(&abds(&min_s16, &zero_s16)));
    assert_eq!(32767, zext(&abds(&zero_s16, &max_s16)));
    assert_eq!(32767, zext(&abds(&max_s16, &zero_s16)));
}

/// Port of `TEST(APIntTest, abdu)`.
#[test]
fn abs_diff_unsigned() {
    let abdu = |a: &ApInt, b: &ApInt| a.abs_diff_unsigned(b).expect("equal widths");

    let max_u1 = unsigned(1, 1);
    let min_u1 = unsigned(1, 0);
    assert_eq!(1, zext(&abdu(&max_u1, &min_u1)));
    assert_eq!(1, zext(&abdu(&min_u1, &max_u1)));

    let max_u4 = unsigned(4, 15);
    let min_u4 = unsigned(4, 0);
    assert_eq!(15, zext(&abdu(&max_u4, &min_u4)));
    assert_eq!(15, zext(&abdu(&min_u4, &max_u4)));

    let max_s8 = signed(8, 127);
    let min_s8 = signed(8, -128);
    assert_eq!(1, zext(&abdu(&max_s8, &min_s8)));
    assert_eq!(1, zext(&abdu(&min_s8, &max_s8)));

    let max_u16 = unsigned(16, 65535);
    let min_u16 = unsigned(16, 0);
    assert_eq!(65535, zext(&abdu(&max_u16, &min_u16)));
    assert_eq!(65535, zext(&abdu(&min_u16, &max_u16)));

    let max_s16 = signed(16, 32767);
    let min_s16 = signed(16, -32768);
    let zero_s16 = signed(16, 0);
    assert_eq!(1, zext(&abdu(&max_s16, &min_s16)));
    assert_eq!(1, zext(&abdu(&min_s16, &max_s16)));
    assert_eq!(32768, zext(&abdu(&zero_s16, &min_s16)));
    assert_eq!(32768, zext(&abdu(&min_s16, &zero_s16)));
    assert_eq!(32767, zext(&abdu(&zero_s16, &max_s16)));
    assert_eq!(32767, zext(&abdu(&max_s16, &zero_s16)));
}

/// Port of `TEST(APIntTest, RoundingUDiv)`.
#[test]
fn rounding_udiv_exhaustive() {
    for ai in 1..=255u64 {
        let a = unsigned(8, ai);
        let zero = unsigned(8, 0);
        assert!(rounding_udiv(&zero, &a, UP).is_zero());
        assert!(rounding_udiv(&zero, &a, DOWN).is_zero());
        assert!(rounding_udiv(&zero, &a, TOWARD_ZERO).is_zero());

        for bi in 1..=255u64 {
            let b = unsigned(8, bi);
            {
                let quotient = rounding_udiv(&a, &b, UP);
                let wide_b = b.zext(16).expect("widening");
                let product = quotient.zext(16).expect("widening").wrapping_mul(&wide_b);
                assert!(product.unsigned_cmp_u64(ai).is_ge());
                if product.unsigned_cmp_u64(ai).is_gt() {
                    let below = quotient
                        .wrapping_sub(&unsigned(8, 1))
                        .zext(16)
                        .expect("widening")
                        .wrapping_mul(&wide_b);
                    assert!(below.unsigned_cmp_u64(ai).is_lt());
                }
            }
            {
                let quotient = a.checked_udiv(&b).expect("non-zero divisor");
                assert_eq!(quotient, rounding_udiv(&a, &b, TOWARD_ZERO));
                assert_eq!(quotient, rounding_udiv(&a, &b, DOWN));
            }
        }
    }
}

/// Port of `TEST(APIntTest, RoundingSDiv)`.
#[test]
fn rounding_sdiv_exhaustive() {
    let one = unsigned(8, 1);
    for ai in -128..=127i64 {
        let a = signed(8, ai);

        if ai != 0 {
            let zero = unsigned(8, 0);
            assert!(rounding_sdiv(&zero, &a, UP).is_zero());
            assert!(rounding_sdiv(&zero, &a, DOWN).is_zero());
            assert!(rounding_sdiv(&zero, &a, TOWARD_ZERO).is_zero());
        }

        for bi in -128..=127i64 {
            if bi == 0 {
                continue;
            }
            let b = signed(8, bi);
            // `sdiv` of the signed minimum by -1 has no representable
            // quotient; upstream leaves that to `sdiv`'s own precondition,
            // llvmkit answers `None`, so the row is skipped here for both.
            let Some(toward_zero) = a.checked_sdiv(&b) else {
                continue;
            };
            let remainder = a.checked_srem(&b).expect("srem shares sdiv's domain");
            {
                let quotient = rounding_sdiv(&a, &b, UP);
                if remainder.is_zero() {
                    assert_eq!(toward_zero, quotient);
                } else if a.is_negative() != b.is_negative() {
                    // The mathematical quotient is negative.
                    assert_eq!(toward_zero, quotient);
                } else {
                    assert_eq!(toward_zero.wrapping_add(&one), quotient);
                }
            }
            {
                let quotient = rounding_sdiv(&a, &b, DOWN);
                if remainder.is_zero() {
                    assert_eq!(toward_zero, quotient);
                } else if a.is_negative() != b.is_negative() {
                    assert_eq!(toward_zero.wrapping_sub(&one), quotient);
                } else {
                    assert_eq!(toward_zero, quotient);
                }
            }
            assert_eq!(toward_zero, rounding_sdiv(&a, &b, TOWARD_ZERO));
        }
    }
}

/// Port of `TEST(APIntTest, Average)`.
#[test]
fn average() {
    let avg_floor_u = |a: &ApInt, b: &ApInt| a.avg_floor_unsigned(b).expect("equal widths");
    let avg_ceil_u = |a: &ApInt, b: &ApInt| a.avg_ceil_unsigned(b).expect("equal widths");
    let avg_floor_s = |a: &ApInt, b: &ApInt| a.avg_floor_signed(b).expect("equal widths");
    let avg_ceil_s = |a: &ApInt, b: &ApInt| a.avg_ceil_signed(b).expect("equal widths");

    let a0 = unsigned(32, 0);
    let a2 = unsigned(32, 2);
    let a100 = unsigned(32, 100);
    let a101 = unsigned(32, 101);
    let a200 = unsigned(32, 200);
    let umax = ApInt::max_value(32);

    assert_eq!(unsigned(32, 150), avg_floor_u(&a100, &a200));
    assert_eq!(
        rounding_udiv(&a100.wrapping_add(&a200), &a2, DOWN),
        avg_floor_u(&a100, &a200)
    );
    assert_eq!(
        rounding_udiv(&a100.wrapping_add(&a200), &a2, UP),
        avg_ceil_u(&a100, &a200)
    );
    assert_eq!(
        rounding_udiv(&a100.wrapping_add(&a101), &a2, DOWN),
        avg_floor_u(&a100, &a101)
    );
    assert_eq!(
        rounding_udiv(&a100.wrapping_add(&a101), &a2, UP),
        avg_ceil_u(&a100, &a101)
    );
    assert_eq!(a0, avg_floor_u(&a0, &a0));
    assert_eq!(a0, avg_ceil_u(&a0, &a0));
    assert_eq!(umax, avg_floor_u(&umax, &umax));
    assert_eq!(umax, avg_ceil_u(&umax, &umax));
    assert_eq!(rounding_udiv(&umax, &a2, DOWN), avg_floor_u(&a0, &umax));
    assert_eq!(rounding_udiv(&umax, &a2, UP), avg_ceil_u(&a0, &umax));

    let ap100 = signed(32, 100);
    let ap101 = signed(32, 101);
    let ap200 = signed(32, 200);
    let am1 = signed(32, -1);
    let am100 = signed(32, -100);
    let am101 = signed(32, -101);
    let am200 = signed(32, -200);
    let smin = ApInt::signed_min_value(32);
    let smax = ApInt::signed_max_value(32);

    assert_eq!(signed(32, 150), avg_floor_s(&ap100, &ap200));
    assert_eq!(
        rounding_sdiv(&ap100.wrapping_add(&ap200), &a2, DOWN),
        avg_floor_s(&ap100, &ap200)
    );
    assert_eq!(
        rounding_sdiv(&ap100.wrapping_add(&ap200), &a2, UP),
        avg_ceil_s(&ap100, &ap200)
    );

    assert_eq!(signed(32, -150), avg_floor_s(&am100, &am200));
    assert_eq!(
        rounding_sdiv(&am100.wrapping_add(&am200), &a2, DOWN),
        avg_floor_s(&am100, &am200)
    );
    assert_eq!(
        rounding_sdiv(&am100.wrapping_add(&am200), &a2, UP),
        avg_ceil_s(&am100, &am200)
    );

    assert_eq!(signed(32, 100), avg_floor_s(&ap100, &ap101));
    assert_eq!(
        rounding_sdiv(&ap100.wrapping_add(&ap101), &a2, DOWN),
        avg_floor_s(&ap100, &ap101)
    );
    assert_eq!(signed(32, 101), avg_ceil_s(&ap100, &ap101));
    assert_eq!(
        rounding_sdiv(&ap100.wrapping_add(&ap101), &a2, UP),
        avg_ceil_s(&ap100, &ap101)
    );

    assert_eq!(signed(32, -101), avg_floor_s(&am100, &am101));
    assert_eq!(
        rounding_sdiv(&am100.wrapping_add(&am101), &a2, DOWN),
        avg_floor_s(&am100, &am101)
    );
    assert_eq!(signed(32, -100), avg_ceil_s(&am100, &am101));
    assert_eq!(
        rounding_sdiv(&am100.wrapping_add(&am101), &a2, UP),
        avg_ceil_s(&am100, &am101)
    );

    assert_eq!(smin, avg_floor_s(&smin, &smin));
    assert_eq!(smin, avg_ceil_s(&smin, &smin));

    assert_eq!(rounding_sdiv(&smin, &a2, DOWN), avg_floor_s(&a0, &smin));
    assert_eq!(rounding_sdiv(&smin, &a2, UP), avg_ceil_s(&a0, &smin));

    assert_eq!(a0, avg_floor_s(&a0, &a0));
    assert_eq!(a0, avg_ceil_s(&a0, &a0));

    assert_eq!(am1, avg_floor_s(&smin, &smax));
    assert_eq!(a0, avg_ceil_s(&smin, &smax));

    assert_eq!(rounding_sdiv(&smax, &a2, DOWN), avg_floor_s(&a0, &smax));
    assert_eq!(rounding_sdiv(&smax, &a2, UP), avg_ceil_s(&a0, &smax));

    assert_eq!(smax, avg_floor_s(&smax, &smax));
    assert_eq!(smax, avg_ceil_s(&smax, &smax));
}

/// Port of `TEST(APIntTest, MultiplicativeInverseExaustive)`.
#[test]
fn multiplicative_inverse_exhaustive() {
    for bit_width in 1..=8u32 {
        let mut value = 1u64;
        while value < (1u64 << bit_width) {
            // Multiplicative inverse exists for all odd numbers.
            let v = unsigned(bit_width, value);
            let inverse = v.multiplicative_inverse().expect("odd values have one");
            assert!(
                v.wrapping_mul(&inverse).is_one(),
                "{bit_width}-bit {value} times its inverse must be one"
            );
            value += 2;
        }
    }
}

/// Port of `TEST(APIntTest, ScaleBitMask)`.
#[test]
fn scale_bit_mask() {
    let scale = |value: &ApInt, width: u32, match_all: bool| {
        value
            .scale_bit_mask(width, match_all)
            .expect("one width divides the other")
    };

    assert_eq!(scale(&unsigned(2, 0x00), 8, false), unsigned(8, 0x00));
    assert_eq!(scale(&unsigned(2, 0x01), 8, false), unsigned(8, 0x0F));
    assert_eq!(scale(&unsigned(2, 0x02), 8, false), unsigned(8, 0xF0));
    assert_eq!(scale(&unsigned(2, 0x03), 8, false), unsigned(8, 0xFF));

    assert_eq!(scale(&unsigned(8, 0x00), 4, false), unsigned(4, 0x00));
    assert_eq!(scale(&unsigned(8, 0xFF), 4, false), unsigned(4, 0x0F));
    assert_eq!(scale(&unsigned(8, 0xE4), 4, false), unsigned(4, 0x0E));

    assert_eq!(scale(&unsigned(8, 0x00), 8, false), unsigned(8, 0x00));

    assert_eq!(
        scale(&ApInt::zero(1024), 4096, false),
        ApInt::zero(4096),
        "a zero of any width scales to a zero"
    );
    assert_eq!(
        scale(&ApInt::all_ones(4096), 256, false),
        ApInt::all_ones(256)
    );
    assert_eq!(
        scale(&ApInt::one_bit_set(4096, 32), 256, false),
        ApInt::one_bit_set(256, 2)
    );

    assert_eq!(scale(&unsigned(2, 0x00), 8, true), unsigned(8, 0x00));
    assert_eq!(scale(&unsigned(2, 0x01), 8, true), unsigned(8, 0x0F));
    assert_eq!(scale(&unsigned(2, 0x02), 8, true), unsigned(8, 0xF0));
    assert_eq!(scale(&unsigned(2, 0x03), 8, true), unsigned(8, 0xFF));

    assert_eq!(scale(&unsigned(8, 0x00), 4, true), unsigned(4, 0x00));
    assert_eq!(scale(&unsigned(8, 0xFF), 4, true), unsigned(4, 0x0F));
    assert_eq!(scale(&unsigned(8, 0xE4), 4, true), unsigned(4, 0x08));
}

/// Port of `TEST(APIntTest, GCD)`.
#[test]
fn greatest_common_divisor() {
    let gcd = |a: &ApInt, b: &ApInt| ApInt::greatest_common_divisor(a, b).expect("equal widths");

    for bits in [1, 2, 32, 63, 64, 65] {
        // Test some corner cases near zero.
        let zero = unsigned(bits, 0);
        let one = unsigned(bits, 1);
        assert_eq!(gcd(&zero, &zero), zero);
        assert_eq!(gcd(&zero, &one), one);
        assert_eq!(gcd(&one, &zero), one);
        assert_eq!(gcd(&one, &one), one);

        if bits > 1 {
            let two = unsigned(bits, 2);
            assert_eq!(gcd(&zero, &two), two);
            assert_eq!(gcd(&one, &two), one);
            assert_eq!(gcd(&two, &two), two);

            // Test some corner cases near the highest representable value.
            let mut max = unsigned(bits, 0);
            max.set_all_bits();
            assert_eq!(gcd(&zero, &max), max);
            assert_eq!(gcd(&one, &max), one);
            assert_eq!(gcd(&two, &max), one);
            assert_eq!(gcd(&max, &max), max);

            let max_over_2 = max.checked_udiv(&two).expect("non-zero divisor");
            assert_eq!(gcd(&max_over_2, &max), one);
            // Max - 1 == Max / 2 * 2, because Max is odd.
            assert_eq!(gcd(&max_over_2, &max.wrapping_sub(&one)), max_over_2);
        }
    }

    // Compute the 20th Mersenne prime.
    let bit_width = 4450;
    let huge_prime = ApInt::low_bits_set(bit_width, 4423);

    // 9931 and 123456 are coprime.
    let a = huge_prime.wrapping_mul(&unsigned(bit_width, 9931));
    let b = huge_prime.wrapping_mul(&unsigned(bit_width, 123456));
    assert_eq!(gcd(&a, &b), huge_prime);
}

/// Port of `TEST(APIntTest, Fshl)`.
#[test]
fn fshl() {
    let fshl =
        |hi: &ApInt, lo: &ApInt, shift: &ApInt| ApInt::fshl(hi, lo, shift).expect("equal widths");

    assert_eq!(
        zext(&fshl(&unsigned(8, 0), &unsigned(8, 255), &unsigned(8, 8))),
        0
    );
    assert_eq!(
        zext(&fshl(&unsigned(8, 255), &unsigned(8, 0), &unsigned(8, 8))),
        255
    );
    assert_eq!(
        zext(&fshl(&unsigned(8, 255), &unsigned(8, 0), &unsigned(8, 15))),
        128
    );
    assert_eq!(
        zext(&fshl(&unsigned(8, 15), &unsigned(8, 15), &unsigned(8, 11))),
        120
    );
    assert_eq!(
        zext(&fshl(&unsigned(8, 2), &unsigned(8, 1), &unsigned(8, 3))),
        16
    );
    assert_eq!(
        zext(&fshl(&unsigned(8, 2), &unsigned(8, 1), &unsigned(8, 1))),
        zext(&fshl(&unsigned(8, 2), &unsigned(8, 1), &unsigned(8, 9)))
    );
    assert_eq!(
        zext(&fshl(&unsigned(8, 2), &unsigned(8, 1), &unsigned(8, 7))),
        zext(&fshl(&unsigned(8, 2), &unsigned(8, 1), &unsigned(8, 15)))
    );
    assert_eq!(
        sext(&fshl(
            &signed(32, 0),
            &signed(32, 2_147_483_647),
            &signed(32, 32)
        )),
        0
    );
    assert_eq!(
        sext(&fshl(&signed(64, 1), &signed(64, 2), &signed(64, 3))),
        8
    );
    assert_eq!(
        sext(&fshl(&signed(16, -2), &signed(16, -1), &signed(16, 3))),
        -9
    );
}

/// Port of `TEST(APIntTest, Fshr)`.
#[test]
fn fshr() {
    let fshr =
        |hi: &ApInt, lo: &ApInt, shift: &ApInt| ApInt::fshr(hi, lo, shift).expect("equal widths");

    assert_eq!(
        zext(&fshr(&unsigned(8, 0), &unsigned(8, 255), &unsigned(8, 8))),
        255
    );
    assert_eq!(
        zext(&fshr(&unsigned(8, 255), &unsigned(8, 0), &unsigned(8, 8))),
        0
    );
    assert_eq!(
        zext(&fshr(&unsigned(8, 255), &unsigned(8, 0), &unsigned(8, 15))),
        254
    );
    assert_eq!(
        zext(&fshr(&unsigned(8, 15), &unsigned(8, 15), &unsigned(8, 11))),
        225
    );
    assert_eq!(
        zext(&fshr(&unsigned(8, 1), &unsigned(8, 2), &unsigned(8, 3))),
        32
    );
    assert_eq!(
        zext(&fshr(&unsigned(8, 1), &unsigned(8, 2), &unsigned(8, 1))),
        zext(&fshr(&unsigned(8, 1), &unsigned(8, 2), &unsigned(8, 9)))
    );
    assert_eq!(
        zext(&fshr(&unsigned(8, 1), &unsigned(8, 2), &unsigned(8, 7))),
        zext(&fshr(&unsigned(8, 1), &unsigned(8, 2), &unsigned(8, 15)))
    );
    assert_eq!(
        sext(&fshr(
            &signed(64, 0),
            &signed(64, 9_223_372_036_854_775_807),
            &signed(64, 64)
        )),
        9_223_372_036_854_775_807
    );
    assert_eq!(
        sext(&fshr(&signed(64, 1), &signed(64, 2), &signed(64, 3))),
        2_305_843_009_213_693_952
    );
    assert_eq!(
        sext(&fshr(&signed(16, -2), &signed(16, -1), &signed(16, 3))),
        -8193
    );
}

/// Port of `TEST(APIntTest, clmulr)`.
#[test]
fn carryless_mul_reversed() {
    let clmulr = |a: &ApInt, b: &ApInt| a.carryless_mul_reversed(b).expect("equal widths");

    assert_eq!(zext(&clmulr(&unsigned(4, 1), &unsigned(4, 2))), 0);
    assert_eq!(zext(&clmulr(&unsigned(4, 5), &unsigned(4, 6))), 3);
    assert_eq!(sext(&clmulr(&signed(4, -4), &unsigned(4, 2))), 3);
    assert_eq!(sext(&clmulr(&signed(4, -4), &signed(4, -5))), -2);
    assert_eq!(zext(&clmulr(&unsigned(8, 0), &unsigned(8, 255))), 0);
    assert_eq!(zext(&clmulr(&unsigned(8, 15), &unsigned(8, 15))), 0);
    assert_eq!(zext(&clmulr(&unsigned(8, 1), &unsigned(8, 2))), 0);
    assert_eq!(
        sext(&clmulr(
            &signed(64, 0),
            &signed(64, 9_223_372_036_854_775_807)
        )),
        0
    );
    assert_eq!(sext(&clmulr(&signed(64, 1), &signed(64, 2))), 0);
    assert_eq!(sext(&clmulr(&signed(16, -2), &signed(16, -1))), -21845);
}

/// Port of `TEST(APIntTest, clmulh)`.
#[test]
fn carryless_mul_high() {
    let clmulh = |a: &ApInt, b: &ApInt| a.carryless_mul_high(b).expect("equal widths");

    assert_eq!(zext(&clmulh(&unsigned(4, 1), &unsigned(4, 2))), 0);
    assert_eq!(zext(&clmulh(&unsigned(4, 5), &unsigned(4, 6))), 1);
    assert_eq!(sext(&clmulh(&signed(4, -4), &unsigned(4, 2))), 1);
    assert_eq!(sext(&clmulh(&signed(4, -4), &signed(4, -5))), 7);
    assert_eq!(zext(&clmulh(&unsigned(8, 0), &unsigned(8, 255))), 0);
    assert_eq!(zext(&clmulh(&unsigned(8, 15), &unsigned(8, 15))), 0);
    assert_eq!(zext(&clmulh(&unsigned(8, 1), &unsigned(8, 2))), 0);
    assert_eq!(
        sext(&clmulh(
            &signed(64, 0),
            &signed(64, 9_223_372_036_854_775_807)
        )),
        0
    );
    assert_eq!(sext(&clmulh(&signed(64, 1), &signed(64, 2))), 0);
    assert_eq!(sext(&clmulh(&signed(16, -2), &signed(16, -1))), 21845);
}
