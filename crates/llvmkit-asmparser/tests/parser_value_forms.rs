//! Value form parsing tests — Session 4 gaps.
//!
//! llvmkit-specific parser subsets for `undef`, `poison`, float literals, and
//! global references in instruction operand position. The stale
//! `test/Assembler/*.ll` fixture names previously cited here are not present in
//! LLVM 22.1.4; `UPSTREAM.md` cites the relevant `LLParser` branches.

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

/// llvmkit-specific subset: `undef` as an operand via `LLParser::parseValID`.
#[test]
fn undef_operand() {
    let src = r#"
define i32 @f() {
  %x = add i32 0, undef
  ret i32 %x
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("%x = add i32 0, undef\n"), "output: {text}");
}

/// llvmkit-specific subset: `poison` as an operand via `LLParser::parseValID`.
#[test]
fn poison_operand() {
    let src = r#"
define i32 @f() {
  %x = add i32 poison, 1
  ret i32 %x
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(text.contains("%x = add i32 poison, 1\n"), "output: {text}");
}

// ── Float literals ───────────────────────────────────────────────────────

/// llvmkit-specific subset: decimal FP literal in `LLParser::parseValID`.
#[test]
fn float_decimal_literal() {
    let src = r#"
define double @f(double %x) {
  %y = fadd double %x, 1.0
  ret double %y
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(
        text.contains("%y = fadd double %x, 1.000000e+00\n"),
        "output: {text}"
    );
}

/// llvmkit-specific subset: hex FP literal in `LLParser::parseValID`.
#[test]
fn float_hex_literal() {
    let src = r#"
define double @f(double %x) {
  %y = fadd double %x, 0x3FF0000000000000
  ret double %y
}
"#;
    let (_, text) = parse_snippet(src);
    assert!(
        text.contains("%y = fadd double %x, 1.000000e+00\n"),
        "output: {text}"
    );
}

// ── zeroinitializer for float ────────────────────────────────────────────

/// llvmkit-specific subset: `zeroinitializer` for float type.
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

/// llvmkit-specific subset: load from a global variable reference.
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
    assert!(text.contains("%v = load i32, ptr @g\n"), "output: {text}");
}

/// llvmkit-specific subset: function call via global reference.
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
    assert!(
        text.contains("%v = call i32 @callee(i32 %x)\n"),
        "output: {text}"
    );
}
