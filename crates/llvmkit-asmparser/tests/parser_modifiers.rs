//! Instruction modifier parsing tests — Session 3.1.
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError};
use llvmkit_ir::Module;
use llvmkit_ir::module_new;

fn parse_fixture(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    format!("{module}")
}

fn parse_err(src: &[u8]) -> ParseError {
    let module = Module::dynamic("parser_modifiers_err");
    Parser::new(src, &module)
        .expect("parse constructor")
        .parse_module()
        .expect_err("parse rejected invalid modifier")
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

/// `add/sub/mul nsw nuw` — reversed flag order from `test/Assembler/flags.ll`
/// (`@add_both_reversed` etc.); must parse and print canonically as `nuw nsw`.
#[test]
fn nsw_nuw_reversed_binops_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/flags/nsw_nuw_reversed_binops_round_trips.ll");

    let text = parse_fixture("nsw_nuw_reversed_binops_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "%z = add nuw nsw i64 %x, %y",
            "%z = sub nuw nsw i64 %x, %y",
            "%z = mul nuw nsw i64 %x, %y",
        ],
    );
}

/// `trunc nuw nsw` — exact `test/Assembler/flags.ll` spelling
/// (`@test_trunc_both`; the upstream vector form needs vector int-cast
/// support, which parse_int_cast lacks).
#[test]
fn nuw_nsw_trunc_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/flags/nuw_nsw_trunc_round_trips.ll");

    let text = parse_fixture("nuw_nsw_trunc_round_trips", FIXTURE);
    assert_check_lines(&text, &["%res = trunc nuw nsw i64 %a to i32"]);
}

/// `trunc nsw nuw` — reversed flag order from `test/Assembler/flags.ll`
/// (`@test_trunc_both_reversed`; the upstream vector form needs vector
/// int-cast support, which parse_int_cast lacks); prints canonically as
/// `nuw nsw`.
#[test]
fn nsw_nuw_reversed_trunc_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/flags/nsw_nuw_reversed_trunc_round_trips.ll");

    let text = parse_fixture("nsw_nuw_reversed_trunc_round_trips", FIXTURE);
    assert_check_lines(&text, &["%res = trunc nuw nsw i64 %a to i32"]);
}

/// `add/sub nsw nuw (...)` constant expressions — reversed flag order from
/// `test/Assembler/flags.ll` (`@add_both_reversed_ce`/`@sub_both_reversed_ce`);
/// print canonically as `nuw nsw`.
#[test]
fn nsw_nuw_reversed_constexpr_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/flags/nsw_nuw_reversed_constexpr_round_trips.ll");

    let text = parse_fixture("nsw_nuw_reversed_constexpr_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "ret i64 add nuw nsw (i64 ptrtoint (ptr @addr to i64), i64 91)",
            "ret i64 sub nuw nsw (i64 ptrtoint (ptr @addr to i64), i64 91)",
        ],
    );
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

/// Ports `test/Assembler/memory-attribute-errors.ll`. Each split's CHECK line
/// pins one `LLParser::parseMemoryAttr` diagnostic verbatim.
///
/// Five of the fixture's eight splits are here. The other three —
/// `memory(foo)`, `memory(other: read)` and `memory(argmem: foo)` — turn on a
/// word that matches no keyword. Upstream's lexer returns a silent
/// `lltok::Error` there and lets `parseMemoryAttr` report; llvmkit's lexer
/// raises `unknown keyword '...'` itself, so the parser never sees the token.
/// Same rejection, wrong layer and wrong text; re-layering that is the
/// lexer-parity item recorded for the end of the parity program.
#[test]
fn memory_attribute_errors_match_upstream_text() {
    for (fixture, expected) in [
        (
            include_bytes!("fixtures/upstream/memory-attribute-errors/missing_args.ll").as_slice(),
            "expected '('",
        ),
        (
            include_bytes!("fixtures/upstream/memory-attribute-errors/empty.ll").as_slice(),
            "expected memory location (argmem, inaccessiblemem, errnomem) or access kind (none, read, write, readwrite)",
        ),
        (
            include_bytes!("fixtures/upstream/memory-attribute-errors/unterminated.ll").as_slice(),
            "unterminated memory attribute",
        ),
        (
            include_bytes!("fixtures/upstream/memory-attribute-errors/missing_colon.ll").as_slice(),
            "expected ':' after location",
        ),
        (
            include_bytes!("fixtures/upstream/memory-attribute-errors/default_after_loc.ll")
                .as_slice(),
            "default access kind must be specified first",
        ),
    ] {
        assert_eq!(parse_err(fixture).to_string(), expected);
    }
}

/// `memory(argmem: read)` writes its colon as a separator, so
/// `LLParser::parseMemoryAttr` puts the lexer in
/// `setIgnoreColonInIdentifiers` mode for the duration. Whitespace around the
/// colon is therefore insignificant. llvmkit matched locations by looking for
/// a *label* token instead, which requires the colon to be glued to the word,
/// so the spaced spelling did not parse.
///
/// `test/Assembler` writes only the unspaced form, so the spacing case is
/// anchored on `parseMemoryAttr` itself (D11).
#[test]
fn memory_attribute_tolerates_space_before_the_colon() {
    let module = module_new!("memory_attribute_spacing").expect("fresh module");
    Parser::new(
        b"declare void @f() memory(argmem : read, inaccessiblemem :write)
",
        &module,
    )
    .expect("parse constructor")
    .parse_module()
    .expect("upstream ignores whitespace around the location colon");
    let printed = format!("{module}");
    assert!(
        printed.contains("memory(argmem: read, inaccessiblemem: write)"),
        "{printed}"
    );
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

/// A comdat may be used before `$name = comdat ...` defines it — upstream
/// `LLParser::getComdat` creates the `Comdat` on first reference and records
/// that its selection kind is still owed.
///
/// No upstream `.ll` fixture isolates the positive case; the rule is
/// `getComdat`'s. The two negative halves below carry upstream's exact text.
#[test]
fn comdat_may_be_used_before_it_is_defined() {
    let text = parse_fixture(
        "comdat_forward",
        b"@g = global i32 0, comdat($c)\n$c = comdat any\n",
    );
    assert!(text.contains("$c = comdat any"), "{text}");
    assert!(text.contains("comdat($c)"), "{text}");
}

/// Mirrors the `ForwardRefComdats` guard at the top of
/// `LLParser::validateEndOfModule`: a comdat referenced but never defined is
/// reported at its first use.
#[test]
fn undefined_comdat_is_rejected() {
    assert_eq!(
        parse_err(b"@g = global i32 0, comdat($c)\n").to_string(),
        "use of undefined comdat '$c'"
    );
}

/// Mirrors the `!ForwardRefComdats.erase(Name)` guard in
/// `LLParser::parseComdat`: a second `$c = comdat ...` is a redefinition,
/// where a definition that merely satisfies an earlier *use* is not.
#[test]
fn redefined_comdat_is_rejected() {
    assert_eq!(
        parse_err(b"$c = comdat any\n$c = comdat largest\n").to_string(),
        "redefinition of comdat '$c'"
    );
}
