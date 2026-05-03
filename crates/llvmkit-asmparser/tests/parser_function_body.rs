//! Function-body parser integration tests (Session 3).
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_and_print(src: &str) -> String {
    let m = Module::new("session3");
    Parser::new(src.as_bytes(), &m)
        .expect("lex")
        .parse_module()
        .expect("parse");
    format!("{m}")
}

/// Mirrors the smallest body fixture: `define void @f() { ret void }`.
/// Anchored on `test/Assembler/return-void.ll` and the
/// `LLParser::parseRet` `void` arm.
#[test]
fn parses_void_function_body() {
    let printed = parse_and_print("define void @f() {\nentry:\n  ret void\n}\n");
    assert!(printed.contains("define void @f"));
    assert!(printed.contains("ret void"));
}

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, ...)`'s
/// add-then-return shape and the `factorial.rs` example's first block.
#[test]
fn parses_int_add_and_ret() {
    let printed = parse_and_print(
        "define i32 @add(i32 %a, i32 %b) {\nentry:\n  %r = add i32 %a, %b\n  ret i32 %r\n}\n",
    );
    assert!(printed.contains("add i32 %a, %b"));
    assert!(printed.contains("ret i32 %r"));
}

/// Numbered parameters are valid local names. Mirrors
/// `LLParser::PerFunctionState`'s numbered-value bookkeeping.
#[test]
fn parses_numbered_params() {
    let printed = parse_and_print(
        "define i32 @add(i32, i32) {\nentry:\n  %r = add i32 %0, %1\n  ret i32 %r\n}\n",
    );
    assert!(printed.contains("ret i32 %r"));
}

/// `unreachable` terminator. Mirrors `test/Assembler/unreachable.ll`.
#[test]
fn parses_unreachable_terminator() {
    let printed = parse_and_print("define void @abort() {\nentry:\n  unreachable\n}\n");
    assert!(printed.contains("unreachable"));
}

/// Conditional branch with `icmp eq` predicate. Mirrors the entry-block
/// arm of `crates/llvmkit-ir/examples/factorial.rs`.
#[test]
fn parses_icmp_and_cond_br() {
    let printed = parse_and_print(
        "define i32 @abs(i32 %x) {\n\
         entry:\n  \
           %is_zero = icmp eq i32 %x, 0\n  \
           br i1 %is_zero, label %zero_path, label %nonzero\n\
         zero_path:\n  \
           ret i32 0\n\
         nonzero:\n  \
           ret i32 %x\n\
         }\n",
    );
    assert!(printed.contains("icmp eq i32 %x, 0"));
    assert!(printed.contains("br i1 %is_zero, label %zero_path, label %nonzero"));
}

/// Forward block reference: `br label %later` before `later:` is parsed.
/// Mirrors `LLParser::PerFunctionState::getBB`'s forward-reference path.
#[test]
fn parses_forward_block_reference() {
    let printed = parse_and_print(
        "define void @forward() {\n\
         entry:\n  \
           br label %later\n\
         later:\n  \
           ret void\n\
         }\n",
    );
    assert!(printed.contains("br label %later"));
    assert!(printed.contains("ret void"));
}

/// Sub / mul arms of `parse_int_binop`. Mirrors the loop body of
/// `crates/llvmkit-ir/examples/factorial.rs` (next_acc / next_i lines).
#[test]
fn parses_sub_and_mul() {
    let printed = parse_and_print(
        "define i32 @poly(i32 %x) {\n\
         entry:\n  \
           %a = sub i32 %x, 1\n  \
           %b = mul i32 %a, %x\n  \
           ret i32 %b\n\
         }\n",
    );
    assert!(printed.contains("sub i32 %x, 1"));
    assert!(printed.contains("mul i32 %a, %x"));
}

/// Negative test: an unsupported opcode in this session is reported as a
/// typed parse error, not a silent miss. Mirrors `LLParser`'s
/// `tokError("expected instruction opcode")` site for the default arm.
#[test]
fn unsupported_opcode_is_typed_error() {
    let m = Module::new("unsupported_opcode");
    let parser = Parser::new(
        b"define void @f() {\nentry:\n  %x = alloca i32\n  ret void\n}\n",
        &m,
    )
    .unwrap();
    let err = parser.parse_module().unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("instruction opcode supported by Session 3"),
        "got: {msg}"
    );
}
