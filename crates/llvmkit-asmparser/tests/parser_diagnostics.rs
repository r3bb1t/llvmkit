//! Diagnostic **text** parity with `LLParser.cpp` / `LLLexer.cpp`.
//!
//! Every other parser test file asks whether a fixture is accepted or
//! rejected. This one asks what the user is *told*, because upstream's
//! wording is contractual: each fixture ported here is a `not llvm-as`
//! case whose `FileCheck` line pins the message byte for byte, so a
//! reworded message is a real regression even when the accept/reject
//! verdict is identical.
//!
//! Assertions therefore compare `err.to_string()` — the rendered text —
//! and never a `ParseError` payload field. Matching a field is what let
//! [`llvmkit_asmparser::parse_error::ParseError::Expected`]'s `expected `
//! prefix silently prepend itself to messages upstream prints bare.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

/// Parse `src` expecting rejection, and return the rendered diagnostic.
fn parse_err(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    match Parser::new(src, &module) {
        // The lexer primes on construction, so a lexeme-level failure
        // surfaces here rather than from `parse_module`.
        Err(e) => e.to_string(),
        Ok(parser) => parser
            .parse_module()
            .expect_err("fixture is rejected")
            .to_string(),
    }
}

/// Ports `test/Assembler/invalid-c-style-comment0.ll`, whose CHECK line is
/// `error: unterminated comment` (`LLLexer::SkipCComment`).
#[test]
fn unterminated_block_comment_matches_upstream_text() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/invalid-c-style-comment/unterminated_block_comment.ll");

    assert_eq!(
        parse_err("unterminated_block_comment", FIXTURE),
        "unterminated comment"
    );
}

/// Ports `test/Assembler/invalid-inttype.ll`, whose CHECK line is
/// `error: bitwidth for integer type out of range` (`LLLexer::LexIdentifier`).
///
/// The offending width and the limit remain available as structured fields
/// on the error; upstream's rendered text names neither, so neither appears
/// in the message.
#[test]
fn integer_bitwidth_out_of_range_matches_upstream_text() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/invalid-inttype/bitwidth_out_of_range.ll");

    assert_eq!(
        parse_err("bitwidth_out_of_range", FIXTURE),
        "bitwidth for integer type out of range"
    );
}

/// Ports both halves of `test/Assembler/hex-float-overflow.ll`
/// (`LLLexer::Lex0x`). Upstream spells the type lowercase in the message.
#[test]
fn hex_float_overflow_matches_upstream_text() {
    const HALF: &[u8] = include_bytes!("fixtures/upstream/hex-float-overflow/half_overflow.ll");
    const BFLOAT: &[u8] = include_bytes!("fixtures/upstream/hex-float-overflow/bfloat_overflow.ll");

    assert_eq!(
        parse_err("half_overflow", HALF),
        "hexadecimal constant too large for half (16-bit)"
    );
    assert_eq!(
        parse_err("bfloat_overflow", BFLOAT),
        "hexadecimal constant too large for bfloat (16-bit)"
    );
}

/// Ports `test/Assembler/internal-hidden-alias.ll`, whose CHECK line is
/// `symbol with local linkage must have default visibility`
/// (`LLParser::parseAliasOrIFunc`).
///
/// Upstream prints this sentence bare, which is why it is a
/// `ParseError::Message` rather than an `Expected` payload.
#[test]
fn internal_hidden_alias_matches_upstream_text() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/internal-hidden/internal_hidden_alias.ll");

    assert_eq!(
        parse_err("internal_hidden_alias", FIXTURE),
        "symbol with local linkage must have default visibility"
    );
}

/// `LLParser::parseValID`'s `lltok::kw_ptrauth` arm reports through
/// `Constants::ptr_auth`'s own wording, which upstream prints bare. Locks
/// that the message is not re-prefixed on its way out of the builder —
/// the fixture family itself is exercised by
/// `parser_constants.rs::ptrauth_invalid_operands_match_upstream_diagnostics`.
#[test]
fn ptrauth_builder_message_is_not_prefixed() {
    let src = b"@var = global i32 0\n@auth_var = global ptr ptrauth (i32 42, i32 0)\n";
    assert_eq!(
        parse_err("ptrauth_base_pointer", src),
        "constant ptrauth base pointer must be a pointer"
    );
}

/// Ports `test/Assembler/dllimport-dsolocal-diag.ll`, whose CHECK line is
/// `error: dso_location and DLL-StorageClass mismatch`
/// (`LLParser::parseOptionalLinkage`).
///
/// The fixture doubles as the clause-order witness: it spells `dso_local`
/// *before* `dllimport`, which is the order `parseOptionalLinkage` reads and
/// `AsmWriter::printFunction` writes. Reaching this diagnostic at all proves
/// both clauses parsed in that order.
#[test]
fn dso_local_dllimport_mismatch_matches_upstream_text() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/dllimport-dsolocal/dso_local_dllimport_mismatch.ll");

    assert_eq!(
        parse_err("dso_local_dllimport_mismatch", FIXTURE),
        "dso_location and DLL-StorageClass mismatch"
    );
}

/// `LLParser::parseParameterList`'s musttail rule. The message *does* start
/// with `expected `, so it stays a `ParseError::Expected` — this locks that
/// the word is contributed exactly once by the rendering rather than also
/// being baked into the stored payload.
#[test]
fn musttail_ellipsis_message_says_expected_once() {
    let src = b"declare void @f(...)\n\
                define void @g(...) {\n\
                entry:\n\
                  musttail call void (...) @f()\n\
                  ret void\n\
                }\n";
    assert_eq!(
        parse_err("musttail_missing_ellipsis", src),
        "expected '...' at end of argument list for musttail call in varargs function"
    );
}
