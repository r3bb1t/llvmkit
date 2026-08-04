//! Integer casts over vectors through the parser.
//!
//! The sibling of `parser_vector_binops.rs`, and for the same reason: llvmkit's
//! typed integer handles carry a *scalar* width, so `<N x iM>` operands route
//! to the erased builder family instead. Upstream needs no such split —
//! `LLParser::parseCast` hands the operand straight to `CastInst::Create`.
//!
//! These lock that path: the IR parses, verifies, and prints back
//! byte-identically.

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
    let result = parser::parse_dynamic(source);
    assert!(
        result.is_err(),
        "expected {what} to be rejected, but it parsed\n--- source ---\n{source}"
    );
}

/// `trunc` / `zext` / `sext` over a fixed vector, widening and narrowing.
///
/// llvmkit-specific in its spelling: no single upstream fixture spans the three
/// opcodes this way. The rule being checked is
/// `llvm/lib/IR/Instructions.cpp::CastInst::castIsValid`'s integer arm, which
/// accepts vector source and destination of equal element count and compares
/// `getScalarSizeInBits`.
#[test]
fn the_three_integer_casts_round_trip_over_a_fixed_vector() {
    round_trip(
        r"
define <4 x i64> @widen(<4 x i16> %x) {
  %z = zext <4 x i16> %x to <4 x i32>
  %s = sext <4 x i32> %z to <4 x i64>
  ret <4 x i64> %s
}
",
    );
    round_trip(
        r"
define <8 x i8> @narrow(<8 x i32> %x) {
  %t = trunc <8 x i32> %x to <8 x i8>
  ret <8 x i8> %t
}
",
    );
}

/// The flags each opcode accepts, over vectors: `trunc nuw nsw` and
/// `zext nneg`.
///
/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::parseCast`'s flag handling, which
/// is opcode-keyed and does not care whether the operand is a vector.
#[test]
fn vector_cast_flags_round_trip() {
    round_trip(
        r"
define <2 x i16> @flags(<2 x i32> %x, <2 x i16> %y) {
  %t = trunc nuw nsw <2 x i32> %x to <2 x i16>
  %z = zext nneg <2 x i16> %y to <2 x i32>
  %r = trunc <2 x i32> %z to <2 x i16>
  ret <2 x i16> %r
}
",
    );
}

/// `ceil_shift4_v4i32` and `ceil_shift4_v8i16` from
/// `llvm/test/Transforms/InstCombine/ceil-shift.ll`, verbatim.
///
/// These are the fixtures the vector cast gap blocked; they are the reason
/// this file exists. `crates/llvmkit-asmparser/tests/strip_null_test.rs`
/// asserts what `stripNullTest` answers for them.
#[test]
fn the_upstream_ceil_shift_vector_fixtures_parse() {
    round_trip(
        r"
define <4 x i1> @ceil_shift4_v4i32(<4 x i32> %arg0) {
  %quot = lshr <4 x i32> %arg0, splat (i32 16)
  %rem = and <4 x i32> %arg0, splat (i32 65535)
  %has_rem = icmp ne <4 x i32> %rem, zeroinitializer
  %zext_has_rem = zext <4 x i1> %has_rem to <4 x i32>
  %quot_or_rem = or <4 x i32> %quot, %zext_has_rem
  %res = icmp eq <4 x i32> %quot_or_rem, zeroinitializer
  ret <4 x i1> %res
}
",
    );
    round_trip(
        r"
define <8 x i1> @ceil_shift4_v8i16(<8 x i16> %arg0) {
  %quot = lshr <8 x i16> %arg0, splat (i16 4)
  %rem = and <8 x i16> %arg0, splat (i16 15)
  %has_rem = icmp ne <8 x i16> %rem, zeroinitializer
  %zext_has_rem = zext <8 x i1> %has_rem to <8 x i16>
  %quot_or_rem = or <8 x i16> %quot, %zext_has_rem
  %res = icmp eq <8 x i16> %quot_or_rem, zeroinitializer
  ret <8 x i1> %res
}
",
    );
}

/// The two ways `castIsValid` rejects an integer vector cast: a change in
/// element count, and a width that moves the wrong way for the opcode.
///
/// `castIsValid` requires `SrcTy->isVectorTy() == DestTy->isVectorTy()` with
/// equal element counts, and a strict width change in the opcode's direction.
#[test]
fn a_vector_cast_that_changes_shape_or_direction_is_rejected() {
    rejects(
        r"
define <2 x i64> @lanes(<4 x i32> %x) {
  %z = zext <4 x i32> %x to <2 x i64>
  ret <2 x i64> %z
}
",
        "a zext that changes the element count",
    );
    rejects(
        r"
define <4 x i16> @backwards(<4 x i32> %x) {
  %z = zext <4 x i32> %x to <4 x i16>
  ret <4 x i16> %z
}
",
        "a zext that narrows",
    );
    rejects(
        r"
define <4 x i64> @backwards_trunc(<4 x i32> %x) {
  %t = trunc <4 x i32> %x to <4 x i64>
  ret <4 x i64> %t
}
",
        "a trunc that widens",
    );
    rejects(
        r"
define <4 x i32> @mixed(<4 x i32> %x) {
  %z = zext <4 x i32> %x to i64
  ret <4 x i32> %x
}
",
        "a cast from a vector to a scalar",
    );
}

/// A vector cast whose operand is a constant: whatever the folder does with it
/// has to still print and re-parse.
///
/// llvmkit-specific. The scalar `build_*_dyn` cast family consults the folder
/// before appending, and the erased path added for vectors does the same; this
/// pins that the vector case is not silently mis-folded.
#[test]
fn a_vector_cast_of_a_constant_round_trips() {
    let source = r"
define <4 x i32> @constant() {
  %z = zext <4 x i16> splat (i16 7) to <4 x i32>
  ret <4 x i32> %z
}
";
    let module = match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("parses: {error:?}\n--- source ---\n{source}"),
    };
    let verified = module
        .verify()
        .unwrap_or_else(|e| panic!("verifies: {e:?}\n--- source ---\n{source}"));
    let printed = format!("{verified}");
    let reparsed = parser::parse_dynamic(&printed)
        .unwrap_or_else(|e| panic!("re-parses: {e:?}\n--- printed ---\n{printed}"));
    assert_eq!(
        format!("{}", reparsed.verify().expect("re-verifies")),
        printed,
        "printing is not idempotent"
    );
}
