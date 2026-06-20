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
