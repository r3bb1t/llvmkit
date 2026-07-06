//! Forward-reference and slot-numbering parser tests.
//!
//! Mirrors upstream assembler diagnostics around `LLParser::validateEndOfModule`
//! and `PerFunctionState::finishFunction`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    Module::with_new("forward_refs", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        format!("{module}")
    })
}

fn parse_err(src: &str) -> ParseError {
    Module::with_new("forward_refs", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("parser rejects malformed input")
    })
}

fn parse_ok(src: &str) {
    Module::with_new("forward_refs", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
    });
}

/// Mirrors `test/Assembler/skip-value-numbers-invalid.ll`: stale numbered
/// SSA values are rejected by monotonic slot checks.
#[test]
fn skip_value_numbers_invalid_is_rejected() {
    let err = parse_err(
        "define i32 @f() {\nentry:\n  %0 = add i32 1, 2\n  %0 = add i32 3, 4\n  ret i32 %0\n}\n",
    );
    assert!(matches!(err, ParseError::InvalidSlotId { .. }));
}

/// Mirrors `test/Assembler/skip-value-numbers-invalid.ll`: numbered SSA
/// definitions may not skip the next unnamed slot.
#[test]
fn skip_ahead_value_number_is_rejected() {
    let err = parse_err("define i32 @f() {\nentry:\n  %2 = add i32 1, 2\n  ret i32 %2\n}\n");
    assert!(matches!(err, ParseError::InvalidSlotId { .. }));
}

/// Mirrors `LLParser::parseArgumentList`: explicit numbered arguments must
/// equal the current unnamed argument slot.
#[test]
fn skip_ahead_numbered_parameter_is_rejected() {
    let err = parse_err("define i32 @f(i32 %2) {\nentry:\n  ret i32 %2\n}\n");
    assert!(matches!(err, ParseError::InvalidSlotId { .. }));
}

/// Mirrors `test/Assembler/2007-03-18-InvalidNumberedVar.ll`: undefined
/// numbered SSA references are rejected structurally.
#[test]
fn invalid_numbered_var_is_rejected() {
    let err = parse_err("define i32 @f() {\nentry:\n  ret i32 %1\n}\n");
    assert!(matches!(err, ParseError::UndefinedSymbol { .. }));
}

/// Mirrors `test/Assembler/2009-02-01-UnnamedForwardRef.ll`: non-phi unnamed
/// forward references remain invalid.
#[test]
fn unnamed_forward_ref_is_rejected() {
    let err = parse_err(
        "define i32 @f() {\nentry:\n  %0 = add i32 %1, 1\n  %1 = add i32 2, 3\n  ret i32 %0\n}\n",
    );
    assert!(matches!(err, ParseError::UndefinedSymbol { .. }));
}

/// Mirrors `test/Assembler/2002-08-15-UnresolvedGlobalReference.ll` and
/// `2003-04-25-UnresolvedGlobalReference.ll`: unresolved globals fail.
#[test]
fn unresolved_global_reference_is_rejected() {
    let err = parse_err("define i32 @f() {\nentry:\n  %0 = call i32 @missing()\n  ret i32 %0\n}\n");
    assert!(matches!(err, ParseError::UndefinedSymbol { .. }));
}

/// Mirrors `test/Assembler/2002-05-02-InvalidForwardRef.ll`: forward function
/// calls resolve to the later declaration when the signatures match.
#[test]
fn upstream_forward_global_reference_fixture_parses() {
    parse_ok(include_str!(
        "fixtures/upstream/Assembler/2002-05-02-InvalidForwardRef.ll"
    ));
}

/// llvmkit-specific regression for the forward-callee path exercised by
/// `test/Assembler/2002-05-02-InvalidForwardRef.ll`: repeated forward
/// references to one function each keep their OWN call-site type rather
/// than being silently rewritten to the first provisional signature. Under
/// opaque pointers a forward-referenced `@foo` is a bare `ptr` like any
/// other callee, so `LLParser::parseCall` lets each call carry its own
/// function type (`CallBase`); the second call stays `i64`, not rewritten
/// to the `i32` the first call provisionally created.
#[test]
fn forward_global_reference_calls_keep_their_own_types() {
    let text = parse_and_render(
        "define void @test() {\nentry:\n  %a = call i32 @foo()\n  %b = call i64 @foo()\n  ret void\n}\ndeclare i32 @foo()\n",
    );
    assert!(text.contains("%a = call i32 @foo()"), "{text}");
    assert!(text.contains("%b = call i64 @foo()"), "{text}");
    assert!(text.contains("declare i32 @foo()"), "{text}");
}

/// Mirrors `test/Assembler/2002-05-02-InvalidForwardRef.ll`: resolving a
/// provisional callee through a later definition preserves parsed parameter
/// names on the function arguments.
#[test]
fn forward_function_definition_applies_parameter_names() {
    let text = parse_and_render(
        "define i32 @caller() {\n\
         entry:\n  \
           %r = call i32 @callee(i32 7)\n  \
           ret i32 %r\n\
         }\n\
         define i32 @callee(i32 %x) {\n\
         entry:\n  \
           ret i32 %x\n\
         }\n",
    );
    assert!(text.contains("define i32 @callee(i32 %x)"), "{text}");
    assert!(text.contains("ret i32 %x"), "{text}");
}

/// Mirrors `LLParser::PerFunctionState::finishFunction`: placeholder blocks
/// created by forward branches must be defined by a later label.
#[test]
fn undefined_block_label_is_rejected() {
    let err = parse_err("define void @f() {\nentry:\n  br label %missing\n}\n");
    assert!(matches!(err, ParseError::UndefinedSymbol { .. }));
}

/// Mirrors `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
/// SlotMappingTest)`: numbered declarations populate global slots.
#[test]
fn numbered_declare_records_slot_mapping() {
    Module::with_new("numbered_declare", |module| {
        let parsed = Parser::new(b"declare void @0()\n", &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        assert!(parsed.slot_mapping.global_values.get(0).is_some());
    });
}
