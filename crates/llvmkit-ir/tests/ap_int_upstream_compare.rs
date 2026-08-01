//! Ports of the `APInt` comparison tests from
//! `llvm/unittests/ADT/APIntTest.cpp` in the vendored `llvmorg-22.1.4` tree.
//!
//! Upstream carries sixteen comparison entry points: `ult`/`ule`/`ugt`/`uge`
//! and `slt`/`sle`/`sgt`/`sge`, each once against an `APInt` and once against a
//! machine word. llvmkit answers all sixteen questions with four total
//! orderings — `unsigned_cmp`, `signed_cmp`, `unsigned_cmp_u64`,
//! `signed_cmp_i64` — so each upstream row is ported as the same comparison
//! read off the corresponding `Ordering`. The predicate being asserted, and the
//! value it is asserted against, are unchanged.

use core::cmp::Ordering;

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

/// Port of `TEST(APIntTest, compare)`.
#[test]
fn compare() {
    let test_values = [
        unsigned(16, 2),
        unsigned(16, 1),
        unsigned(16, 0),
        signed(16, -1),
        signed(16, -2),
    ];

    for arg1 in &test_values {
        for arg2 in &test_values {
            let uv1 = arg1.try_zext_u64().expect("16 bits fit");
            let uv2 = arg2.try_zext_u64().expect("16 bits fit");
            let sv1 = arg1.try_sext_i64().expect("16 bits fit");
            let sv2 = arg2.try_sext_i64().expect("16 bits fit");

            assert_eq!(uv1 < uv2, arg1.ult(arg2));
            assert_eq!(uv1 <= uv2, arg1.ule(arg2));
            assert_eq!(uv1 > uv2, arg1.ugt(arg2));
            assert_eq!(uv1 >= uv2, arg1.uge(arg2));

            assert_eq!(sv1 < sv2, arg1.slt(arg2));
            assert_eq!(sv1 <= sv2, arg1.sle(arg2));
            assert_eq!(sv1 > sv2, arg1.sgt(arg2));
            assert_eq!(sv1 >= sv2, arg1.sge(arg2));

            // The same eight questions, now against the machine word — the
            // rows upstream spells with its scalar overloads.
            assert_eq!(uv1 < uv2, arg1.unsigned_cmp_u64(uv2).is_lt());
            assert_eq!(uv1 <= uv2, arg1.unsigned_cmp_u64(uv2).is_le());
            assert_eq!(uv1 > uv2, arg1.unsigned_cmp_u64(uv2).is_gt());
            assert_eq!(uv1 >= uv2, arg1.unsigned_cmp_u64(uv2).is_ge());

            assert_eq!(sv1 < sv2, arg1.signed_cmp_i64(sv2).is_lt());
            assert_eq!(sv1 <= sv2, arg1.signed_cmp_i64(sv2).is_le());
            assert_eq!(sv1 > sv2, arg1.signed_cmp_i64(sv2).is_gt());
            assert_eq!(sv1 >= sv2, arg1.signed_cmp_i64(sv2).is_ge());

            // The `ApInt`-to-`ApInt` orderings answer the same questions.
            assert_eq!(uv1.cmp(&uv2), arg1.unsigned_cmp(arg2));
            assert_eq!(sv1.cmp(&sv2), arg1.signed_cmp(arg2));
        }
    }
}

/// Port of `TEST(APIntTest, compareWithRawIntegers)`.
#[test]
fn compare_with_raw_integers() {
    assert!(!unsigned(8, 1).unsigned_cmp_u64(256).is_ge());
    assert!(!unsigned(8, 1).unsigned_cmp_u64(256).is_gt());
    assert!(unsigned(8, 1).unsigned_cmp_u64(256).is_le());
    assert!(unsigned(8, 1).unsigned_cmp_u64(256).is_lt());
    assert!(!unsigned(8, 1).signed_cmp_i64(256).is_ge());
    assert!(!unsigned(8, 1).signed_cmp_i64(256).is_gt());
    assert!(unsigned(8, 1).signed_cmp_i64(256).is_le());
    assert!(unsigned(8, 1).signed_cmp_i64(256).is_lt());
    assert!(unsigned(8, 0).unsigned_cmp_u64(256) != Ordering::Equal);
    assert!(unsigned(8, 1).unsigned_cmp_u64(256) != Ordering::Equal);

    let uint64max = u64::MAX;
    let int64max = i64::MAX;
    let int64min = i64::MIN;

    let u64_value = unsigned(128, uint64max);
    let s64_value = signed(128, int64max);
    let big = u64_value.wrapping_add(&unsigned(128, 1));

    assert!(u64_value.unsigned_cmp_u64(uint64max).is_ge());
    assert!(!u64_value.unsigned_cmp_u64(uint64max).is_gt());
    assert!(u64_value.unsigned_cmp_u64(uint64max).is_le());
    assert!(!u64_value.unsigned_cmp_u64(uint64max).is_lt());
    assert!(u64_value.signed_cmp_i64(int64max).is_ge());
    assert!(u64_value.signed_cmp_i64(int64max).is_gt());
    assert!(!u64_value.signed_cmp_i64(int64max).is_le());
    assert!(!u64_value.signed_cmp_i64(int64max).is_lt());
    assert!(u64_value.signed_cmp_i64(int64min).is_ge());
    assert!(u64_value.signed_cmp_i64(int64min).is_gt());
    assert!(!u64_value.signed_cmp_i64(int64min).is_le());
    assert!(!u64_value.signed_cmp_i64(int64min).is_lt());

    assert_eq!(u64_value.unsigned_cmp_u64(uint64max), Ordering::Equal);
    assert!(u64_value.signed_cmp_i64(int64max) != Ordering::Equal);
    assert!(u64_value.signed_cmp_i64(int64min) != Ordering::Equal);

    assert!(!s64_value.unsigned_cmp_u64(uint64max).is_ge());
    assert!(!s64_value.unsigned_cmp_u64(uint64max).is_gt());
    assert!(s64_value.unsigned_cmp_u64(uint64max).is_le());
    assert!(s64_value.unsigned_cmp_u64(uint64max).is_lt());
    assert!(s64_value.signed_cmp_i64(int64max).is_ge());
    assert!(!s64_value.signed_cmp_i64(int64max).is_gt());
    assert!(s64_value.signed_cmp_i64(int64max).is_le());
    assert!(!s64_value.signed_cmp_i64(int64max).is_lt());
    assert!(s64_value.signed_cmp_i64(int64min).is_ge());
    assert!(s64_value.signed_cmp_i64(int64min).is_gt());
    assert!(!s64_value.signed_cmp_i64(int64min).is_le());
    assert!(!s64_value.signed_cmp_i64(int64min).is_lt());

    assert!(s64_value.unsigned_cmp_u64(uint64max) != Ordering::Equal);
    assert_eq!(s64_value.signed_cmp_i64(int64max), Ordering::Equal);
    assert!(s64_value.signed_cmp_i64(int64min) != Ordering::Equal);

    assert!(big.unsigned_cmp_u64(uint64max).is_ge());
    assert!(big.unsigned_cmp_u64(uint64max).is_gt());
    assert!(!big.unsigned_cmp_u64(uint64max).is_le());
    assert!(!big.unsigned_cmp_u64(uint64max).is_lt());
    assert!(big.signed_cmp_i64(int64max).is_ge());
    assert!(big.signed_cmp_i64(int64max).is_gt());
    assert!(!big.signed_cmp_i64(int64max).is_le());
    assert!(!big.signed_cmp_i64(int64max).is_lt());
    assert!(big.signed_cmp_i64(int64min).is_ge());
    assert!(big.signed_cmp_i64(int64min).is_gt());
    assert!(!big.signed_cmp_i64(int64min).is_le());
    assert!(!big.signed_cmp_i64(int64min).is_lt());

    assert!(big.unsigned_cmp_u64(uint64max) != Ordering::Equal);
    assert!(big.signed_cmp_i64(int64max) != Ordering::Equal);
    assert!(big.signed_cmp_i64(int64min) != Ordering::Equal);
}

/// Port of `TEST(APIntTest, compareWithInt64Min)`.
#[test]
fn compare_with_int64_min() {
    let edge = i64::MIN;
    let edge_plus_one = edge + 1;
    let edge_minus_one = i64::MAX;
    let a = signed(64, edge);

    assert!(!a.signed_cmp_i64(edge).is_lt());
    assert!(a.signed_cmp_i64(edge).is_le());
    assert!(!a.signed_cmp_i64(edge).is_gt());
    assert!(a.signed_cmp_i64(edge).is_ge());
    assert!(a.signed_cmp_i64(edge_plus_one).is_lt());
    assert!(a.signed_cmp_i64(edge_plus_one).is_le());
    assert!(!a.signed_cmp_i64(edge_plus_one).is_gt());
    assert!(!a.signed_cmp_i64(edge_plus_one).is_ge());
    assert!(a.signed_cmp_i64(edge_minus_one).is_lt());
    assert!(a.signed_cmp_i64(edge_minus_one).is_le());
    assert!(!a.signed_cmp_i64(edge_minus_one).is_gt());
    assert!(!a.signed_cmp_i64(edge_minus_one).is_ge());
}

/// Port of `TEST(APIntTest, compareWithHalfInt64Max)`.
#[test]
fn compare_with_half_int64_max() {
    let edge = 0x4000_0000_0000_0000u64;
    let edge_plus_one = edge + 1;
    let edge_minus_one = edge - 1;
    let a = unsigned(64, edge);

    assert!(!a.unsigned_cmp_u64(edge).is_lt());
    assert!(a.unsigned_cmp_u64(edge).is_le());
    assert!(!a.unsigned_cmp_u64(edge).is_gt());
    assert!(a.unsigned_cmp_u64(edge).is_ge());
    assert!(a.unsigned_cmp_u64(edge_plus_one).is_lt());
    assert!(a.unsigned_cmp_u64(edge_plus_one).is_le());
    assert!(!a.unsigned_cmp_u64(edge_plus_one).is_gt());
    assert!(!a.unsigned_cmp_u64(edge_plus_one).is_ge());
    assert!(!a.unsigned_cmp_u64(edge_minus_one).is_lt());
    assert!(!a.unsigned_cmp_u64(edge_minus_one).is_le());
    assert!(a.unsigned_cmp_u64(edge_minus_one).is_gt());
    assert!(a.unsigned_cmp_u64(edge_minus_one).is_ge());

    // Upstream passes the same `uint64_t` to the signed overloads, where the
    // implicit conversion makes every one of these three values positive.
    let as_signed = |value: u64| i64::from_ne_bytes(value.to_ne_bytes());
    assert!(!a.signed_cmp_i64(as_signed(edge)).is_lt());
    assert!(a.signed_cmp_i64(as_signed(edge)).is_le());
    assert!(!a.signed_cmp_i64(as_signed(edge)).is_gt());
    assert!(a.signed_cmp_i64(as_signed(edge)).is_ge());
    assert!(a.signed_cmp_i64(as_signed(edge_plus_one)).is_lt());
    assert!(a.signed_cmp_i64(as_signed(edge_plus_one)).is_le());
    assert!(!a.signed_cmp_i64(as_signed(edge_plus_one)).is_gt());
    assert!(!a.signed_cmp_i64(as_signed(edge_plus_one)).is_ge());
    assert!(!a.signed_cmp_i64(as_signed(edge_minus_one)).is_lt());
    assert!(!a.signed_cmp_i64(as_signed(edge_minus_one)).is_le());
    assert!(a.signed_cmp_i64(as_signed(edge_minus_one)).is_gt());
    assert!(a.signed_cmp_i64(as_signed(edge_minus_one)).is_ge());
}

/// Port of `TEST(APIntTest, compareLargeIntegers)`.
#[test]
fn compare_large_integers() {
    // Make sure all the combinations of signed comparisons work with big ints.
    let one = signed(128, 1);
    let two = signed(128, 2);
    let minus_one = signed(128, -1);
    let minus_two = signed(128, -2);

    assert!(!one.slt(&one));
    assert!(!two.slt(&one));
    assert!(minus_one.slt(&one));
    assert!(minus_two.slt(&one));

    assert!(one.slt(&two));
    assert!(!two.slt(&two));
    assert!(minus_one.slt(&two));
    assert!(minus_two.slt(&two));

    assert!(!one.slt(&minus_one));
    assert!(!two.slt(&minus_one));
    assert!(!minus_one.slt(&minus_one));
    assert!(minus_two.slt(&minus_one));

    assert!(!one.slt(&minus_two));
    assert!(!two.slt(&minus_two));
    assert!(!minus_one.slt(&minus_two));
    assert!(!minus_two.slt(&minus_two));
}
