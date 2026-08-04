//! Ports of the assumption and implied-condition analyses — tranche 8 of the
//! `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! Four upstream sources drive these:
//!
//! - The five `IsImpliedCondition*` `TEST_F`s in
//!   `llvm/unittests/Analysis/ValueTrackingTest.cpp`, ported verbatim.
//! - `llvm/test/Analysis/ValueTracking/assume.ll`, whose `@assume_add` `CHECK`
//!   lines record `add i32 %t1, 3` folding to `or disjoint`; that fold is gated
//!   on the assumed `and i32 %t1, 3` being zero reaching `computeKnownBits`
//!   through `computeKnownBitsFromContext`, so the `CHECK` line is the oracle.
//! - `llvm/test/Analysis/ValueTracking/knownbits-select-from-cond.ll`, whose
//!   `select_condition_implies_highbits_op1` / `..._maybe_undef_fail` pair
//!   differ only in a `noundef` on `%y` — exactly the guard
//!   `adjustKnownBitsForSelectArm` applies last.
//! - `llvm/lib/Analysis/AssumptionCache.cpp`'s own contract for
//!   `assumptionsFor`, which is `findValuesAffectedByCondition` over each
//!   assume's condition.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    AssumptionCache, CondContext, DomConditionCache, DominatorTree, DynBrand, IntPredicate,
    KnownBits, Module, PredicateWithSameSign, Unverified, Value, ValueTrackingQuery,
    adjust_known_bits_for_select_arm, compute_known_bits, find_values_affected_by_condition,
    is_implied_by_dom_condition, is_implied_by_dom_condition_decomposed, is_implied_condition,
    is_valid_assume_for_context, will_not_free_between,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The instruction named `%name` in the module's definitions.
fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    instruction(module, name).to_erased()
}

/// The instruction named `%name`, as a view.
fn instruction<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> llvmkit_ir::InstructionView<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|candidate| candidate.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
}

/// The `n`-th `@llvm.assume` in the module, in program order.
fn assume<'m>(
    module: &'m Module<DynBrand, Unverified>,
    index: usize,
) -> llvmkit_ir::InstructionView<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .filter(|candidate| format!("{candidate}").contains("@llvm.assume"))
        .nth(index)
        .expect("fixture has that many assumes")
}

/// The first function with a body.
fn defined_function<'m>(
    module: &'m Module<DynBrand, Unverified>,
) -> llvmkit_ir::FunctionView<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .find(|function| function.basic_blocks().next().is_some())
        .expect("fixture defines a function")
}

/// A dominator tree for the module's first *definition*, which is what
/// upstream's `DominatorTree DT(*F)` builds. Skipping declarations matters:
/// a declaration has no entry block, so a tree built from it is empty and every
/// dominance query silently answers false.
fn dominator_tree(module: &Module<DynBrand, Unverified>) -> DominatorTree {
    DominatorTree::new(module.view(defined_function(module).id()))
}

/// The defined function's parameter at `index`.
fn parameter<'m>(module: &'m Module<DynBrand, Unverified>, index: usize) -> Value<'m, DynBrand> {
    module
        .view(defined_function(module).id())
        .params()
        .nth(index)
        .expect("fixture has that many parameters")
        .into_erased()
}

/// The right-hand operand of the `icmp` named `%name`.
fn compare_rhs<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    instruction(module, name)
        .kind()
        .and_then(|kind| kind.as_cmp())
        .unwrap_or_else(|| panic!("%{name} is a compare"))
        .rhs()
}

// --------------------------------------------------------------------------
// isImpliedCondition
// --------------------------------------------------------------------------

/// Ports `ValueTrackingTest.IsImpliedConditionAnd`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn is_implied_condition_and() {
    let module = parse(
        r"
define void @test(i32 %x, i32 %y) {
  %c1 = icmp ult i32 %x, 10
  %c2 = icmp ult i32 %y, 15
  %A = and i1 %c1, %c2
  ; x < 10 /\ y < 15
  %A2 = icmp ult i32 %x, 20
  %A3 = icmp uge i32 %y, 20
  %A4 = icmp ult i32 %x, 5
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let a = named(&module, "A");
    assert_eq!(
        is_implied_condition(a, named(&module, "A2"), &data_layout, true),
        Some(true)
    );
    assert_eq!(
        is_implied_condition(a, named(&module, "A3"), &data_layout, true),
        Some(false)
    );
    assert_eq!(
        is_implied_condition(a, named(&module, "A4"), &data_layout, true),
        None
    );
}

/// Ports `ValueTrackingTest.IsImpliedConditionAnd2` — the same question with the
/// `and` spelled as a poison-blocking `select`.
#[test]
fn is_implied_condition_and_select() {
    let module = parse(
        r"
define void @test(i32 %x, i32 %y) {
  %c1 = icmp ult i32 %x, 10
  %c2 = icmp ult i32 %y, 15
  %A = select i1 %c1, i1 %c2, i1 false
  ; x < 10 /\ y < 15
  %A2 = icmp ult i32 %x, 20
  %A3 = icmp uge i32 %y, 20
  %A4 = icmp ult i32 %x, 5
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let a = named(&module, "A");
    assert_eq!(
        is_implied_condition(a, named(&module, "A2"), &data_layout, true),
        Some(true)
    );
    assert_eq!(
        is_implied_condition(a, named(&module, "A3"), &data_layout, true),
        Some(false)
    );
    assert_eq!(
        is_implied_condition(a, named(&module, "A4"), &data_layout, true),
        None
    );
}

/// Ports `ValueTrackingTest.IsImpliedConditionAndVec`.
#[test]
fn is_implied_condition_and_vector() {
    let module = parse(
        r"
define void @test(<2 x i8> %x, <2 x i8> %y) {
  %A = icmp ult <2 x i8> %x, %y
  %A2 = icmp ule <2 x i8> %x, %y
  ret void
}
",
    );
    let data_layout = module.data_layout();
    assert_eq!(
        is_implied_condition(
            named(&module, "A"),
            named(&module, "A2"),
            &data_layout,
            true
        ),
        Some(true)
    );
}

/// Ports `ValueTrackingTest.IsImpliedConditionOr`. Upstream's comment marks the
/// `or` as negated: every query passes `LHSIsTrue = false`.
#[test]
fn is_implied_condition_or() {
    let module = parse(
        r"
define void @test(i32 %x, i32 %y) {
  %c1 = icmp ult i32 %x, 10
  %c2 = icmp ult i32 %y, 15
  %A = or i1 %c1, %c2 ; negated
  ; x >= 10 /\ y >= 15
  %A2 = icmp ult i32 %x, 5
  %A3 = icmp uge i32 %y, 10
  %A4 = icmp ult i32 %x, 15
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let a = named(&module, "A");
    assert_eq!(
        is_implied_condition(a, named(&module, "A2"), &data_layout, false),
        Some(false)
    );
    assert_eq!(
        is_implied_condition(a, named(&module, "A3"), &data_layout, false),
        Some(true)
    );
    assert_eq!(
        is_implied_condition(a, named(&module, "A4"), &data_layout, false),
        None
    );
}

/// Ports `ValueTrackingTest.IsImpliedConditionOr2` — the `or` spelled as a
/// poison-blocking `select`.
#[test]
fn is_implied_condition_or_select() {
    let module = parse(
        r"
define void @test(i32 %x, i32 %y) {
  %c1 = icmp ult i32 %x, 10
  %c2 = icmp ult i32 %y, 15
  %A = select i1 %c1, i1 true, i1 %c2 ; negated
  ; x >= 10 /\ y >= 15
  %A2 = icmp ult i32 %x, 5
  %A3 = icmp uge i32 %y, 10
  %A4 = icmp ult i32 %x, 15
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let a = named(&module, "A");
    assert_eq!(
        is_implied_condition(a, named(&module, "A2"), &data_layout, false),
        Some(false)
    );
    assert_eq!(
        is_implied_condition(a, named(&module, "A3"), &data_layout, false),
        Some(true)
    );
    assert_eq!(
        is_implied_condition(a, named(&module, "A4"), &data_layout, false),
        None
    );
}

/// `isImpliedByDomCondition` over the same reasoning, reached through the
/// single-predecessor branch `getDomPredecessorCondition` looks for.
///
/// No direct upstream counterpart: `isImpliedByDomCondition` is exercised
/// upstream only through `InstructionSimplify`'s `.ll` fixtures, which need a
/// pass pipeline llvmkit cannot yet run. The reasoning under test is upstream's
/// (`getDomPredecessorCondition` plus `isImpliedCondition`); only the shape of
/// the fixture is llvmkit's.
#[test]
fn is_implied_by_dom_condition_through_single_predecessor() {
    let module = parse(
        r"
define void @test(i32 %x) {
entry:
  %c = icmp ult i32 %x, 10
  br i1 %c, label %then, label %else

then:
  %wider = icmp ult i32 %x, 20
  %narrower = icmp ult i32 %x, 5
  ret void

else:
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let context = instruction(&module, "wider");
    assert_eq!(
        is_implied_by_dom_condition(named(&module, "wider"), &context, &data_layout),
        Some(true)
    );
    assert_eq!(
        is_implied_by_dom_condition(named(&module, "narrower"), &context, &data_layout),
        None
    );

    // The decomposed overload asks the same question without an `icmp` holding
    // the pieces: `%x u< 20` is implied by `%x u< 10`, `%x u< 5` is not.
    let x = parameter(&module, 0);
    assert_eq!(
        is_implied_by_dom_condition_decomposed(
            PredicateWithSameSign::int(IntPredicate::Ult),
            x,
            compare_rhs(&module, "wider"),
            &context,
            &data_layout,
        ),
        Some(true)
    );
    assert_eq!(
        is_implied_by_dom_condition_decomposed(
            PredicateWithSameSign::int(IntPredicate::Ult),
            x,
            compare_rhs(&module, "narrower"),
            &context,
            &data_layout,
        ),
        None
    );
}

// --------------------------------------------------------------------------
// computeKnownBitsFromContext
// --------------------------------------------------------------------------

/// Ports `@assume_add` from `llvm/test/Analysis/ValueTracking/assume.ll`. Its
/// `CHECK` line turns `add i32 %t1, 3` into `or disjoint`, which is only valid
/// once the assumed `and i32 %t1, 3 == 0` has made `%t1`'s low two bits known
/// zero — the `assume(V & Mask = C)` arm of `computeKnownBitsFromCmp`, reached
/// through `computeKnownBitsFromContext` and gated on `isValidAssumeForContext`.
#[test]
fn known_bits_from_assumed_mask_equality() {
    let module = parse(
        r"
declare void @llvm.assume(i1)

define i32 @assume_add(i32 %a, i32 %b) {
  %t1 = add i32 %a, %b
  %last_two_digits = and i32 %t1, 3
  %t2 = icmp eq i32 %last_two_digits, 0
  call void @llvm.assume(i1 %t2)
  %t3 = add i32 %t1, 3
  ret i32 %t3
}
",
    );
    let data_layout = module.data_layout();
    let cache = AssumptionCache::new(defined_function(&module));
    let context = instruction(&module, "t3");

    let without = compute_known_bits(
        named(&module, "t1"),
        &ValueTrackingQuery::new(&data_layout).with_context_instruction(&context),
    )
    .expect("known bits");
    assert_eq!(without.count_min_trailing_zeros(), 0);

    let with = compute_known_bits(
        named(&module, "t1"),
        &ValueTrackingQuery::new(&data_layout)
            .with_context_instruction(&context)
            .with_assumptions(&cache),
    )
    .expect("known bits");
    assert_eq!(with.count_min_trailing_zeros(), 2);
}

/// An assume placed *after* the context still applies, so long as control
/// provably reaches it — that is the second half of `isValidAssumeForContext`'s
/// first restriction, "or the control flow must reach the assume whenever it
/// reaches the context".
///
/// Upstream's `@test2` in `llvm/test/Analysis/ValueTracking/assume.ll` is the
/// oracle: its `icmp eq ptr %0, null` precedes the `assume`, and the `CHECK`
/// line still folds it to `ret i1 false`.
///
/// The negative half is llvmkit's own construction — upstream has no unit test
/// that isolates the scan — but the reasoning is upstream's: an intervening
/// call that may not return means control need not reach the assume.
#[test]
fn assumption_after_the_context_applies_only_if_control_reaches_it() {
    let source = |call_attributes: &str| {
        format!(
            r"
declare void @llvm.assume(i1)
declare void @opaque()

define i32 @late_assume(i32 %a, i32 %b) {{
  %t1 = add i32 %a, %b
  %t3 = add i32 %t1, 3
  call void @opaque() {call_attributes}
  %last_two_digits = and i32 %t1, 3
  %t2 = icmp eq i32 %last_two_digits, 0
  call void @llvm.assume(i1 %t2)
  ret i32 %t3
}}
"
        )
    };

    // The intervening call returns, so control reaches the assume.
    let module = parse(&source("willreturn nounwind"));
    let data_layout = module.data_layout();
    let cache = AssumptionCache::new(defined_function(&module));
    let context = instruction(&module, "t3");
    assert!(is_valid_assume_for_context(
        &assume(&module, 0),
        &context,
        None,
        false
    ));
    let known = compute_known_bits(
        named(&module, "t1"),
        &ValueTrackingQuery::new(&data_layout)
            .with_context_instruction(&context)
            .with_assumptions(&cache),
    )
    .expect("known bits");
    assert_eq!(known.count_min_trailing_zeros(), 2);

    // Without `willreturn` the call may not return, so the assume is not
    // guaranteed to be reached and teaches nothing.
    let module = parse(&source(""));
    let data_layout = module.data_layout();
    let cache = AssumptionCache::new(defined_function(&module));
    let context = instruction(&module, "t3");
    assert!(!is_valid_assume_for_context(
        &assume(&module, 0),
        &context,
        None,
        false
    ));
    let known = compute_known_bits(
        named(&module, "t1"),
        &ValueTrackingQuery::new(&data_layout)
            .with_context_instruction(&context)
            .with_assumptions(&cache),
    )
    .expect("known bits");
    assert_eq!(known.count_min_trailing_zeros(), 0);
}

/// An assume in a dominating block reaches a context in a dominated one, which
/// is the `DT->dominates(Inv, CxtI)` arm of `isValidAssumeForContext`.
///
/// No direct upstream counterpart as a unit test; the arm is exercised upstream
/// only through pass fixtures. The reasoning is upstream's.
#[test]
fn assumption_in_a_dominating_block_applies() {
    let module = parse(
        r"
declare void @llvm.assume(i1)

define i32 @dominating_assume(i32 %a, i32 %b, i1 %cond) {
entry:
  %t1 = add i32 %a, %b
  %last_two_digits = and i32 %t1, 3
  %t2 = icmp eq i32 %last_two_digits, 0
  call void @llvm.assume(i1 %t2)
  br i1 %cond, label %then, label %else

then:
  %t3 = add i32 %t1, 3
  ret i32 %t3

else:
  ret i32 0
}
",
    );
    let data_layout = module.data_layout();
    let cache = AssumptionCache::new(defined_function(&module));
    let dominator_tree = dominator_tree(&module);
    let context = instruction(&module, "t3");
    let known = compute_known_bits(
        named(&module, "t1"),
        &ValueTrackingQuery::new(&data_layout)
            .with_context_instruction(&context)
            .with_dominator_tree(&dominator_tree)
            .with_assumptions(&cache),
    )
    .expect("known bits");
    assert_eq!(known.count_min_trailing_zeros(), 2);
}

/// A dominating branch condition constrains the value in the block it guards —
/// the `Q.DC && Q.DT` arm of `computeKnownBitsFromContext`.
///
/// No direct upstream counterpart as a unit test; `DomConditionCache` is
/// exercised upstream through `InstCombine`. The reasoning is upstream's.
#[test]
fn known_bits_from_a_dominating_branch_condition() {
    let module = parse(
        r"
define i32 @guarded(i32 %x, i1 %unused) {
entry:
  %c = icmp ult i32 %x, 4
  br i1 %c, label %then, label %else

then:
  %use = add i32 %x, 0
  ret i32 %use

else:
  ret i32 0
}
",
    );
    let data_layout = module.data_layout();
    let function = defined_function(&module);
    let dominator_tree = dominator_tree(&module);
    let mut conditions = DomConditionCache::new();
    let branch = function
        .basic_blocks()
        .next()
        .and_then(|block| block.instructions().last())
        .expect("the entry block has a terminator");
    conditions.register_branch(branch.to_erased());

    let context = instruction(&module, "use");
    let known = compute_known_bits(
        parameter(&module, 0),
        &ValueTrackingQuery::new(&data_layout)
            .with_context_instruction(&context)
            .with_dominator_tree(&dominator_tree)
            .with_dominating_conditions(&conditions),
    )
    .expect("known bits");
    assert_eq!(known.count_min_leading_zeros(), 30);
}

/// An injected condition constrains the value without any branch at all — the
/// `Q.CC` arm of `computeKnownBitsFromContext`.
///
/// No direct upstream counterpart as a unit test; `CondContext` is exercised
/// upstream through `SimplifyWithOpReplaced`. The reasoning is upstream's.
#[test]
fn known_bits_from_an_injected_condition() {
    let module = parse(
        r"
define i32 @injected(i32 %x) {
entry:
  %c = icmp ult i32 %x, 4
  %use = add i32 %x, 0
  ret i32 %use
}
",
    );
    let data_layout = module.data_layout();
    let context = instruction(&module, "use");
    let condition = CondContext::new(named(&module, "c"));
    let known = compute_known_bits(
        parameter(&module, 0),
        &ValueTrackingQuery::new(&data_layout)
            .with_context_instruction(&context)
            .with_condition_context(&condition),
    )
    .expect("known bits");
    assert_eq!(known.count_min_leading_zeros(), 30);
}

// --------------------------------------------------------------------------
// adjustKnownBitsForSelectArm
// --------------------------------------------------------------------------

/// Ports the `select_condition_implies_highbits_op1` /
/// `select_condition_implies_highbits_op1_maybe_undef_fail` pair from
/// `llvm/test/Analysis/ValueTracking/knownbits-select-from-cond.ll`. The two
/// fixtures differ only in the `noundef` on `%y`, and only the first folds its
/// `add i8 %sel, 32` to `or disjoint` — so `noundef` is exactly the difference
/// `adjustKnownBitsForSelectArm`'s closing `isGuaranteedNotToBeUndef` makes.
#[test]
fn select_arm_takes_high_bits_from_its_condition() {
    let with_noundef = parse(
        r"
define i8 @select_condition_implies_highbits_op1(i8 %xx, i8 noundef %y) {
  %x = and i8 %xx, 15
  %cond = icmp ult i8 %y, 3
  %sel = select i1 %cond, i8 %y, i8 %x
  %r = add i8 %sel, 32
  ret i8 %r
}
",
    );
    let without_noundef = parse(
        r"
define i8 @select_condition_implies_highbits_op1_maybe_undef_fail(i8 %xx, i8 %y) {
  %x = and i8 %xx, 15
  %cond = icmp ult i8 %y, 3
  %sel = select i1 %cond, i8 %y, i8 %x
  %r = add i8 %sel, 32
  ret i8 %r
}
",
    );

    for (module, expected_leading_zeros) in [(&with_noundef, 6), (&without_noundef, 0)] {
        let data_layout = module.data_layout();
        let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);
        let arm = parameter(module, 1);
        let adjusted = adjust_known_bits_for_select_arm(
            KnownBits::unknown(8),
            named(module, "cond"),
            arm,
            false,
            &query,
        )
        .expect("select arm adjustment");
        assert_eq!(adjusted.count_min_leading_zeros(), expected_leading_zeros);
    }
}

// --------------------------------------------------------------------------
// findValuesAffectedByCondition and willNotFreeBetween
// --------------------------------------------------------------------------

/// `findValuesAffectedByCondition` over an assume's condition is what
/// `AssumptionCache::updateAffectedValues` calls, so the cache's own contract —
/// "`assumptionsFor(V)` returns the assumes that mention `V`" — is the oracle
/// (`llvm/lib/Analysis/AssumptionCache.cpp`).
///
/// `icmp eq (and %t1, 3), 0` affects the compare, the `and`, and — through the
/// `HasRHSC` arm that peels an `and` apart — `%t1` and the constant.
#[test]
fn values_affected_by_an_assumed_mask_equality() {
    let module = parse(
        r"
declare void @llvm.assume(i1)

define i32 @assume_add(i32 %a, i32 %b) {
  %t1 = add i32 %a, %b
  %last_two_digits = and i32 %t1, 3
  %t2 = icmp eq i32 %last_two_digits, 0
  call void @llvm.assume(i1 %t2)
  ret i32 %t1
}
",
    );
    let mut affected = Vec::new();
    find_values_affected_by_condition(named(&module, "t2"), true, |value| {
        if let Some(name) = value.name() {
            affected.push(name);
        }
    });
    affected.sort();
    affected.dedup();
    assert_eq!(affected, vec!["last_two_digits", "t1", "t2"]);

    // The cache reaches the same set: each affected value names the assume.
    let cache = AssumptionCache::new(defined_function(&module));
    for name in ["t1", "last_two_digits", "t2"] {
        assert_eq!(
            cache.assumptions_for(named(&module, name)).len(),
            1,
            "%{name} is affected by the assume"
        );
    }
    assert!(cache.assumptions_for(parameter(&module, 0)).is_empty());
}

/// A branch condition affects less than an assumed one: `AddCmpOperands` records
/// the right-hand operand only for an assume, and the walk descends through
/// `and`/`or` only for a branch. Same upstream source.
#[test]
fn a_branch_condition_affects_less_than_an_assumed_one() {
    let module = parse(
        r"
define void @test(i32 %x, i32 %y) {
  %c1 = icmp ult i32 %x, 10
  %c2 = icmp ult i32 %y, %x
  %both = and i1 %c1, %c2
  br i1 %both, label %then, label %else

then:
  ret void

else:
  ret void
}
",
    );
    let mut as_branch = Vec::new();
    find_values_affected_by_condition(named(&module, "both"), false, |value| {
        if let Some(name) = value.name() {
            as_branch.push(name);
        }
    });
    as_branch.sort();
    as_branch.dedup();
    // A branch condition descends *through* the `and` without recording it —
    // `AddAffected(V)` sits inside `if (IsAssume)`. Of the two legs, only `%c1`
    // compares against a constant, so `AddCmpOperands` records just its left
    // operand.
    assert_eq!(as_branch, vec!["x"]);

    let mut as_assume = Vec::new();
    find_values_affected_by_condition(named(&module, "both"), true, |value| {
        if let Some(name) = value.name() {
            as_assume.push(name);
        }
    });
    as_assume.sort();
    as_assume.dedup();
    // Does not descend through the `and`, so only the `and` itself.
    assert_eq!(as_assume, vec!["both"]);
}

/// `willNotFreeBetween` needs `nosync` on the enclosing function and `nofree`
/// on every call in the range. The `nofree nosync` pair on upstream's `@test4`
/// in `llvm/test/Analysis/ValueTracking/assume.ll` is there for exactly this
/// predicate; the fixture is reduced to the range it scans.
#[test]
fn will_not_free_between_needs_nosync_and_nofree_calls() {
    let source = |attributes: &str, call_attributes: &str| {
        format!(
            r"
declare void @llvm.assume(i1)
declare void @opaque()

define i32 @test(i32 %x) {attributes} {{
  call void @llvm.assume(i1 true)
  call void @opaque() {call_attributes}
  %use = add i32 %x, 0
  ret i32 %use
}}
"
        )
    };

    // `nosync` on the function and `nofree` on the intervening call: nothing in
    // between can free.
    let module = parse(&source("nosync", "nofree"));
    assert!(will_not_free_between(
        &assume(&module, 0),
        &instruction(&module, "use")
    ));

    // Drop `nofree` from the call and the scan finds something that might free.
    let module = parse(&source("nosync", ""));
    assert!(!will_not_free_between(
        &assume(&module, 0),
        &instruction(&module, "use")
    ));

    // Drop `nosync` and another thread might free on this function's behalf.
    let module = parse(&source("", "nofree"));
    assert!(!will_not_free_between(
        &assume(&module, 0),
        &instruction(&module, "use")
    ));
}
