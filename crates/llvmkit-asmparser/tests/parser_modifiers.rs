//! Instruction modifier parsing tests — Session 3.1.
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError};
use llvmkit_ir::Module;
use llvmkit_ir::module_new;

pub mod support;

use support::line_and_column;

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

/// getelementptr nusw nuw on a vector base —
/// `test/Assembler/flags.ll::gep_nusw_nuw_vec`, the one upstream fixture
/// pairing a `<N x ptr>` base with the no-wrap flags. It locks that
/// `GepNoWrapFlags` printing is unaffected by the vector shape, and that the
/// scalar index survives unsplatted: `GetElementPtrInst::Create` does no
/// splatting, unlike `ConstantExpr::getGetElementPtr`.
#[test]
fn gep_nusw_nuw_vec_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/flags/gep_nusw_nuw_vec_round_trips.ll");

    let text = parse_fixture("gep_nusw_nuw_vec_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["%gep = getelementptr nusw nuw i8, <2 x ptr> %p, i64 %idx"],
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
/// All eight splits. The last three — `memory(foo)`, `memory(other: read)` and
/// `memory(argmem: foo)` — turn on a word that matches no keyword, and were
/// unreachable until llvmkit's lexer started returning `Token::Error` for one
/// instead of failing outright: the message came from the lexer naming the
/// lexeme, where upstream's comes from `parseMemoryAttr` naming what it wanted.
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
        (
            include_bytes!("fixtures/upstream/memory-attribute-errors/invalid_kind.ll").as_slice(),
            "expected memory location (argmem, inaccessiblemem, errnomem) or access kind (none, read, write, readwrite)",
        ),
        (
            include_bytes!("fixtures/upstream/memory-attribute-errors/other.ll").as_slice(),
            "expected memory location (argmem, inaccessiblemem, errnomem) or access kind (none, read, write, readwrite)",
        ),
        (
            include_bytes!("fixtures/upstream/memory-attribute-errors/invalid_access_kind.ll")
                .as_slice(),
            "expected access kind (none, read, write, readwrite)",
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

/// Legacy memory keywords **intersect**: `upgradeMemoryAttr` (`LLParser.cpp`)
/// is `ME &= MemoryEffects::X()` per keyword over an accumulator starting at
/// `unknown()`, emitted once after the whole list, and
/// `MemoryEffectsBase::operator&=` is a raw AND of the packed word.
///
/// No upstream `.ll` pins the intersection of two keywords — the closest are
/// `test/Analysis/AliasSet/argmemonly.ll` (`argmemonly writeonly` on a
/// declaration, no CHECK on the printed attribute) and
/// `test/Bitcode/upgrade-masked-keep-metadata.ll` (both intersections below
/// in `attributes #N` groups, likewise unchecked) — so these are anchored on
/// the symbols, with those two fixtures as the corroborating in-tree usage.
/// llvmkit used to store one `memory(...)` per keyword, so
/// `readonly writeonly` printed `memory(read) memory(write)`.
#[test]
fn legacy_memory_keywords_intersect() {
    for (spelled, expected) in [
        ("readonly writeonly", "memory(none)"),
        ("readnone readonly", "memory(none)"),
        ("argmemonly writeonly", "memory(argmem: write)"),
        (
            "inaccessiblemem_or_argmemonly readonly",
            "memory(argmem: read, inaccessiblemem: read)",
        ),
    ] {
        let text = parse_fixture(
            "legacy_memory_keywords_intersect",
            format!("declare void @f() {spelled}\n").as_bytes(),
        );
        assert_check_lines(&text, &[&format!("declare void @f() {expected}")]);
        assert_eq!(
            text.matches("memory(").count(),
            1,
            "{spelled} must yield exactly one memory attribute:\n{text}"
        );

        // The accumulator is per attribute list, so an `attributes #N` group
        // intersects the same way.
        let text = parse_fixture(
            "legacy_memory_keywords_intersect_group",
            format!("define void @f() #0 {{ ret void }}\nattributes #0 = {{ {spelled} }}\n")
                .as_bytes(),
        );
        assert_check_lines(&text, &[&format!("attributes #0 = {{ {expected} }}")]);
    }
}

/// The accumulated effects are emitted *after* the list and
/// `addAttributeImpl` replaces by kind, so a legacy keyword discards an
/// explicit `memory(...)` written in the same list — in either source order.
/// Anchored on `LLParser::parseFnAttributeValuePairs`'s
/// `if (ME != MemoryEffects::unknown()) B.addMemoryAttr(ME);` epilogue and
/// `addAttributeImpl`'s `std::swap` branch (`lib/IR/Attributes.cpp`); no
/// upstream `.ll` combines the two forms.
#[test]
fn legacy_memory_keyword_overwrites_explicit_memory() {
    for spelled in ["memory(none) readonly", "readonly memory(none)"] {
        let text = parse_fixture(
            "legacy_memory_keyword_overwrites_explicit_memory",
            format!("declare void @f() {spelled}\n").as_bytes(),
        );
        assert_check_lines(&text, &["declare void @f() memory(read)"]);
        assert_eq!(
            text.matches("memory(").count(),
            1,
            "{spelled} must yield exactly one memory attribute:\n{text}"
        );
    }
}

/// An attribute list may not hold two attributes of one kind: the second wins.
///
/// **Anchored on the routine; no upstream `.ll` writes a doubled attribute.**
/// `addAttributeImpl` (`lib/IR/Attributes.cpp`) is `lower_bound` then
/// `if (It != Attrs.end() && It->hasAttribute(Kind)) std::swap(*It, Attr);`,
/// and every `AttrBuilder::addAttribute` overload goes through it, so an
/// `AttrBuilder` structurally cannot accumulate two of a kind — `align(4)`
/// followed by `align(8)` leaves `align(8)`. String attributes match on the
/// key alone, which is why `"k"="1" "k"="2"` collapses while `"k"="1"
/// "j"="2"` does not.
///
/// llvmkit de-duplicated by full structural equality instead, so every pair
/// below round-tripped with *both* members present. Ledger entry 24 covered
/// that and the print *order*; both halves are closed and the entry is gone —
/// `lower_bound(Attrs, Kind, AttributeComparator())` is ported, so the last
/// case below now prints `"j"` first, as upstream does.
#[test]
fn an_attribute_list_holds_one_attribute_per_kind() {
    for (spelled, expected) in [
        // Enum-with-value: the alignment move takes the one stored attribute.
        ("align 4 align 8", "align 8"),
        ("alignstack(4) alignstack(8)", "alignstack(8)"),
        // A kind that carries a payload rather than an integer.
        ("memory(read) memory(write)", "memory(write)"),
        // Plain enum attributes were already covered by structural equality,
        // and must stay covered.
        ("nounwind nounwind", "nounwind"),
        // String attributes key on the key, not the pair.
        ("\"k\"=\"1\" \"k\"=\"2\"", "\"k\"=\"2\""),
    ] {
        let text = parse_fixture(
            "an_attribute_list_holds_one_attribute_per_kind",
            format!("declare void @f() {spelled}\n").as_bytes(),
        );
        assert_check_lines(&text, &[&format!("declare void @f() {expected}\n")]);
    }

    // The same rule on a parameter index, with a kind that only lives there.
    let text = parse_fixture(
        "an_attribute_list_holds_one_attribute_per_kind_param",
        b"declare void @f(float nofpclass(nan) nofpclass(inf))\n",
    );
    // The trailing `%0` is divergence D14 — a `declare` prints its parameter
    // names here — not part of what this test is about.
    assert_check_lines(&text, &["declare void @f(float nofpclass(inf) %0)\n"]);

    // The negative: two string attributes with *different* keys both survive,
    // because `hasAttribute(Kind)` compares the key — and they come back in
    // `AttributeImpl::cmp`'s order (its string arm is
    // `getKindAsString().compare(AI.getKindAsString())`), not the order they
    // were written in.
    let text = parse_fixture(
        "an_attribute_list_holds_one_attribute_per_kind_distinct_keys",
        b"declare void @f() \"k\"=\"1\" \"j\"=\"2\"\n",
    );
    assert_check_lines(&text, &["declare void @f() \"j\"=\"2\" \"k\"=\"1\"\n"]);
}

/// The same `expected access kind (none, read, write, readwrite)` arm, reached
/// from the *other* side: `readonly` is a real token that `keywordToModRef`
/// does not accept, where the upstream fixture's `foo` is a word that is no
/// token at all.
///
/// This existed because the upstream trigger was unreachable; it is ported now
/// (`memory_attribute_errors_match_upstream_text`, the `invalid-access-kind`
/// split), and this stays as the keyword-trigger half. Anchored on
/// `LLParser::parseMemoryAttr` (D11: no upstream counterpart uses a keyword
/// trigger).
#[test]
fn memory_access_kind_diagnostic_fires_on_keyword_input() {
    assert_eq!(
        parse_err(b"declare void @f() memory(argmem: readonly)\n").to_string(),
        "expected access kind (none, read, write, readwrite)"
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

/// Ports `test/Assembler/invalid-comdat.ll` verbatim, asserting its CHECK
/// line. The rule is the `ForwardRefComdats` guard at the top of
/// `LLParser::validateEndOfModule`: a comdat referenced but never defined is
/// reported at its first use.
#[test]
fn undefined_comdat_is_rejected() {
    assert_eq!(
        parse_err(b"@v = global i32 0, comdat($v)\n").to_string(),
        "use of undefined comdat '$v'"
    );
}

/// Ports `test/Assembler/invalid-comdat2.ll` verbatim, asserting its CHECK
/// line. The rule is the `!ForwardRefComdats.erase(Name)` guard in
/// `LLParser::parseComdat`: a second `$v = comdat ...` is a redefinition,
/// where a definition that merely satisfies an earlier *use* is not.
///
/// Note the fixture repeats the *same* selection kind, so this pins that the
/// rejection is about redefining at all, not about disagreeing.
#[test]
fn redefined_comdat_is_rejected() {
    assert_eq!(
        parse_err(b"$v = comdat any\n$v = comdat any\n").to_string(),
        "redefinition of comdat '$v'"
    );
}

/// `LLParser::parseComdat`'s three `tokError` sites, each at the token that
/// failed.
///
/// **Anchored on the routine, not on a fixture.**
/// `rg --no-ignore --hidden -e "comdat type" -e "comdat keyword" -e "unknown
/// selection kind" llvm/test/` returns exactly one line, and it belongs to a
/// different assembler: `test/MC/COFF/section-invalid-flags.s`'s
/// `expected comdat type such as 'discard' or 'largest' after protection
/// bits`. The two `LLParser` comdat negatives that do exist —
/// `test/Assembler/invalid-comdat.ll` and `invalid-comdat2.ll` — pin
/// `use of undefined comdat` and `redefinition of comdat` instead (ported
/// above).
///
/// The middle case is the routine's oddity. Upstream writes
/// `if (parseToken(lltok::kw_comdat, "expected comdat keyword"))
/// return tokError("expected comdat type");`, raising **two** messages on the
/// one failure at the one token: `parseToken` leaves the token unconsumed, and
/// both calls reach `LLLexer::Error` at `ErrorPriority::Parser`, which
/// early-returns only on `Priority < ErrorInfo.Priority`. `Parser < Parser` is
/// false, so the second overwrites the first — `expected comdat keyword` is
/// dead text that cannot reach a user from this site. llvmkit printed
/// `expected 'comdat'` until that divergence was closed.
///
/// The caret is asserted, not just the text: a message-only assertion is what
/// let a wrong anchor ship elsewhere in this crate.
#[test]
fn comdat_definition_diagnostics_match_upstream_text_and_anchor() {
    for (src, expected, expected_loc) in [
        // `parseToken(lltok::equal, "expected '=' here")`.
        ("$v notcomdat any\n", "expected '=' here", (1_u32, 4_u32)),
        // `parseToken(lltok::kw_comdat, …)` then `tokError("expected comdat
        // type")`, both at the unconsumed `notcomdat`.
        (
            "$v = notcomdat any\n",
            "expected comdat type",
            (1_u32, 6_u32),
        ),
        // The selection-kind switch's `default: return tokError("unknown
        // selection kind");`.
        (
            "$v = comdat notakind\n",
            "unknown selection kind",
            (1_u32, 13_u32),
        ),
    ] {
        let err = parse_err(src.as_bytes());
        assert_eq!(err.to_string(), expected, "message text for `{src}`");
        let start = err
            .loc()
            .unwrap_or_else(|| panic!("`{expected}` should carry a location"))
            .span
            .start;
        let offset = usize::try_from(start).unwrap_or(usize::MAX);
        assert_eq!(
            line_and_column(src.as_bytes(), offset),
            expected_loc,
            "caret position for `{expected}`"
        );
    }
}

/// Ports both `test/Assembler/alloca-addrspace-parse-error-{0,1}.ll`, which
/// pin that a trailing comma after an `alloca` clause demands metadata: the
/// index-list loop breaks on `MetadataVar`, so a comma with anything else
/// after it — or nothing — is `expected metadata after comma`.
///
/// The second is the interesting one: `addrspace(1), align 4` is the *wrong
/// clause order*, and upstream reports it through the same message rather
/// than a dedicated one.
#[test]
fn alloca_addrspace_parse_errors_match_upstream_text() {
    for fixture in [
        b"target datalayout = \"A1\"\ndefine void @use_alloca() {\n  %alloca = alloca i32, addrspace(1),\n  ret void\n}\n!0 = !{}\n"
            .as_slice(),
        b"target datalayout = \"A1\"\ndefine void @use_alloca() {\n  %alloca = alloca i32, addrspace(1), align 4\n  ret void\n}\n!0 = !{}\n"
            .as_slice(),
    ] {
        assert_eq!(
            parse_err(fixture).to_string(),
            "expected metadata after comma"
        );
    }
}
