//! Floating-point operations over vectors, through the parser.
//!
//! The FP sibling of `parser_vector_binops.rs` and `parser_vector_casts.rs`,
//! and for the same reason: llvmkit's typed float handles carry a *scalar*
//! `FloatKind`, so `<N x double>` has no typed handle and cannot route through
//! `IntoFloatValue`. `IrBuilder::build_fp_binop_erased`, `build_fp_cmp_erased`
//! and `build_fp_neg_erased` carry the shapes the typed family cannot.
//!
//! Upstream needs no such split: `LLParser::parseArithmetic` and
//! `parseCompare` hand their operands straight to `BinaryOperator::Create` and
//! `CmpInst::Create`.
//!
//! These lock the path end to end — the IR parses, **verifies**, and prints
//! back byte-identically. Parsing alone would not be enough; the sibling
//! vector-cast work shipped IR the verifier rejected until a round-trip test
//! caught it.
//!
//! The rejection cases come from `BinaryOperator::Create`'s assertion that both
//! operands share a type, and the verifier's `FloatOpNonFloatOperand` rule.

use llvmkit_asmparser::parser;

/// The indented lines of a function body — the instructions, in order.
///
/// Compared instead of the whole text because the printer materialises the
/// entry block's implicit label (`0:`), which the source omits.
fn body_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim_end)
        .filter(|line| line.starts_with("  "))
        .collect()
}

fn round_trip(source: &str) {
    let module = match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("parses: {error:?}\n--- source ---\n{source}"),
    };
    let verified = module
        .verify()
        .unwrap_or_else(|e| panic!("verifies: {e:?}\n--- source ---\n{source}"));
    let printed = format!("{verified}");
    assert_eq!(
        body_lines(&printed),
        body_lines(source),
        "round trip changed the instructions\n--- printed ---\n{printed}\n--- source ---\n{source}"
    );
}

fn rejects(source: &str, what: &str) {
    assert!(
        parser::parse_dynamic(source).is_err(),
        "expected {what} to be rejected, but it parsed\n--- source ---\n{source}"
    );
}

/// All five FP binary operators over a fixed vector.
///
/// llvmkit-specific in its spelling — no single upstream fixture spans the five
/// — but the rule is `BinaryOperator::Create`'s, which accepts a float vector
/// wherever it accepts a float.
#[test]
fn the_five_fp_binops_round_trip_over_a_fixed_vector() {
    round_trip(
        r"
define <4 x float> @all(<4 x float> %a, <4 x float> %b) {
  %s = fadd <4 x float> %a, %b
  %d = fsub <4 x float> %s, %b
  %m = fmul <4 x float> %d, %b
  %q = fdiv <4 x float> %m, %b
  %r = frem <4 x float> %q, %b
  ret <4 x float> %r
}
",
    );
}

/// The same over a scalable vector, whose lane count is a minimum — the shape
/// that first exposed the gap, while writing the scalable-splat printing
/// fixtures.
#[test]
fn fp_binops_round_trip_over_a_scalable_vector() {
    round_trip(
        r"
define <vscale x 2 x double> @scalable(<vscale x 2 x double> %a) {
  %s = fadd <vscale x 2 x double> %a, splat (double 1.500000e+00)
  ret <vscale x 2 x double> %s
}
",
    );
}

/// Fast-math flags survive on a vector operator, as they do on a scalar one —
/// `fadd` is an `FPMathOperator` whatever its operand shape.
#[test]
fn fast_math_flags_round_trip_on_a_vector_binop() {
    round_trip(
        r"
define <4 x float> @flags(<4 x float> %a, <4 x float> %b) {
  %f = fadd fast <4 x float> %a, %b
  %n = fmul nnan ninf <4 x float> %f, %b
  ret <4 x float> %n
}
",
    );
}

/// `fneg` over a vector, with and without flags.
#[test]
fn fneg_round_trips_over_a_vector() {
    round_trip(
        r"
define <4 x float> @neg(<4 x float> %a) {
  %n = fneg <4 x float> %a
  %m = fneg fast <4 x float> %n
  ret <4 x float> %m
}
",
    );
}

/// `fcmp` over a vector yields `<N x i1>`, matching the operand lane count.
#[test]
fn fcmp_round_trips_over_a_vector() {
    round_trip(
        r"
define <4 x i1> @cmp(<4 x float> %a, <4 x float> %b) {
  %c = fcmp oeq <4 x float> %a, %b
  ret <4 x i1> %c
}
",
    );
}

/// `fcmp` keeps its fast-math flags too — it is an `FPMathOperator` upstream,
/// so the same FMF slot applies.
#[test]
fn fcmp_keeps_fast_math_flags_over_a_vector() {
    round_trip(
        r"
define <2 x i1> @cmp(<2 x double> %a, <2 x double> %b) {
  %c = fcmp nnan olt <2 x double> %a, %b
  ret <2 x i1> %c
}
",
    );
}

/// Both operands must share a type — `BinaryOperator::Create`'s assertion.
#[test]
fn mismatched_vector_lane_counts_are_rejected() {
    rejects(
        r"
define <4 x float> @bad(<4 x float> %a, <2 x float> %b) {
  %s = fadd <4 x float> %a, %b
  ret <4 x float> %s
}
",
        "a 4-lane operand added to a 2-lane one",
    );
}

/// An integer vector is not a floating-point operand — the verifier's
/// `FloatOpNonFloatOperand` rule, applied at construction.
#[test]
fn an_integer_vector_is_not_an_fp_binop_operand() {
    rejects(
        r"
define <4 x i32> @bad(<4 x i32> %a, <4 x i32> %b) {
  %s = fadd <4 x i32> %a, %b
  ret <4 x i32> %s
}
",
        "fadd over an integer vector",
    );
}

/// Scalar floating-point operators are untouched by the erased routing.
#[test]
fn scalar_fp_operators_still_round_trip() {
    round_trip(
        r"
define i1 @scalar(float %a, float %b) {
  %s = fadd float %a, %b
  %n = fneg float %s
  %c = fcmp one float %n, %b
  ret i1 %c
}
",
    );
}
