//! Ports of representative `llvm/unittests/ADT/APIntTest.cpp` cases.
//!
//! The Rust surface returns `Option`/`IrResult` for LLVM assertion-preconditioned
//! operations, but the bit-level results mirror APInt.

use llvmkit_ir::{ApInt, ApIntSignedness, ApIntTruncation, IrError};

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
        ApIntSignedness::Unsigned,
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
        qr.quotient().to_string_radix(10, ApIntSignedness::Unsigned),
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
}
