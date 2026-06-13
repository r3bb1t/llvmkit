//! Central ValID parsing/conversion regression tests.

use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_asmparser::{ll_parser::Parser, parser};
use llvmkit_ir::Module;

fn parse_err(src: &str) -> ParseError {
    let module = Module::new("parser_val_id");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("parser rejects unsupported value form")
}

/// Mirrors `llvm/include/llvm/AsmParser/Parser.h::parseConstantValue` and
/// `llvm/lib/AsmParser/LLParser.cpp::parseStandaloneConstantValue`: standalone
/// constant parsing consumes exactly one constant and then requires EOF.
#[test]
fn standalone_constant_rejects_trailing_token() {
    let module = Module::new("parser_val_id_constant");
    let err =
        parser::parse_constant_value(b"42 trailing", &module, module.i32_type().as_type(), None)
            .expect_err("parser rejects trailing token after standalone constant");
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, "end of string"),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID`: floating
/// constexpr opcodes like `fadd` are explicitly rejected in LLVM 22.1.4.
#[test]
fn fadd_constant_expr_rejected_as_unsupported() {
    let err = parse_err("@x = global double fadd (double 1.0, double 2.0)\n");
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "fadd constexprs are no longer supported")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}
