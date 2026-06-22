use core::ops::Not;
use llvmkit_ir::{ApInt, IrError, KnownBits};

type UnaryBitsFn = fn(&KnownBits) -> KnownBits;
type UnaryIntFn = fn(&ApInt) -> Option<ApInt>;
type BinaryBitsFn = fn(&KnownBits, &KnownBits) -> KnownBits;
type BinaryIntFn = fn(&ApInt, &ApInt) -> Option<ApInt>;

fn ap(width: u32, value: u64) -> ApInt {
    ApInt::from_words(width, &[value])
}

fn kb(width: u32, zero: u64, one: u64) -> KnownBits {
    KnownBits::from_zero_one(ap(width, zero), ap(width, one)).expect("matching widths")
}

fn all_known_from_results(width: u32, values: &[ApInt]) -> Option<KnownBits> {
    if values.is_empty() {
        return None;
    }

    let mut known_one = ApInt::all_ones(width);
    let mut known_zero = ApInt::all_ones(width);
    for value in values {
        known_one = known_one.bitand(value);
        known_zero = known_zero.bitand(&value.not());
    }

    Some(KnownBits::from_zero_one(known_zero, known_one).expect("matching widths"))
}

fn values_matching(known: &KnownBits) -> Vec<ApInt> {
    let width = known.bit_width();
    let limit = 1u64 << width;
    let zero = known.zero_mask();
    let one = known.one_mask();
    let mut values = Vec::new();
    for value in 0..limit {
        let candidate = ap(width, value);
        if candidate.bitand(zero).is_zero() && candidate.clone().not().bitand(one).is_zero() {
            values.push(candidate);
        }
    }
    values
}

fn foreach_known_bits(width: u32, mut f: impl FnMut(KnownBits)) {
    let limit = 1u64 << width;
    for zero in 0..limit {
        for one in 0..limit {
            f(kb(width, zero, one));
        }
    }
}

fn check_unary_exhaustive(name: &str, bits_fn: UnaryBitsFn, int_fn: UnaryIntFn) {
    foreach_known_bits(4, |known| {
        let mut outputs = Vec::new();
        for value in values_matching(&known) {
            if let Some(result) = int_fn(&value) {
                outputs.push(result);
            }
        }
        let Some(exact) = all_known_from_results(4, &outputs) else {
            return;
        };
        let computed = bits_fn(&known);
        assert_eq!(
            computed, exact,
            "{name}: input={known} computed={computed} exact={exact}"
        );
    });
}

fn check_binary_exhaustive(
    name: &str,
    bits_fn: BinaryBitsFn,
    int_fn: BinaryIntFn,
    require_optimal: bool,
) {
    foreach_known_bits(4, |lhs| {
        foreach_known_bits(4, |rhs| {
            let mut outputs = Vec::new();
            for left in values_matching(&lhs) {
                for right in values_matching(&rhs) {
                    if let Some(result) = int_fn(&left, &right) {
                        outputs.push(result);
                    }
                }
            }
            let Some(exact) = all_known_from_results(4, &outputs) else {
                return;
            };
            let computed = bits_fn(&lhs, &rhs);
            if require_optimal {
                assert_eq!(
                    computed, exact,
                    "{name}: lhs={lhs} rhs={rhs} computed={computed} exact={exact}"
                );
            } else {
                assert!(
                    computed.zero_mask().is_subset_of(exact.zero_mask()),
                    "{name}: lhs={lhs} rhs={rhs} computed={computed} exact={exact} has unsound known-zero bits"
                );
                assert!(
                    computed.one_mask().is_subset_of(exact.one_mask()),
                    "{name}: lhs={lhs} rhs={rhs} computed={computed} exact={exact} has unsound known-one bits"
                );
            }
        });
    });
}

fn signed_abs(value: &ApInt) -> ApInt {
    if value.is_negative() {
        value.negate()
    } else {
        value.clone()
    }
}

fn shift_amount(value: &ApInt) -> Option<u32> {
    let amount = u32::try_from(value.try_zext_u64()?).ok()?;
    (amount < value.bit_width()).then_some(amount)
}

fn assert_known_result(name: &str, exact: Option<KnownBits>, computed: KnownBits) {
    if let Some(exact) = exact {
        assert_eq!(computed, exact, "{name}: computed={computed} exact={exact}");
    }
}

fn exact_known(width: u32, values: Vec<ApInt>) -> Option<KnownBits> {
    all_known_from_results(width, &values)
}

fn signed_value(value: &ApInt) -> i64 {
    value.try_sext_i64().expect("test values fit in i64")
}

fn signed_ap(width: u32, value: i64) -> ApInt {
    let modulus = 1i64
        .checked_shl(width)
        .expect("test widths fit in i64 shifts");
    let raw = if value < 0 { value + modulus } else { value };
    ap(
        width,
        u64::try_from(raw).expect("test value is non-negative"),
    )
}

fn avg_floor_signed(width: u32, lhs: &ApInt, rhs: &ApInt) -> ApInt {
    let sum = signed_value(lhs) + signed_value(rhs);
    let mut avg = sum / 2;
    if sum < 0 && sum % 2 != 0 {
        avg -= 1;
    }
    signed_ap(width, avg)
}

fn avg_ceil_signed(width: u32, lhs: &ApInt, rhs: &ApInt) -> ApInt {
    let sum = signed_value(lhs) + signed_value(rhs);
    let mut avg = sum / 2;
    if sum > 0 && sum % 2 != 0 {
        avg += 1;
    }
    signed_ap(width, avg)
}

fn mul_high_unsigned(width: u32, lhs: &ApInt, rhs: &ApInt) -> ApInt {
    let product = lhs
        .zext_or_trunc(width * 2)
        .wrapping_mul(&rhs.zext_or_trunc(width * 2));
    product.extract_bits(width, width)
}

fn mul_high_signed(width: u32, lhs: &ApInt, rhs: &ApInt) -> ApInt {
    let product = lhs
        .sext_or_trunc(width * 2)
        .wrapping_mul(&rhs.sext_or_trunc(width * 2));
    product.extract_bits(width, width)
}

fn exact_reduce_add(width: u32, known: &KnownBits, num_elts: u32) -> Option<KnownBits> {
    let choices = values_matching(known);
    let mut sums = vec![ApInt::zero(width)];
    let mut depth = 0;
    while depth < num_elts {
        let mut next = Vec::new();
        for sum in &sums {
            for choice in &choices {
                let candidate = sum.wrapping_add(choice);
                if !next.contains(&candidate) {
                    next.push(candidate);
                }
            }
        }
        sums = next;
        depth += 1;
    }
    exact_known(width, sums)
}

/// Port of `llvm/lib/Support/KnownBits.cpp::KnownBits::print`.
#[test]
fn display_prints_msb_to_lsb_with_conflict_marker() -> Result<(), IrError> {
    assert_eq!(kb(4, 0b1001, 0b1010).to_string(), "!?10");
    assert_eq!(KnownBits::unknown(4).to_string(), "????");
    assert_eq!(KnownBits::from_ap_int(ap(4, 0b1010)).to_string(), "1010");
    Ok(())
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, UnaryExhaustive)`.
#[test]
fn unary_not_and_abs_are_exact_for_four_bit_known_bits() {
    check_unary_exhaustive("not", KnownBits::not, |value| Some(value.not()));
    check_unary_exhaustive("abs", KnownBits::abs, |value| Some(signed_abs(value)));
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, BinaryExhaustive)`.
#[test]
fn bitwise_transfers_are_exact_for_four_bit_known_bits() {
    check_binary_exhaustive(
        "and",
        KnownBits::bitand,
        |lhs, rhs| Some(lhs.bitand(rhs)),
        true,
    );
    check_binary_exhaustive(
        "or",
        KnownBits::bitor,
        |lhs, rhs| Some(lhs.bitor(rhs)),
        true,
    );
    check_binary_exhaustive(
        "xor",
        KnownBits::bitxor,
        |lhs, rhs| Some(lhs.bitxor(rhs)),
        true,
    );
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, BinaryExhaustive)`.
#[test]
fn arithmetic_and_shift_transfers_are_sound_for_four_bit_known_bits() {
    check_binary_exhaustive(
        "add",
        KnownBits::add,
        |lhs, rhs| Some(lhs.wrapping_add(rhs)),
        true,
    );
    check_binary_exhaustive(
        "sub",
        KnownBits::sub,
        |lhs, rhs| Some(lhs.wrapping_sub(rhs)),
        true,
    );
    check_binary_exhaustive(
        "mul",
        KnownBits::mul,
        |lhs, rhs| Some(lhs.wrapping_mul(rhs)),
        false,
    );
    check_binary_exhaustive(
        "shl",
        KnownBits::shl,
        |lhs, rhs| lhs.checked_shl(shift_amount(rhs)?),
        true,
    );
    check_binary_exhaustive(
        "lshr",
        KnownBits::lshr,
        |lhs, rhs| lhs.checked_lshr(shift_amount(rhs)?),
        true,
    );
    check_binary_exhaustive(
        "ashr",
        KnownBits::ashr,
        |lhs, rhs| lhs.checked_ashr(shift_amount(rhs)?),
        true,
    );
    check_binary_exhaustive(
        "udiv",
        KnownBits::udiv,
        |lhs, rhs| lhs.checked_udiv(rhs),
        false,
    );
    check_binary_exhaustive(
        "urem",
        KnownBits::urem,
        |lhs, rhs| lhs.checked_urem(rhs),
        false,
    );
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, BinaryExhaustive)`.
#[test]
fn minmax_and_absolute_difference_transfers_are_exact_for_four_bit_known_bits() {
    check_binary_exhaustive(
        "umin",
        KnownBits::umin,
        |lhs, rhs| {
            Some(if lhs.ule(rhs) {
                lhs.clone()
            } else {
                rhs.clone()
            })
        },
        true,
    );
    check_binary_exhaustive(
        "umax",
        KnownBits::umax,
        |lhs, rhs| {
            Some(if lhs.uge(rhs) {
                lhs.clone()
            } else {
                rhs.clone()
            })
        },
        true,
    );
    check_binary_exhaustive(
        "smin",
        KnownBits::smin,
        |lhs, rhs| {
            Some(if lhs.sle(rhs) {
                lhs.clone()
            } else {
                rhs.clone()
            })
        },
        true,
    );
    check_binary_exhaustive(
        "smax",
        KnownBits::smax,
        |lhs, rhs| {
            Some(if lhs.sge(rhs) {
                lhs.clone()
            } else {
                rhs.clone()
            })
        },
        true,
    );
    check_binary_exhaustive(
        "abdu",
        KnownBits::abdu,
        |lhs, rhs| {
            Some(if lhs.uge(rhs) {
                lhs.wrapping_sub(rhs)
            } else {
                rhs.wrapping_sub(lhs)
            })
        },
        true,
    );
    check_binary_exhaustive(
        "abds",
        KnownBits::abds,
        |lhs, rhs| {
            Some(if lhs.sge(rhs) {
                lhs.wrapping_sub(rhs)
            } else {
                rhs.wrapping_sub(lhs)
            })
        },
        true,
    );
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, AddCarryExhaustive)`,
/// `TEST(KnownBitsTest, AddSubExhaustive)`, and `TEST(KnownBitsTest, SubBorrowExhaustive)`.
#[test]
fn add_carry_add_sub_and_borrow_match_upstream_exhaustive() {
    foreach_known_bits(4, |lhs| {
        foreach_known_bits(4, |rhs| {
            foreach_known_bits(1, |carry| {
                let mut outputs = Vec::new();
                for left in values_matching(&lhs) {
                    for right in values_matching(&rhs) {
                        for carry_value in values_matching(&carry) {
                            let carry_addend = if carry_value.bool_value() {
                                ApInt::from_words(4, &[1])
                            } else {
                                ApInt::zero(4)
                            };
                            outputs.push(left.wrapping_add(&right).wrapping_add(&carry_addend));
                        }
                    }
                }
                assert_known_result(
                    "add carry",
                    exact_known(4, outputs),
                    KnownBits::compute_for_add_carry(&lhs, &rhs, &carry),
                );
            });

            for add in [true, false] {
                for (nsw, nuw) in [(false, false), (true, false), (false, true), (true, true)] {
                    let mut outputs = Vec::new();
                    for left in values_matching(&lhs) {
                        for right in values_matching(&rhs) {
                            let (signed_result, signed_overflow) = if add {
                                left.sadd_ov(&right)
                            } else {
                                left.ssub_ov(&right)
                            };
                            let (_, unsigned_overflow) = if add {
                                left.uadd_ov(&right)
                            } else {
                                left.usub_ov(&right)
                            };
                            if (!nsw || !signed_overflow) && (!nuw || !unsigned_overflow) {
                                outputs.push(signed_result);
                            }
                        }
                    }
                    assert_known_result(
                        "add/sub flags",
                        exact_known(4, outputs),
                        KnownBits::compute_for_add_sub(add, nsw, nuw, &lhs, &rhs),
                    );
                }
            }

            foreach_known_bits(1, |borrow| {
                let mut outputs = Vec::new();
                for left in values_matching(&lhs) {
                    for right in values_matching(&rhs) {
                        for borrow_value in values_matching(&borrow) {
                            let borrow_subtrahend = if borrow_value.bool_value() {
                                ApInt::from_words(4, &[1])
                            } else {
                                ApInt::zero(4)
                            };
                            outputs
                                .push(left.wrapping_sub(&right).wrapping_sub(&borrow_subtrahend));
                        }
                    }
                }
                assert_known_result(
                    "sub borrow",
                    exact_known(4, outputs),
                    KnownBits::compute_for_sub_borrow(&lhs, rhs.clone(), &borrow),
                );
            });
        });
    });
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, WideShifts)`.
#[test]
fn wide_shifts_match_upstream_knownbits() {
    let width = 128;
    let unknown = KnownBits::unknown(width);
    let all_ones = KnownBits::make_constant(ApInt::all_ones(width));

    assert_eq!(
        KnownBits::shl(&all_ones, &unknown).to_string(),
        format!("1{}", "?".repeat(127))
    );
    assert_eq!(
        KnownBits::lshr(&all_ones, &unknown).to_string(),
        format!("{}1", "?".repeat(127))
    );
    assert_eq!(KnownBits::ashr(&all_ones, &unknown), all_ones);
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, ICmpExhaustive)`.
#[test]
fn icmp_results_match_upstream_exhaustive() {
    foreach_known_bits(4, |lhs| {
        foreach_known_bits(4, |rhs| {
            if lhs.has_conflict() || rhs.has_conflict() {
                return;
            }
            let mut all_eq = true;
            let mut none_eq = true;
            let mut all_ne = true;
            let mut none_ne = true;
            let mut all_ugt = true;
            let mut none_ugt = true;
            let mut all_uge = true;
            let mut none_uge = true;
            let mut all_ult = true;
            let mut none_ult = true;
            let mut all_ule = true;
            let mut none_ule = true;
            let mut all_sgt = true;
            let mut none_sgt = true;
            let mut all_sge = true;
            let mut none_sge = true;
            let mut all_slt = true;
            let mut none_slt = true;
            let mut all_sle = true;
            let mut none_sle = true;

            for left in values_matching(&lhs) {
                for right in values_matching(&rhs) {
                    all_eq &= left.eq_ap_int(&right);
                    none_eq &= !left.eq_ap_int(&right);
                    all_ne &= !left.eq_ap_int(&right);
                    none_ne &= left.eq_ap_int(&right);
                    all_ugt &= left.ugt(&right);
                    none_ugt &= !left.ugt(&right);
                    all_uge &= left.uge(&right);
                    none_uge &= !left.uge(&right);
                    all_ult &= left.ult(&right);
                    none_ult &= !left.ult(&right);
                    all_ule &= left.ule(&right);
                    none_ule &= !left.ule(&right);
                    all_sgt &= left.sgt(&right);
                    none_sgt &= !left.sgt(&right);
                    all_sge &= left.sge(&right);
                    none_sge &= !left.sge(&right);
                    all_slt &= left.slt(&right);
                    none_slt &= !left.slt(&right);
                    all_sle &= left.sle(&right);
                    none_sle &= !left.sle(&right);
                }
            }

            assert_eq!(
                KnownBits::eq(&lhs, &rhs),
                all_eq.then_some(true).or(none_eq.then_some(false))
            );
            assert_eq!(
                KnownBits::ne(&lhs, &rhs),
                all_ne.then_some(true).or(none_ne.then_some(false))
            );
            assert_eq!(
                KnownBits::ugt(&lhs, &rhs),
                all_ugt.then_some(true).or(none_ugt.then_some(false))
            );
            assert_eq!(
                KnownBits::uge(&lhs, &rhs),
                all_uge.then_some(true).or(none_uge.then_some(false))
            );
            assert_eq!(
                KnownBits::ult(&lhs, &rhs),
                all_ult.then_some(true).or(none_ult.then_some(false))
            );
            assert_eq!(
                KnownBits::ule(&lhs, &rhs),
                all_ule.then_some(true).or(none_ule.then_some(false))
            );
            assert_eq!(
                KnownBits::sgt(&lhs, &rhs),
                all_sgt.then_some(true).or(none_sgt.then_some(false))
            );
            assert_eq!(
                KnownBits::sge(&lhs, &rhs),
                all_sge.then_some(true).or(none_sge.then_some(false))
            );
            assert_eq!(
                KnownBits::slt(&lhs, &rhs),
                all_slt.then_some(true).or(none_slt.then_some(false))
            );
            assert_eq!(
                KnownBits::sle(&lhs, &rhs),
                all_sle.then_some(true).or(none_sle.then_some(false))
            );
        });
    });
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, SExtInReg)`,
/// `TEST(KnownBitsTest, CommonBitsSet)`, `TEST(KnownBitsTest, ConcatBits)`, and
/// `TEST(KnownBitsTest, ReduceAddExhaustive)`.
#[test]
fn structural_helpers_match_upstream_exhaustive() {
    foreach_known_bits(4, |known| {
        if !known.has_conflict() {
            for from_bits in 1..=4 {
                let ext_bits = 4 - from_bits;
                let outputs = values_matching(&known)
                    .into_iter()
                    .map(|value| value.shl(ext_bits).ashr(ext_bits))
                    .collect::<Vec<_>>();
                assert_known_result(
                    "sext in reg",
                    exact_known(4, outputs),
                    known.sext_in_reg(from_bits),
                );
            }

            for num_elts in [2, 4, 5] {
                let computed = known.reduce_add(num_elts);
                let exact = exact_reduce_add(4, &known, num_elts);
                assert!(
                    computed
                        .zero_mask()
                        .is_subset_of(exact.as_ref().expect("outputs").zero_mask()),
                    "reduce_add: input={known} computed={computed} exact={}",
                    exact.as_ref().expect("outputs")
                );
                assert!(
                    computed
                        .one_mask()
                        .is_subset_of(exact.as_ref().expect("outputs").one_mask()),
                    "reduce_add: input={known} computed={computed} exact={}",
                    exact.as_ref().expect("outputs")
                );
            }
        }
    });

    foreach_known_bits(4, |lhs| {
        foreach_known_bits(4, |rhs| {
            if !lhs.has_conflict() && !rhs.has_conflict() {
                let mut has_common = false;
                for left in values_matching(&lhs) {
                    for right in values_matching(&rhs) {
                        has_common |= left.intersects(&right);
                    }
                }
                assert_eq!(KnownBits::have_no_common_bits_set(&lhs, &rhs), !has_common);
            }
        });
    });

    for lo_bits in 1..4 {
        let hi_bits = 4 - lo_bits;
        foreach_known_bits(lo_bits, |lo| {
            foreach_known_bits(hi_bits, |hi| {
                let all = hi.concat(&lo);
                assert_eq!(all.extract_bits(lo_bits, 0), lo);
                assert_eq!(all.extract_bits(hi_bits, lo_bits), hi);
            });
        });
    }
}

/// Port of `llvm/unittests/Support/KnownBitsTest.cpp::TEST(KnownBitsTest, BinaryExhaustive)`,
/// `TEST(KnownBitsTest, UnaryExhaustive)`, and `TEST(KnownBitsTest, MulExhaustive)`.
#[test]
fn extended_unary_and_binary_transfers_match_upstream() {
    check_unary_exhaustive(
        "abs(true)",
        |known| known.abs_with_int_min_poison(true),
        |value| (!value.is_min_signed_value()).then(|| signed_abs(value)),
    );
    check_unary_exhaustive("blsi", KnownBits::blsi, |value| {
        Some(value.bitand(&value.negate()))
    });
    check_unary_exhaustive("blsmsk", KnownBits::blsmsk, |value| {
        Some(value.bitxor(&value.wrapping_sub(&ApInt::from_words(value.bit_width(), &[1]))))
    });

    check_binary_exhaustive(
        "sdiv",
        |lhs, rhs| KnownBits::sdiv_with_exact(lhs, rhs, false),
        ApInt::checked_sdiv,
        false,
    );
    check_binary_exhaustive(
        "sdiv exact",
        |lhs, rhs| KnownBits::sdiv_with_exact(lhs, rhs, true),
        |lhs, rhs| {
            let rem = lhs.checked_srem(rhs)?;
            rem.is_zero().then(|| lhs.checked_sdiv(rhs)).flatten()
        },
        false,
    );
    check_binary_exhaustive("srem", KnownBits::srem, ApInt::checked_srem, false);
    check_binary_exhaustive(
        "sadd_sat",
        KnownBits::sadd_sat,
        |lhs, rhs| Some(lhs.sadd_sat(rhs)),
        true,
    );
    check_binary_exhaustive(
        "uadd_sat",
        KnownBits::uadd_sat,
        |lhs, rhs| Some(lhs.uadd_sat(rhs)),
        true,
    );
    check_binary_exhaustive(
        "ssub_sat",
        KnownBits::ssub_sat,
        |lhs, rhs| Some(lhs.ssub_sat(rhs)),
        true,
    );
    check_binary_exhaustive(
        "usub_sat",
        KnownBits::usub_sat,
        |lhs, rhs| Some(lhs.usub_sat(rhs)),
        true,
    );
    check_binary_exhaustive(
        "avgFloorS",
        KnownBits::avg_floor_s,
        |lhs, rhs| Some(avg_floor_signed(lhs.bit_width(), lhs, rhs)),
        false,
    );
    check_binary_exhaustive(
        "avgFloorU",
        KnownBits::avg_floor_u,
        |lhs, rhs| {
            let sum = lhs.try_zext_u64()?.checked_add(rhs.try_zext_u64()?)?;
            Some(ap(lhs.bit_width(), sum / 2))
        },
        false,
    );
    check_binary_exhaustive(
        "avgCeilS",
        KnownBits::avg_ceil_s,
        |lhs, rhs| Some(avg_ceil_signed(lhs.bit_width(), lhs, rhs)),
        false,
    );
    check_binary_exhaustive(
        "avgCeilU",
        KnownBits::avg_ceil_u,
        |lhs, rhs| {
            let sum = lhs
                .try_zext_u64()?
                .checked_add(rhs.try_zext_u64()?)?
                .checked_add(1)?;
            Some(ap(lhs.bit_width(), sum / 2))
        },
        false,
    );
    check_binary_exhaustive(
        "mulhs",
        KnownBits::mulhs,
        |lhs, rhs| Some(mul_high_signed(lhs.bit_width(), lhs, rhs)),
        false,
    );
    check_binary_exhaustive(
        "mulhu",
        KnownBits::mulhu,
        |lhs, rhs| Some(mul_high_unsigned(lhs.bit_width(), lhs, rhs)),
        false,
    );
}

/// Mirrors `llvm/lib/Support/KnownBits.cpp::computeForSatAddSub`: unsigned
/// add saturation clamps to all ones when overflow is guaranteed.
#[test]
fn uadd_sat_known_overflow_clamps_to_all_ones_without_enumeration() {
    let sign = ApInt::one_bit_set(64, 63);
    let lhs = KnownBits::from_zero_one(ApInt::zero(64), sign.clone()).unwrap();
    let rhs = KnownBits::from_zero_one(ApInt::zero(64), sign).unwrap();
    let known = KnownBits::uadd_sat(&lhs, &rhs);
    assert_eq!(known.one_mask(), &ApInt::all_ones(64));
    assert_eq!(known.zero_mask(), &ApInt::zero(64));
}

/// Mirrors `llvm/lib/Support/KnownBits.cpp::computeForSatAddSub`: unsigned
/// sub saturation clamps to zero when underflow is guaranteed.
#[test]
fn usub_sat_known_underflow_clamps_to_zero_without_enumeration() {
    let sign = ApInt::one_bit_set(64, 63);
    let lhs = KnownBits::from_zero_one(sign.clone(), ApInt::zero(64)).unwrap();
    let rhs = KnownBits::from_zero_one(ApInt::zero(64), sign).unwrap();
    let known = KnownBits::usub_sat(&lhs, &rhs);
    assert_eq!(known.one_mask(), &ApInt::zero(64));
    assert_eq!(known.zero_mask(), &ApInt::all_ones(64));
}

/// Mirrors `llvm/lib/Support/KnownBits.cpp::computeForSatAddSub`: signed
/// add saturation clamps to signed max when positive overflow is guaranteed.
#[test]
fn sadd_sat_known_positive_overflow_clamps_to_signed_max_without_enumeration() {
    let sign = ApInt::one_bit_set(64, 63);
    let next = ApInt::one_bit_set(64, 62);
    let lhs = KnownBits::from_zero_one(sign.clone(), next.clone()).unwrap();
    let rhs = KnownBits::from_zero_one(sign.clone(), next).unwrap();
    let known = KnownBits::sadd_sat(&lhs, &rhs);
    assert_eq!(known.one_mask(), &ApInt::signed_max_value(64));
    assert_eq!(known.zero_mask(), &sign);
}

/// Mirrors `llvm/lib/Support/KnownBits.cpp::computeForSatAddSub`: signed
/// sub saturation clamps to signed min when negative overflow is guaranteed.
#[test]
fn ssub_sat_known_negative_overflow_clamps_to_signed_min_without_enumeration() {
    let sign = ApInt::one_bit_set(64, 63);
    let next = ApInt::one_bit_set(64, 62);
    let lhs = KnownBits::from_zero_one(next.clone(), sign.clone()).unwrap();
    let rhs = KnownBits::from_zero_one(sign.clone(), next).unwrap();
    let known = KnownBits::ssub_sat(&lhs, &rhs);
    assert_eq!(known.one_mask(), &sign);
    assert_eq!(known.zero_mask(), &ApInt::signed_max_value(64));
}
