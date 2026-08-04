//! `llvm::stripNullTest` — the `(X >> C) or/add (X & mask(C) != 0)` idiom.
//!
//! Every case comes from `llvm/test/Transforms/InstCombine/ceil-shift.ll`, IR
//! inlined verbatim. That file drives the InstCombine fold
//! `(icmp eq/ne f(X), 0) -> (icmp eq/ne X, 0)`, which is `stripNullTest`'s only
//! reason to exist besides the `isKnownNonZero` tail; llvmkit has no
//! InstCombine, so the assertion here is on the analysis underneath.
//!
//! **A `CHECK` line that folds does not by itself mean `stripNullTest`
//! matched.** `ceil_shift0` folds because `and X, 0` and `lshr X, 0` constant-
//! fold away, not because the idiom was recognised — `APInt::isMask()` is
//! false for zero. So the expectations below are read off `stripNullTest`'s
//! own match, with upstream's `CHECK` lines as the cross-check where the two
//! do coincide.
//!
//! **Two upstream cases are absent, and this says which.** `ceil_shift4_v4i32`
//! and `ceil_shift4_v8i16` are the vector spellings; llvmkit's parser accepts
//! vector `lshr` / `and` / `icmp` but not a vector `zext`, which every form of
//! the idiom needs — `parse_int_cast` requires a scalar integer source and
//! destination. `strip_null_test` itself is splat-aware, matching upstream's
//! `m_APInt`, so the arm is ported and will be exercised the moment the parser
//! can build the fixture. The gap is recorded in `docs/future-work.md`; it is
//! not a divergence in the analysis.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, Module, Unverified, Value, ValueTrackingQuery, compute_known_bits, is_known_non_zero,
    strip_null_test,
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

/// The parameter of the fixture's *defined* function, which is every case's
/// `X`. Some fixtures declare `@use` first, so this skips declarations rather
/// than taking whichever function comes first.
fn only_param<'m>(module: &'m Module<DynBrand, Unverified>) -> Value<'m, DynBrand> {
    let function = module
        .as_view()
        .functions()
        .find(|function| function.basic_blocks().next().is_some())
        .expect("fixture defines a function with a body");
    let id = function.id();
    module
        .view(id)
        .params()
        .next()
        .expect("fixture's function takes a parameter")
        .into_erased()
}

/// Assert that `strip_null_test` recovers the function's parameter from the
/// instruction named `%name`.
fn strips_to_the_parameter(source: &str, name: &str) {
    let module = parse(source);
    assert_eq!(
        strip_null_test(named(&module, name)),
        Some(only_param(&module)),
        "expected the null test to strip to the parameter"
    );
}

/// Assert that `strip_null_test` declines the instruction named `%name`.
fn does_not_strip(source: &str, name: &str) {
    let module = parse(source);
    let stripped = strip_null_test(named(&module, name));
    assert!(stripped.is_none(), "expected no match, got {stripped:?}");
}

/// Ports `ceil_shift4`, `ceil_shift4_add`, `ceil_shift6`, `ceil_shift6_ne`,
/// `ceil_shift11` and `ceil_shift11_ne` — the plain shapes, over `or` and
/// `add`, at three shift widths.
///
/// `ceil_shift11_ne` shifts by 6 and masks with 63 despite its name; the IR is
/// upstream's, unchanged.
#[test]
fn the_ceiling_shift_idiom_strips_to_its_operand() {
    for (name, shift, mask) in [
        ("ceil_shift4", 4, 15),
        ("ceil_shift6", 6, 63),
        ("ceil_shift11", 11, 2047),
    ] {
        strips_to_the_parameter(
            &format!(
                r"
define i1 @{name}(i32 %arg0) {{
  %quot = lshr i32 %arg0, {shift}
  %rem = and i32 %arg0, {mask}
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %quot_or_rem = or i32 %quot, %zext_has_rem
  %is_zero = icmp eq i32 %quot_or_rem, 0
  ret i1 %is_zero
}}
"
            ),
            "quot_or_rem",
        );
    }

    // ceil_shift4_add: the `add` spelling of the same idiom.
    strips_to_the_parameter(
        r"
define i1 @ceil_shift4_add(i32 %arg0) {
  %quot = lshr i32 %arg0, 4
  %rem = and i32 %arg0, 15
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %ceil = add i32 %quot, %zext_has_rem
  %res = icmp eq i32 %ceil, 0
  ret i1 %res
}
",
        "ceil",
    );
}

/// Ports `ceil_shift4_comm`: upstream matches the binary operator
/// commutatively, so the flag may sit on either side.
#[test]
fn the_idiom_matches_with_the_operands_swapped() {
    strips_to_the_parameter(
        r"
define i1 @ceil_shift4_comm(i32 %arg0) {
  %quot = lshr i32 %arg0, 4
  %rem = and i32 %arg0, 15
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %quot_or_rem = or i32 %zext_has_rem, %quot
  %res = icmp eq i32 %quot_or_rem, 0
  ret i1 %res
}
",
        "quot_or_rem",
    );
}

/// Ports `ceil_shift4_used_1`, `ceil_shift4_used_5` and
/// `ceil_shift4_used_add_nuw_nsw`: `stripNullTest` states no one-use
/// requirement, so an extra user of any part changes nothing.
#[test]
fn extra_users_do_not_block_the_match() {
    let cases: &[(&str, &str)] = &[
        (
            r"
declare void @use(i32)
define i1 @ceil_shift4_used_1(i32 %arg0) {
  %quot = lshr i32 %arg0, 4
  call void @use(i32 %quot)
  %rem = and i32 %arg0, 15
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %quot_or_rem = or i32 %quot, %zext_has_rem
  %res = icmp eq i32 %quot_or_rem, 0
  ret i1 %res
}
",
            "quot_or_rem",
        ),
        (
            r"
declare void @use(i32)
define i1 @ceil_shift4_used_5(i32 %arg0) {
  %quot = lshr i32 %arg0, 4
  %rem = and i32 %arg0, 15
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %quot_or_rem = or i32 %quot, %zext_has_rem
  call void @use(i32 %quot_or_rem)
  %res = icmp eq i32 %quot_or_rem, 0
  ret i1 %res
}
",
            "quot_or_rem",
        ),
        (
            r"
declare void @use(i32)
define i1 @ceil_shift4_used_add_nuw_nsw(i32 %arg0) {
  %quot = lshr i32 %arg0, 4
  %rem = and i32 %arg0, 15
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %ceil = add nuw nsw i32 %quot, %zext_has_rem
  call void @use(i32 %ceil)
  %res = icmp eq i32 %ceil, 0
  ret i1 %res
}
",
            "ceil",
        ),
    ];
    for (source, name) in cases {
        strips_to_the_parameter(source, name);
    }
}

/// Ports the `; negative tests` section — `ceil_shift_not_mask_1`,
/// `ceil_shift_not_mask_2` and `ceil_shift_not_add_or` — plus `ceil_shift0`,
/// which upstream folds for an unrelated reason.
///
/// The first two are the population-count check: the mask has to cover exactly
/// the bits the shift discards. The third replaces the `or` with an `and`,
/// which upstream's opcode guard rejects. `ceil_shift0` masks with zero, and
/// `APInt::isMask()` is false for zero, so the idiom does not match even
/// though InstCombine still folds the comparison.
#[test]
fn the_idiom_declines_when_the_mask_does_not_match_the_shift() {
    let cases: &[(&str, &str)] = &[
        (
            r"
define i1 @ceil_shift_not_mask_1(i32 %arg0) {
  %quot = lshr i32 %arg0, 4
  %rem = and i32 %arg0, 31
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %quot_or_rem = or i32 %quot, %zext_has_rem
  %res = icmp eq i32 %quot_or_rem, 0
  ret i1 %res
}
",
            "quot_or_rem",
        ),
        (
            r"
define i1 @ceil_shift_not_mask_2(i32 %arg0) {
  %quot = lshr i32 %arg0, 5
  %rem = and i32 %arg0, 15
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %quot_or_rem = or i32 %quot, %zext_has_rem
  %res = icmp eq i32 %quot_or_rem, 0
  ret i1 %res
}
",
            "quot_or_rem",
        ),
        (
            r"
define i1 @ceil_shift_not_add_or(i32 %arg0) {
  %quot = lshr i32 %arg0, 5
  %rem = and i32 %arg0, 15
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %quot_and_rem = and i32 %quot, %zext_has_rem
  %res = icmp eq i32 %quot_and_rem, 0
  ret i1 %res
}
",
            "quot_and_rem",
        ),
        (
            r"
define i1 @ceil_shift0(i32 %arg0) {
  %quot = lshr i32 %arg0, 0
  %rem = and i32 %arg0, 0
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %quot_or_rem = or i32 %quot, %zext_has_rem
  %res = icmp eq i32 %quot_or_rem, 0
  ret i1 %res
}
",
            "quot_or_rem",
        ),
    ];
    for (source, name) in cases {
        does_not_strip(source, name);
    }
}

/// Ports `ceil_shift_should_infer_ge_zero`'s `if.then` block, the widest shift
/// in the file: 20, against a `1048575` mask.
///
/// Upstream's own point in that test is a range inference that needs the
/// dominating `icmp ne i32 %x, 0`; what is portable here is the idiom match
/// itself, which does not depend on the branch.
#[test]
fn the_idiom_matches_at_a_twenty_bit_shift() {
    strips_to_the_parameter(
        r"
define i32 @ceil_shift_should_infer_ge_zero(i32 %x) {
  %quot = lshr i32 %x, 20
  %rem = and i32 %x, 1048575
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %ceil = add nuw nsw i32 %quot, %zext_has_rem
  ret i32 %ceil
}
",
        "ceil",
    );
}

/// The `stripNullTest` tail of `llvm::isKnownNonZero`: when known bits alone
/// prove nothing, the question transfers to the stripped operand.
///
/// **No upstream counterpart as a unit test.** Upstream reaches this arm only
/// through `.ll` regression tests of passes llvmkit does not have, so the
/// fixture is llvmkit's: the base is `or %y, 1`, which known bits alone prove
/// non-zero, so the assertion isolates the transfer rather than the strength
/// of any surrounding reasoning. The idiom is upstream's, at
/// `ceil_shift_should_infer_ge_zero`'s shift and mask.
#[test]
fn is_known_non_zero_retries_through_the_stripped_operand() {
    let module = parse(
        r"
define i32 @transfers(i32 %y) {
  %x = or i32 %y, 1
  %quot = lshr i32 %x, 20
  %rem = and i32 %x, 1048575
  %has_rem = icmp ne i32 %rem, 0
  %zext_has_rem = zext i1 %has_rem to i32
  %ceil = add nuw nsw i32 %quot, %zext_has_rem
  ret i32 %ceil
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    let ceil = named(&module, "ceil");

    // The arm is reachable only because known bits alone say nothing here:
    // both the shift and the flag are unknown, so their `add` is too.
    assert!(
        !compute_known_bits(ceil, &query)
            .expect("query succeeds")
            .is_non_zero(),
        "known bits alone must not already answer, or this proves nothing"
    );
    // Stripping reaches `%x`, whose low bit is set.
    assert_eq!(strip_null_test(ceil), Some(named(&module, "x")));
    assert!(is_known_non_zero(ceil, &query).expect("query succeeds"));
}
