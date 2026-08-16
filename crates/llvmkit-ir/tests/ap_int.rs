//! Ports of representative `llvm/unittests/ADT/APIntTest.cpp` cases.
//!
//! The Rust surface returns `Option`/`IrResult` for LLVM assertion-preconditioned
//! operations, but the bit-level results mirror APInt.

use llvmkit_ir::{ApInt, ApIntTruncation, IrError, Signedness};

/// Port of `llvm/unittests/ADT/APIntTest.cpp` construction and bit-count checks.
#[test]
fn construction_normalizes_top_word_and_counts_bits() -> Result<(), IrError> {
    let value = ApInt::from_words(129, &[u64::MAX, u64::MAX, u64::MAX]);

    assert_eq!(value.bit_width(), 129);
    assert_eq!(value.words(), &[u64::MAX, u64::MAX, 1]);
    assert_eq!(value.active_bits(), 129);
    assert_eq!(value.count_leading_zeros(), 0);
    assert_eq!(value.popcount(), 129);
    assert!(value.is_all_ones());

    Ok(())
}

/// Port of `llvm/unittests/ADT/APIntTest.cpp` arithmetic wrap coverage.
#[test]
fn arithmetic_wraps_past_128_bits() -> Result<(), IrError> {
    let max = ApInt::all_ones(257);
    let one = ApInt::new(
        257,
        1,
        Signedness::Unsigned,
        ApIntTruncation::RejectOverflow,
    )?;

    let sum = max.wrapping_add(&one);
    assert_eq!(sum, ApInt::zero(257));

    let product = ApInt::one_bit_set(257, 128).wrapping_mul(&ApInt::one_bit_set(257, 64));
    assert_eq!(product, ApInt::one_bit_set(257, 192));

    Ok(())
}

/// Port of `llvm/unittests/ADT/APIntTest.cpp` udiv/sdiv remainder cases.
#[test]
fn division_remainder_and_signed_min_overflow() -> Result<(), IrError> {
    let n = ApInt::from_string(257, "340282366920938463463374607431768211456", 10)?;
    let d = ApInt::from_string(257, "3", 10)?;
    let qr = n.udivrem(&d).expect("non-zero divisor");

    assert_eq!(
        qr.quotient().to_string_radix(10, Signedness::Unsigned),
        "113427455640312821154458202477256070485"
    );
    assert_eq!(qr.remainder().try_zext_u64(), Some(1));

    let signed_min = ApInt::signed_min_value(257);
    let minus_one = ApInt::all_ones(257);
    assert!(signed_min.checked_sdiv(&minus_one).is_none());

    Ok(())
}

/// Port of `llvm/unittests/ADT/APIntTest.cpp` shift edge cases.
#[test]
fn checked_shifts_reject_width_and_fold_width_minus_one() {
    let one = ApInt::one_bit_set(129, 0);

    assert_eq!(one.checked_shl(128), Some(ApInt::one_bit_set(129, 128)));
    assert_eq!(one.checked_shl(129), None);

    let sign = ApInt::one_bit_set(129, 128);
    assert_eq!(sign.checked_ashr(128), Some(ApInt::all_ones(129)));
    assert_eq!(sign.checked_lshr(128), Some(ApInt::one_bit_set(129, 0)));
}

/// Port of `llvm/unittests/ADT/APIntTest.cpp` trunc/zext/sext behavior.
#[test]
fn trunc_zext_sext_preserve_signed_and_unsigned_views() {
    let neg = ApInt::all_ones(129);

    assert_eq!(neg.trunc(64).expect("narrow"), ApInt::all_ones(64));
    assert_eq!(
        ApInt::all_ones(64).zext(129).expect("widen"),
        ApInt::from_words(129, &[u64::MAX])
    );
    assert_eq!(
        ApInt::all_ones(64).sext(129).expect("widen"),
        ApInt::all_ones(129)
    );
    assert_eq!(
        ApInt::from_words(4, &[0b1011])
            .sext(8)
            .expect("same-word widen"),
        ApInt::from_words(8, &[0b1111_1011])
    );
}

/// Port of `llvm/unittests/ADT/APIntTest.cpp` bit mutation and extraction
/// helper coverage.
#[test]
fn bit_mutators_and_slices_match_apint_test_helpers() {
    let mut value = ApInt::zero(16);
    value.set_bit(3);
    value.set_sign_bit();
    value.set_bit(99);
    assert_eq!(value, ApInt::from_words(16, &[0x8008]));

    value.clear_bit(3);
    value.clear_sign_bit();
    value.clear_bit(99);
    assert_eq!(value, ApInt::zero(16));

    value.set_low_bits(5);
    assert_eq!(value, ApInt::from_words(16, &[0x001f]));
    value.set_high_bits(4);
    assert_eq!(value, ApInt::from_words(16, &[0xf01f]));
    value.set_bits_from(12);
    assert_eq!(value, ApInt::from_words(16, &[0xf01f]));

    let mut inserted = ApInt::from_words(16, &[0xffff]);
    inserted.insert_bits(&ApInt::from_words(4, &[0x5]), 4);
    assert_eq!(inserted, ApInt::from_words(16, &[0xff5f]));
    inserted.insert_bits(&ApInt::from_words(8, &[0xa5]), 12);
    assert_eq!(inserted, ApInt::from_words(16, &[0x5f5f]));

    assert_eq!(inserted.extract_bits(8, 4), ApInt::from_words(8, &[0xf5]));
    assert_eq!(inserted.extract_bits(8, 12), ApInt::from_words(8, &[0x05]));
    assert_eq!(
        ApInt::from_words(4, &[0xa]).concat(&ApInt::from_words(2, &[0x3])),
        ApInt::from_words(6, &[0x2b])
    );

    assert_eq!(inserted.shl(16), ApInt::zero(16));
    assert_eq!(inserted.lshr(16), ApInt::zero(16));
}

/// Port of `llvm/unittests/ADT/APIntTest.cpp` byte-swap helpers and
/// `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, UnaryExhaustive)`.
#[test]
fn byte_swap_and_reverse_bits_match_knownbits_unary_tests() {
    let value = ApInt::from_words(32, &[0x1234_abcd]);
    assert_eq!(value.byte_swap(), ApInt::from_words(32, &[0xcdab_3412]));

    let asymmetric = ApInt::from_words(8, &[0b0001_0110]);
    assert_eq!(
        asymmetric.reverse_bits(),
        ApInt::from_words(8, &[0b0110_1000])
    );
    assert_eq!(asymmetric.shl(3), ApInt::from_words(8, &[0b1011_0000]));
    assert_eq!(asymmetric.lshr(2), ApInt::from_words(8, &[0b0000_0101]));

    let negative = ApInt::from_words(8, &[0b1001_0000]);
    assert_eq!(negative.ashr(3), ApInt::from_words(8, &[0b1111_0010]));
}

/// Shifting arithmetically by at least the bit width fills with the sign bit,
/// not with zero.
///
/// Mirrors `llvm/include/llvm/ADT/APInt.h::APInt::ashrInPlace`, whose
/// `ShiftAmt == BitWidth` arm is `SExtVAL >> (APINT_BITS_PER_WORD - 1)` —
/// "Fill with sign bit". The `APInt` overload reaches that arm through
/// `getLimitedValue(BitWidth)`, so any larger amount saturates identically.
///
/// This is a regression test: `ApInt::ashr` previously returned zero for every
/// out-of-range amount, which is right for a non-negative value and wrong for
/// a negative one. `ConstantRange::ashr` surfaced it — the range for
/// `-8 ashr 4` at four bits dropped `-1`.
#[test]
fn ashr_at_or_beyond_the_width_fills_with_the_sign_bit() {
    let negative = ApInt::from_words(4, &[0b1000]); // -8
    let positive = ApInt::from_words(4, &[0b0111]); //  7

    for amount in [4_u32, 5, 64, 1000] {
        assert_eq!(
            negative.ashr(amount),
            ApInt::all_ones(4),
            "a negative value saturates to -1 at shift {amount}"
        );
        assert_eq!(
            positive.ashr(amount),
            ApInt::zero(4),
            "a non-negative value saturates to 0 at shift {amount}"
        );
    }

    // One below the width still shifts normally: -8 >> 3 == -1, 7 >> 3 == 0.
    assert_eq!(negative.ashr(3), ApInt::all_ones(4));
    assert_eq!(positive.ashr(3), ApInt::zero(4));

    // `checked_ashr` keeps its stricter contract and declines out of range.
    assert!(negative.checked_ashr(4).is_none());
}
