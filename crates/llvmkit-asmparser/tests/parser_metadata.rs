//! Metadata parsing tests — Session 4.
//!
//! Each `#[test]` mirrors constructive `.ll` fixtures from upstream LLVM.
//! Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::{IrError, Module};

#[derive(Clone, Copy)]
struct ModuleStats {
    metadata_count: usize,
    named_metadata_count: usize,
}

impl ModuleStats {
    fn metadata_count(self) -> usize {
        self.metadata_count
    }

    fn named_metadata_count(self) -> usize {
        self.named_metadata_count
    }
}

fn parse_snippet(src: &str) -> (ModuleStats, String) {
    Module::with_new("test", |module| {
        let _ = Parser::new(src.as_bytes(), &module)
            .expect("parse constructor")
            .parse_module()
            .expect("parse succeeded");
        let stats = ModuleStats {
            metadata_count: module.metadata_count(),
            named_metadata_count: module.named_metadata_count(),
        };
        let text = format!("{module}");
        (stats, text)
    })
}

fn parse_fails(src: &str) -> String {
    Module::with_new("test", |module| {
        let err = Parser::new(src.as_bytes(), &module)
            .expect("parse constructor")
            .parse_module()
            .expect_err("parse should fail");
        err.to_string()
    })
}

fn parse_and_verify(src: &str) -> Result<(), IrError> {
    Module::with_new("test", |module| {
        let _ = Parser::new(src.as_bytes(), &module)
            .expect("parse constructor")
            .parse_module()
            .expect("parse succeeded");
        module.verify_borrowed()
    })
}

fn parse_and_verify_failure_message(src: &str) -> String {
    let err = parse_and_verify(src).expect_err("verify should fail");
    match err {
        IrError::VerifierFailure { message, .. } => message,
        other => panic!("expected verifier failure, got {other:?}"),
    }
}

fn fixture_function_with_metadata(
    fixture: &str,
    function_marker: &str,
    metadata_marker: &str,
) -> String {
    let function_start = fixture
        .find(function_marker)
        .unwrap_or_else(|| panic!("missing function marker {function_marker}"));
    let function_tail = &fixture[function_start..];
    let function_end = function_tail
        .find("\n}")
        .map(|idx| function_start + idx + 3)
        .unwrap_or_else(|| panic!("missing function end for {function_marker}"));
    let metadata = fixture
        .lines()
        .find(|line| line.starts_with(metadata_marker))
        .unwrap_or_else(|| panic!("missing metadata marker {metadata_marker}"));
    format!("{}\n{}\n", &fixture[function_start..function_end], metadata)
}

// ── Standalone metadata: string operands ─────────────────────────────────

/// `!0 = !"hello"` is not valid standalone metadata in LLVM textual IR.
/// Mirrors `LLParser::parseStandaloneMetadata`, which requires `!N = !{...}`
/// or a specialized metadata node after the `=`.
#[test]
fn standalone_metadata_string_is_rejected() {
    let err = parse_fails(r#"!0 = !"hello""#);
    assert!(
        err.contains("metadata string or tuple") || err.contains("'{'"),
        "unexpected error: {err}"
    );
}

/// Specialized standalone metadata definitions also require the `!` metadata
/// sigil before the node kind.
/// Mirrors `LLParser::parseStandaloneMetadata` rejecting non-metadata tokens.
#[test]
fn standalone_metadata_bare_dieexpression_is_rejected() {
    let err = parse_fails(r#"!0 = DIExpression()"#);
    assert!(
        err.contains("metadata string or tuple") || err.contains("'!'"),
        "unexpected error: {err}"
    );
}

/// `!0 = !{!"hello"}` is the LLVM-valid form for a tuple containing an
/// MDString operand.
/// Mirrors `test/Assembler/metadata.ll` tuple metadata coverage.
#[test]
fn standalone_metadata_tuple_with_inline_string() {
    let src = r#"!0 = !{!"hello"}"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 1);
    assert!(text.contains(r#"!0 = !{!"hello"}"#), "output: {text}");
}

// ── Standalone metadata: tuple nodes ─────────────────────────────────────

/// `!0 = !{}` — empty tuple.
/// Mirrors `test/Assembler/metadata.ll`.
#[test]
fn standalone_metadata_empty_tuple() {
    let src = "!0 = !{}";
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 1);
    assert!(text.contains("!0 = !{}"), "output: {text}");
}

/// Tuple containing an inline metadata string operand.
///
/// An `MDString` operand is printed inline in the tuple body
/// (`!0 = !{!"hello"}`) rather than as a numbered standalone node: LLVM
/// never numbers `MDString`s as standalone nodes, and a top-level
/// `!0 = !"hello"` is rejected by `clang`/`llvm-as`.
#[test]
fn standalone_metadata_tuple_with_ref() {
    let src = r#"!0 = !{!"hello"}"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 1);
    assert!(text.contains(r#"!0 = !{!"hello"}"#), "output: {text}");
    assert!(!text.contains(r#"!1 = !"hello""#), "output: {text}");
}

/// Multi-operand tuple. `MDString` operands inline into the tuple body.
#[test]
fn standalone_metadata_tuple_multi_operand() {
    let src = r#"!0 = !{!"a", !"b"}"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 1);
    assert!(text.contains(r#"!0 = !{!"a", !"b"}"#), "output: {text}");
}

/// Mirrors `llvm/test/Analysis/ValueTracking/known-bits-from-range-md.ll`
/// typed `!range` endpoint operands.
#[test]
fn typed_constant_metadata_tuple_round_trips() {
    let (_stats, text) = parse_snippet("!0 = !{i64 1, i64 5}\n");
    assert!(text.contains("!0 = !{i64 1, i64 5}"), "{text}");
}

/// Mirrors `llvm/test/Analysis/ValueTracking/known-bits-from-range-md.ll`
/// load metadata attachments with typed integer endpoints.
#[test]
fn range_metadata_attachment_round_trips() {
    let src = include_str!("fixtures/upstream/Analysis/ValueTracking/known-bits-from-range-md.ll");
    let (_stats, text) = parse_snippet(src);
    assert!(
        text.contains("  %val = load i8, ptr %ptr, !range !0"),
        "{text}"
    );
    assert!(
        text.contains("  %val = load i8, ptr %ptr, !range !1"),
        "{text}"
    );
    assert!(
        text.contains("  %val = load i8, ptr %ptr, !range !2"),
        "{text}"
    );
    assert!(text.contains("!0 = !{i8 -50, i8 0}"), "{text}");
    assert!(text.contains("!1 = !{i8 64, i8 -128}"), "{text}");
    assert!(text.contains("!2 = !{i8 64, i8 -127}"), "{text}");
}

/// Mirrors `llvm/test/Verifier/range-2.ll`: the assembler accepts the valid
/// load/call/invoke `!range` metadata forms in the fixture without rewrites.
#[test]
fn upstream_valid_range_metadata_fixture_parses() {
    let src = include_str!("fixtures/upstream/Verifier/range-2.ll");
    let (_stats, text) = parse_snippet(src);
    parse_and_verify(src).expect("range-2 fixture verifies");
    assert!(text.contains("call i8 @f1(ptr %x), !range !0"), "{text}");
    assert!(text.contains("invoke i8 @f1(ptr %x)"), "{text}");
    assert!(
        text.contains("personality ptr @__gxx_personality_v0"),
        "{text}"
    );
    assert!(text.contains("filter [0 x ptr] zeroinitializer"), "{text}");
    assert!(
        text.contains("declare i32 @__gxx_personality_v0(...)"),
        "{text}"
    );
    assert!(text.contains("!range !0"), "{text}");
}

/// Mirrors `llvm/test/Verifier/range-1.ll`: every invalid `!range` case
/// reports the same verifier message checked by the upstream fixture.
#[test]
fn upstream_invalid_range_metadata_fixture_messages_match() {
    let fixture = include_str!("fixtures/upstream/Verifier/range-1.ll");
    let cases = [
        (
            "define void @f1",
            "!0 = ",
            "Ranges are only for loads, calls and invokes!",
        ),
        (
            "define i8 @f2",
            "!1 = ",
            "It should have at least one range!",
        ),
        ("define i8 @f3", "!2 = ", "Unfinished range!"),
        (
            "define i8 @f4",
            "!3 = ",
            "The lower limit must be an integer!",
        ),
        (
            "define i8 @f5",
            "!4 = ",
            "The upper limit must be an integer!",
        ),
        ("define i8 @f6", "!5 = ", "Range pair types must match!"),
        ("define i8 @f7", "!6 = ", "Range pair types must match!"),
        (
            "define i8 @f8",
            "!7 = ",
            "Range types must match instruction type!",
        ),
        ("define i8 @f9", "!8 = ", "Range must not be empty!"),
        ("define i8 @f10", "!9 = ", "Intervals are overlapping"),
        ("define i8 @f11", "!10 = ", "Intervals are contiguous"),
        ("define i8 @f12", "!11 = ", "Intervals are not in order"),
        ("define i8 @f13", "!12 = ", "Intervals are contiguous"),
        ("define i8 @f14", "!13 = ", "Intervals are overlapping"),
        ("define i8 @f15", "!14 = ", "Intervals are overlapping"),
        ("define i8 @f16", "!16 = ", "Intervals are overlapping"),
        ("define i8 @f17", "!17 = ", "Intervals are contiguous"),
        (
            "define i8 @f18",
            "!18 = ",
            "It should have at least one range!",
        ),
        (
            "define <2 x i8> @vector_range_wrong_type",
            "!19 = ",
            "Range types must match instruction type!",
        ),
        (
            "define i32 @range_assert",
            "!20 = ",
            "The upper and lower limits cannot be the same value",
        ),
    ];
    for (function_marker, metadata_marker, expected) in cases {
        let src = fixture_function_with_metadata(fixture, function_marker, metadata_marker);
        let message = parse_and_verify_failure_message(&src);
        assert_eq!(message, expected, "case {function_marker}");
    }
}

/// Tuple operands accept specialized metadata only in LLVM's bang-bearing form.
/// Mirrors `LLParser::parseMDTuple` delegating to `parseMetadata`.
#[test]
fn standalone_metadata_tuple_with_inline_dieexpression() {
    let src = r#"!0 = !{!DIExpression()}"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 2);
    assert!(text.contains("!0 = !{!DIExpression()}"), "output: {text}");
}

/// Bare specialized metadata is not a metadata tuple operand.
/// Mirrors `LLParser::parseMDTuple` rejecting non-metadata tokens.
#[test]
fn standalone_metadata_tuple_bare_dieexpression_is_rejected() {
    let err = parse_fails(r#"!0 = !{DIExpression()}"#);
    assert!(
        err.contains("'!' in metadata tuple operand") || err.contains("metadata tuple operand"),
        "unexpected error: {err}"
    );
}

/// `distinct` keyword is accepted and transparent.
/// Mirrors `test/Assembler/distinct-mdnode.ll`.
#[test]
fn standalone_metadata_distinct() {
    let src = r#"!0 = distinct !{}"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 1);
    assert!(text.contains("!0 = distinct !{}"), "output: {text}");
}

// ── Named metadata ───────────────────────────────────────────────────────

/// `!llvm.ident = !{!0}` — basic named metadata.
/// Mirrors `test/Assembler/named-metadata.ll`.
#[test]
fn named_metadata_basic() {
    let src = r#"
!0 = !{!"clang version 16.0.0"}
!llvm.ident = !{!0}
"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 1);
    assert_eq!(m.named_metadata_count(), 1);
    assert!(text.contains("!llvm.ident = !{!0}"), "output: {text}");
}

/// Multiple named metadata nodes.
#[test]
fn named_metadata_multiple() {
    let src = r#"
!0 = !{!"a"}
!1 = !{!"b"}
!llvm.ident = !{!0}
!llvm.module.flags = !{!1}
"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.named_metadata_count(), 2);
    assert!(text.contains("!llvm.ident = !{!0}"), "output: {text}");
    assert!(
        text.contains("!llvm.module.flags = !{!1}"),
        "output: {text}"
    );
}

/// Named metadata with multiple operands.
#[test]
fn named_metadata_multi_operand() {
    let src = r#"
!0 = !{!"a"}
!1 = !{!"b"}
!foo = !{!0, !1}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("!foo = !{!0, !1}"), "output: {text}");
}

/// Empty named metadata.
#[test]
fn named_metadata_empty() {
    let src = "!empty = !{}";
    let (m, text) = parse_snippet(src);
    assert_eq!(m.named_metadata_count(), 1);
    assert!(text.contains("!empty = !{}"), "output: {text}");
}

// ── Combined: metadata with other module-level entities ──────────────────

/// Metadata after function definitions round-trips correctly.
#[test]
fn metadata_after_function() {
    let src = r#"
define void @f() {
  ret void
}

!0 = !{!"test"}
!llvm.ident = !{!0}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("define void @f()"), "output: {text}");
    assert!(text.contains(r#"!0 = !{!"test"}"#), "output: {text}");
    assert!(text.contains("!llvm.ident = !{!0}"), "output: {text}");
}

// ── Instruction trailing metadata attachments ────────────────────────────

/// Instructions with trailing `, !dbg !N` metadata are accepted.
/// Mirrors `test/Assembler/metadata.ll`.
#[test]
fn instruction_trailing_metadata() {
    let src = r#"
define i32 @f(i32 %x, i32 %y) {
  %z = add i32 %x, %y, !dbg !0
  ret i32 %z
}

!0 = !{}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("add i32 %x, %y, !dbg !0"), "output: {text}");
}

/// Multiple trailing metadata attachments on one instruction.
#[test]
fn instruction_multiple_trailing_metadata() {
    let src = r#"
define void @f() {
  ret void, !dbg !0, !tbaa !1
}

!0 = !{}
!1 = !{}
"#;
    let (_, text) = parse_snippet(src);
    assert!(
        text.contains("ret void, !dbg !0, !tbaa !1"),
        "output: {text}"
    );
}

/// Trailing metadata attachments require the metadata sigil before specialized
/// metadata operands.
/// Mirrors `LLParser::parseInstructionMetadata` metadata operand parsing.
#[test]
fn trailing_metadata_bare_dieexpression_is_rejected() {
    let err = parse_fails(
        r#"
define void @f() {
  ret void, !dbg DIExpression()
}
"#,
    );
    assert!(
        err.contains("metadata attachment operand") || err.contains("metadata field value"),
        "unexpected error: {err}"
    );
}

/// Undefined trailing instruction metadata references are rejected at end of
/// module.
/// Mirrors `LLParser::validateEndOfModule` forward-reference validation.
#[test]
fn undefined_trailing_metadata_operand_is_rejected() {
    let src = r#"
define void @f() {
  ret void, !dbg !42
}
"#;
    let err = parse_fails(src);
    assert!(
        err.contains("undefined") && err.contains("42"),
        "unexpected error: {err}"
    );
}
/// Trailing metadata attachments require a preceding comma.
/// Upstream `LLParser` rejects the no-comma variant.
#[test]
fn instruction_trailing_metadata_no_comma() {
    let src = r#"
define i32 @f(i32 %x) {
  %y = add i32 %x, 1 !dbg !0
  ret i32 %y
}

!0 = !{}
"#;
    let err = parse_fails(src);
    assert!(
        err.contains("expected ',' before trailing metadata"),
        "unexpected error: {err}"
    );
}

// ── `metadata` as a call argument (MetadataAsValue) ──────────────────────

/// A `call` whose argument is a `metadata` node — the shape the
/// named-register intrinsics (`@llvm.read_register`,
/// `@llvm.write_register`) require. The `metadata !N` operand parses
/// back to a `MetadataAsValue` and re-prints unchanged.
/// Mirrors `test/CodeGen/Generic/read-write-register.ll`.
#[test]
fn call_with_metadata_argument_roundtrip() {
    let src = r#"
declare i64 @llvm.read_register.i64(metadata)

define i64 @get_sp() {
  %rsp = call i64 @llvm.read_register.i64(metadata !0)
  ret i64 %rsp
}

!0 = !{}
"#;
    let (_, text) = parse_snippet(src);
    assert!(
        text.contains("call i64 @llvm.read_register.i64(metadata !0)"),
        "output: {text}"
    );
}

/// `void`-returning `write_register` variant: a `metadata` argument
/// followed by a normal SSA value argument.
#[test]
fn call_with_metadata_and_value_argument_roundtrip() {
    let src = r#"
declare void @llvm.write_register.i64(metadata, i64)

define void @set_sp(i64 %v) {
  call void @llvm.write_register.i64(metadata !0, i64 %v)
  ret void
}

!0 = !{}
"#;
    let (_, text) = parse_snippet(src);
    assert!(
        text.contains("call void @llvm.write_register.i64(metadata !0, i64 %v)"),
        "output: {text}"
    );
}

/// Inline metadata tuple operands in `metadata`-typed call arguments are legal.
/// Mirrors `LLParser::parseMetadataAsValue` delegating to `parseMetadata`.
#[test]
fn call_metadata_inline_tuple_operand_round_trips() {
    let src = r#"
declare void @g(metadata)
define void @f() {
entry:
  call void @g(metadata !{})
  ret void
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("call void @g(metadata !0)"), "output: {text}");
    assert!(text.contains("!0 = !{}"), "output: {text}");
}

/// Inline metadata string operands in `metadata`-typed call arguments are legal.
/// Mirrors `LLParser::parseMetadata` `MDString` arm.
#[test]
fn call_metadata_inline_string_operand_round_trips() {
    let src = r#"
declare void @g(metadata)
define void @f() {
entry:
  call void @g(metadata !"rsp")
  ret void
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(
        text.contains(r#"call void @g(metadata !"rsp")"#),
        "output: {text}"
    );
}

/// Inline specialized metadata operands in `metadata`-typed call arguments are legal.
/// Mirrors `LLParser::parseMetadataAsValue` delegating to `parseMetadata`.
#[test]
fn call_metadata_inline_dieexpression_operand_round_trips() {
    let src = r#"
declare void @g(metadata)
define void @f() {
entry:
  call void @g(metadata !DIExpression())
  ret void
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(
        text.contains("call void @g(metadata !DIExpression())"),
        "output: {text}"
    );
}

/// Inline specialized metadata field values keep the leading `!` form accepted
/// by LLVM's metadata parser.
/// Mirrors `LLParser::parseMDField` delegating to `parseMetadata`.
#[test]
fn specialized_metadata_field_inline_dieexpression_round_trips() {
    let src = r#"
!0 = !DIGlobalVariable(name: "g")
!1 = !DIGlobalVariableExpression(var: !0, expr: !DIExpression())
"#;
    let (_, text) = parse_snippet(src);
    assert!(
        text.contains("!1 = !DIGlobalVariableExpression(var: !0, expr: !DIExpression())"),
        "output: {text}"
    );
}

/// A specialized metadata value still requires LLVM's `!` metadata sigil.
/// Mirrors `LLParser::parseMetadataAsValue` rejecting non-metadata tokens.
#[test]
fn call_metadata_bare_dieexpression_operand_is_rejected() {
    let err = parse_fails(
        r#"
declare void @g(metadata)
define void @f() {
entry:
  call void @g(metadata DIExpression())
  ret void
}
"#,
    );
    assert!(
        err.contains("constant initializer"),
        "unexpected error: {err}"
    );
}

/// Metadata fields likewise require the leading `!` for specialized metadata.
/// Mirrors `LLParser::parseMDField` rejecting non-metadata tokens.
#[test]
fn specialized_metadata_field_bare_dieexpression_is_rejected() {
    let err = parse_fails(
        r#"
!0 = !DIGlobalVariable(name: "g")
!1 = !DIGlobalVariableExpression(var: !0, expr: DIExpression())
"#,
    );
    assert!(
        err.contains("metadata field value") || err.contains("metadata node"),
        "unexpected error: {err}"
    );
}

// ── Writer/parser round-trip robustness ──────────────────────────────────

/// The AsmWriter inlines `MDString` tuple operands as `!{!"hello"}`; the
/// parser must read that form back, so the writer's own output reparses.
/// Regression test for the inline-string emission.
#[test]
fn inline_string_tuple_reparses() {
    let src = r#"!0 = !{!"hello"}"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains(r#"!0 = !{!"hello"}"#), "output: {text}");
    // Re-parse the writer's output: it must be accepted, and re-printing
    // it must reproduce the same inline-string node (stable round-trip).
    let reparsed = Module::with_new("test", |m2| {
        Parser::new(text.as_bytes(), &m2)
            .expect("ctor")
            .parse_module()
            .expect("writer output must reparse");
        format!("{m2}")
    });
    assert_eq!(reparsed, text, "round-trip must be stable");
}

/// Textual metadata slots need not be dense or 0-based: a `metadata !3`
/// reference with `!3` defined later resolves to a real node (not a
/// dangling `!3`). The slot is remapped to its arena id on print.
/// Regression test for the slot/arena-index decoupling.
#[test]
fn nonzero_metadata_slot_resolves() {
    let src = r#"
declare i64 @g(metadata)
define i64 @f() {
  %r = call i64 @g(metadata !3)
  ret i64 %r
}
!3 = !{}
"#;
    let (_, text) = parse_snippet(src);
    // The reference and its definition agree on a single slot number, and
    // re-parsing succeeds (no dangling reference).
    Module::with_new("rt", |m2| {
        Parser::new(text.as_bytes(), &m2)
            .expect("ctor")
            .parse_module()
            .expect("output must reparse with a resolvable metadata slot");
    });
    // The node the call references is actually defined in the output.
    assert!(text.contains("@g(metadata !0)"), "output: {text}");
    assert!(text.contains("!0 = !{}"), "output: {text}");
}

/// Undefined metadata references are rejected at end of module.
/// Mirrors `LLParser::validateEndOfModule` reporting `use of undefined metadata '!N'`.
#[test]
fn undefined_named_metadata_operand_is_rejected() {
    let err = parse_fails("!foo = !{!42}");
    assert!(
        err.contains("undefined") && err.contains("42"),
        "unexpected error: {err}"
    );
}

/// Undefined metadata-as-value operands are rejected at end of module.
/// Mirrors `LLParser::validateEndOfModule` forward-reference validation.
#[test]
fn undefined_metadata_value_operand_is_rejected() {
    let src = r#"
declare void @g(metadata)
define void @f() {
entry:
  call void @g(metadata !42)
  ret void
}
"#;
    let err = parse_fails(src);
    assert!(
        err.contains("undefined") && err.contains("42"),
        "unexpected error: {err}"
    );
}

/// A `!N` token is only a valid operand where the declared type is
/// `metadata`; a stray metadata reference in a non-metadata slot is a
/// parse error rather than a silently mistyped value.
#[test]
fn metadata_ref_in_non_metadata_type_is_rejected() {
    let src = r#"
declare void @g(i64)
define void @f() {
  call void @g(i64 !0)
  ret void
}
!0 = !{}
"#;
    let err = Module::with_new("t", |m| {
        Parser::new(src.as_bytes(), &m)
            .expect("ctor")
            .parse_module()
            .expect_err("i64 !0 must be rejected")
    });
    assert!(
        err.to_string()
            .contains("expected `metadata` type for a metadata operand"),
        "unexpected error: {err}"
    );
}
