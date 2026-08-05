//! `select` with a `<N x i1>` condition, through the parser.
//!
//! The third sibling of `parser_vector_binops.rs` and `parser_vector_casts.rs`,
//! and for the same reason: llvmkit's typed handles cannot name a vector
//! condition (`IntValue<bool>` is scalar) or a vector arm (no `IntWidth`
//! describes `<N x iM>`), so `IrBuilder::build_select_erased` carries the shape
//! that `IrBuilder::build_select` cannot. Upstream needs no split —
//! `LLParser::parseSelect` hands all three operands to `SelectInst::Create`.
//!
//! These lock that path end to end: the IR parses, **verifies**, and prints
//! back byte-identically. Parsing alone would not be enough — the sibling
//! vector-cast work shipped IR the verifier rejected until a round-trip test
//! caught it.
//!
//! The rejection cases come straight from
//! `llvm/lib/IR/Instructions.cpp::SelectInst::areInvalidOperands`, one per
//! diagnostic it can return for a vector condition.

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

/// A lane-wise select over integer, float and pointer vectors.
///
/// llvmkit-specific in its spelling — no single upstream fixture spans the
/// three arm categories — but the rule being checked is
/// `SelectInst::areInvalidOperands`, which accepts any non-token arm type once
/// the condition's element count matches.
#[test]
fn a_vector_condition_round_trips_over_each_arm_category() {
    round_trip(
        r"
define <4 x i32> @ints(<4 x i1> %c, <4 x i32> %t, <4 x i32> %f) {
  %s = select <4 x i1> %c, <4 x i32> %t, <4 x i32> %f
  ret <4 x i32> %s
}
",
    );
    round_trip(
        r"
define <2 x double> @floats(<2 x i1> %c, <2 x double> %t, <2 x double> %f) {
  %s = select <2 x i1> %c, <2 x double> %t, <2 x double> %f
  ret <2 x double> %s
}
",
    );
    round_trip(
        r"
define <2 x ptr> @pointers(<2 x i1> %c, <2 x ptr> %t, <2 x ptr> %f) {
  %s = select <2 x i1> %c, <2 x ptr> %t, <2 x ptr> %f
  ret <2 x ptr> %s
}
",
    );
}

/// A scalable vector condition, whose element count is a minimum rather than a
/// count. `ElementCount` equality compares scalability too, so a scalable
/// condition needs scalable arms.
#[test]
fn a_scalable_vector_condition_round_trips() {
    round_trip(
        r"
define <vscale x 4 x i32> @test(<vscale x 4 x i1> %c, <vscale x 4 x i32> %t, <vscale x 4 x i32> %f) {
  %s = select <vscale x 4 x i1> %c, <vscale x 4 x i32> %t, <vscale x 4 x i32> %f
  ret <vscale x 4 x i32> %s
}
",
    );
}

/// A scalar `i1` condition still selects between two vectors — the condition
/// is not required to be a vector just because the arms are.
#[test]
fn a_scalar_condition_over_vector_arms_round_trips() {
    round_trip(
        r"
define <4 x i32> @test(i1 %c, <4 x i32> %t, <4 x i32> %f) {
  %s = select i1 %c, <4 x i32> %t, <4 x i32> %f
  ret <4 x i32> %s
}
",
    );
}

/// `"vector select requires selected vectors to have the same vector length as
/// select condition"`.
#[test]
fn a_vector_condition_of_the_wrong_length_is_rejected() {
    rejects(
        r"
define <4 x i32> @test(<2 x i1> %c, <4 x i32> %t, <4 x i32> %f) {
  %s = select <2 x i1> %c, <4 x i32> %t, <4 x i32> %f
  ret <4 x i32> %s
}
",
        "a 2-lane condition selecting between 4-lane arms",
    );
}

/// `"selected values for vector select must be vectors"`.
#[test]
fn a_vector_condition_with_scalar_arms_is_rejected() {
    rejects(
        r"
define i32 @test(<4 x i1> %c, i32 %t, i32 %f) {
  %s = select <4 x i1> %c, i32 %t, i32 %f
  ret i32 %s
}
",
        "a vector condition selecting between scalar arms",
    );
}

/// `"vector select condition element type must be i1"`.
#[test]
fn a_vector_condition_of_wider_elements_is_rejected() {
    rejects(
        r"
define <4 x i32> @test(<4 x i8> %c, <4 x i32> %t, <4 x i32> %f) {
  %s = select <4 x i8> %c, <4 x i32> %t, <4 x i32> %f
  ret <4 x i32> %s
}
",
        "a <4 x i8> select condition",
    );
}

/// `"both values to select must have same type"`, reached through the vector
/// path.
#[test]
fn mismatched_vector_arms_are_rejected() {
    rejects(
        r"
define <4 x i32> @test(<4 x i1> %c, <4 x i32> %t, <4 x i64> %f) {
  %s = select <4 x i1> %c, <4 x i32> %t, <4 x i64> %f
  ret <4 x i32> %s
}
",
        "arms of different vector element types",
    );
}
