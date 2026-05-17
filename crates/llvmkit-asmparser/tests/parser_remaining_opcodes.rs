//! Parser integration tests for remaining S3.2 opcodes.
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

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

// ── Cast ops ─────────────────────────────────────────────────────────────────

/// `bitcast i32 %x to float` — same-bit-width reinterpret cast.
/// Mirrors `test/Assembler/bitcast.ll`.
#[test]
fn bitcast_round_trips() {
    let (_, text) = parse_snippet(
        r#"define float @f(i32 %x) {
%r = bitcast i32 %x to float
ret float %r
}
"#,
    );
    assert!(text.contains("bitcast"), "got: {text}");
}

/// `fptrunc double %x to float` — narrowing FP cast.
/// Mirrors `test/Assembler/fptrunc.ll`.
#[test]
fn fptrunc_round_trips() {
    let (_, text) = parse_snippet(
        r#"define float @f(double %x) {
%r = fptrunc double %x to float
ret float %r
}
"#,
    );
    assert!(text.contains("fptrunc"), "got: {text}");
}

/// `fpext float %x to double` — widening FP cast.
/// Mirrors `test/Assembler/fpext.ll`.
#[test]
fn fpext_round_trips() {
    let (_, text) = parse_snippet(
        r#"define double @f(float %x) {
%r = fpext float %x to double
ret double %r
}
"#,
    );
    assert!(text.contains("fpext"), "got: {text}");
}

// ── Vector ops ────────────────────────────────────────────────────────────────

/// `extractelement <4 x i32> %v, i32 0` — extract one lane.
/// Mirrors `test/Assembler/extractelement.ll`.
#[test]
fn extractelement_round_trips() {
    let (_, text) = parse_snippet(
        r#"define i32 @f(<4 x i32> %v) {
%r = extractelement <4 x i32> %v, i32 0
ret i32 %r
}
"#,
    );
    assert!(text.contains("extractelement"), "got: {text}");
}

/// `insertelement <4 x i32> %v, i32 %x, i32 0` — write one lane.
/// Mirrors `test/Assembler/insertelement.ll`.
#[test]
fn insertelement_round_trips() {
    let (_, text) = parse_snippet(
        r#"define <4 x i32> @f(<4 x i32> %v, i32 %x) {
%r = insertelement <4 x i32> %v, i32 %x, i32 0
ret <4 x i32> %r
}
"#,
    );
    assert!(text.contains("insertelement"), "got: {text}");
}

/// `shufflevector` with an explicit 4-element mask.
/// Mirrors `test/Assembler/shufflevector.ll`.
#[test]
fn shufflevector_round_trips() {
    let (_, text) = parse_snippet(
        r#"define <4 x i32> @f(<4 x i32> %a, <4 x i32> %b) {
%r = shufflevector <4 x i32> %a, <4 x i32> %b, <i32 0, i32 1, i32 4, i32 5>
ret <4 x i32> %r
}
"#,
    );
    assert!(text.contains("shufflevector"), "got: {text}");
}

// ── Aggregate ops ─────────────────────────────────────────────────────────────

/// `extractvalue { i32, i64 } %s, 0` — read a struct field.
/// Mirrors `test/Assembler/extractvalue.ll`.
#[test]
fn extractvalue_round_trips() {
    let (_, text) = parse_snippet(
        r#"define i32 @f({ i32, i64 } %s) {
%r = extractvalue { i32, i64 } %s, 0
ret i32 %r
}
"#,
    );
    assert!(text.contains("extractvalue"), "got: {text}");
}

/// `insertvalue { i32, i64 } %s, i32 %x, 0` — write a struct field.
/// Mirrors `test/Assembler/insertvalue.ll`.
#[test]
fn insertvalue_round_trips() {
    let (_, text) = parse_snippet(
        r#"define { i32, i64 } @f({ i32, i64 } %s, i32 %x) {
%r = insertvalue { i32, i64 } %s, i32 %x, 0
ret { i32, i64 } %r
}
"#,
    );
    assert!(text.contains("insertvalue"), "got: {text}");
}

// ── SSA/control-flow ──────────────────────────────────────────────────────────

/// `phi i32` in a simple counted loop. Tests forward-reference resolution.
/// Mirrors `test/Assembler/phi.ll`.
#[test]
fn phi_int_round_trips() {
    let (_, text) = parse_snippet(
        r#"define i32 @f(i32 %n) {
entry:
  br label %loop
loop:
  %i = phi i32 [ 0, %entry ], [ %next, %loop ]
  %next = add i32 %i, 1
  %done = icmp eq i32 %next, %n
  br i1 %done, label %exit, label %loop
exit:
  ret i32 %i
}
"#,
    );
    assert!(text.contains("phi i32"), "got: {text}");
}

/// `call i32 @add(i32 %x, i32 %y)` — direct function call.
/// Mirrors `test/Assembler/call.ll`.
#[test]
fn call_function_round_trips() {
    let (_, text) = parse_snippet(
        r#"declare i32 @add(i32, i32)
define i32 @f(i32 %x, i32 %y) {
%r = call i32 @add(i32 %x, i32 %y)
ret i32 %r
}
"#,
    );
    assert!(text.contains("call"), "got: {text}");
}

/// `freeze i32 %x` — freeze an undef/poison value.
/// Mirrors `test/Assembler/freeze.ll`.
#[test]
fn freeze_round_trips() {
    let (_, text) = parse_snippet(
        r#"define i32 @f(i32 %x) {
%r = freeze i32 %x
ret i32 %r
}
"#,
    );
    assert!(text.contains("freeze"), "got: {text}");
}

// ── Terminators ───────────────────────────────────────────────────────────────

/// `switch i32 %x, label %default [ i32 0, label %c0  i32 1, label %c1 ]`.
/// Mirrors `test/Assembler/switch.ll`.
#[test]
fn switch_round_trips() {
    let (_, text) = parse_snippet(
        r#"define i32 @f(i32 %x) {
entry:
  switch i32 %x, label %default [
    i32 0, label %c0
    i32 1, label %c1
  ]
default:
  ret i32 2
c0:
  ret i32 0
c1:
  ret i32 1
}
"#,
    );
    assert!(text.contains("switch"), "got: {text}");
}

/// `indirectbr ptr %addr, [label %d1, label %d2]` — indirect branch.
/// Mirrors `test/Assembler/indirectbr.ll`.
#[test]
fn indirectbr_round_trips() {
    let (_, text) = parse_snippet(
        r#"define void @f(ptr %addr) {
entry:
  indirectbr ptr %addr, [label %d1, label %d2]
d1:
  ret void
d2:
  ret void
}
"#,
    );
    assert!(text.contains("indirectbr"), "got: {text}");
}

// ── Atomic ops ────────────────────────────────────────────────────────────────

/// `fence acquire` — memory ordering barrier.
/// Mirrors `test/Assembler/fence.ll`.
#[test]
fn fence_round_trips() {
    let (_, text) = parse_snippet(
        r#"define void @f() {
  fence acquire
  ret void
}
"#,
    );
    assert!(text.contains("fence"), "got: {text}");
}

/// `cmpxchg ptr %p, i32 %cmp, i32 %new seq_cst seq_cst` — atomic CAS.
/// Mirrors `test/Assembler/cmpxchg.ll`.
#[test]
fn cmpxchg_round_trips() {
    let (_, text) = parse_snippet(
        r#"define void @f(ptr %p, i32 %cmp, i32 %new) {
  %r = cmpxchg ptr %p, i32 %cmp, i32 %new seq_cst seq_cst
  ret void
}
"#,
    );
    assert!(text.contains("cmpxchg"), "got: {text}");
}

/// `atomicrmw add ptr %p, i32 %v seq_cst` — atomic read-modify-write.
/// Mirrors `test/Assembler/atomicrmw.ll`.
#[test]
fn atomicrmw_round_trips() {
    let (_, text) = parse_snippet(
        r#"define void @f(ptr %p, i32 %v) {
  %r = atomicrmw add ptr %p, i32 %v seq_cst
  ret void
}
"#,
    );
    assert!(text.contains("atomicrmw"), "got: {text}");
}
