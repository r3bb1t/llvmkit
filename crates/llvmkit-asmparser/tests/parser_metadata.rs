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

// ── Standalone metadata: string nodes ────────────────────────────────────

/// `!0 = !"hello"` — simplest metadata string node.
/// Mirrors `test/Assembler/metadata.ll`.
#[test]
fn standalone_metadata_string() {
    let src = r#"!0 = !"hello""#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 1);
    assert!(text.contains(r#"!0 = !"hello""#), "output: {text}");
}

/// Multiple string nodes.
#[test]
fn standalone_metadata_multiple_strings() {
    let src = r#"
!0 = !"first"
!1 = !"second"
"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 2);
    assert!(text.contains(r#"!0 = !"first""#), "output: {text}");
    assert!(text.contains(r#"!1 = !"second""#), "output: {text}");
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

/// `!0 = !"str"` followed by `!1 = !{!0}` — tuple referencing a string.
#[test]
fn standalone_metadata_tuple_with_ref() {
    let src = r#"
!0 = !"hello"
!1 = !{!0}
"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 2);
    assert!(text.contains("!1 = !{!0}"), "output: {text}");
}

/// Multi-operand tuple.
#[test]
fn standalone_metadata_tuple_multi_operand() {
    let src = r#"
!0 = !"a"
!1 = !"b"
!2 = !{!0, !1}
"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.metadata_count(), 3);
    assert!(text.contains("!2 = !{!0, !1}"), "output: {text}");
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
!0 = !"clang version 16.0.0"
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
!0 = !"a"
!1 = !"b"
!llvm.ident = !{!0}
!llvm.module.flags = !{!1}
"#;
    let (m, text) = parse_snippet(src);
    assert_eq!(m.named_metadata_count(), 2);
    assert!(text.contains("!llvm.ident = !{!0}"), "output: {text}");
    assert!(text.contains("!llvm.module.flags = !{!1}"), "output: {text}");
}

/// Named metadata with multiple operands.
#[test]
fn named_metadata_multi_operand() {
    let src = r#"
!0 = !"a"
!1 = !"b"
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

!0 = !"test"
!llvm.ident = !{!0}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("define void @f()"), "output: {text}");
    assert!(text.contains(r#"!0 = !"test""#), "output: {text}");
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
