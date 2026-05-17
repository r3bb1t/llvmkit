//! Instruction modifier parsing tests — Session 3.1.
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

// ── Integer overflow flags on binops ──────────────────────────────────────

/// `add nuw nsw` — mirrors `test/Assembler/flags.ll`.
#[test]
fn nuw_nsw_add_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define i32 @f(i32 %x, i32 %y) {
%r = add nuw nsw i32 %x, %y
ret i32 %r
}
"#,
    );
    assert!(text.contains("add nuw nsw i32"), "got: {text}");
}

/// `sub nuw nsw` — mirrors same upstream fixture `test/Assembler/flags.ll`.
#[test]
fn nuw_nsw_sub_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define i32 @f(i32 %x, i32 %y) {
%r = sub nuw nsw i32 %x, %y
ret i32 %r
}
"#,
    );
    assert!(text.contains("sub nuw nsw i32"), "got: {text}");
}

/// `udiv exact` — mirrors `test/Assembler/flags.ll`.
#[test]
fn exact_udiv_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define i32 @f(i32 %x, i32 %y) {
%r = udiv exact i32 %x, %y
ret i32 %r
}
"#,
    );
    assert!(text.contains("udiv exact i32"), "got: {text}");
}

// ── Fast-math flags on fp ops ─────────────────────────────────────────────

/// nnan fadd — FMF propagated via build_fp_add_fmf. Mirrors unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags).
#[test]
fn fmf_fadd_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define float @f(float %x, float %y) {
%r = fadd nnan ninf float %x, %y
ret float %r
}
"#,
    );
    assert!(text.contains("fadd nnan"), "got: {text}");
}

/// `nnan fneg` — FNeg propagates FMF via `build_float_neg_with_flags`.
/// Upstream: `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`.
#[test]
fn fmf_fneg_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define float @f(float %x) {
%r = fneg nnan float %x
ret float %r
}
"#,
    );
    assert!(text.contains("fneg nnan float"), "got: {text}");
}

// ── Alignment on alloca / load / store ────────────────────────────────────

/// `alloca`, align — mirrors `test/Assembler/align-inst-alloca.ll`.
#[test]
fn alloca_align_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define ptr @f() {
%p = alloca i32, align 4
ret ptr %p
}
"#,
    );
    assert!(text.contains("align 4"), "got: {text}");
}

/// load with align — align propagated to IR. Mirrors test/Assembler/align-inst-load.ll.
#[test]
fn load_align_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define i32 @f(ptr %p) {
%v = load i32, ptr %p, align 4
ret i32 %v
}
"#,
    );
    assert!(text.contains("align 4"), "got: {text}");
}

// ── GEP flags ─────────────────────────────────────────────────────────────

/// getelementptr inbounds nuw — GepNoWrapFlags propagated. Mirrors test/Assembler/flags.ll.
#[test]
fn gep_inbounds_nuw_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define ptr @f(ptr %p) {
%g = getelementptr inbounds nuw i32, ptr %p, i32 1
ret ptr %g
}
"#,
    );
    assert!(text.contains("inbounds") && text.contains("nuw"), "got: {text}");
}

// ── samesign on icmp ──────────────────────────────────────────────────────

/// icmp samesign — samesign propagated to CmpInstData. Mirrors test/Assembler/flags.ll.
#[test]
fn samesign_icmp_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define i1 @f(i32 %x, i32 %y) {
%c = icmp samesign eq i32 %x, %y
ret i1 %c
}
"#,
    );
    assert!(text.contains("icmp samesign eq"), "got: {text}");
}

// ── disjoint on or ────────────────────────────────────────────────────────

/// or disjoint — disjoint flag propagated to BinaryOpData. Mirrors test/Assembler/flags.ll.
#[test]
fn disjoint_or_round_trips() {
    let (_m, text) = parse_snippet(
        r#"define i32 @f(i32 %x, i32 %y) {
%r = or disjoint i32 %x, %y
ret i32 %r
}
"#,
    );
    assert!(text.contains("or disjoint"), "got: {text}");
}