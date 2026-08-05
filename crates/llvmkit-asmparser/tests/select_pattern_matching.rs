//! Ports of `matchSelectPattern` — tranche 4b of the `ValueTracking.h` port
//! (see `docs/future-work.md`). The flavour vocabulary is tranche 4a and lives
//! in `crates/llvmkit-ir/tests/select_pattern.rs`.
//!
//! Every case comes from `class MatchSelectPatternTest` in
//! `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR inlined verbatim. That
//! harness runs `matchSelectPattern(A, LHS, RHS, &CastOp)` on the instruction
//! named `%A` and compares all three fields of the `SelectPatternResult`, so
//! that is what the table below carries. Passing `&CastOp` is
//! `look_through_cast = true` here.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, MinMaxIntrinsic, MinMaxKind, MinMaxOperation, Module, SelectPatternFlavor,
    SelectPatternNaNBehavior, Unverified, Value, ValueTrackingQuery,
    can_convert_to_min_or_max_intrinsic, match_select_pattern,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

/// One upstream `TEST_F`: its name, its IR, and the `expectPattern` argument.
///
/// `None` is upstream's `{SPF_UNKNOWN, SPNB_NA, false}` — the answer that says
/// the out-parameters must not be read, which is why it is the `None` of the
/// `Option` llvmkit returns.
type Case = (
    &'static str,
    &'static str,
    Option<(SelectPatternFlavor, SelectPatternNaNBehavior, bool)>,
);

fn check(cases: &[Case]) {
    for (name, source, expected) in cases {
        let module = parse(source);
        let data_layout = module.data_layout();
        let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
        let matched = match_select_pattern(named(&module, "A"), true, &query, 0)
            .expect("query succeeds")
            .map(|matched| {
                (
                    matched.result.flavor,
                    matched.result.nan_behavior,
                    matched.result.ordered,
                )
            });
        assert_eq!(&matched, expected, "MatchSelectPatternTest::{name}");
    }
}

/// The floating-point cases: `SimpleFMin`, `SimpleFMax`, `SwappedFMax`,
/// `SwappedFMax2`, `SwappedFMax3`, `FastFMin`, `FastFMinUnordered`.
///
/// These are what pin the NaN-behaviour derivation, which is the subtlest part
/// of the port: whether the comparison is ordered decides which operand a NaN
/// input flows to, and the `nnan` flag on the `fcmp` collapses the question.
#[test]
fn floating_point_min_max_fixtures() {
    check(&[
        (
            "SimpleFMin",
            r"
define float @test(float %a) {
  %1 = fcmp ult float %a, 5.0
  %A = select i1 %1, float %a, float 5.0
  ret float %A
}
",
            Some((
                SelectPatternFlavor::FMinNum,
                SelectPatternNaNBehavior::ReturnsNaN,
                false,
            )),
        ),
        (
            "SimpleFMax",
            r"
define float @test(float %a) {
  %1 = fcmp ogt float %a, 5.0
  %A = select i1 %1, float %a, float 5.0
  ret float %A
}
",
            Some((
                SelectPatternFlavor::FMaxNum,
                SelectPatternNaNBehavior::ReturnsOther,
                true,
            )),
        ),
        (
            "SwappedFMax",
            r"
define float @test(float %a) {
  %1 = fcmp olt float 5.0, %a
  %A = select i1 %1, float %a, float 5.0
  ret float %A
}
",
            Some((
                SelectPatternFlavor::FMaxNum,
                SelectPatternNaNBehavior::ReturnsOther,
                false,
            )),
        ),
        (
            "SwappedFMax2",
            r"
define float @test(float %a) {
  %1 = fcmp olt float %a, 5.0
  %A = select i1 %1, float 5.0, float %a
  ret float %A
}
",
            Some((
                SelectPatternFlavor::FMaxNum,
                SelectPatternNaNBehavior::ReturnsNaN,
                false,
            )),
        ),
        (
            "SwappedFMax3",
            r"
define float @test(float %a) {
  %1 = fcmp ult float %a, 5.0
  %A = select i1 %1, float 5.0, float %a
  ret float %A
}
",
            Some((
                SelectPatternFlavor::FMaxNum,
                SelectPatternNaNBehavior::ReturnsOther,
                true,
            )),
        ),
        (
            "FastFMin",
            r"
define float @test(float %a) {
  %1 = fcmp nnan olt float %a, 5.0
  %A = select i1 %1, float %a, float 5.0
  ret float %A
}
",
            Some((
                SelectPatternFlavor::FMinNum,
                SelectPatternNaNBehavior::ReturnsAny,
                true,
            )),
        ),
        (
            "FastFMinUnordered",
            r"
define float @test(float %a) {
  %1 = fcmp nnan ult float %a, 5.0
  %A = select i1 %1, float %a, float 5.0
  ret float %A
}
",
            Some((
                SelectPatternFlavor::FMinNum,
                SelectPatternNaNBehavior::ReturnsAny,
                false,
            )),
        ),
    ]);
}

/// `DoubleCastU`, `DoubleCastS` and `DoubleCastBad` — the `look_through_cast`
/// path.
///
/// The first two look through the same cast on both arms; the third proves the
/// casts must *match*, so a `zext`/`sext` pair is not a `umin`.
#[test]
fn double_cast_fixtures() {
    check(&[
        (
            "DoubleCastU",
            r"
define i32 @test(i8 %a, i8 %b) {
  %1 = icmp ult i8 %a, %b
  %2 = zext i8 %a to i32
  %3 = zext i8 %b to i32
  %A = select i1 %1, i32 %2, i32 %3
  ret i32 %A
}
",
            Some((
                SelectPatternFlavor::UMin,
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )),
        ),
        (
            "DoubleCastS",
            r"
define i32 @test(i8 %a, i8 %b) {
  %1 = icmp slt i8 %a, %b
  %2 = sext i8 %a to i32
  %3 = sext i8 %b to i32
  %A = select i1 %1, i32 %2, i32 %3
  ret i32 %A
}
",
            Some((
                SelectPatternFlavor::SMin,
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )),
        ),
        (
            "DoubleCastBad",
            r"
define i32 @test(i8 %a, i8 %b) {
  %1 = icmp ult i8 %a, %b
  %2 = zext i8 %a to i32
  %3 = sext i8 %b to i32
  %A = select i1 %1, i32 %2, i32 %3
  ret i32 %A
}
",
            None,
        ),
    ]);
}

/// The `NotNot*` family: a min/max disguised by inverting both arms.
///
/// The scalar cases are ported; upstream's `<2 x i8>` variants spell the
/// all-ones splat as `<i8 -1, i8-1>`, which LLVM 22 prints as
/// `splat (i8 -1)` — the same value, and the same code path, so porting both
/// spellings of one constant would not test anything the scalar case does not.
#[test]
fn not_not_min_max_fixtures() {
    check(&[
        (
            "NotNotSMin",
            r"
define i8 @test(i8 %a, i8 %b) {
  %cmp = icmp sgt i8 %a, %b
  %an = xor i8 %a, -1
  %bn = xor i8 %b, -1
  %A = select i1 %cmp, i8 %an, i8 %bn
  ret i8 %A
}
",
            Some((
                SelectPatternFlavor::SMin,
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )),
        ),
        (
            "NotNotSMax",
            r"
define i8 @test(i8 %a, i8 %b) {
  %cmp = icmp slt i8 %a, %b
  %an = xor i8 %a, -1
  %bn = xor i8 %b, -1
  %A = select i1 %cmp, i8 %an, i8 %bn
  ret i8 %A
}
",
            Some((
                SelectPatternFlavor::SMax,
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )),
        ),
        (
            "NotNotUMinSwap",
            r"
define i8 @test(i8 %a, i8 %b) {
  %cmp = icmp ult i8 %a, %b
  %an = xor i8 %a, -1
  %bn = xor i8 %b, -1
  %A = select i1 %cmp, i8 %bn, i8 %an
  ret i8 %A
}
",
            Some((
                SelectPatternFlavor::UMin,
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )),
        ),
    ]);
}

/// The plain `(icmp X, Y) ? X : Y` shape and the `abs` / `nabs` idioms.
///
/// **No upstream `MatchSelectPatternTest` case covers these**; LLVM exercises
/// them through InstCombine's min/max and abs folds. The oracle is
/// `getSelectPattern`'s own table — already ported and tested in tranche 4a —
/// and the comments upstream puts on each abs arm, quoted at
/// `select_pattern.rs::match_abs`:
/// `(X >s 0) ? X : -X --> ABS(X)`, `(X <s 0) ? X : -X --> NABS(X)`.
#[test]
fn direct_and_abs_shapes() {
    check(&[
        (
            "(icmp slt X, Y) ? X : Y is SMIN",
            r"
define i32 @test(i32 %x, i32 %y) {
  %cmp = icmp slt i32 %x, %y
  %A = select i1 %cmp, i32 %x, i32 %y
  ret i32 %A
}
",
            Some((
                SelectPatternFlavor::SMin,
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )),
        ),
        (
            "(X >s 0) ? X : -X is ABS",
            r"
define i32 @test(i32 %x) {
  %neg = sub i32 0, %x
  %cmp = icmp sgt i32 %x, 0
  %A = select i1 %cmp, i32 %x, i32 %neg
  ret i32 %A
}
",
            Some((
                SelectPatternFlavor::Abs,
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )),
        ),
        (
            "(X <s 0) ? X : -X is NABS",
            r"
define i32 @test(i32 %x) {
  %neg = sub i32 0, %x
  %cmp = icmp slt i32 %x, 0
  %A = select i1 %cmp, i32 %x, i32 %neg
  ret i32 %A
}
",
            Some((
                SelectPatternFlavor::NAbs,
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )),
        ),
        (
            "an equality compare is never a min/max",
            r"
define i32 @test(i32 %x, i32 %y) {
  %cmp = icmp eq i32 %x, %y
  %A = select i1 %cmp, i32 %x, i32 %y
  ret i32 %A
}
",
            None,
        ),
    ]);
}

/// `matchSelectPattern` reports the two values the idiom chooses between.
///
/// **No upstream counterpart as a unit test** — `MatchSelectPatternTest`
/// declares `LHS`/`RHS` and never reads them. The oracle is upstream's own
/// contract on those out-parameters, "Assume success. If there's no match,
/// callers should not use these anyway", which is why llvmkit puts the whole
/// record behind an `Option`: this asserts the operands are the ones the
/// compare named, and that a non-match hands back nothing at all.
#[test]
fn a_match_reports_the_operands_and_a_non_match_reports_nothing() {
    let module = parse(
        r"
define i32 @test(i32 %x, i32 %y) {
  %cmp = icmp slt i32 %x, %y
  %A = select i1 %cmp, i32 %x, i32 %y
  ret i32 %A
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);

    let params: Vec<_> = module
        .as_view()
        .functions()
        .map(|function| function.id())
        .flat_map(|id| module.view(id).params().collect::<Vec<_>>())
        .collect();
    let matched = match_select_pattern(named(&module, "A"), true, &query, 0)
        .expect("query succeeds")
        .expect("this is an smin");
    assert_eq!(matched.lhs, params[0].as_erased());
    assert_eq!(matched.rhs, params[1].as_erased());
    assert_eq!(matched.cast, None);

    // A `select` whose condition is not a compare at all matches nothing.
    let module = parse(
        r"
define i32 @test(i1 %c, i32 %x, i32 %y) {
  %A = select i1 %c, i32 %x, i32 %y
  ret i32 %A
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    assert!(
        match_select_pattern(named(&module, "A"), true, &query, 0)
            .expect("query succeeds")
            .is_none()
    );
}

/// Run `can_convert_to_min_or_max_intrinsic` over the named instructions.
fn convert(source: &str, names: &[&str]) -> Option<(MinMaxOperation, bool)> {
    let module = parse(source);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    let values: Vec<_> = names.iter().map(|name| named(&module, name)).collect();
    can_convert_to_min_or_max_intrinsic(values, &query).expect("query succeeds")
}

/// `llvm::canConvertToMinOrMaxIntrinsic` names the intrinsic a set of `select`s
/// could become. Its switch is wider than `getMinMaxIntrinsic`: besides the
/// four integer flavours it maps `SPF_FMAXNUM` and `SPF_FMINNUM` to
/// `Intrinsic::maxnum` and `Intrinsic::minnum`.
///
/// **Upstream has no unit test for this function.** Its only caller is
/// `SLPVectorizer`, which reaches it through `.ll` regression tests of a pass
/// llvmkit does not have. The inputs below are therefore upstream's own
/// `MatchSelectPatternTest` IR — `SimpleFMax` and `SimpleFMin` verbatim from
/// the fixtures above — and the expected answers are what the switch in
/// `canConvertToMinOrMaxIntrinsic` returns for the flavour those fixtures pin.
#[test]
fn converting_a_select_names_the_intrinsic_including_the_two_float_flavors() {
    // MatchSelectPatternTest::SimpleFMax -> SPF_FMAXNUM -> Intrinsic::maxnum.
    assert_eq!(
        convert(
            r"
define float @test(float %a) {
  %1 = fcmp ogt float %a, 5.0
  %A = select i1 %1, float %a, float 5.0
  ret float %A
}
",
            &["A"],
        ),
        Some((MinMaxOperation::Float(MinMaxKind::MaxNum), true)),
    );

    // MatchSelectPatternTest::SimpleFMin -> SPF_FMINNUM -> Intrinsic::minnum.
    assert_eq!(
        convert(
            r"
define float @test(float %a) {
  %1 = fcmp ult float %a, 5.0
  %A = select i1 %1, float %a, float 5.0
  ret float %A
}
",
            &["A"],
        ),
        Some((MinMaxOperation::Float(MinMaxKind::MinNum), true)),
    );

    // The integer half is unchanged: SPF_SMIN -> Intrinsic::smin.
    assert_eq!(
        convert(
            r"
define i32 @test(i32 %x, i32 %y) {
  %cmp = icmp slt i32 %x, %y
  %A = select i1 %cmp, i32 %x, i32 %y
  ret i32 %A
}
",
            &["A"],
        ),
        Some((MinMaxOperation::Integer(MinMaxIntrinsic::SMin), true)),
    );

    // Upstream bails as soon as two values disagree on the flavour, because one
    // intrinsic has to serve them all.
    assert_eq!(
        convert(
            r"
define i32 @test(i32 %x, i32 %y) {
  %c1 = icmp slt i32 %x, %y
  %A = select i1 %c1, i32 %x, i32 %y
  %c2 = icmp sgt i32 %x, %y
  %B = select i1 %c2, i32 %x, i32 %y
  ret i32 %A
}
",
            &["A", "B"],
        ),
        None,
    );

    // And on anything that is not a min or a max at all.
    assert_eq!(
        convert(
            r"
define i32 @test(i1 %c, i32 %x, i32 %y) {
  %A = select i1 %c, i32 %x, i32 %y
  ret i32 %A
}
",
            &["A"],
        ),
        None,
    );
}
