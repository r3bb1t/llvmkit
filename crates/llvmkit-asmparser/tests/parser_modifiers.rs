//! Instruction modifier parsing tests — Session 3.1.
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_fixture(module_name: &str, src: &[u8]) -> String {
    let module = Module::new(module_name);
    Parser::new(src, &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    format!("{module}")
}

fn assert_check_lines(text: &str, check_lines: &[&str]) {
    let mut offset = 0;
    for expected in check_lines {
        let tail = &text[offset..];
        let found = tail.find(expected).unwrap_or_else(|| {
            panic!("missing upstream CHECK line `{expected}` after byte {offset}; got:\n{text}")
        });
        offset += found + expected.len();
    }
}

// ── Integer overflow flags on binops ──────────────────────────────────────

/// `add nuw nsw` — exact `test/Assembler/flags.ll` spelling.
#[test]
fn nuw_nsw_add_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/flags/nuw_nsw_add_round_trips.ll");

    let text = parse_fixture("nuw_nsw_add_round_trips", FIXTURE);
    assert_check_lines(&text, &["%z = add nuw nsw i64 %x, %y"]);
}

/// `sub nuw nsw` — exact `test/Assembler/flags.ll` spelling.
#[test]
fn nuw_nsw_sub_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/flags/nuw_nsw_sub_round_trips.ll");

    let text = parse_fixture("nuw_nsw_sub_round_trips", FIXTURE);
    assert_check_lines(&text, &["%z = sub nuw nsw i64 %x, %y"]);
}

/// `udiv exact` — exact `test/Assembler/flags.ll` spelling.
#[test]
fn exact_udiv_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/flags/exact_udiv_round_trips.ll");

    let text = parse_fixture("exact_udiv_round_trips", FIXTURE);
    assert_check_lines(&text, &["%z = udiv exact i64 %x, %y"]);
}

// ── Fast-math flags on fp ops ─────────────────────────────────────────────

/// `fadd ninf nnan` canonicalizes to upstream FMF order from `test/Assembler/fast-math-flags.ll`.
#[test]
fn fmf_fadd_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/fast-math-flags/fmf_fadd_round_trips.ll");

    let text = parse_fixture("fmf_fadd_round_trips", FIXTURE);
    assert_check_lines(&text, &["  %a = fadd nnan ninf float %x, %y"]);
}

/// `fneg nnan` — exact `test/Assembler/fast-math-flags.ll` spelling.
#[test]
fn fmf_fneg_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/fast-math-flags/fmf_fneg_round_trips.ll");

    let text = parse_fixture("fmf_fneg_round_trips", FIXTURE);
    assert_check_lines(&text, &["  %f = fneg nnan float %x"]);
}

// ── Alignment on alloca / load / store ────────────────────────────────────

/// `alloca`, align — mirrors the constructive alignment acceptance in `test/Assembler/align-inst.ll`.
#[test]
fn alloca_align_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/align-inst/alloca_align_round_trips.ll");

    let text = parse_fixture("alloca_align_round_trips", FIXTURE);
    assert_check_lines(&text, &["  %p = alloca i1, align 4294967296"]);
}

/// load with align — mirrors the constructive alignment acceptance in `test/Assembler/align-inst.ll`.
#[test]
fn load_align_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/align-inst/load_align_round_trips.ll");

    let text = parse_fixture("load_align_round_trips", FIXTURE);
    assert_check_lines(&text, &["  %1 = load i1, ptr %p, align 4294967296"]);
}

// ── GEP flags ─────────────────────────────────────────────────────────────

/// getelementptr inbounds nuw — exact `test/Assembler/flags.ll` GEP flag spelling.
#[test]
fn gep_inbounds_nuw_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/flags/gep_inbounds_nuw_round_trips.ll");

    let text = parse_fixture("gep_inbounds_nuw_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["%gep = getelementptr inbounds nuw i8, ptr %p, i64 %idx"],
    );
}

// ── samesign on icmp ──────────────────────────────────────────────────────

/// `icmp samesign ult` — exact `test/Assembler/flags.ll` spelling.
#[test]
fn samesign_icmp_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/flags/samesign_icmp_round_trips.ll");

    let text = parse_fixture("samesign_icmp_round_trips", FIXTURE);
    assert_check_lines(&text, &["%res = icmp samesign ult i32 %a, %b"]);
}

// ── disjoint on or ────────────────────────────────────────────────────────

/// `or disjoint` — exact `test/Assembler/flags.ll` spelling.
#[test]
fn disjoint_or_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/flags/disjoint_or_round_trips.ll");

    let text = parse_fixture("disjoint_or_round_trips", FIXTURE);
    assert_check_lines(&text, &["%res = or disjoint i64 %a, %b"]);
}
