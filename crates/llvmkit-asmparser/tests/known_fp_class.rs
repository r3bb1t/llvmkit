//! Ports of the floating-point classification analysis — tranche 7 of the
//! `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! Upstream's unit tests live in the `ComputeKnownFPClassTest` fixture of
//! `llvm/unittests/Analysis/ValueTrackingTest.cpp`, whose `expectKnownFPClass`
//! helper asserts the class mask and the sign bit; the dominating-condition
//! cases come from `llvm/test/Transforms/InstCombine/fpclass-from-dom-cond.ll`.
//! Every test here names the case it ports.
//!
//! An earlier revision of this file claimed upstream had no unit tests for
//! `computeKnownFPClass` and wrote its own expectations. That was wrong; those
//! tests are now ports.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DomConditionCache, DominatorTree, DynBrand, FpClassTest, FunctionView, InstructionView, Module,
    Unverified, Value, ValueTrackingQuery, compute_known_fp_class, compute_known_fp_class_all,
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

/// The instruction named `%name`, as a view.
fn instruction<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> InstructionView<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|candidate| candidate.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
}

/// The first function with a body.
fn defined_function<'m>(module: &'m Module<DynBrand, Unverified>) -> FunctionView<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .find(|function| function.basic_blocks().next().is_some())
        .expect("fixture defines a function")
}

/// The definition's parameter at `index`.
fn parameter<'m>(module: &'m Module<DynBrand, Unverified>, index: usize) -> Value<'m, DynBrand> {
    module
        .view(defined_function(module).id())
        .params()
        .nth(index)
        .expect("fixture has that many parameters")
        .into_erased()
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

/// `ninf` on an `fneg` is applied on the way out of the arm.
///
/// Ports `ComputeKnownFPClassTest.FNegNInf`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn fneg_ninf() {
    let module = parse(
        r"
define float @test(float %arg) {
  %A = fneg ninf float %arg
  ret float %A
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let known = compute_known_fp_class_all(named(&module, "A"), &query);
    assert_eq!(known.classes(), FpClassTest::INFINITY.complement());
    assert_eq!(known.sign_bit(), None);
}

/// The `fabs` and `fneg` arms, and the flags that refine them.
///
/// Ports the four `ComputeKnownFPClassTest` cases `FabsUnknown`,
/// `FNegFabsUnknown`, `NegFabsNInf` and `FNegFabsNNaN`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`), each with upstream's own
/// expected mask and sign bit. The last two carry fast-math flags on a *call*,
/// which llvmkit could not parse until this slice.
#[test]
fn fabs_and_fneg_of_an_unknown_operand() {
    let cases: &[(&str, FpClassTest, Option<bool>)] = &[
        // FabsUnknown: fcPositive | fcNan, sign false.
        (
            r"
declare float @llvm.fabs.f32(float)
define float @test(float %arg) {
  %A = call float @llvm.fabs.f32(float %arg)
  ret float %A
}
",
            FpClassTest::POSITIVE.union(FpClassTest::NAN),
            Some(false),
        ),
        // FNegFabsUnknown: fcNegative | fcNan, sign true.
        (
            r"
declare float @llvm.fabs.f32(float)
define float @test(float %arg) {
  %fabs = call float @llvm.fabs.f32(float %arg)
  %A = fneg float %fabs
  ret float %A
}
",
            FpClassTest::NEGATIVE.union(FpClassTest::NAN),
            Some(true),
        ),
        // NegFabsNInf: (fcNegative & ~fcNegInf) | fcNan, sign true.
        (
            r"
declare float @llvm.fabs.f32(float)
define float @test(float %arg) {
  %fabs = call ninf float @llvm.fabs.f32(float %arg)
  %A = fneg float %fabs
  ret float %A
}
",
            FpClassTest::NEGATIVE
                .difference(FpClassTest::NEGATIVE_INFINITY)
                .union(FpClassTest::NAN),
            Some(true),
        ),
        // FNegFabsNNaN: fcNegative, sign true.
        (
            r"
declare float @llvm.fabs.f32(float)
define float @test(float %arg) {
  %fabs = call nnan float @llvm.fabs.f32(float %arg)
  %A = fneg float %fabs
  ret float %A
}
",
            FpClassTest::NEGATIVE,
            Some(true),
        ),
    ];

    for (source, expected_classes, expected_sign) in cases {
        let module = parse(source);
        let data_layout = module.data_layout();
        let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);
        let known = compute_known_fp_class_all(named(&module, "A"), &query);
        assert_eq!(known.classes(), *expected_classes, "classes for:{source}");
        assert_eq!(known.sign_bit(), *expected_sign, "sign for:{source}");
    }
}

/// The `uitofp` arm, including its `ilogb(getLargest(FPTy)) >= IntSize`
/// overflow test — an `i32` fits in a `float`, an `i16` does not fit in a
/// `half`, so the second keeps its infinity.
///
/// Ports `ComputeKnownFPClassTest.UIToFP`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn unsigned_integer_to_float() {
    let module = parse(
        r"
define float @test(i32 %arg0, i16 %arg1) {
  %A = uitofp i32 %arg0 to float
  %A2 = uitofp i16 %arg1 to half
  ret float %A
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let known = compute_known_fp_class_all(named(&module, "A"), &query);
    assert_eq!(
        known.classes(),
        FpClassTest::POSITIVE_FINITE.difference(FpClassTest::SUBNORMAL)
    );
    assert_eq!(known.sign_bit(), Some(false));

    let known = compute_known_fp_class_all(named(&module, "A2"), &query);
    assert_eq!(
        known.classes(),
        FpClassTest::POSITIVE.difference(FpClassTest::SUBNORMAL)
    );
    assert_eq!(known.sign_bit(), Some(false));
}

/// The `sitofp` arm. The sign is unknown, `-0` is impossible, and the `i17`
/// case overflows a `half` and so keeps its infinity.
///
/// Ports `ComputeKnownFPClassTest.SIToFP`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn signed_integer_to_float() {
    let module = parse(
        r"
define float @test(i32 %arg0, i16 %arg1, i17 %arg2) {
  %A = sitofp i32 %arg0 to float
  %A2 = sitofp i16 %arg1 to half
  %A3 = sitofp i17 %arg2 to half
  ret float %A
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let finite_no_negative_zero_no_subnormal = FpClassTest::FINITE
        .difference(FpClassTest::NEGATIVE_ZERO)
        .difference(FpClassTest::SUBNORMAL);
    for name in ["A", "A2"] {
        let known = compute_known_fp_class_all(named(&module, name), &query);
        assert_eq!(
            known.classes(),
            finite_no_negative_zero_no_subnormal,
            "%{name}"
        );
        assert_eq!(known.sign_bit(), None, "%{name}");
    }

    // An `i17` needs 16 magnitude bits, past `half`'s exponent range.
    let known = compute_known_fp_class_all(named(&module, "A3"), &query);
    assert_eq!(
        known.classes(),
        FpClassTest::NAN
            .union(FpClassTest::NEGATIVE_ZERO)
            .union(FpClassTest::SUBNORMAL)
            .complement()
    );
    assert_eq!(known.sign_bit(), None);
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

// --------------------------------------------------------------------------
// The select arm
// --------------------------------------------------------------------------

/// Both arms positive zero, so the select is too — sign known clear.
///
/// Ports `ComputeKnownFPClassTest.SelectPos0`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn select_of_two_positive_zeros_is_a_positive_zero() {
    let module = parse(
        r"
define float @test(i1 %cond) {
  %A = select i1 %cond, float 0.0, float 0.0
  ret float %A
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let known = compute_known_fp_class_all(named(&module, "A"), &query);
    assert_eq!(known.classes(), FpClassTest::POSITIVE_ZERO);
    assert_eq!(known.sign_bit(), Some(false));
}

/// Both arms negative zero — sign known set.
///
/// Ports `ComputeKnownFPClassTest.SelectNeg0`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn select_of_two_negative_zeros_is_a_negative_zero() {
    let module = parse(
        r"
define float @test(i1 %cond) {
  %A = select i1 %cond, float -0.0, float -0.0
  ret float %A
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let known = compute_known_fp_class_all(named(&module, "A"), &query);
    assert_eq!(known.classes(), FpClassTest::NEGATIVE_ZERO);
    assert_eq!(known.sign_bit(), Some(true));
}

/// One arm of each sign: still a zero, but the sign is no longer known — the
/// intersection of the two arms, which is what the `Select` arm computes.
///
/// Ports `ComputeKnownFPClassTest.SelectPosOrNeg0`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn select_of_opposite_zeros_keeps_the_class_and_loses_the_sign() {
    let module = parse(
        r"
define float @test(i1 %cond) {
  %A = select i1 %cond, float 0.0, float -0.0
  ret float %A
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let known = compute_known_fp_class_all(named(&module, "A"), &query);
    assert_eq!(known.classes(), FpClassTest::ZERO);
    assert_eq!(known.sign_bit(), None);
}

/// The select arm's *condition* refinement: the true arm is reached only when
/// the condition holds, so the value is known ordered there even though nothing
/// is known about it on its own.
///
/// No upstream counterpart isolates `adjustKnownFPClassForSelectArm` — upstream
/// exercises it through InstCombine folds. The reasoning is upstream's: the
/// false arm is a non-NaN constant, and `fcmp ord %x, 0.0` being true rules NaN
/// out of the true arm, so neither arm can be a NaN.
#[test]
fn a_select_arm_learns_from_the_condition_that_reaches_it() {
    let module = parse(
        r"
define float @guarded(float %x) {
  %ord = fcmp ord float %x, 0.0
  %A = select i1 %ord, float %x, float 1.0
  ret float %A
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let known = compute_known_fp_class_all(named(&module, "A"), &query);
    assert!(known.is_known_never_nan());

    // Without the condition there is nothing to learn: the same select on an
    // unrelated predicate leaves the true arm unconstrained.
    let unguarded = parse(
        r"
define float @unguarded(float %x, i1 %cond) {
  %A = select i1 %cond, float %x, float 1.0
  ret float %A
}
",
    );
    let data_layout = unguarded.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);
    assert!(!compute_known_fp_class_all(named(&unguarded, "A"), &query).is_known_never_nan());
}

// --------------------------------------------------------------------------
// The context arm
// --------------------------------------------------------------------------

/// A dominating `fcmp ueq x, 0.0` that is *false* proves the value is neither a
/// NaN nor a zero.
///
/// Ports `@test1` from
/// `llvm/test/Transforms/InstCombine/fpclass-from-dom-cond.ll` — the fixture
/// verbatim, with upstream's own `%ret` as the context instruction. Its CHECK
/// line rewrites `is.fpclass(x, 783)` to `is.fpclass(x, 780)` in the else
/// block; `783 & ~780` is `0x3`, exactly the two NaN bits, so the fold is
/// licensed by `%x` being known never NaN there.
#[test]
fn a_dominating_false_unordered_equality_rules_out_nan_and_zero() {
    let module = parse(
        r"
declare i1 @llvm.is.fpclass.f32(float, i32)

define i1 @test1(float %x) {
entry:
  %cond = fcmp ueq float %x, 0.000000e+00
  br i1 %cond, label %if.then, label %if.else

if.then:
  ret i1 false

if.else:
  %ret = call i1 @llvm.is.fpclass.f32(float %x, i32 783)
  ret i1 %ret
}
",
    );
    let data_layout = module.data_layout();
    let function = defined_function(&module);
    let dominator_tree = DominatorTree::new(module.view(function.id()));
    let mut conditions = DomConditionCache::new();
    let branch = function
        .basic_blocks()
        .next()
        .and_then(|block| block.instructions().last())
        .expect("the entry block has a terminator");
    conditions.register_branch(branch.to_erased());

    let context = instruction(&module, "ret");
    let known = compute_known_fp_class_all(
        parameter(&module, 0),
        &ValueTrackingQuery::new(&data_layout)
            .with_context_instruction(&context)
            .with_dominator_tree(&dominator_tree)
            .with_dominating_conditions(&conditions),
    );

    // `ueq` false means ordered and unequal: no NaN, and not zero either.
    assert!(known.is_known_never_nan());
    assert!(known.is_known_never_zero());

    // The fold upstream checks for, spelled out: the bits `is.fpclass` is asked
    // about, minus the ones the value provably is not, are upstream's 780.
    let asked = FpClassTest::from_bits(783).expect("783 is a class mask");
    assert_eq!(
        asked.intersection(known.classes()).bits(),
        780,
        "the dominating condition licenses upstream's 783 -> 780 rewrite"
    );

    // Without the dominating condition the same value is unconstrained.
    let plain: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);
    assert!(!compute_known_fp_class_all(parameter(&module, 0), &plain).is_known_never_nan());
}

/// A dominating `fcmp olt x, k` that is false, for a small positive `k`, leaves
/// the value at least `k` — so it cannot be zero.
///
/// Ports `@test2` from
/// `llvm/test/Transforms/InstCombine/fpclass-from-dom-cond.ll` — the fixture
/// verbatim, with upstream's own `%cmp.i` as the context instruction. Its CHECK
/// line folds that `fcmp oeq double %x, 0.0` to `false`, which is licensed by
/// `%x` being known never zero there.
#[test]
fn a_dominating_false_ordered_less_than_rules_out_zero() {
    let module = parse(
        r"
define i1 @test2(double %x) {
entry:
  %cmp = fcmp olt double %x, 0x3EB0C6F7A0000000
  br i1 %cmp, label %if.then, label %if.end

if.then:
  ret i1 false

if.end:
  %cmp.i = fcmp oeq double %x, 0.000000e+00
  ret i1 %cmp.i
}
",
    );
    let data_layout = module.data_layout();
    let function = defined_function(&module);
    let dominator_tree = DominatorTree::new(module.view(function.id()));
    let mut conditions = DomConditionCache::new();
    let branch = function
        .basic_blocks()
        .next()
        .and_then(|block| block.instructions().last())
        .expect("the entry block has a terminator");
    conditions.register_branch(branch.to_erased());

    let context = instruction(&module, "cmp.i");
    let known = compute_known_fp_class_all(
        parameter(&module, 0),
        &ValueTrackingQuery::new(&data_layout)
            .with_context_instruction(&context)
            .with_dominator_tree(&dominator_tree)
            .with_dominating_conditions(&conditions),
    );

    // What makes upstream fold `%cmp.i` to false.
    assert!(known.is_known_never_zero());

    // Without the dominating condition there is nothing to fold on.
    let plain: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);
    assert!(!compute_known_fp_class_all(parameter(&module, 0), &plain).is_known_never_zero());
}

/// `nsz` on a `sqrt` call is read through upstream's `Q.IIQ`, so a query told to
/// ignore instruction flags must not see it — and the negative zero the flag
/// would have excluded comes back.
///
/// Ports the `%A` and `%A2` blocks of
/// `ComputeKnownFPClassTest.SqrtNszSignBit`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`), with upstream's own two
/// masks. Its `%A3`/`%A4` blocks are **not** ported: they need a
/// `nofpclass(nan)` parameter attribute, which llvmkit does not model —
/// recorded in `docs/future-work.md`.
///
/// The masks being exact also pins the denormal mode: it comes from
/// `Function::getDenormalMode`, and a function carrying no `denormal-fp-math`
/// attribute has the IEEE mode, not the dynamic one.
#[test]
fn sqrt_nsz_is_hidden_from_a_query_that_ignores_instruction_flags() {
    let module = parse(
        r"
declare float @llvm.sqrt.f32(float)

define float @test(float %arg) {
  %A = call float @llvm.sqrt.f32(float %arg)
  %A2 = call nsz float @llvm.sqrt.f32(float %arg)
  ret float %A
}
",
    );
    let data_layout = module.data_layout();

    let sqrt_mask = FpClassTest::POSITIVE_INFINITY
        .union(FpClassTest::POSITIVE_NORMAL)
        .union(FpClassTest::ZERO)
        .union(FpClassTest::NAN);
    let nsz_sqrt_mask = FpClassTest::POSITIVE_INFINITY
        .union(FpClassTest::POSITIVE_NORMAL)
        .union(FpClassTest::POSITIVE_ZERO)
        .union(FpClassTest::NAN);

    let with_flags: ValueTrackingQuery<'_, '_, DynBrand> =
        ValueTrackingQuery::new(&data_layout).with_instruction_info();
    let without_flags: ValueTrackingQuery<'_, '_, DynBrand> =
        ValueTrackingQuery::new(&data_layout).without_instruction_info();

    // A plain `sqrt` reads the same either way — there is no flag to hide.
    let plain = named(&module, "A");
    let known = compute_known_fp_class_all(plain, &with_flags);
    assert_eq!(known.classes(), sqrt_mask);
    assert_eq!(known.sign_bit(), None);
    let known = compute_known_fp_class_all(plain, &without_flags);
    assert_eq!(known.classes(), sqrt_mask);
    assert_eq!(known.sign_bit(), None);

    // `nsz` excludes the negative zero — but only for a query that reads flags.
    let nsz = named(&module, "A2");
    let known = compute_known_fp_class_all(nsz, &with_flags);
    assert_eq!(known.classes(), nsz_sqrt_mask);
    assert_eq!(known.sign_bit(), None);
    let known = compute_known_fp_class_all(nsz, &without_flags);
    assert_eq!(known.classes(), sqrt_mask);
    assert_eq!(known.sign_bit(), None);
}

/// The `nsz` this file relies on survives a print/re-parse round trip, which is
/// the parser/printer contract for the flags `parse_call` now accepts.
///
/// No upstream counterpart: LLVM tests the round trip through `llvm-as`/
/// `llvm-dis` rather than a unit test.
#[test]
fn fast_math_flags_on_a_call_round_trip() {
    let source = "declare float @llvm.sqrt.f32(float)\n\
                  \n\
                  define float @test(float %arg) {\n\
                  \x20\x20%A = call nsz float @llvm.sqrt.f32(float %arg)\n\
                  \x20\x20ret float %A\n\
                  }\n";
    let module = parse(source);
    let printed = format!("{module}");
    assert!(
        printed.contains("call nsz float @llvm.sqrt.f32(float %arg)"),
        "the flags are printed back:\n{printed}"
    );
    // And the printed text parses to the same thing.
    let reparsed = parse(&printed);
    assert_eq!(printed, format!("{reparsed}"));
}
