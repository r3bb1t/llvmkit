//! Ports of the floating-point classification analysis — tranche 7 of the
//! `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! Upstream has no unit-test file for `computeKnownFPClass`; it is exercised
//! through InstCombine's `.ll` fixtures, chiefly
//! `llvm/test/Transforms/InstCombine/known-fpclass-*.ll` and
//! `llvm/test/Analysis/ValueTracking/known-fpclass.ll`. The tests here read the
//! predicate directly, and each says which upstream arm it is covering.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, FpClassTest, Module, Unverified, Value, ValueTrackingQuery, cannot_be_negative_zero,
    cannot_be_ordered_less_than_zero, compute_known_fp_class, compute_known_fp_class_all,
    compute_known_fp_sign_bit, is_known_never_infinity, is_known_never_infinity_or_nan,
    is_known_never_nan,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The instruction named `%name`.
fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|candidate| candidate.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

/// A constant leaf pins the class and the sign exactly — the `ConstantFP` arm
/// of `computeKnownFPClass`.
#[test]
fn a_float_constant_is_classified_exactly() {
    // `fneg` of a constant reaches the constant leaf through the one unary arm
    // that is ported, so the constant's own classification is observable.
    let module = parse(
        r"
define void @constants() {
  %from_positive = fneg float 1.0
  %from_negative = fneg float -1.0
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    // `fneg 1.0` — the leaf says positive-normal, sign clear; `fneg` flips both.
    let from_positive = compute_known_fp_class_all(named(&module, "from_positive"), &query);
    assert_eq!(from_positive.classes(), FpClassTest::NEGATIVE_NORMAL);
    assert_eq!(from_positive.sign_bit(), Some(true));
    assert!(from_positive.is_known_never_nan());
    assert!(from_positive.is_known_never_infinity());

    // ... and the other way round.
    let from_negative = compute_known_fp_class_all(named(&module, "from_negative"), &query);
    assert_eq!(from_negative.classes(), FpClassTest::POSITIVE_NORMAL);
    assert_eq!(from_negative.sign_bit(), Some(false));
}

/// The `FNeg` arm: negating moves the class to its opposite sign and flips the
/// sign bit.
#[test]
fn fneg_moves_the_class_and_the_sign() {
    let module = parse(
        r"
define float @negate(float %x) {
  %positive = call float @llvm.fabs.f32(float %x)
  %negated = fneg float %positive
  ret float %negated
}

declare float @llvm.fabs.f32(float)
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    // `fabs` forces the sign bit clear.
    let absolute = compute_known_fp_class_all(named(&module, "positive"), &query);
    assert_eq!(absolute.sign_bit(), Some(false));
    assert!(absolute.is_known_never(FpClassTest::NEGATIVE));

    // `fneg` of it forces the sign bit set.
    let negated = compute_known_fp_class_all(named(&module, "negated"), &query);
    assert_eq!(negated.sign_bit(), Some(true));
    assert!(negated.is_known_never(FpClassTest::POSITIVE));
    assert_eq!(
        compute_known_fp_sign_bit(named(&module, "negated"), &query),
        Some(true)
    );
}

/// The `sqrt` intrinsic arm over `KnownFPClass::sqrt`: the only negative value
/// it can return is `-0`, so it is never ordered less than zero.
#[test]
fn sqrt_is_never_ordered_less_than_zero() {
    let module = parse(
        r"
define float @root(float %x) {
  %r = call float @llvm.sqrt.f32(float %x)
  ret float %r
}

declare float @llvm.sqrt.f32(float)
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);
    let root = named(&module, "r");

    let known = compute_known_fp_class_all(root, &query);
    assert!(known.is_known_never(FpClassTest::NEGATIVE_NORMAL));
    assert!(known.is_known_never(FpClassTest::NEGATIVE_INFINITY));
    assert!(cannot_be_ordered_less_than_zero(root, &query));
}

/// The fast-math-flag refinement, which upstream applies on the way out of
/// every arm through a `scope_exit` — so it holds even for an arm that learns
/// nothing on its own.
#[test]
fn fast_math_flags_rule_out_nan_and_infinity() {
    let module = parse(
        r"
define float @flagged(float %x, float %y) {
  %plain = fadd float %x, %y
  %no_nan = fadd nnan float %x, %y
  %no_inf = fadd ninf float %x, %y
  %neither = fadd nnan ninf float %x, %y
  ret float %plain
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    // The `fadd` arm itself is not ported, so without flags nothing is known.
    assert!(!is_known_never_nan(named(&module, "plain"), &query));
    assert!(!is_known_never_infinity(named(&module, "plain"), &query));

    assert!(is_known_never_nan(named(&module, "no_nan"), &query));
    assert!(!is_known_never_infinity(named(&module, "no_nan"), &query));

    assert!(is_known_never_infinity(named(&module, "no_inf"), &query));
    assert!(!is_known_never_nan(named(&module, "no_inf"), &query));

    assert!(is_known_never_infinity_or_nan(
        named(&module, "neither"),
        &query
    ));
}

/// The `uitofp` / `sitofp` arm. Neither can produce a NaN or a subnormal, and
/// both turn a zero into `+0`; `uitofp` additionally forces the sign clear.
///
/// The infinity test is upstream's exponent comparison,
/// `ilogb(getLargest(FPTy)) >= IntSize`. It is easy to get backwards: `float`
/// *can* hold every `i64` without overflowing, because its largest finite value
/// has exponent 127 and the widest `u64` needs 64 — so that pairing rules
/// infinity out. `half` tops out at exponent 15, so that is where infinity
/// survives.
#[test]
fn integer_to_float_conversions() {
    let module = parse(
        r"
define void @conversions(i8 %narrow, i64 %wide) {
  %unsigned = uitofp i8 %narrow to float
  %signed = sitofp i8 %narrow to float
  %wide_unsigned = uitofp i64 %wide to float
  %overflowing = uitofp i64 %wide to half
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let unsigned = compute_known_fp_class_all(named(&module, "unsigned"), &query);
    assert!(unsigned.is_known_never_nan());
    assert!(unsigned.is_known_never_subnormal());
    assert!(unsigned.is_known_never_negative_zero());
    assert_eq!(unsigned.sign_bit(), Some(false));
    // Every `i8` fits in a `float`, so the result is finite.
    assert!(unsigned.is_known_never_infinity());

    let signed = compute_known_fp_class_all(named(&module, "signed"), &query);
    assert!(signed.is_known_never_nan());
    assert!(signed.is_known_never_infinity());
    // `sitofp` says nothing about the sign.
    assert_eq!(signed.sign_bit(), None);
    assert!(cannot_be_negative_zero(named(&module, "signed"), &query));

    // Every `u64` still fits inside `float`'s exponent range.
    let wide = compute_known_fp_class_all(named(&module, "wide_unsigned"), &query);
    assert!(wide.is_known_never_infinity());

    // `half` tops out well below `2^64`, so infinity survives there.
    let overflowing = compute_known_fp_class_all(named(&module, "overflowing"), &query);
    assert!(!overflowing.is_known_never_infinity());
    // The rest of the arm still holds.
    assert!(overflowing.is_known_never_nan());
    assert!(overflowing.is_known_never_subnormal());
}

/// The `interested_classes` hint. Upstream's contract — "Queries not specified
/// in `InterestedClasses` should be reliable if they are determined during the
/// query" — means asking for less may answer less, never something false.
#[test]
fn narrowing_the_interest_never_answers_something_false() {
    let module = parse(
        r"
define float @root(float %x) {
  %r = call float @llvm.sqrt.f32(float %x)
  ret float %r
}

declare float @llvm.sqrt.f32(float)
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);
    let root = named(&module, "r");

    let everything = compute_known_fp_class_all(root, &query);
    let narrow = compute_known_fp_class(root, FpClassTest::NAN, &query);

    // Whatever the narrow query rules out, the full one rules out too.
    assert!(
        everything
            .classes()
            .contains(everything.classes().intersection(narrow.classes()))
    );
    assert!(narrow.classes().contains(everything.classes()));
}
