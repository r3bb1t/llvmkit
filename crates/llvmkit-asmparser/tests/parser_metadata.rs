//! Metadata parsing tests — Session 4.
//!
//! Each `#[test]` mirrors constructive `.ll` fixtures from upstream LLVM.
//! Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::{IrError, Module, module_new};

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
    let module = Module::dynamic("test");
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
}

fn parse_fails(src: &str) -> String {
    let module = Module::dynamic("test");
    let err = Parser::new(src.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect_err("parse should fail");
    err.to_string()
}

fn parse_and_verify(src: &str) -> Result<(), IrError> {
    let module = Module::dynamic("test");
    let _ = Parser::new(src.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    module.verify_borrowed()
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
        text.contains("  %val = load i8, ptr %ptr, align 1, !range !0"),
        "{text}"
    );
    assert!(
        text.contains("  %val = load i8, ptr %ptr, align 1, !range !1"),
        "{text}"
    );
    assert!(
        text.contains("  %val = load i8, ptr %ptr, align 1, !range !2"),
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
///
/// `parseMetadata` sends anything that is not a `!` to
/// `parseValueAsMetadata(MD, "expected metadata operand", PFS)`, so the
/// complaint is about the operand, not about a missing bang. This used to
/// accept llvmkit's own `'!' in metadata tuple operand` behind an `||`.
#[test]
fn standalone_metadata_tuple_bare_dieexpression_is_rejected() {
    assert_eq!(
        parse_fails(r#"!0 = !{DIExpression()}"#),
        "expected metadata operand"
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
///
/// Mirrors `LLParser::parseMetadataAttachment`, which hands straight to
/// `parseMDNode`: without a leading `!` the name is not a `MetadataVar`, so the
/// specialized-node branch is not taken and the fallthrough
/// `parseToken(lltok::exclaim, "expected '!' here")` is what reports it.
#[test]
fn trailing_metadata_bare_dieexpression_is_rejected() {
    let err = parse_fails(
        r#"
define void @f() {
  ret void, !dbg DIExpression()
}
"#,
    );
    assert_eq!(err, "expected '!' here");
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
///
/// Mirrors `LLParser::parseParameterList`, which routes a `metadata`-typed
/// argument to `parseMetadataAsValue` and so to `parseMetadata`. A bare
/// `DIExpression` is neither a `MetadataVar` nor a `!`, so the fallthrough
/// `parseValueAsMetadata(MD, "expected metadata operand", PFS)` takes it — and
/// that message is the *type* message it hands to `parseType`, which is what
/// fails on a keyword that names no type.
///
/// llvmkit used to answer `expected value token` here, from `parseValID`'s
/// default arm: the metadata-typed argument never reached
/// `parseMetadataAsValue` at all, which is the same gap that made
/// `metadata i32 %a` unparseable.
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
    assert_eq!(err, "expected metadata operand");
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
    let reparsed = {
        let m2 = module_new!("test").expect("fresh module");
        Parser::new(text.as_bytes(), &m2)
            .expect("ctor")
            .parse_module()
            .expect("writer output must reparse");
        format!("{m2}")
    };
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
    {
        let m2 = module_new!("rt").expect("fresh module");
        Parser::new(text.as_bytes(), &m2)
            .expect("ctor")
            .parse_module()
            .expect("output must reparse with a resolvable metadata slot");
    }
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
    let err = {
        let m = module_new!("t").expect("fresh module");
        Parser::new(src.as_bytes(), &m)
            .expect("ctor")
            .parse_module()
            .expect_err("i64 !0 must be rejected")
    };
    assert!(
        err.to_string()
            .contains("expected `metadata` type for a metadata operand"),
        "unexpected error: {err}"
    );
}

/// A `target("...")` extension type is a legal `!{...}` tuple operand.
///
/// `LLParser::parseMetadata` sends anything that is not a `!` to
/// `parseValueAsMetadata`, which calls `parseType`; `parseType`'s leading
/// switch has a `lltok::kw_target` case, so a target extension type is a type
/// like any other there. llvmkit's tuple-element lookahead spelled that token
/// set out by hand and omitted `target`, so the element was never routed to
/// the type path at all and the parse died on the missing `!`. The lookahead
/// is now `Parser::peek_begins_a_type`, a single rendering of that switch's
/// case labels, shared with the operand form.
///
/// **Anchored on the routine, not on a fixture** — no `.ll` file was found
/// with a target extension type in a metadata tuple. The operand is `poison`
/// rather than `zeroinitializer` because `poison` is the shape that isolates
/// the lookahead: `spirv.Image` carries no `HasZeroInit` in
/// `llvm/lib/IR/Type.cpp`'s `getTargetTypeInfo`, so
/// `LLParser::convertValIDToValue`'s `ValID::t_Zero` arm rejects
/// `target("spirv.Image") zeroinitializer` upstream too, with
/// `invalid type for null constant`. llvmkit rejects it as well, with that
/// text at that token —
/// `parser_constants.rs::zeroinitializer_of_an_unzeroable_type_is_an_invalid_null_constant`
/// pins the arm.
#[test]
fn a_target_extension_type_is_a_legal_metadata_tuple_operand() {
    let (_, text) = parse_snippet("!0 = !{ target(\"spirv.Image\") poison }\n");
    assert!(
        text.contains("!0 = !{target(\"spirv.Image\") poison}"),
        "output:\n{text}"
    );
}

/// A malformed type in a `!{...}` tuple operand reports the type's own
/// complaint, not `parseValueAsMetadata`'s `TypeMsg`.
///
/// Same policy as
/// `parser_calls.rs::a_malformed_metadata_operand_type_keeps_the_type_s_own_message`,
/// reached through `parseMDNodeVector` -> `parseMetadata` ->
/// `parseValueAsMetadata` instead of through a `metadata`-typed operand.
/// `LLParser::parseType` reads its `Msg` argument only in the `default:` arm of
/// its leading switch, so a token that *does* begin a type never sees it.
///
/// **Anchored on that policy, not on a fixture.**
#[test]
fn a_malformed_metadata_tuple_operand_type_keeps_the_type_s_own_message() {
    assert_eq!(parse_fails("!0 = !{ { i32, } undef }\n"), "expected type");
    assert_eq!(
        parse_fails("!0 = !{ void undef }\n"),
        "void type only allowed for function results"
    );
    assert_eq!(
        parse_fails("!0 = !{ ptr* undef }\n"),
        "ptr* is invalid - use ptr instead"
    );
    assert_eq!(
        parse_fails("!0 = !{ label* undef }\n"),
        "basic block pointers are invalid"
    );
}

/// Mirrors `llvm/test/Verifier/dbg-declare-invalid-debug-loc.ll`, vendored
/// whole under `fixtures/upstream/Verifier/`.
///
/// The fixture's `RUN` line is `opt %s -o /dev/null -S 2>&1 | FileCheck %s`, so
/// most of its `CHECK` block is the *verifier's* rendering, which llvmkit does
/// not reproduce (`docs/divergences.md` entry 121), and llvmkit does not
/// auto-upgrade `@llvm.dbg.declare` into a `#dbg_declare` record either
/// (`docs/future-work.md`, AutoUpgrade). One token in that block is pure
/// `AsmWriter` output, and it is the one asserted here: `ptr %1`, the
/// intrinsic's `ValueAsMetadata` operand, from
/// `; CHECK-NEXT: #dbg_declare(ptr %1, …)`.
///
/// `%1` is an **unnamed** local. Until `AssemblyWriter`'s `SlotTracker` was
/// threaded into the metadata sub-printer — upstream carries it on
/// `AsmWriterContext::Machine` and bottoms out in
/// `writeAsOperandInternal(Out, V->getValue(), WriterCtx, /*PrintType=*/true)`
/// — llvmkit printed `%<unnumbered>` here, and the printed module then failed
/// to re-parse. A *named* value prints correctly either way, which is why the
/// operand-bundle test in `parser_calls.rs` did not catch it; this one pins the
/// unnamed spelling and the re-parse.
///
/// Byte equality across two prints is deliberately **not** asserted: this
/// fixture is one of the files whose `!N` numbering moves on a second print,
/// which is the separate `SlotTracker::processModule` pre-pass gap
/// (`docs/fixture-coverage.md` G19). The operand this test is about is
/// asserted on both prints instead.
#[test]
fn upstream_dbg_declare_fixture_numbers_an_unnamed_metadata_operand() {
    const OPERAND: &str = "call void @llvm.dbg.declare(metadata ptr %1, ";
    let src = include_str!("fixtures/upstream/Verifier/dbg-declare-invalid-debug-loc.ll");
    let (_stats, text) = parse_snippet(src);
    assert!(text.contains(OPERAND), "{text}");
    assert!(!text.contains("<unnumbered>"), "{text}");
    // The printed module must re-parse --- the half that `%<unnumbered>` broke.
    let (_stats, reprinted) = parse_snippet(&text);
    assert!(reprinted.contains(OPERAND), "{reprinted}");
    assert!(!reprinted.contains("<unnumbered>"), "{reprinted}");
}
