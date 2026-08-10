//! Type-definition parsing: identity, forward references, and aliases.
//!
//! Mirrors `LLParser::parseStructDefinition` and the `%name` / `%N` arms of
//! `LLParser::parseType`. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError};
use llvmkit_ir::Module;

fn parse_render(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    format!("{module}")
}

fn parse_err(src: &[u8]) -> ParseError {
    let module = Module::dynamic("parser_types_err");
    Parser::new(src, &module)
        .expect("parse constructor")
        .parse_module()
        .expect_err("parse rejected")
}

/// Two numbered types with identical bodies are *distinct* types, because
/// `%N = type ...` creates an identified struct (`StructType::create`), not a
/// literal one (`StructType::get`). llvmkit minted a literal struct for each,
/// so structural uniquing silently merged them.
///
/// No upstream `.ll` isolates this; the rule is the `StructType::create` call
/// in `LLParser::parseStructDefinition`, and the observable consequence is
/// that both definitions survive the round-trip.
#[test]
fn numbered_types_with_equal_bodies_stay_distinct() {
    let text = parse_render(
        "numbered_type_identity",
        b"%0 = type { i32 }\n%1 = type { i32 }\n@g = global %1 zeroinitializer\n",
    );
    assert!(text.contains("%0 = type { i32 }"), "{text}");
    assert!(text.contains("%1 = type { i32 }"), "{text}");
    assert!(text.contains("@g = global %1 zeroinitializer"), "{text}");
}

/// A numbered type may be defined at any slot: upstream's `NumberedTypes` is
/// a plain map with no monotonicity rule. llvmkit required each definition to
/// equal the running frontier and rejected `%5` as the first one.
///
/// No upstream `.ll` isolates this; the rule is the absence of any check in
/// `LLParser::parseStructDefinition`'s numbered path.
#[test]
fn a_numbered_type_may_skip_slots() {
    let text = parse_render("numbered_type_skip", b"%5 = type { i32 }\n");
    assert!(text.contains("type { i32 }"), "{text}");
}

/// `%t = type opaque` is a definition even though it leaves the body unset,
/// so a later `%t = type {i32}` is a redefinition. Mirrors the
/// `Entry.first && !Entry.second.isValid()` guard at the top of
/// `LLParser::parseStructDefinition`, whose wording this pins.
#[test]
fn redefining_a_type_after_opaque_is_rejected() {
    assert_eq!(
        parse_err(b"%t = type opaque\n%t = type { i32 }\n").to_string(),
        "redefinition of type"
    );
}

/// The same guard for two bodied definitions.
#[test]
fn redefining_a_bodied_type_is_rejected() {
    assert_eq!(
        parse_err(b"%t = type { i32 }\n%t = type { i64 }\n").to_string(),
        "redefinition of type"
    );
}

/// A `type` directive whose right-hand side is not a struct is a plain alias,
/// accepted "for compatibility with old files" — and, because there is no
/// identified struct to fill in later, it may not have been forward
/// referenced. Mirrors the `if (Entry.first) return error(...)` arm of
/// `LLParser::parseStructDefinition`.
#[test]
fn a_forward_referenced_alias_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(%t)\n%t = type i32\n").to_string(),
        "forward references to non-struct type"
    );
}

/// The same alias without a prior reference is legal, and the alias resolves
/// to the aliased type rather than to a struct.
#[test]
fn a_type_alias_resolves_to_the_aliased_type() {
    let text = parse_render(
        "type_alias",
        b"%t = type i32\n@g = global %t zeroinitializer\n",
    );
    assert!(text.contains("@g = global i32 0"), "{text}");
}

/// A numbered type referenced but never defined. Mirrors the `NumberedTypes`
/// loop in `LLParser::validateEndOfModule`; llvmkit used to leave the
/// reference as a silently opaque struct, with a comment asserting that
/// upstream does not diagnose this.
#[test]
fn an_undefined_numbered_type_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(%3)\n").to_string(),
        "use of undefined type '%3'"
    );
}

/// The named twin, whose noun differs: upstream says `type named 'x'` where
/// the numbered form says `type '%N'`.
#[test]
fn an_undefined_named_type_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(%t)\n").to_string(),
        "use of undefined type named 't'"
    );
}

/// A forward-referenced numbered type resolves to the *same* type its later
/// definition fills in — the property an anonymous identified struct exists
/// for. A literal struct could not do this: the forward reference and the
/// definition would be two different types.
#[test]
fn a_forward_referenced_numbered_type_resolves_to_its_definition() {
    let text = parse_render(
        "numbered_type_forward",
        b"@g = global ptr null\n%0 = type { i32 }\n@h = global %0 zeroinitializer\n",
    );
    assert!(text.contains("%0 = type { i32 }"), "{text}");
    assert!(text.contains("@h = global %0 zeroinitializer"), "{text}");
}
