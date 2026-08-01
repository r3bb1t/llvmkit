//! Ports of the `APInt` bit-level tests from
//! `llvm/unittests/ADT/APIntTest.cpp` in the vendored `llvmorg-22.1.4` tree:
//! the `getBitsSet` factories, the `setBits` mutators, bit access, splat
//! detection, bit reversal, insert/extract, the width conversions, and
//! `GetMostSignificantDifferentBit`.
//!
//! Spelling differences, none of which change the logic:
//!
//! - `trunc`/`zext`/`sext` return `Option` here, since a conversion in the
//!   wrong direction has no answer; upstream asserts instead.
//! - `extractBitsAsZExtValue` is `extract_bits(..).try_zext_u64()`, the same
//!   two steps upstream fuses into one entry point.
//! - `insertBits`'s `(uint64_t, bitPosition, numBits)` overload has no llvmkit
//!   counterpart; the rows that use it name the same bits as an `ApInt` of
//!   `numBits` width, which is what the overload builds internally.

use std::collections::HashMap;

use llvmkit_ir::{ApInt, ApIntSignedness, ApIntTruncation};

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
    value.try_zext_u64().expect("the row fits in 64 bits")
}

fn sext(value: &ApInt) -> i64 {
    value.try_sext_i64().expect("the row fits in 64 bits")
}

fn extract_bits_as_zext_value(value: &ApInt, num_bits: u32, bit_position: u32) -> u64 {
    zext(&value.extract_bits(num_bits, bit_position))
}

/// The shape every counting row checks.
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

/// Port of `TEST(APIntTest, getBitsSet)`.
#[test]
fn get_bits_set() {
    assert_counts(&ApInt::bits_set(64, 1, 63), 0, 1, 63, 1, 0, 62);
    assert_counts(&ApInt::bits_set(127, 1, 126), 0, 1, 126, 1, 0, 125);
}

/// Port of `TEST(APIntTest, getBitsSetWithWrap)`.
#[test]
fn get_bits_set_with_wrap() {
    assert_counts(&ApInt::bits_set_with_wrap(64, 1, 63), 0, 1, 63, 1, 0, 62);
    assert_counts(
        &ApInt::bits_set_with_wrap(127, 1, 126),
        0,
        1,
        126,
        1,
        0,
        125,
    );
    assert_counts(&ApInt::bits_set_with_wrap(64, 63, 1), 1, 0, 64, 0, 1, 2);
    assert_counts(&ApInt::bits_set_with_wrap(127, 126, 1), 1, 0, 127, 0, 1, 2);
    assert_counts(&ApInt::bits_set_with_wrap(32, 10, 10), 32, 0, 32, 0, 32, 32);
}

/// Port of `TEST(APIntTest, getBitsSetFrom)`.
#[test]
fn get_bits_set_from() {
    assert_counts(&ApInt::bits_set_from(64, 33), 31, 0, 64, 33, 0, 31);
}

/// Port of `TEST(APIntTest, getHighBitsSet)`.
#[test]
fn get_high_bits_set() {
    assert_counts(&ApInt::high_bits_set(64, 32), 32, 0, 64, 32, 0, 32);
}

/// Port of `TEST(APIntTest, getLowBitsSet)`.
#[test]
fn get_low_bits_set() {
    assert_counts(&ApInt::low_bits_set(128, 64), 0, 64, 64, 0, 64, 64);
}

/// Port of `TEST(APIntTest, setLowBits)`.
#[test]
fn set_low_bits() {
    let mut i64lo32 = unsigned(64, 0);
    i64lo32.set_low_bits(32);
    assert_counts(&i64lo32, 0, 32, 32, 0, 32, 32);

    let mut i128lo64 = unsigned(128, 0);
    i128lo64.set_low_bits(64);
    assert_counts(&i128lo64, 0, 64, 64, 0, 64, 64);

    let mut i128lo24 = unsigned(128, 0);
    i128lo24.set_low_bits(24);
    assert_counts(&i128lo24, 0, 104, 24, 0, 24, 24);

    let mut i128lo104 = unsigned(128, 0);
    i128lo104.set_low_bits(104);
    assert_counts(&i128lo104, 0, 24, 104, 0, 104, 104);

    let mut i128lo0 = unsigned(128, 0);
    i128lo0.set_low_bits(0);
    assert_counts(&i128lo0, 0, 128, 0, 128, 0, 0);

    let mut i80lo79 = unsigned(80, 0);
    i80lo79.set_low_bits(79);
    assert_counts(&i80lo79, 0, 1, 79, 0, 79, 79);
}

/// Port of `TEST(APIntTest, setHighBits)`.
#[test]
fn set_high_bits() {
    let mut i64hi32 = unsigned(64, 0);
    i64hi32.set_high_bits(32);
    assert_counts(&i64hi32, 32, 0, 64, 32, 0, 32);

    let mut i128hi64 = unsigned(128, 0);
    i128hi64.set_high_bits(64);
    assert_counts(&i128hi64, 64, 0, 128, 64, 0, 64);

    let mut i128hi24 = unsigned(128, 0);
    i128hi24.set_high_bits(24);
    assert_counts(&i128hi24, 24, 0, 128, 104, 0, 24);

    let mut i128hi104 = unsigned(128, 0);
    i128hi104.set_high_bits(104);
    assert_counts(&i128hi104, 104, 0, 128, 24, 0, 104);

    let mut i128hi0 = unsigned(128, 0);
    i128hi0.set_high_bits(0);
    assert_counts(&i128hi0, 0, 128, 0, 128, 0, 0);

    let mut i80hi1 = unsigned(80, 0);
    i80hi1.set_high_bits(1);
    assert_counts(&i80hi1, 1, 0, 80, 79, 0, 1);

    let mut i32hi16 = unsigned(32, 0);
    i32hi16.set_high_bits(16);
    assert_counts(&i32hi16, 16, 0, 32, 16, 0, 16);
}

/// Port of `TEST(APIntTest, setBitsFrom)`.
#[test]
fn set_bits_from() {
    let mut i64from63 = unsigned(64, 0);
    i64from63.set_bits_from(63);
    assert_counts(&i64from63, 1, 0, 64, 63, 0, 1);
}

/// Port of `TEST(APIntTest, isOneBitSet)`.
#[test]
fn is_one_bit_set() {
    assert!(!unsigned(5, 0x00).is_one_bit_set(0));
    assert!(!unsigned(5, 0x02).is_one_bit_set(0));
    assert!(!unsigned(5, 0x03).is_one_bit_set(0));
    assert!(unsigned(5, 0x02).is_one_bit_set(1));
    assert!(unsigned(32, u64::from(0xffu32 << 31)).is_one_bit_set(31));

    assert!(ApInt::one_bit_set(255, 13).is_one_bit_set(13));
}

/// Port of `TEST(APIntTest, IsSplat)`.
#[test]
fn is_splat() {
    let a = unsigned(32, 0x0101_0101);
    assert!(!a.is_splat(1));
    assert!(!a.is_splat(2));
    assert!(!a.is_splat(4));
    assert!(a.is_splat(8));
    assert!(a.is_splat(16));
    assert!(a.is_splat(32));

    let b = unsigned(24, 0x00AA_AAAA);
    assert!(!b.is_splat(1));
    assert!(b.is_splat(2));
    assert!(b.is_splat(4));
    assert!(b.is_splat(8));
    assert!(b.is_splat(24));

    let c = unsigned(24, 0x00AB_AAAB);
    assert!(!c.is_splat(1));
    assert!(!c.is_splat(2));
    assert!(!c.is_splat(4));
    assert!(!c.is_splat(8));
    assert!(c.is_splat(24));

    let d = unsigned(32, 0xABBA_ABBA);
    assert!(!d.is_splat(1));
    assert!(!d.is_splat(2));
    assert!(!d.is_splat(4));
    assert!(!d.is_splat(8));
    assert!(d.is_splat(16));
    assert!(d.is_splat(32));

    let e = unsigned(32, 0);
    assert!(e.is_splat(1));
    assert!(e.is_splat(2));
    assert!(e.is_splat(4));
    assert!(e.is_splat(8));
    assert!(e.is_splat(16));
    assert!(e.is_splat(32));
}

/// Port of `TEST(APIntTest, reverseBits)`.
#[test]
fn reverse_bits() {
    assert_eq!(1, zext(&unsigned(1, 1).reverse_bits()));
    assert_eq!(0, zext(&unsigned(1, 0).reverse_bits()));

    assert_eq!(3, zext(&unsigned(2, 3).reverse_bits()));

    assert_eq!(0xb, zext(&unsigned(4, 0xd).reverse_bits()));
    assert_eq!(0xd, zext(&unsigned(4, 0xb).reverse_bits()));
    assert_eq!(0xf, zext(&unsigned(4, 0xf).reverse_bits()));

    assert_eq!(0x30, zext(&unsigned(7, 0x6).reverse_bits()));
    assert_eq!(0x5a, zext(&unsigned(7, 0x2d).reverse_bits()));

    assert_eq!(0x0f, zext(&unsigned(8, 0xf0).reverse_bits()));
    assert_eq!(0xf0, zext(&unsigned(8, 0x0f).reverse_bits()));

    assert_eq!(0x0f0f, zext(&unsigned(16, 0xf0f0).reverse_bits()));
    assert_eq!(0xf0f0, zext(&unsigned(16, 0x0f0f).reverse_bits()));

    assert_eq!(0x0f0f_0f0f, zext(&unsigned(32, 0xf0f0_f0f0).reverse_bits()));
    assert_eq!(0xf0f0_f0f0, zext(&unsigned(32, 0x0f0f_0f0f).reverse_bits()));

    assert_eq!(
        0x4028_80a0 >> 1,
        zext(&unsigned(31, 0x0501_1402).reverse_bits())
    );

    assert_eq!(
        0x0f0f_0f0f_0f0f_0f0f,
        zext(&unsigned(64, 0xf0f0_f0f0_f0f0_f0f0).reverse_bits())
    );
    assert_eq!(
        0xf0f0_f0f0_f0f0_f0f0,
        zext(&unsigned(64, 0x0f0f_0f0f_0f0f_0f0f).reverse_bits())
    );

    for n in [1, 8, 16, 24, 31, 32, 33, 63, 64, 65, 127, 128, 257, 1024] {
        for i in 0..n {
            let x = ApInt::one_bit_set(n, i);
            let y = ApInt::one_bit_set(n, n - (i + 1));
            assert_eq!(y, x.reverse_bits());
            assert_eq!(x, y.reverse_bits());
        }
    }
}

/// Port of `TEST(APIntTest, arrayAccess)`, which reads `APInt::operator[]`.
#[test]
fn array_access() {
    // Single word check.
    let e1 = 0x2CA7_F46B_F656_9915u64;
    let a1 = unsigned(64, e1);
    for i in 0..64 {
        assert_eq!((e1 & (1u64 << i)) != 0, a1.bit(i));
    }

    // Multiword check.
    let e2 = [
        0xEB6E_B136_591C_BA21u64,
        0x7B93_58BD_6A33_F10Au64,
        0x07E7_FFA5_EADD_8846u64,
        0x305F_341C_A00B_613Du64,
    ];
    let a2 = ApInt::from_words(64 * 4, &e2);
    for (i, word) in e2.iter().enumerate() {
        for j in 0..64u32 {
            let index = u32::try_from(i).expect("four words") * 64 + j;
            assert_eq!((word & (1u64 << j)) != 0, a2.bit(index));
        }
    }
}

/// Port of `TEST(APIntTest, ShiftLeftByZero)`.
#[test]
fn shift_left_by_zero() {
    let one = ApInt::zero(65).wrapping_add(&unsigned(65, 1));
    let shifted = one.shl(0);
    assert!(shifted.bit(0));
    assert!(!shifted.bit(1));
}

/// Port of `TEST(APIntTest, i61_Count)`.
#[test]
fn i61_count() {
    let mut i61 = unsigned(61, 1 << 15);
    assert_eq!(45, i61.count_leading_zeros());
    assert_eq!(0, i61.count_leading_ones());
    assert_eq!(16, i61.active_bits());
    assert_eq!(15, i61.count_trailing_zeros());
    assert_eq!(1, i61.popcount());
    assert_eq!(1 << 15, sext(&i61));
    assert_eq!(1 << 15, zext(&i61));

    i61.set_bits(8, 19);
    assert_eq!(42, i61.count_leading_zeros());
    assert_eq!(0, i61.count_leading_ones());
    assert_eq!(19, i61.active_bits());
    assert_eq!(8, i61.count_trailing_zeros());
    assert_eq!(11, i61.popcount());
    assert_eq!((1 << 19) - (1 << 8), sext(&i61));
    assert_eq!((1 << 19) - (1 << 8), zext(&i61));
}

/// Port of `TEST(APIntTest, i64_ArithmeticRightShiftNegative)`.
#[test]
fn i64_arithmetic_right_shift_negative() {
    let neg_one = signed(64, -1);
    assert_eq!(neg_one, neg_one.ashr(7));
}

/// Port of `TEST(APIntTest, FromArray)`.
#[test]
fn from_array() {
    assert_eq!(unsigned(32, 1), ApInt::from_words(32, &[1]));
}

/// Port of `TEST(APIntTest, ValueInit)`.
///
/// Upstream's subject is `APInt()`, the default constructor, which yields a
/// one-bit zero. llvmkit has no default constructor — every `ApInt` names its
/// width — so the row is spelled with the width it would have produced.
#[test]
fn value_init() {
    let zero = ApInt::zero(1);
    assert!(zero.is_zero());
    assert!(zero.zext(64).expect("widening").is_zero());
    assert!(zero.sext(64).expect("widening").is_zero());
}

/// Port of `TEST(APIntTest, extractBitsAsZExtValue)`.
#[test]
fn extract_bits_as_zext_value_rows() {
    let i32_value = unsigned(32, 0x0123_4567);
    assert_eq!(0x3456, extract_bits_as_zext_value(&i32_value, 16, 4));

    let i257 = signed(
        257,
        i64::from_ne_bytes(0xFFFF_FFFF_FF00_00FFu64.to_ne_bytes()),
    );
    assert_eq!(0xFF, extract_bits_as_zext_value(&i257, 16, 0));
    assert_eq!(0xFF >> 1, extract_bits_as_zext_value(&i257, 16, 1));
    assert_eq!(0xFFFF_FFFF, extract_bits_as_zext_value(&i257, 32, 64));
    assert_eq!(u64::MAX, extract_bits_as_zext_value(&i257, 64, 128));
    assert_eq!(u64::MAX, extract_bits_as_zext_value(&i257, 64, 192));
    assert_eq!(u64::MAX, extract_bits_as_zext_value(&i257, 64, 191));
    assert_eq!(0x3, extract_bits_as_zext_value(&i257, 2, 255));
    assert_eq!(
        0xFFFF_FFFF_FF80_007F,
        extract_bits_as_zext_value(&i257, 64, 1)
    );
    assert_eq!(u64::MAX, extract_bits_as_zext_value(&i257, 64, 65));
    assert_eq!(0x1, extract_bits_as_zext_value(&i257, 1, 129));

    let i144 = ApInt::from_string(144, "281474976710655", 10).expect("upstream spells this");
    assert_eq!(0, extract_bits_as_zext_value(&i144, 48, 48));
    assert_eq!(
        0x0000_ffff_ffff_ffff,
        extract_bits_as_zext_value(&i144, 48, 0)
    );
    assert_eq!(
        0x0000_7fff_ffff_ffff,
        extract_bits_as_zext_value(&i144, 48, 1)
    );
}

/// Port of `TEST(APIntTest, insertBitsUInt64)`, whose scalar source operands
/// are spelled as same-width `ApInt`s here.
#[test]
fn insert_bits() {
    let source = 0x0012_3456u64;

    // Direct copy.
    let mut i31 = unsigned(31, 0x7654_3210);
    i31.insert_bits(&unsigned(31, source), 0);
    assert_eq!(0x0012_3456, sext(&i31));

    // Single word src/dst insertion.
    let mut i63 = unsigned(63, 0x0123_4567_FFFF_FFFF);
    i63.insert_bits(&unsigned(31, source), 4);
    assert_eq!(0x0123_4560_0123_456F, sext(&i63));

    // Insert single word src into one word of dst.
    let mut i120 = signed(120, -1);
    i120.insert_bits(&unsigned(31, source), 8);
    assert_eq!(
        i64::from_ne_bytes(0xFFFF_FF80_1234_56FFu64.to_ne_bytes()),
        sext(&i120)
    );

    // Insert single word src into two words of dst.
    let mut i127 = signed(127, -1);
    i127.insert_bits(&unsigned(31, source), 48);
    assert_eq!(
        0x3456_FFFF_FFFF_FFFF,
        extract_bits_as_zext_value(&i127, 64, 0)
    );
    assert_eq!(
        0x7FFF_FFFF_FFFF_8012,
        extract_bits_as_zext_value(&i127, 63, 64)
    );

    // Insert on word boundaries.
    let mut i128 = unsigned(128, 0);
    i128.insert_bits(&signed(64, -1), 0);
    i128.insert_bits(&signed(64, -1), 64);
    assert_eq!(-1, sext(&i128));

    let mut i257 = unsigned(257, 0);
    i257.insert_bits(&signed(96, -1), 64);
    assert_eq!(0, extract_bits_as_zext_value(&i257, 64, 0));
    assert_eq!(u64::MAX, extract_bits_as_zext_value(&i257, 64, 64));
    assert_eq!(
        0x0000_0000_FFFF_FFFF,
        extract_bits_as_zext_value(&i257, 64, 128)
    );
    assert_eq!(0, extract_bits_as_zext_value(&i257, 64, 192));
    assert_eq!(0, extract_bits_as_zext_value(&i257, 1, 256));

    // General insertion.
    let mut i260 = signed(260, -1);
    i260.insert_bits(&unsigned(129, 1u64 << 48), 15);
    assert_eq!(
        0x8000_0000_0000_7FFF,
        extract_bits_as_zext_value(&i260, 64, 0)
    );
    assert_eq!(0, extract_bits_as_zext_value(&i260, 64, 64));
    assert_eq!(
        0xFFFF_FFFF_FFFF_0000,
        extract_bits_as_zext_value(&i260, 64, 128)
    );
    assert_eq!(u64::MAX, extract_bits_as_zext_value(&i260, 64, 192));
    assert_eq!(0xF, extract_bits_as_zext_value(&i260, 4, 256));
}

/// Port of `TEST(APIntTest, concat)`.
#[test]
fn concat() {
    let int1 = unsigned(4, 0x1);
    let int3 = unsigned(4, 0x3);

    assert_eq!(0x31, zext(&int3.concat(&int1)));
    assert_eq!(unsigned(12, 0x313), int3.concat(&int1).concat(&int3));
    assert_eq!(
        unsigned(16, 0x3313),
        int3.concat(&int3).concat(&int1).concat(&int3)
    );

    let i64_value = unsigned(64, 0x3);
    assert_eq!(
        i64_value,
        i64_value
            .concat(&i64_value)
            .lshr(64)
            .trunc(64)
            .expect("narrowing")
    );

    let i65 = unsigned(65, 0x3);
    let i0 = ApInt::zero_width();
    assert_eq!(i65, i65.concat(&i0));
    assert_eq!(i65, i0.concat(&i65));
}

/// Port of `TEST(APIntTest, sext)`.
#[test]
fn sext_rows() {
    assert_eq!(0, zext(&unsigned(1, 0).sext(64).expect("widening")));
    assert_eq!(u64::MAX, zext(&unsigned(1, 1).sext(64).expect("widening")));

    let i32_max = ApInt::signed_max_value(32).sext(63).expect("widening");
    assert_eq!(i32_max, i32_max.sext(63).expect("same width"));
    assert_eq!(32, i32_max.count_leading_zeros());
    assert_eq!(0, i32_max.count_trailing_zeros());
    assert_eq!(31, i32_max.popcount());

    let i32_min = ApInt::signed_min_value(32).sext(63).expect("widening");
    assert_eq!(i32_min, i32_min.sext(63).expect("same width"));
    assert_eq!(32, i32_min.count_leading_ones());
    assert_eq!(31, i32_min.count_trailing_zeros());
    assert_eq!(32, i32_min.popcount());

    let i32_neg1 = unsigned(32, u64::from(u32::MAX))
        .sext(63)
        .expect("widening");
    assert_eq!(i32_neg1, i32_neg1.sext(63).expect("same width"));
    assert_eq!(63, i32_neg1.count_leading_ones());
    assert_eq!(0, i32_neg1.count_trailing_zeros());
    assert_eq!(63, i32_neg1.popcount());

    assert_eq!(
        unsigned(32, 0),
        ApInt::zero_width().sext(32).expect("widening")
    );
    assert_eq!(
        unsigned(64, 0),
        ApInt::zero_width().sext(64).expect("widening")
    );
}

/// Port of `TEST(APIntTest, trunc)`.
#[test]
fn trunc_rows() {
    let value = unsigned(32, 0xFFFF_FFFF);
    assert_eq!(0xFFFF, zext(&value.trunc(16).expect("narrowing")));
    assert_eq!(0xFFFF_FFFF, zext(&value.trunc(32).expect("same width")));
}

/// Port of `TEST(APIntTest, TryExt)`.
#[test]
fn try_ext() {
    let small = unsigned(32, 42);
    let large = ApInt::from_words(128, &[0xffff, 0xffff]);
    assert!(small.try_zext_u64().is_some());
    assert!(small.try_sext_i64().is_some());
    assert!(large.try_zext_u64().is_none());
    assert!(large.try_sext_i64().is_none());
    assert_eq!(small.try_sext_i64().unwrap_or(41), 42);
    assert_eq!(large.try_sext_i64().unwrap_or(41), 41);

    let mut neg_one32 = unsigned(32, 0);
    neg_one32.set_all_bits();
    assert_eq!(neg_one32.try_sext_i64().unwrap_or(42), -1);
    let mut neg_one64 = unsigned(64, 0);
    neg_one64.set_all_bits();
    assert_eq!(neg_one64.try_sext_i64().unwrap_or(42), -1);
    let mut neg_one128 = unsigned(128, 0);
    neg_one128.set_all_bits();
    assert_eq!(neg_one128.try_sext_i64().unwrap_or(42), -1);
    assert_eq!(42, unsigned(128, u64::MAX).try_sext_i64().unwrap_or(42));
}

/// Port of `TEST(APIntTest, isSubsetOf)`.
#[test]
fn is_subset_of() {
    let i32_1 = unsigned(32, 1);
    let i32_2 = unsigned(32, 2);
    let i32_3 = unsigned(32, 3);
    assert!(!i32_3.is_subset_of(&i32_1));
    assert!(i32_1.is_subset_of(&i32_3));
    assert!(!i32_2.is_subset_of(&i32_1));
    assert!(!i32_1.is_subset_of(&i32_2));
    assert!(i32_3.is_subset_of(&i32_3));

    let mut i128_1 = unsigned(128, 1);
    let mut i128_2 = unsigned(128, 2);
    let mut i128_3 = unsigned(128, 3);
    assert!(!i128_3.is_subset_of(&i128_1));
    assert!(i128_1.is_subset_of(&i128_3));
    assert!(!i128_2.is_subset_of(&i128_1));
    assert!(!i128_1.is_subset_of(&i128_2));
    assert!(i128_3.is_subset_of(&i128_3));

    i128_1 = i128_1.shl(64);
    i128_2 = i128_2.shl(64);
    i128_3 = i128_3.shl(64);
    assert!(!i128_3.is_subset_of(&i128_1));
    assert!(i128_1.is_subset_of(&i128_3));
    assert!(!i128_2.is_subset_of(&i128_1));
    assert!(!i128_1.is_subset_of(&i128_2));
    assert!(i128_3.is_subset_of(&i128_3));
}

/// Port of `TEST(APIntTest, GetMostSignificantDifferentBit)`.
#[test]
fn get_most_significant_different_bit() {
    let bit =
        |a: u64, b: u64| ApInt::most_significant_different_bit(&unsigned(8, a), &unsigned(8, b));
    assert_eq!(bit(0, 0), None);
    assert_eq!(bit(42, 42), None);
    assert_eq!(bit(0, 1), Some(0));
    assert_eq!(bit(0, 2), Some(1));
    assert_eq!(bit(0, 3), Some(1));
    assert_eq!(bit(1, 0), Some(0));
    assert_eq!(bit(1, 1), None);
    assert_eq!(bit(1, 2), Some(1));
    assert_eq!(bit(1, 3), Some(1));
    assert_eq!(bit(42, 112), Some(6));
}

/// Port of `TEST(APIntTest, DenseMap)`, which checks that a zero-width `APInt`
/// is a usable map key. llvmkit has no `DenseMap`; the property under test is
/// `ApInt`'s `Hash`/`Eq`, so the port uses the standard `HashMap`.
#[test]
fn hash_map_key() {
    let mut map = HashMap::new();
    let zero_width = ApInt::zero_width();
    map.insert(zero_width.clone(), 123);
    assert_eq!(map.get(&zero_width), Some(&123));
}
