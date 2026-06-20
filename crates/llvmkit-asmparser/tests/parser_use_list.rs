//! Use-list order directive parser tests.
//!
//! Mirrors upstream `LLParser.cpp::parseUseListOrder` and
//! `AsmWriter.cpp::AssemblyWriter::printUseListOrder`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_and_print(src: &str) -> String {
    Module::with_new("parser_use_list", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        format!("{module}")
    })
}

fn parse_err(src: &str) -> ParseError {
    Module::with_new("parser_use_list", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("parser rejects malformed use-list directive")
    })
}

/// Mirrors `LLParser.cpp::parseUseListOrder`: module-level
/// `uselistorder Type Value, { ... }` resolves a global value and is
/// emitted by `AsmWriter.cpp::AssemblyWriter::printUseListOrder`.
#[test]
fn module_uselistorder_round_trips_global_callee() {
    let src = r#"
declare void @callee()

define void @user() {
entry:
  call void @callee()
  call void @callee()
  ret void
}

uselistorder ptr @callee, { 1, 0 }
"#;

    let printed = parse_and_print(src);
    assert!(printed.contains("uselistorder ptr @callee, { 1, 0 }\n"));

    let reparsed = parse_and_print(&printed);
    assert!(reparsed.contains("uselistorder ptr @callee, { 1, 0 }\n"));
}

/// Mirrors `LLParser.cpp::parseUseListOrder(PerFunctionState*)`: a
/// function-local directive resolves a local SSA value and prints inside the
/// function body before the closing brace.
#[test]
fn function_uselistorder_round_trips_local_value() {
    let src = r#"
define i32 @f(i32 %a) {
entry:
  %x = add i32 %a, 1
  %y = add i32 %x, 2
  %z = add i32 %x, 3
  ret i32 %z

  uselistorder i32 %x, { 1, 0 }
}
"#;

    let printed = parse_and_print(src);
    assert!(printed.contains("  uselistorder i32 %x, { 1, 0 }\n}"));

    let reparsed = parse_and_print(&printed);
    assert!(reparsed.contains("  uselistorder i32 %x, { 1, 0 }\n}"));
}

/// Mirrors `LLParser.cpp::parseUseListOrderIndexes`: identity index lists are
/// malformed because they do not change use-list order.
#[test]
fn ordered_uselistorder_indexes_are_rejected() {
    let src = r#"
declare void @callee()

define void @user() {
entry:
  call void @callee()
  call void @callee()
  ret void
}

uselistorder ptr @callee, { 0, 1 }
"#;

    match parse_err(src) {
        ParseError::Expected { expected, .. } => {
            assert_eq!(
                expected,
                "expected uselistorder indexes to change the order"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
