//! Ports of the `APInt` arithmetic tests from
//! `llvm/unittests/ADT/APIntTest.cpp` in the vendored `llvmorg-22.1.4` tree:
//! the saturating family, multiplication, `pow`, and the multiply/floor-divide
//! overflow flags.
//!
//! Spelling differences, none of which change the logic:
//!
//! - Upstream's scalar operand overloads (`APInt * uint64_t`, `*= uint64_t`)
//!   build a same-width `APInt` internally; the ports name that `ApInt`.
//! - Upstream's shift-saturating entry points take the amount as an `APInt` and
//!   reduce it with `getLimitedValue(BitWidth)` before shifting. llvmkit takes
//!   a `u32`, so the ports apply that same reduction where upstream's row
//!   depends on it — see [`shift_amount`].

use llvmkit_ir::{ApInt, ApIntTruncation, Signedness};

fn unsigned(bit_width: u32, value: u64) -> ApInt {
    ApInt::new(
        bit_width,
        value,
        Signedness::Unsigned,
        ApIntTruncation::Truncate,
    )
    .expect("truncating construction cannot overflow")
}

fn signed(bit_width: u32, value: i64) -> ApInt {
    ApInt::new(
        bit_width,
        u64::from_ne_bytes(value.to_ne_bytes()),
        Signedness::Signed,
        ApIntTruncation::Truncate,
    )
    .expect("truncating construction cannot overflow")
}

/// Upstream's `ushl_sat`/`sshl_sat` overloads pass the amount through
/// `getLimitedValue(BitWidth)`; llvmkit's take a `u32` directly.
fn shift_amount(amount: &ApInt, bit_width: u32) -> u32 {
    let limited = amount.limited_value(u64::from(bit_width));
    u32::try_from(limited).unwrap_or(bit_width)
}

/// Port of `TEST(APIntTest, SaturatingMath)`.
#[test]
fn saturating_math() {
    let ap_10 = unsigned(8, 10);
    let ap_42 = unsigned(8, 42);
    let ap_100 = unsigned(8, 100);
    let ap_200 = unsigned(8, 200);

    assert_eq!(unsigned(8, 100), ap_100.trunc_usat(8));
    assert_eq!(unsigned(7, 100), ap_100.trunc_usat(7));
    assert_eq!(unsigned(6, 63), ap_100.trunc_usat(6));
    assert_eq!(unsigned(5, 31), ap_100.trunc_usat(5));

    assert_eq!(unsigned(8, 200), ap_200.trunc_usat(8));
    assert_eq!(unsigned(7, 127), ap_200.trunc_usat(7));
    assert_eq!(unsigned(6, 63), ap_200.trunc_usat(6));
    assert_eq!(unsigned(5, 31), ap_200.trunc_usat(5));

    assert_eq!(unsigned(8, 42), ap_42.trunc_ssat(8));
    assert_eq!(unsigned(7, 42), ap_42.trunc_ssat(7));
    assert_eq!(unsigned(6, 31), ap_42.trunc_ssat(6));
    assert_eq!(unsigned(5, 15), ap_42.trunc_ssat(5));

    assert_eq!(signed(8, -56), ap_200.trunc_ssat(8));
    assert_eq!(signed(7, -56), ap_200.trunc_ssat(7));
    assert_eq!(signed(6, -32), ap_200.trunc_ssat(6));
    assert_eq!(signed(5, -16), ap_200.trunc_ssat(5));

    assert_eq!(unsigned(8, 200), ap_100.uadd_sat(&ap_100));
    assert_eq!(unsigned(8, 255), ap_100.uadd_sat(&ap_200));
    assert_eq!(
        unsigned(8, 255),
        unsigned(8, 255).uadd_sat(&unsigned(8, 255))
    );

    assert_eq!(unsigned(8, 110), ap_10.sadd_sat(&ap_100));
    assert_eq!(unsigned(8, 127), ap_100.sadd_sat(&ap_100));
    assert_eq!(signed(8, -128), ap_100.negate().sadd_sat(&ap_100.negate()));
    assert_eq!(signed(8, -128), signed(8, -128).sadd_sat(&signed(8, -128)));

    assert_eq!(unsigned(8, 90), ap_100.usub_sat(&ap_10));
    assert_eq!(unsigned(8, 0), ap_100.usub_sat(&ap_200));
    assert_eq!(unsigned(8, 0), unsigned(8, 0).usub_sat(&unsigned(8, 255)));

    assert_eq!(signed(8, -90), ap_10.ssub_sat(&ap_100));
    assert_eq!(unsigned(8, 127), ap_100.ssub_sat(&ap_100.negate()));
    assert_eq!(signed(8, -128), ap_100.negate().ssub_sat(&ap_100));
    assert_eq!(signed(8, -128), signed(8, -128).ssub_sat(&unsigned(8, 127)));

    assert_eq!(unsigned(8, 250), unsigned(8, 50).umul_sat(&unsigned(8, 5)));
    assert_eq!(unsigned(8, 255), unsigned(8, 50).umul_sat(&unsigned(8, 6)));
    assert_eq!(unsigned(8, 255), signed(8, -128).umul_sat(&unsigned(8, 3)));
    assert_eq!(unsigned(8, 255), unsigned(8, 3).umul_sat(&signed(8, -128)));
    assert_eq!(unsigned(8, 255), signed(8, -128).umul_sat(&signed(8, -128)));

    assert_eq!(unsigned(8, 125), unsigned(8, 25).smul_sat(&unsigned(8, 5)));
    assert_eq!(unsigned(8, 127), unsigned(8, 25).smul_sat(&unsigned(8, 6)));
    assert_eq!(
        unsigned(8, 127),
        unsigned(8, 127).smul_sat(&unsigned(8, 127))
    );
    assert_eq!(signed(8, -125), signed(8, -25).smul_sat(&unsigned(8, 5)));
    assert_eq!(signed(8, -125), unsigned(8, 25).smul_sat(&signed(8, -5)));
    assert_eq!(unsigned(8, 125), signed(8, -25).smul_sat(&signed(8, -5)));
    assert_eq!(unsigned(8, 125), unsigned(8, 25).smul_sat(&unsigned(8, 5)));
    assert_eq!(signed(8, -128), signed(8, -25).smul_sat(&unsigned(8, 6)));
    assert_eq!(signed(8, -128), unsigned(8, 25).smul_sat(&signed(8, -6)));
    assert_eq!(unsigned(8, 127), signed(8, -25).smul_sat(&signed(8, -6)));
    assert_eq!(unsigned(8, 127), unsigned(8, 25).smul_sat(&unsigned(8, 6)));

    let ushl_sat = |value: &ApInt, amount: &ApInt| value.ushl_sat(shift_amount(amount, 8));
    assert_eq!(unsigned(8, 128), ushl_sat(&unsigned(8, 4), &unsigned(8, 5)));
    assert_eq!(unsigned(8, 255), ushl_sat(&unsigned(8, 4), &unsigned(8, 6)));
    assert_eq!(unsigned(8, 128), ushl_sat(&unsigned(8, 1), &unsigned(8, 7)));
    assert_eq!(unsigned(8, 255), ushl_sat(&unsigned(8, 1), &unsigned(8, 8)));
    assert_eq!(
        unsigned(8, 255),
        ushl_sat(&signed(8, -128), &unsigned(8, 2))
    );
    assert_eq!(
        unsigned(8, 255),
        ushl_sat(&unsigned(8, 64), &unsigned(8, 2))
    );
    assert_eq!(unsigned(8, 255), ushl_sat(&unsigned(8, 64), &signed(8, -2)));

    let sshl_sat = |value: &ApInt, amount: &ApInt| value.sshl_sat(shift_amount(amount, 8));
    assert_eq!(unsigned(8, 64), sshl_sat(&unsigned(8, 4), &unsigned(8, 4)));
    assert_eq!(unsigned(8, 127), sshl_sat(&unsigned(8, 4), &unsigned(8, 5)));
    assert_eq!(unsigned(8, 127), sshl_sat(&unsigned(8, 1), &unsigned(8, 8)));
    assert_eq!(signed(8, -64), sshl_sat(&signed(8, -4), &unsigned(8, 4)));
    assert_eq!(signed(8, -128), sshl_sat(&signed(8, -4), &unsigned(8, 5)));
    assert_eq!(signed(8, -128), sshl_sat(&signed(8, -4), &unsigned(8, 6)));
    assert_eq!(signed(8, -128), sshl_sat(&signed(8, -1), &unsigned(8, 7)));
    assert_eq!(signed(8, -128), sshl_sat(&signed(8, -1), &unsigned(8, 8)));
}

/// Port of `TEST(APIntTest, multiply)`.
#[test]
fn multiply() {
    let i64_value = unsigned(64, 1234);
    assert_eq!(
        7_006_652,
        i64_value
            .wrapping_mul(&unsigned(64, 5678))
            .try_zext_u64()
            .expect("fits")
    );

    let i128 = ApInt::one_bit_set(128, 64);
    let i128_1234 = unsigned(128, 1234).shl(64);
    assert_eq!(i128_1234, i128.wrapping_mul(&unsigned(128, 1234)));

    let i96 = ApInt::one_bit_set(96, 64).wrapping_mul(&unsigned(96, u64::MAX));
    assert_eq!(32, i96.count_leading_ones());
    assert_eq!(32, i96.popcount());
    assert_eq!(64, i96.count_trailing_zeros());
}

/// Port of `TEST(APIntTest, umul_ov)`.
#[test]
fn umul_ov() {
    let overflows: [(u64, u64); 4] = [
        (0x8000_0000_0000_0000, 2),
        (0x5555_5555_5555_5556, 3),
        (4_294_967_296, 4_294_967_296),
        (4_294_967_295, 4_294_967_298),
    ];
    let non_overflows: [(u64, u64); 3] = [
        (0x7fff_ffff_ffff_ffff, 2),
        (0x5555_5555_5555_5555, 3),
        (4_294_967_295, 4_294_967_297),
    ];

    for (lhs, rhs) in overflows {
        let (_, overflow) = unsigned(64, lhs).umul_ov(&unsigned(64, rhs));
        assert!(overflow, "{lhs} * {rhs} overflows 64 bits");
    }
    for (lhs, rhs) in non_overflows {
        let (_, overflow) = unsigned(64, lhs).umul_ov(&unsigned(64, rhs));
        assert!(!overflow, "{lhs} * {rhs} fits 64 bits");
    }

    for bits in 1..=5u32 {
        for a in 0..(1u64 << bits) {
            for b in 0..(1u64 << bits) {
                let n1 = unsigned(bits, a);
                let n2 = unsigned(bits, b);
                let (narrow, overflow) = n1.umul_ov(&n2);
                let wide = n1
                    .zext(2 * bits)
                    .expect("widening")
                    .wrapping_mul(&n2.zext(2 * bits).expect("widening"));
                assert_eq!(wide.trunc(bits).expect("narrowing"), narrow);
                assert_eq!(narrow.zext(2 * bits).expect("widening") != wide, overflow);
            }
        }
    }
}

/// Port of `TEST(APIntTest, smul_ov)`.
#[test]
fn smul_ov() {
    for bits in 1..=5u32 {
        for a in 0..(1u64 << bits) {
            for b in 0..(1u64 << bits) {
                let n1 = unsigned(bits, a);
                let n2 = unsigned(bits, b);
                let (narrow, overflow) = n1.smul_ov(&n2);
                let wide = n1
                    .sext(2 * bits)
                    .expect("widening")
                    .wrapping_mul(&n2.sext(2 * bits).expect("widening"));
                assert_eq!(wide.trunc(bits).expect("narrowing"), narrow);
                assert_eq!(narrow.sext(2 * bits).expect("widening") != wide, overflow);
            }
        }
    }
}

/// Port of `TEST(APIntTest, sfloordiv_ov)`.
#[test]
fn sfloordiv_ov() {
    // The signed minimum divided by -1 overflows at every width upstream
    // checks.
    for bits in [16u32, 32, 64] {
        let divisor = ApInt::signed_min_value(bits);
        let dividend = signed(bits, -1);
        let (_, overflow) = divisor.sfloordiv_ov(&dividend);
        assert!(overflow, "{bits}-bit signed minimum over -1 overflows");
    }

    // Test all of int8.
    for i in -128..128i64 {
        for j in -128..128i64 {
            if j == 0 {
                continue;
            }
            let divisor = signed(8, i);
            let dividend = signed(8, j);
            let (quotient, overflow) = divisor.sfloordiv_ov(&dividend);

            if i == -128 && j == -1 {
                assert!(overflow);
                continue;
            }

            let expected = if ((i >= 0 && j > 0) || (i <= 0 && j < 0)) || (i % j == 0) {
                // If the quotient is non-negative and the remainder is zero,
                // floor-division agrees with truncating division.
                i / j
            } else {
                i / j - 1
            };
            assert_eq!(
                quotient.try_sext_i64().expect("8 bits fit"),
                expected,
                "floor({i} / {j})"
            );
            assert!(!overflow);
        }
    }
}

/// Port of `TEST(APIntTest, PowZeroTo5)` through
/// `TEST(APIntTest, ZeroToZero)` — the whole `APIntOps::pow` family, which
/// upstream splits across nine one-row tests.
#[test]
fn pow() {
    let zero = ApInt::zero(32);
    assert!(zero.is_zero());
    assert!(zero.pow(5).is_zero());

    let one = unsigned(32, 1);
    assert_eq!(one, one.pow(16));

    assert_eq!(unsigned(32, 1024), unsigned(32, 2).pow(10));
    assert_eq!(unsigned(32, 27), unsigned(32, 3).pow(3));

    let signed_max = ApInt::signed_max_value(32);
    assert_eq!(signed_max, signed_max.pow(3));

    let max = ApInt::max_value(32);
    assert_eq!(max, max.pow(3));

    let signed_min = ApInt::signed_min_value(32);
    assert!(signed_min.pow(3).is_zero());
    assert_eq!(signed_min, signed_min.pow(1));

    assert_eq!(one, zero.pow(0));
}
