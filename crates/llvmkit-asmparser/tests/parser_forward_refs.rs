//! Forward-reference and slot-numbering parser tests.
//!
//! Mirrors upstream assembler diagnostics around `LLParser::validateEndOfModule`
//! and `PerFunctionState::finishFunction`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_err(src: &str) -> ParseError {
    Module::with_new("forward_refs", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("parser rejects malformed input")
    })
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
