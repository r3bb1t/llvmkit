//! Instruction modifier parsing tests — Session 3.1.
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError};
use llvmkit_ir::Module;

fn parse_fixture(module_name: &str, src: &[u8]) -> String {
    Module::with_new(module_name, |module| {
        Parser::new(src, &module)
            .expect("parse constructor")
            .parse_module()
            .expect("parse succeeded");
        format!("{module}")
    })
}

fn parse_err(src: &[u8]) -> ParseError {
    Module::with_new("parser_modifiers_err", |module| {
        Parser::new(src, &module)
            .expect("parse constructor")
            .parse_module()
            .expect_err("parse rejected invalid modifier")
    })
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

/// getelementptr nusw nuw — `test/Assembler/flags.ll::gep_nusw_nuw`. This is
/// AsmWriter's own canonical flag order, so failing to parse it means the
/// printer emits IR the parser cannot read back.
#[test]
fn gep_nusw_nuw_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/flags/gep_nusw_nuw_round_trips.ll");

    let text = parse_fixture("gep_nusw_nuw_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["%gep = getelementptr nusw nuw i8, ptr %p, i64 %idx"],
    );
}

/// getelementptr nuw nusw inbounds — `test/Assembler/flags.ll::gep_nuw_nusw_inbounds`:
/// GEP flags parse in ANY order (upstream `LLParser::parseGetElementPtr`
/// loops) and re-print canonically, with nusw suppressed under inbounds.
#[test]
fn gep_reversed_flag_order_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/flags/gep_reversed_flag_order_round_trips.ll");

    let text = parse_fixture("gep_reversed_flag_order_round_trips", FIXTURE);
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

// ── Function memory attributes ────────────────────────────────────────────

/// Mirrors `llvm/test/Assembler/memory-attribute.ll` and
/// `lib/IR/Attributes.cpp::Attribute::getAsString`: exact `memory(...)`
/// attributes parse, and legacy memory keywords upgrade to the canonical form.
#[test]
fn memory_attribute_round_trips() {
    let text = parse_fixture(
        "memory_attribute_round_trips",
        b"declare void @f() memory(argmem: read, target_mem0: write)\ndeclare void @g() readonly\n",
    );

    assert_check_lines(
        &text,
        &[
            "declare void @f() memory(argmem: read, target_mem0: write)",
            "declare void @g() memory(read)",
        ],
    );
}

/// Mirrors `llvm/test/Assembler/memory-attribute-errors.ll`: `other` is the
/// default memory access class in `Attribute::getAsString`, not an explicit
/// `memory(...)` location accepted by the LLVM parser.
#[test]
fn memory_attribute_rejects_explicit_other_location() {
    let err = parse_err(b"declare void @f() memory(other: read, argmem: write)\n");
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "memory attribute access kind");
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `llvm/test/Bitcode/upgrade-memory-intrinsics.ll`: legacy memory
/// keywords on pointer parameters remain parameter attributes, while bare
/// function attributes upgrade to `memory(...)`.
#[test]
fn parameter_legacy_memory_keywords_remain_parameter_attrs() {
    let text = parse_fixture(
        "parameter_legacy_memory_keywords_remain_parameter_attrs",
        b"declare void @f(ptr readonly, ptr writeonly, ptr readnone)\n",
    );

    assert_check_lines(
        &text,
        &["ptr readonly %0", "ptr writeonly %1", "ptr readnone %2"],
    );
}

/// Mirrors `llvm/test/Bitcode/upgrade-memory-intrinsics.ll`: legacy memory
/// keywords on call operands remain parameter attributes rather than upgrading
/// the call's function attributes.
#[test]
fn call_parameter_legacy_memory_keywords_remain_parameter_attrs() {
    let text = parse_fixture(
        "call_parameter_legacy_memory_keywords_remain_parameter_attrs",
        b"declare void @g(ptr, ptr, ptr)\ndefine void @f(ptr %p, ptr %q, ptr %r) {\nentry:\n  call void @g(ptr readonly %p, ptr writeonly %q, ptr readnone %r)\n  ret void\n}\n",
    );

    assert_check_lines(
        &text,
        &["call void @g(ptr readonly %p, ptr writeonly %q, ptr readnone %r)"],
    );
    assert!(!text.contains("memory("), "{text}");
}

/// Mirrors `llvm/test/Assembler/memory-attribute-errors.ll`: after a
/// location-specific component, LLVM requires an explicit access kind; a bare
/// default access kind is not another component.
#[test]
fn memory_attribute_rejects_default_access_after_location() {
    let err = Module::with_new("memory_attribute_error", |module| {
        Parser::new(b"declare void @f() memory(argmem: read, write)\n", &module)
            .expect("parse constructor")
            .parse_module()
            .expect_err("memory attribute is malformed")
    });

    match err {
        llvmkit_asmparser::parse_error::ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "memory attribute access kind")
        }
        other => panic!("unexpected parse error: {other:?}"),
    }
}
