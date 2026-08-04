//! Ports of the value-level `ValueTracking.h` predicates — tranche 1 of the
//! `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! Two upstream sources drive these:
//!
//! - `TEST_F(ValueTrackingTest, HaveNoCommonBitsSet)` in
//!   `llvm/unittests/Analysis/ValueTrackingTest.cpp`, whose IR is inlined here
//!   verbatim and whose API is called directly.
//! - `llvm/test/Analysis/ValueTracking/known-non-equal.ll` and
//!   `known-power-of-two.ll`, which drive `instsimplify` / `instcombine`. Each
//!   function there ends in a compare whose `CHECK` line records whether the
//!   fold fired; the fold's gate *is* the predicate under test, so the `CHECK`
//!   line is the oracle. Functions are inlined verbatim, unchanged.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, KnownBits, Module, Unverified, Value, ValueTrackingQuery,
    analyze_known_bits_from_and_xor_or, compute_known_bits, have_no_common_bits_set,
    is_known_non_equal, is_known_to_be_a_power_of_two,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The instruction or parameter named `%name` in the module's single function.
///
/// Parameters go through `Module::view` because the pass-layer `FunctionView`
/// exposes blocks but not parameters; only the `FunctionValue` handle has
/// `params()`.
fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    let view = module.as_view();
    let instruction = view
        .functions()
        .flat_map(|f| f.basic_blocks())
        .flat_map(|block| block.instructions())
        .map(|instruction| instruction.to_erased())
        .find(|value| value.name().as_deref() == Some(name));
    if let Some(instruction) = instruction {
        return instruction;
    }
    let ids: Vec<_> = view.functions().map(|f| f.id()).collect();
    ids.into_iter()
        .flat_map(|id| module.view(id).params())
        .map(|param| param.into_erased())
        .find(|value| value.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
}

/// The three blocks of `TEST_F(ValueTrackingTest, HaveNoCommonBitsSet)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// Upstream asserts both operand orders for every pair, which is what exercises
/// `haveNoCommonBitsSetSpecialCases` being tried twice.
///
/// The vector block spells its all-ones constant as `splat (i32 -1)` rather
/// than upstream's `<i32 -1, i32 -1>`: LLVM 22 prints splat constants in the
/// shorthand, so that is what llvmkit's printer round-trips to. The constant
/// is the same value.
#[test]
fn have_no_common_bits_set_fixtures() {
    /// One upstream block: a name, its IR, and the `%lhs` / `%rhs` pairs it
    /// asserts disjoint in both operand orders.
    type Block = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
    );

    let cases: &[Block] = &[
        (
            // Check for an inverted mask: (X & ~M) op (Y & M).
            "inverted mask",
            r"
define i32 @test(i32 %X, i32 %Y, i32 noundef %M) {
  %1 = xor i32 %M, -1
  %LHS = and i32 %1, %X
  %RHS = and i32 %Y, %M
  %Ret = add i32 %LHS, %RHS
  ret i32 %Ret
}
",
            &[("LHS", "RHS")],
        ),
        (
            // Check for (A & B) and ~(A | B).
            "and versus nor",
            r"
define void @test(i32 noundef %A, i32 noundef %B) {
  %LHS = and i32 %A, %B
  %or = or i32 %A, %B
  %RHS = xor i32 %or, -1

  %LHS2 = and i32 %B, %A
  %or2 = or i32 %A, %B
  %RHS2 = xor i32 %or2, -1

  ret void
}
",
            &[("LHS", "RHS"), ("LHS2", "RHS2")],
        ),
        (
            // Check for (A & B) and ~(A | B) in vector version.
            "and versus nor, vector",
            r"
define void @test(<2 x i32> noundef %A, <2 x i32> noundef %B) {
  %LHS = and <2 x i32> %A, %B
  %or = or <2 x i32> %A, %B
  %RHS = xor <2 x i32> %or, splat (i32 -1)

  %LHS2 = and <2 x i32> %B, %A
  %or2 = or <2 x i32> %A, %B
  %RHS2 = xor <2 x i32> %or2, splat (i32 -1)

  ret void
}
",
            &[("LHS", "RHS"), ("LHS2", "RHS2")],
        ),
    ];

    for (name, source, pairs) in cases {
        let module = parse(source);
        let data_layout = module.data_layout();
        let query = ValueTrackingQuery::new(&data_layout);
        for (lhs, rhs) in *pairs {
            let left = named(&module, lhs);
            let right = named(&module, rhs);
            assert!(
                have_no_common_bits_set(left, right, &query).expect("query"),
                "{name}: haveNoCommonBitsSet(%{lhs}, %{rhs})"
            );
            assert!(
                have_no_common_bits_set(right, left, &query).expect("query"),
                "{name}: haveNoCommonBitsSet(%{rhs}, %{lhs})"
            );
        }
    }
}

/// Functions from `llvm/test/Analysis/ValueTracking/known-non-equal.ll`, IR
/// unchanged.
///
/// Each ends in `icmp eq %lhs, %rhs`. `instsimplify` folds that compare to
/// `false` exactly when `isKnownNonEqual` proves the operands differ, so the
/// `CHECK` line is the expectation: `ret i1 false` means true here, and a
/// `CHECK` that leaves the compare standing means false.
///
/// The `nsw`/`nuw` flags in these fixtures are written in the source rather
/// than inferred, so no missing transform stands between the IR and the
/// analysis.
#[test]
fn known_non_equal_fixtures() {
    // (upstream function name, source, lhs, rhs, folded by upstream)
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        (
            "test2",
            r"
define i1 @test2(i8 %a, i8 %b) {
  %A = or i8 %a, 2
  %B = and i8 %b, -3
  %cmp = icmp eq i8 %A, %B
  ret i1 %cmp
}
",
            "A",
            "B",
            true,
        ),
        (
            "test3",
            r"
define i1 @test3(i8 %B) {
  %A = add nsw i8 %B, 1
  %cmp = icmp eq i8 %A, %B
  ret i1 %cmp
}
",
            "A",
            "B",
            true,
        ),
        (
            "add1",
            r"
define i1 @add1(i8 %B, i8 %C) {
  %A = add i8 %B, 1
  %A.op = add i8 %A, %C
  %B.op = add i8 %B, %C
  %cmp = icmp eq i8 %A.op, %B.op
  ret i1 %cmp
}
",
            "A.op",
            "B.op",
            true,
        ),
        (
            "sub1",
            r"
define i1 @sub1(i8 %B, i8 %C) {
  %A = add i8 %B, 1
  %A.op = sub i8 %A, %C
  %B.op = sub i8 %B, %C
  %cmp = icmp eq i8 %A.op, %B.op
  ret i1 %cmp
}
",
            "A.op",
            "B.op",
            true,
        ),
        (
            "mul_nuw",
            r"
define i1 @mul_nuw(i16 %x) {
  %nz = or i16 %x, 2
  %mul = mul nuw i16 %nz, 2
  %cmp = icmp eq i16 %nz, %mul
  ret i1 %cmp
}
",
            "nz",
            "mul",
            true,
        ),
        (
            "mul_nsw",
            r"
define i1 @mul_nsw(i16 %x) {
  %nz = or i16 %x, 2
  %mul = mul nsw i16 %nz, 2
  %cmp = icmp eq i16 %nz, %mul
  ret i1 %cmp
}
",
            "nz",
            "mul",
            true,
        ),
        (
            "mul_may_wrap",
            r"
define i1 @mul_may_wrap(i16 %x) {
  %nz = or i16 %x, 2
  %mul = mul i16 %nz, 2
  %cmp = icmp eq i16 %nz, %mul
  ret i1 %cmp
}
",
            "nz",
            "mul",
            false,
        ),
        (
            "shl_nuw",
            r"
define i1 @shl_nuw(i16 %x) {
  %nz = or i16 %x, 2
  %mul = shl nuw i16 %nz, 1
  %cmp = icmp eq i16 %nz, %mul
  ret i1 %cmp
}
",
            "nz",
            "mul",
            true,
        ),
        (
            "shl_may_wrap",
            r"
define i1 @shl_may_wrap(i16 %x) {
  %nz = or i16 %x, 2
  %mul = shl i16 %nz, 1
  %cmp = icmp eq i16 %nz, %mul
  ret i1 %cmp
}
",
            "nz",
            "mul",
            false,
        ),
        (
            "shl_shl_nuw",
            r"
define i1 @shl_shl_nuw(i8 %B, i8 %shift) {
  %A = add i8 %B, 1
  %A.op = shl nuw i8 %A, %shift
  %B.op = shl nuw i8 %B, %shift
  %cmp = icmp eq i8 %A.op, %B.op
  ret i1 %cmp
}
",
            "A.op",
            "B.op",
            true,
        ),
        (
            "ashr_ashr_exact",
            r"
define i1 @ashr_ashr_exact(i8 %B, i8 %shift) {
  %A = add i8 %B, 1
  %A.op = ashr exact i8 %A, %shift
  %B.op = ashr exact i8 %B, %shift
  %A.op2 = mul nuw i8 %A.op, 3
  %B.op2 = mul nuw i8 %B.op, 3
  %cmp = icmp eq i8 %A.op2, %B.op2
  ret i1 %cmp
}
",
            "A.op2",
            "B.op2",
            true,
        ),
        (
            "ashr_ashr_may_be_equal",
            r"
define i1 @ashr_ashr_may_be_equal(i8 %A, i8 %B, i8 %shift) {
  %A.op = ashr exact i8 %A, %shift
  %B.op = ashr exact i8 %B, %shift
  %A.op2 = mul nuw i8 %A.op, 3
  %B.op2 = mul nuw i8 %B.op, 3
  %cmp = icmp eq i8 %A.op2, %B.op2
  ret i1 %cmp
}
",
            "A.op2",
            "B.op2",
            false,
        ),
    ];

    for (name, source, lhs, rhs, expected) in cases {
        let module = parse(source);
        let data_layout = module.data_layout();
        let query = ValueTrackingQuery::new(&data_layout);
        let left = named(&module, lhs);
        let right = named(&module, rhs);
        assert_eq!(
            is_known_non_equal(left, right, &query).expect("query"),
            *expected,
            "@{name}: isKnownNonEqual(%{lhs}, %{rhs})"
        );
        // The relation is symmetric; upstream's helpers are all tried in both
        // orders, so both directions have to agree.
        assert_eq!(
            is_known_non_equal(right, left, &query).expect("query"),
            *expected,
            "@{name}: isKnownNonEqual(%{rhs}, %{lhs})"
        );
    }
}

/// The `shl` group of `llvm/test/Analysis/ValueTracking/known-power-of-two.ll`,
/// IR unchanged.
///
/// `instcombine` rewrites `icmp eq (and Y, XX), XX` to `icmp ne (and Y, XX), 0`
/// only when `isKnownToBeAPowerOfTwo(XX, /*OrZero=*/false)` holds, so the
/// `CHECK` line records the predicate.
///
/// `@shl_is_pow2` is the group's positive case and is asserted **false** here,
/// not true: its `shl` carries no `nuw`/`nsw` in the source, and upstream only
/// reaches `true` because `instcombine` infers those flags first — the printed
/// `CHECK` line shows `shl nuw nsw`. llvmkit has no flag inference, so the
/// analysis sees the plain `shl` its own fixture wrote. Closing that gap is a
/// missing transform, not a missing analysis; the assertion is written to trip
/// if the gap ever closes.
#[test]
fn known_power_of_two_shl_fixtures() {
    // (upstream function name, source, value, proven a power of two)
    let cases: &[(&str, &str, &str, bool)] = &[
        (
            "shl_is_pow2",
            r"
define i1 @shl_is_pow2(i16 %x, i16 %y) {
  %xsmall = and i16 %x, 7
  %xx = shl i16 4, %xsmall
  %and = and i16 %y, %xx
  %r = icmp eq i16 %and, %xx
  ret i1 %r
}
",
            "xx",
            false,
        ),
        (
            "shl_is_pow2_fail",
            r"
define i1 @shl_is_pow2_fail(i16 %x, i16 %y) {
  %xsmall = and i16 %x, 7
  %xx = shl i16 512, %xsmall
  %and = and i16 %y, %xx
  %r = icmp eq i16 %and, %xx
  ret i1 %r
}
",
            "xx",
            false,
        ),
        (
            "shl_is_pow2_fail2",
            r"
define i1 @shl_is_pow2_fail2(i16 %x, i16 %y) {
  %xsmall = and i16 %x, 7
  %xx = shl i16 5, %xsmall
  %and = and i16 %y, %xx
  %r = icmp eq i16 %and, %xx
  ret i1 %r
}
",
            "xx",
            false,
        ),
    ];

    for (name, source, value, expected) in cases {
        let module = parse(source);
        let data_layout = module.data_layout();
        let query = ValueTrackingQuery::new(&data_layout);
        let target = named(&module, value);
        assert_eq!(
            is_known_to_be_a_power_of_two(target, false, &query).expect("query"),
            *expected,
            "@{name}: isKnownToBeAPowerOfTwo(%{value}, OrZero=false)"
        );
    }
}

/// `shl i16 1, %x` is a power of two unconditionally, and `lshr i16 -32768, %x`
/// likewise — the two pattern matches `isKnownToBeAPowerOfTwo` performs before
/// it ever consults an opcode arm.
///
/// No upstream counterpart: every fixture in `known-power-of-two.ll` reaches
/// these two shapes only after `instcombine` has canonicalised something into
/// them, so none of them can be inlined here as-written. The IR below is the
/// literal shape of upstream's `m_Shl(m_One(), m_Value())` and
/// `m_LShr(m_SignMask(), m_Value())` matches.
#[test]
fn one_shifted_left_and_sign_mask_shifted_right_are_powers_of_two() {
    let module = parse(
        r"
define void @test(i16 %x) {
  %shifted_one = shl i16 1, %x
  %shifted_sign_mask = lshr i16 -32768, %x
  %shifted_three = shl i16 3, %x
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);

    for name in ["shifted_one", "shifted_sign_mask"] {
        assert!(
            is_known_to_be_a_power_of_two(named(&module, name), false, &query).expect("query"),
            "%{name}"
        );
    }
    assert!(
        !is_known_to_be_a_power_of_two(named(&module, "shifted_three"), false, &query)
            .expect("query"),
        "%shifted_three is not a power of two"
    );
}

// --------------------------------------------------------------------------
// getKnownBitsFromAndXorOr — the two idiom arms, and the public entry point
// --------------------------------------------------------------------------

/// `and(x, -x)` isolates the lowest set bit, so the answer is `KnownBits::blsi`
/// of whichever operand has the fewer possible trailing zeros.
///
/// No upstream test isolates this arm: LLVM reaches it through InstCombine's
/// demanded-bits machinery, and the nearest fixtures
/// (`test/Transforms/InstCombine/ispow2.ll`) go through
/// `isKnownToBeAPowerOfTwo` instead. The expectation here is not derived by
/// hand — it is upstream's own formula, `KnownLHS.blsi()`, applied to the
/// operand bits `compute_known_bits` reports, so what the test pins is that the
/// matcher routes to `blsi` at all.
#[test]
fn and_of_a_value_and_its_negation_isolates_the_lowest_set_bit() {
    let module = parse(
        r"
define i8 @blsi(i8 %a) {
  %x = or i8 %a, 4
  %negated = sub i8 0, %x
  %isolated = and i8 %x, %negated
  ret i8 %isolated
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    // `or %a, 4` puts a known one at bit 2, which is what both idiom arms need.
    let source = compute_known_bits(named(&module, "x"), &query).expect("known bits");
    assert!(source.is_known_one(2), "the or pins bit 2");

    let isolated = compute_known_bits(named(&module, "isolated"), &query).expect("known bits");
    assert_eq!(isolated, source.blsi());

    // Without the arm the plain `and` of the two would say nothing about the
    // bits above the lowest one; `blsi` clears them.
    assert!(isolated.count_min_leading_zeros() > 0);
}

/// `xor(x, x - 1)` sets every bit up to and including the lowest set one, so
/// the answer is `KnownBits::blsmsk` of `x`.
///
/// Same provenance note as the `blsi` case above: no upstream test isolates the
/// arm, and the expectation is upstream's own `XBits.blsmsk()`.
#[test]
fn xor_of_a_value_and_its_predecessor_masks_the_low_bits() {
    let module = parse(
        r"
define i8 @blsmsk(i8 %a) {
  %x = or i8 %a, 4
  %minus_one = add i8 %x, -1
  %mask = xor i8 %x, %minus_one
  ret i8 %mask
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    let source = compute_known_bits(named(&module, "x"), &query).expect("known bits");
    let mask = compute_known_bits(named(&module, "mask"), &query).expect("known bits");
    assert_eq!(mask, source.blsmsk());
    assert!(mask.count_min_leading_zeros() > 0);
}

/// The public entry point answers the same thing as the operator walk when it
/// is handed the operand bits that walk would have computed, and declines a
/// value that is not an `and` / `or` / `xor`.
///
/// Ports `llvm::analyzeKnownBitsFromAndXorOr`, which upstream exposes so
/// `SimplifyDemandedUseBits` can reuse the reasoning with bits it has already
/// narrowed. Upstream has no unit test for it; the agreement asserted here is
/// the contract that makes the sharing sound.
#[test]
fn analyze_known_bits_from_and_xor_or_agrees_with_the_operator_walk() {
    let module = parse(
        r"
define void @ops(i8 %a, i8 %b) {
  %x = or i8 %a, 4
  %negated = sub i8 0, %x
  %isolated = and i8 %x, %negated
  %plain = xor i8 %a, %b
  %not_bitwise = add i8 %a, %b
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query: ValueTrackingQuery<'_, '_, DynBrand> = ValueTrackingQuery::new(&data_layout);

    for (name, lhs, rhs) in [("isolated", "x", "negated"), ("plain", "a", "b")] {
        let operation = named(&module, name);
        let known_lhs = compute_known_bits(named(&module, lhs), &query).expect("known bits");
        let known_rhs = compute_known_bits(named(&module, rhs), &query).expect("known bits");
        let direct =
            analyze_known_bits_from_and_xor_or(operation, &known_lhs, &known_rhs, &query, 0)
                .expect("no error")
                .expect("an and/or/xor");
        assert_eq!(
            direct,
            compute_known_bits(operation, &query).expect("known bits"),
            "%{name}"
        );
    }

    // An `add` is not one of the three; upstream reaches an `llvm_unreachable`.
    let unknown = KnownBits::unknown(8);
    assert_eq!(
        analyze_known_bits_from_and_xor_or(
            named(&module, "not_bitwise"),
            &unknown,
            &unknown,
            &query,
            0,
        )
        .expect("no error"),
        None
    );
}
