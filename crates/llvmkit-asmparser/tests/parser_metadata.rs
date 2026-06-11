//! Metadata parsing tests — Session 4.
//!
//! Each `#[test]` mirrors constructive `.ll` fixtures from upstream LLVM.
//! Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_snippet(src: &str) -> (Module<'_>, String) {
    let module = Module::new("test");
    let _ = Parser::new(src.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    let text = format!("{module}");
    (module, text)
}

fn parse_fails(src: &str) -> String {
    let module = Module::new("test");
    let err = Parser::new(src.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect_err("parse should fail");
    err.to_string()
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

/// `distinct` keyword is accepted and transparent.
/// Mirrors `test/Assembler/distinct-mdnode.ll`.
#[test]
fn standalone_metadata_distinct() {
    let src = r#"!0 = distinct !{}"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 1);
    // We don't print `distinct` in the constructive subset
    assert!(text.contains("!0 = !{}"), "output: {text}");
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
    // The metadata attachment is silently consumed; the instruction prints
    // without it (per-instruction metadata storage is future work).
    assert!(text.contains("add i32 %x, %y"), "output: {text}");
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
    let (_, _text) = parse_snippet(src);
    // Parses without error — that's the assertion.
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
/// Trailing metadata without comma (no-comma variant).
#[test]
fn instruction_trailing_metadata_no_comma() {
    let src = r#"
define i32 @f(i32 %x) {
  %y = add i32 %x, 1 !dbg !0
  ret i32 %y
}

!0 = !{}
"#;
    let (_, _text) = parse_snippet(src);
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
    let m2 = Module::new("test");
    Parser::new(text.as_bytes(), &m2)
        .expect("ctor")
        .parse_module()
        .expect("writer output must reparse");
    assert_eq!(format!("{m2}"), text, "round-trip must be stable");
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
    let m2 = Module::new("rt");
    Parser::new(text.as_bytes(), &m2)
        .expect("ctor")
        .parse_module()
        .expect("output must reparse with a resolvable metadata slot");
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
    let m = Module::new("t");
    let res = Parser::new(src.as_bytes(), &m)
        .expect("ctor")
        .parse_module();
    assert!(res.is_err(), "i64 !0 must be rejected, got: {res:?}");
}
