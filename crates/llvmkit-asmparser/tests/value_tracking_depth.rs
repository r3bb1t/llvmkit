//! Where `computeKnownBits`' recursion cutoff sits, and what it lets through.
//!
//! **No upstream counterpart.** `MaxAnalysisRecursionDepth` is not exercised by
//! any `llvm/test` fixture or `ValueTrackingTest.cpp` case — reaching it needs
//! an operator chain deeper than the limit, and no fixture is written that way.
//! The rule is read off `llvm/lib/Analysis/ValueTracking.cpp::computeKnownBits`
//! directly, where two statements settle it:
//!
//! ```text
//!   const APInt *C;
//!   if (match(V, m_APInt(C))) { Known = KnownBits::makeConstant(*C); return; }
//!   ...
//!   // All recursive calls that increase depth must come after this.
//!   if (Depth == MaxAnalysisRecursionDepth)
//!     return;
//! ```
//!
//! So the cutoff is `==`, not `>` — a value reached at depth
//! `MaxAnalysisRecursionDepth` contributes nothing through the operator walk —
//! and it sits **after** the constant fast path, so a constant reports its
//! exact value at any depth. `MaxAnalysisRecursionDepth` is 6 in
//! `llvm/include/llvm/Analysis/ValueTracking.h`, which is
//! `llvmkit_ir::MAX_ANALYSIS_RECURSION_DEPTH`.

use llvmkit_asmparser::parser;
use llvmkit_ir::{DynBrand, Module, Unverified, ValueTrackingQuery, compute_known_bits};

fn known_leading_zeros(source: &str, function: &str, name: &str) -> u32 {
    let module: Module<DynBrand, Unverified> =
        parser::parse_dynamic(source).expect("fixture parses");
    let f = module
        .as_view()
        .functions()
        .find(|f| f.name() == function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    let value = f
        .basic_blocks()
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("@{function} defines %{name}"))
        .to_erased();
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    compute_known_bits(value, &query)
        .expect("known bits")
        .count_min_leading_zeros()
}

/// An operator seven levels down is past upstream's cutoff and contributes
/// nothing.
///
/// `%v6` is a `zext i8 -> i32`, which proves 24 leading zeros *without reading
/// its operand*, so it is the cheapest way to ask "was this level walked at
/// all". The six `add ..., 0` levels above it preserve whatever `%v6` proved.
/// `%v6` sits at depth 6 — `MaxAnalysisRecursionDepth` — so upstream's
/// `Depth == MaxAnalysisRecursionDepth` return fires before
/// `computeKnownBitsFromOperator` sees it and nothing is proved.
#[test]
fn an_operator_at_the_recursion_limit_is_not_walked() {
    let source = "\
define i32 @f(i8 %y) {
entry:
  %v6 = zext i8 %y to i32
  %v5 = add i32 %v6, 0
  %v4 = add i32 %v5, 0
  %v3 = add i32 %v4, 0
  %v2 = add i32 %v3, 0
  %v1 = add i32 %v2, 0
  %v0 = add i32 %v1, 0
  ret i32 %v0
}
";
    assert_eq!(llvmkit_ir::MAX_ANALYSIS_RECURSION_DEPTH, 6);
    // %v0 is depth 0, so %v6 is depth 6 — the limit.
    assert_eq!(known_leading_zeros(source, "f", "v0"), 0);
    // One level shallower, the same `zext` is depth 5 and is walked.
    assert_eq!(known_leading_zeros(source, "f", "v1"), 24);
}

/// A **constant** operand at the recursion limit still reports its exact value,
/// because upstream's `m_APInt` fast path returns before the cutoff.
///
/// `%v5 = and i32 %x, 15` sits at depth 5, so it is walked; its constant
/// operand `15` sits at depth 6. If the cutoff were asked before the constant
/// fast path, that operand would come back unknown and the `and` would prove
/// nothing.
///
/// This one is a **coupling** guard, not a bug-catcher: it passed before the
/// cutoff was corrected too, because the old guard was `depth > max_depth` and
/// `6 > 6` is false. What it pins is that tightening the comparison to `>=`
/// cannot be done without also moving the constant fast path above it — with
/// the old placement and the new comparison, this assertion reads 0.
#[test]
fn a_constant_at_the_recursion_limit_still_reports_its_value() {
    let source = "\
define i32 @g(i32 %x) {
entry:
  %v5 = and i32 %x, 15
  %v4 = add i32 %v5, 0
  %v3 = add i32 %v4, 0
  %v2 = add i32 %v3, 0
  %v1 = add i32 %v2, 0
  %v0 = add i32 %v1, 0
  ret i32 %v0
}
";
    assert_eq!(known_leading_zeros(source, "g", "v0"), 28);
}
