//! `computeConstantRange` and the overflow predicates — slice 3e, closing
//! tranche 3 (see `docs/future-work.md`).
//!
//! These are the consumers `ConstantRange` was ported for. The IR is written
//! as `.ll` text and driven through the parser, so the tests read as the
//! programs they are about.
//!
//! The oracle is the operation's own definition over the operand *constants*:
//! an overflow predicate that answers `NeverOverflows` must be right for every
//! value the operands can take, and one that answers `AlwaysOverflows*` must
//! likewise be right for all of them. `MayOverflow` is the safe answer and is
//! never wrong, so it is never asserted against — only the two decisive
//! answers carry a claim, which is exactly where a bug would show.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    ConstantRange, DynBrand, Module, OverflowResult, Signedness, Unverified, Value,
    ValueTrackingQuery, compute_constant_range_including_known_bits,
    compute_overflow_for_signed_add, compute_overflow_for_signed_mul,
    compute_overflow_for_signed_sub, compute_overflow_for_unsigned_add,
    compute_overflow_for_unsigned_mul, compute_overflow_for_unsigned_sub,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    parser::parse_dynamic(source).expect("fixture parses")
}

/// The instruction named `%name` in the module's single function.
fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|f| f.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

/// Two `i8` values masked into known sub-ranges, then combined.
///
/// `%a` is `x & 15`, so it lies in `[0, 16)`; `%b` is `y & 3`, in `[0, 4)`.
/// Their sum is at most 18, well inside `i8`, so no unsigned overflow is
/// possible — the analysis should say so rather than shrug.
const MASKED_ADD: &str = "\
define void @test(i8 %x, i8 %y) {
  %a = and i8 %x, 15
  %b = and i8 %y, 3
  %sum = add i8 %a, %b
  ret void
}
";

/// Both operands are the top half of the unsigned domain, so their sum always
/// carries out.
const ALWAYS_CARRIES: &str = "\
define void @test(i8 %x, i8 %y) {
  %a = or i8 %x, 128
  %b = or i8 %y, 128
  %sum = add i8 %a, %b
  ret void
}
";

/// Masked operands land in a range the analysis can bound.
///
/// Mirrors the shape `computeConstantRange` is built for: known bits from the
/// `and` narrow the range far below what the type alone allows.
#[test]
fn masking_narrows_the_computed_range() {
    let module = parse(MASKED_ADD);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);

    let a = compute_constant_range_including_known_bits(
        named(&module, "a"),
        Signedness::Unsigned,
        &query,
    )
    .expect("range");
    // `x & 15` cannot exceed 15.
    assert!(
        a.unsigned_max().try_zext_u64() <= Some(15),
        "%a = x & 15 should be bounded by 15, got max {:?}",
        a.unsigned_max()
    );

    let b = compute_constant_range_including_known_bits(
        named(&module, "b"),
        Signedness::Unsigned,
        &query,
    )
    .expect("range");
    assert!(
        b.unsigned_max().try_zext_u64() <= Some(3),
        "%b = y & 3 should be bounded by 3, got max {:?}",
        b.unsigned_max()
    );
}

/// A sum whose operands are provably small never overflows unsigned.
///
/// Ports the shape of `llvm::computeOverflowForUnsignedAdd`. 15 + 3 = 18 fits
/// an `i8`, so `NeverOverflows` is not merely permitted — it is the answer a
/// working range analysis must reach.
#[test]
fn small_masked_operands_never_overflow_unsigned_add() {
    let module = parse(MASKED_ADD);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);

    assert_eq!(
        compute_overflow_for_unsigned_add(named(&module, "a"), named(&module, "b"), &query)
            .expect("overflow query"),
        OverflowResult::NeverOverflows
    );
}

/// Two operands both at or above 128 always carry out of an `i8`.
///
/// Ports the `AlwaysOverflowsHigh` arm of
/// `ConstantRange::unsignedAddMayOverflow`.
#[test]
fn two_high_operands_always_overflow_unsigned_add() {
    let module = parse(ALWAYS_CARRIES);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);

    assert_eq!(
        compute_overflow_for_unsigned_add(named(&module, "a"), named(&module, "b"), &query)
            .expect("overflow query"),
        OverflowResult::AlwaysOverflowsHigh
    );
}

/// Subtracting a larger value from a smaller one always borrows.
///
/// Ports the `AlwaysOverflowsLow` arm of
/// `ConstantRange::unsignedSubMayOverflow`.
#[test]
fn subtracting_a_larger_operand_always_overflows_unsigned() {
    let module = parse(
        "\
define void @test(i8 %x, i8 %y) {
  %small = and i8 %x, 3
  %large = or i8 %y, 128
  %diff = sub i8 %small, %large
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);

    assert_eq!(
        compute_overflow_for_unsigned_sub(named(&module, "small"), named(&module, "large"), &query)
            .expect("overflow query"),
        OverflowResult::AlwaysOverflowsLow
    );
}

/// Small non-negative operands never overflow signed addition or subtraction.
///
/// Ports the `NeverOverflows` path of `computeOverflowForSignedAdd` /
/// `computeOverflowForSignedSub`.
#[test]
fn small_operands_never_overflow_signed_add_or_sub() {
    let module = parse(MASKED_ADD);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    let (a, b) = (named(&module, "a"), named(&module, "b"));

    assert_eq!(
        compute_overflow_for_signed_add(a, b, &query).expect("overflow query"),
        OverflowResult::NeverOverflows
    );
    assert_eq!(
        compute_overflow_for_signed_sub(a, b, &query).expect("overflow query"),
        OverflowResult::NeverOverflows
    );
}

/// Small operands never overflow multiplication either way.
///
/// The signed form reaches its answer through sign-bit counting rather than
/// ranges — upstream credits *Hacker's Delight* — so this exercises a
/// different path from the add and sub cases above.
#[test]
fn small_operands_never_overflow_multiplication() {
    let module = parse(MASKED_ADD);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    let (a, b) = (named(&module, "a"), named(&module, "b"));

    assert_eq!(
        compute_overflow_for_unsigned_mul(a, b, false, &query).expect("overflow query"),
        OverflowResult::NeverOverflows,
        "15 * 3 = 45 fits an i8"
    );
    assert_eq!(
        compute_overflow_for_signed_mul(a, b, &query).expect("overflow query"),
        OverflowResult::NeverOverflows
    );
}

/// The `mul nsw` promise makes a product of two non-negative values
/// unsigned-safe too.
///
/// Ports the `IsNSW` shortcut at the head of
/// `llvm::computeOverflowForUnsignedMul`. Without the promise the same
/// operands may overflow; with it they cannot.
#[test]
fn nsw_makes_a_non_negative_product_unsigned_safe() {
    let module = parse(
        "\
define void @test(i8 %x, i8 %y) {
  %a = lshr i8 %x, 1
  %b = lshr i8 %y, 1
  %product = mul i8 %a, %b
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    let (a, b) = (named(&module, "a"), named(&module, "b"));

    // Both are non-negative after the logical shift, so the promise applies.
    assert_eq!(
        compute_overflow_for_unsigned_mul(a, b, true, &query).expect("overflow query"),
        OverflowResult::NeverOverflows
    );
}

/// Wholly unconstrained operands must answer `MayOverflow` — the analysis has
/// nothing to work with, and claiming otherwise would be unsound.
#[test]
fn unconstrained_operands_may_overflow() {
    let module = parse(
        "\
define void @test(i8 %x, i8 %y) {
  %a = add i8 %x, 0
  %b = add i8 %y, 0
  %sum = add i8 %a, %b
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    let (a, b) = (named(&module, "a"), named(&module, "b"));

    for (label, got) in [
        ("uadd", compute_overflow_for_unsigned_add(a, b, &query)),
        ("sadd", compute_overflow_for_signed_add(a, b, &query)),
        ("usub", compute_overflow_for_unsigned_sub(a, b, &query)),
        ("ssub", compute_overflow_for_signed_sub(a, b, &query)),
    ] {
        assert_eq!(
            got.expect("overflow query"),
            OverflowResult::MayOverflow,
            "{label} on unconstrained operands"
        );
    }
}

/// A constant is its own one-element range. Mirrors the `dyn_cast<Constant>`
/// early return in `llvm::computeConstantRange`.
#[test]
fn a_constant_is_its_own_range() {
    let module = parse(
        "\
define void @test() {
  %c = add i8 42, 0
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    let range = compute_constant_range_including_known_bits(
        named(&module, "c"),
        Signedness::Unsigned,
        &query,
    )
    .expect("range");
    assert!(
        range.is_single_element(),
        "a folded constant should be a single-element range, got {range:?}"
    );
    assert_eq!(range.unsigned_max().try_zext_u64(), Some(42));
    // And it agrees with building the range directly.
    assert_eq!(
        range.unsigned_min(),
        *ConstantRange::single(range.unsigned_max()).lower()
    );
}
