//! Parser safety regression tests.
//!
//! These lock parser diagnostics that should never silently fall back to a
//! different IR construct.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_err(src: &str) -> ParseError {
    Module::with_new("parser_errors", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("parser rejects malformed input")
    })
}

/// Mirrors `LLParser.cpp::parseType`: integer widths outside LLVM's modeled
/// range are rejected instead of falling back to another integer type.
#[test]
fn malformed_integer_type_rejects_width_overflow() {
    let err = parse_err("@x = global i16777216 0\n");
    match err {
        ParseError::IntegerWidthOutOfRange { width, .. } => assert_eq!(width, 16_777_216),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `LLParser.cpp::parseValID`: a shufflevector mask element must be a
/// valid integer literal or poison marker, never a silently substituted value.
#[test]
fn malformed_shuffle_mask_rejects_bad_element() {
    let err = parse_err(
        "define <4 x i32> @shuffle(<4 x i32> %a, <4 x i32> %b) {\n\
entry:\n\
  %r = shufflevector <4 x i32> %a, <4 x i32> %b, <4 x i32> <i32 0, i32 bad>\n\
  ret <4 x i32> %r\n\
}\n",
    );
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "valid shufflevector mask element")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `LLParser.cpp::parseShuffleVector` and
/// `Instructions.cpp::ShuffleVectorInst::isValidOperands`: the mask operand
/// must be a vector of i32, not any integer vector later coerced to i32.
#[test]
fn shufflevector_rejects_non_i32_mask_type() {
    let err = parse_err(
        "define <2 x i8> @shuffle(<2 x i8> %a, <2 x i8> %b) {\n\
entry:\n\
  %r = shufflevector <2 x i8> %a, <2 x i8> %b, <2 x i64> <i64 0, i64 1>\n\
  ret <2 x i8> %r\n\
}\n",
    );
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "valid shufflevector mask")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Parse `src` and return `Ok(())` on success, propagating any parse error.
fn parse_ok(src: &str) -> Result<(), ParseError> {
    Module::with_new("parser_ok", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .map(|_| ())
    })
}

/// Phi W1: a `phi` appearing after a non-phi instruction is a parse error.
///
/// With the auto-hoisting phi builders, feeding a misplaced `phi` to a builder
/// would silently reorder it into valid position, laundering ill-formed `.ll`
/// into valid IR. The parser rejects it up front instead.
///
/// Uses a zero-input `phi` (as in `zero-input-phi/phi_int_round_trips.ll`) so
/// the test isolates *placement*: the guard fires before `parse_phi` runs, and
/// no incoming-edge resolution is involved.
#[test]
fn phi_after_non_phi_is_a_parse_error() {
    let src = r#"
define void @f() {
entry:
  ret void

return:
  %x = add i32 0, 1
  %r = phi i32
  ret void
}
"#;
    let err = parse_err(src);
    let msg = err.to_string();
    assert!(
        msg.contains("phi must be grouped at the top"),
        "expected phi-placement parse error, got: {msg}"
    );
}

/// A `phi` that appears before the first non-phi instruction still parses,
/// even when a non-phi instruction follows it in the same block.
#[test]
fn leading_phis_still_parse() {
    let src = r#"
define void @f() {
entry:
  ret void

return:
  %r = phi i32
  %x = add i32 %r, 1
  ret void
}
"#;
    parse_ok(src).expect("well-placed phi must keep parsing");
}
