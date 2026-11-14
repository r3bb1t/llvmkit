//! Ports of representative `llvm/unittests/ADT/APIntTest.cpp` cases.
//!
//! The Rust surface returns `Option`/`IrResult` for LLVM assertion-preconditioned
//! operations, but the bit-level results mirror APInt.

use llvmkit_ir::{ApInt, ApIntTruncation, IrError, Signedness};

/// Port of `llvm/unittests/ADT/APIntTest.cpp` construction and bit-count checks.
#[test]
fn construction_normalizes_top_word_and_counts_bits() {
    let value = ApInt::from_words(129, &[u64::MAX, u64::MAX, u64::MAX]);

    assert_eq!(value.bit_width(), 129);
    assert_eq!(value.words(), &[u64::MAX, u64::MAX, 1]);
    assert_eq!(value.active_bits(), 129);
    assert_eq!(value.count_leading_zeros(), 0);
    assert_eq!(value.popcount(), 129);
    assert!(value.is_all_ones());
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

/// A wide integer with a small value prints in the time of its value's words,
/// and a multi-word value prints every digit.
///
/// **No upstream test isolates the cost.** `APInt::toString` divides by the
/// radix through `udivrem(Tmp, Radix, Tmp, Digit)`, a division by one word
/// over the dividend's active words; llvmkit divided through a full-width
/// long division, so `i65536 1` took seconds to print and `i8388608 1` far
/// longer. The digits are upstream's: `UINT128_MAX` in decimal and in hex.
#[test]
fn a_wide_integer_prints_in_the_time_of_its_value() {
    within_a_minute(
        "a_wide_integer_prints_in_the_time_of_its_value",
        "still printing after a minute: a full-width division per digit",
        print_wide_values,
    );
}

/// Division at the widest integer type, `i8388608`: 2^20 divisions of a small
/// value by a small value, every bit set by a one-digit divisor (short
/// division), and every bit set by 2^64 (Knuth's Algorithm D, with a
/// normalizing shift of 31).
///
/// **No upstream test isolates the cost.** `APInt::udivrem` (`APInt.cpp`)
/// divides a one-word dividend natively and otherwise runs `APInt::divide`,
/// whose cost follows the operands' significant digits. llvmkit's `udivrem`
/// was a bit-at-a-time long division, so each division here walked 2^23
/// bits. After that was ported, its size facts still walked the width:
/// `count_leading_zeros` tested a bit at a time, and the `X < Y` arm's
/// `unsigned_cmp` read all 2^17 words. The 2^20 small divisions make any
/// per-division walk of the width (2^17 words or more each) run past the
/// deadline.
#[test]
fn division_at_the_widest_integer_follows_the_significant_digits() {
    within_a_minute(
        "division_at_the_widest_integer_follows_the_significant_digits",
        "still dividing after a minute: division walks the width",
        divide_wide_values,
    );
}

fn divide_wide_values() {
    const WIDTH: u32 = 1 << 23;
    let seven = ApInt::from_words(WIDTH, &[7]);
    let hundred = ApInt::from_words(WIDTH, &[100]);
    for _ in 0..1 << 20 {
        let small = hundred.udivrem(&seven).expect("non-zero divisor");
        assert_eq!(small.quotient().words(), &[14]);
        assert_eq!(small.remainder().words(), &[2]);
    }
    // `7 / 100`: the `X < Y` arm.
    let less = seven.udivrem(&hundred).expect("non-zero divisor");
    assert!(less.quotient().is_zero());
    assert_eq!(less.remainder(), &seven);

    let all_ones = ApInt::all_ones(WIDTH);
    // 2^WIDTH - 1 is divisible by 3 at an even width: 0x5555... remains.
    let by_three = all_ones
        .udivrem(&ApInt::from_words(WIDTH, &[3]))
        .expect("non-zero divisor");
    let fives = vec![0x5555_5555_5555_5555_u64; 1 << 17];
    assert_eq!(by_three.quotient(), &ApInt::from_words(WIDTH, &fives));
    assert!(by_three.remainder().is_zero());

    let by_two_to_the_64 = all_ones
        .udivrem(&ApInt::from_words(WIDTH, &[0, 1]))
        .expect("non-zero divisor");
    assert_eq!(by_two_to_the_64.quotient(), &all_ones.lshr(64));
    assert_eq!(
        by_two_to_the_64.remainder(),
        &ApInt::from_words(WIDTH, &[u64::MAX])
    );
}

/// Runs `body` on a worker and exits the binary if it has not finished within
/// a minute, naming `test` and `cost`. The worker cannot be stopped, so the
/// binary exits rather than keep it running behind a failed test, at the cost
/// of the other tests' results in this binary (see
/// `long_splat_folds.rs::within_a_minute`).
fn within_a_minute(test: &'static str, cost: &'static str, body: fn()) {
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        body();
        done.send(()).ok();
    });
    match finished.recv_timeout(std::time::Duration::from_secs(60)) {
        Ok(()) => {}
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Straight to the process's stderr: the test harness captures
            // `eprintln!`, and the exit below would drop what it captured.
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr(), "{test}: {cost}");
            std::process::exit(1);
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
    }
}

fn print_wide_values() {
    let one = ApInt::from_words(1 << 23, &[1]);
    assert_eq!(one.to_string_radix(10, Signedness::Unsigned), "1");
    let max = ApInt::from_words(128, &[u64::MAX, u64::MAX]);
    assert_eq!(
        max.to_string_radix(10, Signedness::Unsigned),
        "340282366920938463463374607431768211455"
    );
    assert_eq!(
        max.to_string_radix(16, Signedness::Unsigned),
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"
    );
    // Every radix over a value whose digits cross word and chunk edges.
    let mixed = ApInt::from_words(128, &[0x0123_4567_89AB_CDEF, 0x0000_0000_0000_0042]);
    assert_eq!(
        mixed.to_string_radix(10, Signedness::Unsigned),
        "1217567094394046893551"
    );
    assert_eq!(
        mixed.to_string_radix(16, Signedness::Unsigned),
        "420123456789ABCDEF"
    );
    assert_eq!(
        mixed.to_string_radix(8, Signedness::Unsigned),
        "204004432126361152746757"
    );
    // A lower chunk that needs its leading zeros: 10^19 + 5 divides by
    // 10^19 into 1 and 5, and 36^12 + 7 and 36^25 + 1 split the same way
    // in radix 36. Expected digits from Python's integer formatting.
    let ten_19_plus_5 = ApInt::from_words(128, &[10_000_000_000_000_000_005]);
    assert_eq!(
        ten_19_plus_5.to_string_radix(10, Signedness::Unsigned),
        "10000000000000000005"
    );
    let base36_chunk = ApInt::from_words(128, &[36_u64.pow(12) + 7]);
    assert_eq!(
        base36_chunk.to_string_radix(36, Signedness::Unsigned),
        "1000000000007"
    );
    let base36_chunks =
        ApInt::from_words(192, &[0x8f24_0000_0000_0001, 0x6015_4fc3_6cbf_4277, 0x2]);
    assert_eq!(
        base36_chunks.to_string_radix(36, Signedness::Unsigned),
        "10000000000000000000000001"
    );
    let wide_ones = ApInt::all_ones(1 << 16);
    assert_eq!(
        wide_ones.to_string_radix(2, Signedness::Unsigned).len(),
        1 << 16
    );
}
