//! Intrinsic parser tests.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError};
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    Module::with_new("parser_intrinsics", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        format!("{module}")
    })
}

fn parse_err(src: &str) -> ParseError {
    Module::with_new("parser_intrinsics_err", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("parser rejects intrinsic misuse")
    })
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::int_lifetime_start` and
/// `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseCall`: known direct
/// intrinsic callees may be declared from the callsite.
#[test]
fn known_intrinsic_auto_declares_direct_callee() {
    let text = parse_and_render(
        "define void @f(ptr %p) {\nentry:\n  call void @llvm.lifetime.start.p0(i64 4, ptr %p)\n  ret void\n}\n",
    );
    assert!(
        text.contains("declare void @llvm.lifetime.start.p0(i64 %0, ptr %1)"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("declare void @llvm.lifetime.start.p0(i64 %0, ptr %1)"));
}

/// Mirrors `llvm/lib/IR/Verifier.cpp` intrinsic validation: unknown `llvm.*`
/// names are rejected rather than modeled as ordinary functions.
#[test]
fn unknown_intrinsic_is_rejected() {
    let err = parse_err(
        "define void @f() {\nentry:\n  call void @llvm.not.a.real.intrinsic()\n  ret void\n}\n",
    );
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, "unknown intrinsic"),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors LLVM intrinsic verifier rules: intrinsic globals are callable
/// symbols, not ordinary pointer constants.
#[test]
fn intrinsic_non_callee_use_is_rejected() {
    let err = parse_err("@p = global ptr @llvm.lifetime.start.p0\n");
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "intrinsic can only be used as callee")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::int_lifetime_start`: direct
/// intrinsic calls must match the canonical signature.
#[test]
fn intrinsic_signature_mismatch_is_rejected() {
    let err = parse_err(
        "define void @f(ptr %p) {\nentry:\n  call void @llvm.lifetime.start.p0(i32 4, ptr %p)\n  ret void\n}\n",
    );
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "intrinsic signature mismatch")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}
