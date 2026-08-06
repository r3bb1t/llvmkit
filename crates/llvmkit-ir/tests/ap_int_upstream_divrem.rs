//! Ports of the `APInt` division and remainder tests from
//! `llvm/unittests/ADT/APIntTest.cpp` in the vendored `llvmorg-22.1.4` tree.
//!
//! Upstream's `testDiv` helpers check `(a * b + c) / a` through every div/rem
//! entry point; the rows that follow are chosen to drive the rare branches of
//! upstream's Knuth algorithm D. llvmkit's `udivrem` is a schoolbook
//! bit-at-a-time long division rather than Knuth D, so the rows do not exercise
//! the *same internal steps* — they do assert the same answers at the same
//! magnitudes, which is what the fixture is for.
//!
//! Two spelling differences, neither of which changes the logic:
//!
//! - Upstream's `q`/`r` are out-parameters and its scalar overloads
//!   (`udiv(uint64_t)`, `srem(int64_t)`) return machine words. llvmkit returns
//!   `Option<ApInt>` / `Option<ApIntDivRem>` and has no scalar overload, so the
//!   scalar divisor is built as a same-width `ApInt` — which is what upstream's
//!   overloads construct internally — and the comparisons are full-width.
//! - Where upstream writes `EXPECT_EQ(-c, -r)` it is working around
//!   `operator==(APInt, uint64_t)`, which cannot represent a negative value.
//!   The negation is kept so the port reads against the same text.

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

fn parse(bit_width: u32, text: &str, radix: u8) -> ApInt {
    ApInt::from_string(bit_width, text, radix)
        .unwrap_or_else(|e| panic!("upstream spells {text:?} in radix {radix}: {e}"))
}

fn udiv(lhs: &ApInt, rhs: &ApInt) -> ApInt {
    lhs.checked_udiv(rhs).expect("divisor is non-zero")
}

fn urem(lhs: &ApInt, rhs: &ApInt) -> ApInt {
    lhs.checked_urem(rhs).expect("divisor is non-zero")
}

fn sdiv(lhs: &ApInt, rhs: &ApInt) -> ApInt {
    lhs.checked_sdiv(rhs).expect("divisor is non-zero")
}

fn srem(lhs: &ApInt, rhs: &ApInt) -> ApInt {
    lhs.checked_srem(rhs).expect("divisor is non-zero")
}

fn udivrem(lhs: &ApInt, rhs: &ApInt) -> (ApInt, ApInt) {
    lhs.udivrem(rhs).expect("divisor is non-zero").into_parts()
}

fn sdivrem(lhs: &ApInt, rhs: &ApInt) -> (ApInt, ApInt) {
    lhs.sdivrem(rhs).expect("divisor is non-zero").into_parts()
}

/// Port of the `testDiv(APInt a, APInt b, APInt c)` helper: checks the
/// different div/rem variants using the scheme `(a * b + c) / a`.
fn test_div(a: &ApInt, b: &ApInt, c: &ApInt) {
    assert!(a.uge(b), "upstream asserts a >= b");
    assert!(a.ugt(c), "upstream asserts a > c");

    let p = a.wrapping_mul(b).wrapping_add(c);

    let q = udiv(&p, a);
    let r = urem(&p, a);
    assert_eq!(*b, q);
    assert_eq!(*c, r);
    let (q, r) = udivrem(&p, a);
    assert_eq!(*b, q);
    assert_eq!(*c, r);
    let q = sdiv(&p, a);
    let r = srem(&p, a);
    assert_eq!(*b, q);
    assert_eq!(*c, r);
    let (q, r) = sdivrem(&p, a);
    assert_eq!(*b, q);
    assert_eq!(*c, r);

    if b.ugt(c) {
        // Test also symmetric case.
        let q = udiv(&p, b);
        let r = urem(&p, b);
        assert_eq!(*a, q);
        assert_eq!(*c, r);
        let (q, r) = udivrem(&p, b);
        assert_eq!(*a, q);
        assert_eq!(*c, r);
        let q = sdiv(&p, b);
        let r = srem(&p, b);
        assert_eq!(*a, q);
        assert_eq!(*c, r);
        let (q, r) = sdivrem(&p, b);
        assert_eq!(*a, q);
        assert_eq!(*c, r);
    }
}

/// Port of the `testDiv(APInt a, uint64_t b, APInt c)` overload.
fn test_div_word(a: &ApInt, b: u64, c: &ApInt) {
    let b = unsigned(a.bit_width(), b);
    let p = a.wrapping_mul(&b).wrapping_add(c);

    // Unsigned division will only work if our original number wasn't negative.
    if !a.is_negative() {
        let q = udiv(&p, &b);
        let r = urem(&p, &b);
        assert_eq!(*a, q);
        assert_eq!(*c, r);
        let (q, r) = udivrem(&p, &b);
        assert_eq!(*a, q);
        assert_eq!(*c, r);
    }
    let q = sdiv(&p, &b);
    let r = srem(&p, &b);
    assert_eq!(*a, q);
    if c.is_negative() {
        // Need to negate so the uint64_t compare will work.
        assert_eq!(c.negate(), r.negate());
    } else {
        assert_eq!(*c, r);
    }
    let (q, r) = sdivrem(&p, &b);
    assert_eq!(*a, q);
    if c.is_negative() {
        assert_eq!(c.negate(), r.negate());
    } else {
        assert_eq!(*c, r);
    }
}

/// Port of `TEST(APIntTest, divrem_big1)` — tests KnuthDiv rare step D6.
#[test]
fn divrem_big1() {
    test_div(
        &parse(256, "1ffffffffffffffff", 16),
        &parse(256, "1ffffffffffffffff", 16),
        &unsigned(256, 0),
    );
}

/// Port of `TEST(APIntTest, divrem_big2)` — tests KnuthDiv rare step D6.
#[test]
fn divrem_big2() {
    test_div(
        &parse(
            1024,
            concat!(
                "112233ceff",
                "cecece000000ffffffffffffffffffff",
                "ffffffffffffffffffffffffffffffff",
                "ffffffffffffffffffffffffffffffff",
                "ffffffffffffffffffffffffffffff33",
            ),
            16,
        ),
        &parse(
            1024,
            concat!(
                "111111ffffffffffffffff",
                "ffffffffffffffffffffffffffffffff",
                "fffffffffffffffffffffffffffffccf",
                "ffffffffffffffffffffffffffffff00",
            ),
            16,
        ),
        &unsigned(1024, 7919),
    );
}

/// Port of `TEST(APIntTest, divrem_big3)` — tests the KnuthDiv case without
/// shift.
#[test]
fn divrem_big3() {
    test_div(
        &parse(256, "80000001ffffffffffffffff", 16),
        &parse(256, "ffffffffffffff0000000", 16),
        &unsigned(256, 4219),
    );
}

/// Port of `TEST(APIntTest, divrem_big4)` — tests heap allocation in `divide()`
/// enforced by huge numbers.
#[test]
fn divrem_big4() {
    test_div(
        &unsigned(4096, 5).shl(2001),
        &unsigned(4096, 1).shl(2000),
        &unsigned(4096, 4219 * 13),
    );
}

/// Port of `TEST(APIntTest, divrem_big5)` — tests the one-word divisor case of
/// `divide()`.
#[test]
fn divrem_big5() {
    test_div(
        &unsigned(1024, 19).shl(811),
        &unsigned(1024, 4356013), // one word
        &unsigned(1024, 1),
    );
}

/// Port of `TEST(APIntTest, divrem_big6)` — tests some rare "borrow" cases in
/// the D4 step.
#[test]
fn divrem_big6() {
    test_div(
        &parse(512, "ffffffffffffffff00000000000000000000000001", 16),
        &parse(512, "10000000000000001000000000000001", 16),
        &parse(512, "10000000000000000000000000000000", 16),
    );
}

/// Port of `TEST(APIntTest, divrem_big7)` — yet another test for KnuthDiv rare
/// step D6.
#[test]
fn divrem_big7() {
    test_div(
        &parse(224, "800000008000000200000005", 16),
        &parse(224, "fffffffd", 16),
        &parse(224, "80000000800000010000000f", 16),
    );
}

/// Port of `TEST(APIntTest, divremuint)`.
#[test]
fn divremuint() {
    // Single word APInt.
    test_div_word(&unsigned(64, 9), 2, &unsigned(64, 1));

    // Single word negative APInt.
    test_div_word(&unsigned(64, 9).negate(), 2, &unsigned(64, 1).negate());

    // Multiword dividend with only one significant word.
    test_div_word(&unsigned(256, 9), 2, &unsigned(256, 1));

    // Negative dividend.
    test_div_word(&unsigned(256, 9).negate(), 2, &unsigned(256, 1).negate());

    // Multiword dividend.
    test_div_word(
        &unsigned(1024, 19).shl(811),
        4356013, // one word
        &unsigned(1024, 1),
    );
}

/// Port of `TEST(APIntTest, divrem_simple)`.
#[test]
fn divrem_simple() {
    // Test simple cases.
    let a = unsigned(65, 2);
    let b = unsigned(65, 2);

    // X / X
    let (q, r) = sdivrem(&a, &b);
    assert_eq!(q, unsigned(65, 1));
    assert_eq!(r, unsigned(65, 0));
    let (q, r) = udivrem(&a, &b);
    assert_eq!(q, unsigned(65, 1));
    assert_eq!(r, unsigned(65, 0));

    // 0 / X
    let o = unsigned(65, 0);
    let (q, r) = sdivrem(&o, &b);
    assert_eq!(q, unsigned(65, 0));
    assert_eq!(r, unsigned(65, 0));
    let (q, r) = udivrem(&o, &b);
    assert_eq!(q, unsigned(65, 0));
    assert_eq!(r, unsigned(65, 0));

    // X / 1
    let i = unsigned(65, 1);
    let (q, r) = sdivrem(&a, &i);
    assert_eq!(q, a);
    assert_eq!(r, unsigned(65, 0));
    let (q, r) = udivrem(&a, &i);
    assert_eq!(q, a);
    assert_eq!(r, unsigned(65, 0));
}
