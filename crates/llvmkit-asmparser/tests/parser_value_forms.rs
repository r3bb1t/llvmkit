//! Value form parsing tests — Session 4 gaps.
//!
//! Tests for `undef`, `poison`, float literals, and global variable
//! references in instruction operand position.

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

// ── undef / poison ───────────────────────────────────────────────────────

/// `undef` as an operand. Mirrors `test/Assembler/undef.ll`.
#[test]
fn undef_operand() {
    let src = r#"
define i32 @f() {
  %x = add i32 0, undef
  ret i32 %x
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("add i32 0, undef"), "output: {text}");
}

/// `poison` as an operand. Mirrors `test/Assembler/poison.ll`.
#[test]
fn poison_operand() {
    let src = r#"
define i32 @f() {
  %x = add i32 poison, 1
  ret i32 %x
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("add i32 poison, 1"), "output: {text}");
}

// ── Float literals ───────────────────────────────────────────────────────

/// Decimal float literal in `fadd`. Mirrors `test/Assembler/float.ll`.
#[test]
fn float_decimal_literal() {
    let src = r#"
define double @f(double %x) {
  %y = fadd double %x, 1.0
  ret double %y
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("fadd double %x,"), "output: {text}");
}

/// Hex float literal. Mirrors `test/Assembler/float-hex.ll`.
#[test]
fn float_hex_literal() {
    let src = r#"
define double @f(double %x) {
  %y = fadd double %x, 0x3FF0000000000000
  ret double %y
}
"#;
    let (_, text) = parse_snippet(src);
    // 0x3FF0000000000000 = 1.0 in IEEE double
    assert!(text.contains("fadd double %x,"), "output: {text}");
}

// ── zeroinitializer for float ────────────────────────────────────────────

/// `zeroinitializer` for float type.
#[test]
fn zeroinitializer_float() {
    let src = r#"
define double @f(double %x) {
  %y = fadd double %x, zeroinitializer
  ret double %y
}
"#;
    let (_, _text) = parse_snippet(src);
    // Parses without error
}

// ── Global variable references ───────────────────────────────────────────

/// Load from a global variable. Mirrors `test/Assembler/globalvariable.ll`.
#[test]
fn global_variable_reference() {
    let src = r#"
@g = global i32 42

define i32 @f() {
  %v = load i32, ptr @g
  ret i32 %v
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("load i32, ptr @g"), "output: {text}");
}

/// Function call via global reference.
#[test]
fn function_call_global_reference() {
    let src = r#"
declare i32 @callee(i32)

define i32 @f(i32 %x) {
  %v = call i32 @callee(i32 %x)
  ret i32 %v
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("call i32 @callee(i32 %x)"), "output: {text}");
}
