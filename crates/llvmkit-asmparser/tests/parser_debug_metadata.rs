//! Specialized debug metadata parser tests.

use llvmkit_asmparser::ParseError;
use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;
use llvmkit_ir::metadata::SpecializedMetadataKind;

fn parse_and_render(src: &str) -> String {
    let module = Module::dynamic("parser_debug_metadata");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    format!("{module}")
}

/// `llvm-as | llvm-dis | llvm-as | llvm-dis` — **two** round trips, which is
/// what `test/Assembler/debug-info.ll` and `test/Assembler/diexpression.ll`
/// spell on their first `RUN` line. Their `CHECK` lines are therefore matched
/// against the *second* `llvm-dis`, so a construct that survives one trip and
/// not the next would still fail upstream. This is the parser/printer half of
/// that pipeline; the fixtures' second `RUN` line, `verify-uselistorder %s`,
/// has no llvmkit counterpart and is unported.
fn parse_render_reparse(src: &str) -> String {
    parse_and_render(&parse_and_render(src))
}

const DEBUG_MODULE: &str = r#"
@g = global i32 0, !dbg !15

define i32 @f() !dbg !3 {
entry:
  ret i32 0, !dbg !4
}

!0 = !DIFile(filename: "a.c", directory: "/tmp")
!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, producer: "llvmkit")
!2 = !DISubroutineType(types: !{!7, !7})
!3 = distinct !DISubprogram(name: "f", file: !0, type: !2, unit: !1)
!4 = !DILocation(line: 1, column: 2, scope: !3)
!5 = !DILocalVariable(name: "x", file: !0, type: !7, scope: !3)
!6 = !DIExpression()
!7 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
!8 = !DIDerivedType(tag: DW_TAG_pointer_type, name: "ptr", baseType: !7, size: 64)
!9 = !DISubrange(count: 4)
!10 = !DICompositeType(tag: DW_TAG_array_type, name: "arr", baseType: !7, elements: !{!9})
!11 = !DINamespace(name: "ns", scope: !3)
!12 = !DIEnumerator(name: "A", value: 1)
!13 = !DIModule(name: "m", scope: !11)
!14 = !DIGlobalVariable(name: "g", file: !0, type: !7, scope: !13)
!15 = !DIGlobalVariableExpression(var: !14, expr: !6)
!16 = !DITemplateTypeParameter(name: "T", type: !7)
!17 = !DITemplateValueParameter(name: "N", type: !7, value: 7)
"#;

/// Mirrors `LLParser.cpp::parseSpecializedMDNode` for the core DI node set.
#[test]
fn specialized_debug_nodes_round_trip() {
    let text = parse_and_render(DEBUG_MODULE);
    for needle in [
        "!DIFile(",
        "!DICompileUnit(",
        "distinct !DISubprogram(",
        "!DILocation(",
        "!DILocalVariable(",
        "!DIBasicType(",
        "!DIDerivedType(",
        "!DICompositeType(",
        "!DISubrange(",
        "!DINamespace(",
        "!DIExpression(",
        "!DIGlobalVariable(",
        "!DIGlobalVariableExpression(",
        "!DISubroutineType(",
        "!DIEnumerator(",
        "!DIModule(",
        "!DITemplateTypeParameter(",
        "!DITemplateValueParameter(",
    ] {
        assert!(text.contains(needle), "missing {needle} in:\n{text}");
    }
}

/// Mirrors `AsmWriter.cpp::SlotTracker::CreateMetadataSlot`: debug metadata
/// attachments are printed with canonical dense slots, not literal source ids.
///
/// The separators are `AssemblyWriter::printMetadataAttachments`'s: globals
/// and instructions take `", "`, a function header takes `" "`. The function
/// line used to be asserted with a comma, which is a spelling that appears
/// nowhere in `test/Assembler` — upstream writes `define i32 @f() !dbg !1 {`.
#[test]
fn function_and_global_debug_attachments_round_trip() {
    let text = parse_and_render(DEBUG_MODULE);
    assert!(
        text.contains("@g = global i32 0, !dbg !0"),
        "output:\n{text}"
    );
    assert!(text.contains("define i32 @f() !dbg !1"), "output:\n{text}");
    assert!(text.contains("ret i32 0, !dbg !2"), "output:\n{text}");
}

/// Mirrors `test/Assembler/dbg_declare_value.ll`.
#[test]
fn dbg_declare_value_record_round_trip() {
    let text = parse_and_render(
        r#"
define void @foo(double %x) !dbg !0 {
entry:
  #dbg_declare_value(double %x, !1, !DIExpression(), !2)
  ret void, !dbg !2
}

!0 = distinct !DISubprogram(name: "foo", type: !3, unit: !4)
!1 = !DILocalVariable(name: "x", scope: !0, type: !5)
!2 = !DILocation(line: 1, column: 17, scope: !0)
!3 = !DISubroutineType(types: !{null, !5})
!4 = distinct !DICompileUnit(language: DW_LANG_C11, file: !6, producer: "llvmkit")
!5 = !DIBasicType(name: "double", size: 64, encoding: DW_ATE_float)
!6 = !DIFile(filename: "test.c", directory: "/tmp")
"#,
    );
    assert!(
        text.contains("#dbg_declare_value(double %x, !1, !DIExpression(), !2)"),
        "output:\n{text}"
    );
}

/// `parseDebugRecord` parses its value field with
/// `parseMetadata(ValLocMD, &PFS)` — the whole routine — whose non-`!`
/// fall-through is
/// `parseValueAsMetadata(MD, "expected metadata operand", PFS)`, i.e.
/// `parseType(Ty, TypeMsg, Loc)`, then
/// `if (Ty->isMetadataTy()) return error(Loc, "invalid metadata-value-metadata
/// roundtrip");`, then `parseValue`.
///
/// llvmkit wrote that tail out instead of delegating, so it carried neither
/// the `TypeMsg` nor the roundtrip guard: a non-type reported `parse_type`'s
/// own `expected type`, and `metadata i32 %a` ran on into the value parse and
/// blamed `%a` for the inner type. The same shape as a `call` argument was
/// already correct, because that path calls the routine.
///
/// No `test/Assembler` fixture writes a bad value operand in a `#dbg_*`
/// record — the `dbg-record-invalid-*.ll` family covers a bad record *type*
/// and wrong operand *counts*. The routines are the anchor, and the two
/// columns are `parseType`'s current token and `parseType`'s out-parameter
/// `Loc` respectively.
#[test]
fn a_debug_record_value_operand_goes_through_parse_metadata() {
    const HEADER: &str = "define void @f() !dbg !3 {\nentry:\n  %a = add i32 0, 0\n";
    const TRAILER: &str = concat!(
        "  ret void\n}\n\n",
        "!0 = !DIFile(filename: \"a.c\", directory: \"/tmp\")\n",
        "!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, producer: \"llvmkit\")\n",
        "!2 = !DISubroutineType(types: !{null})\n",
        "!3 = distinct !DISubprogram(name: \"f\", file: !0, type: !2, unit: !1)\n",
        "!4 = !DILocation(line: 1, column: 1, scope: !3)\n",
        "!5 = !DILocalVariable(name: \"x\", scope: !3, type: !6)\n",
        "!6 = !DIBasicType(name: \"int\", size: 32, encoding: DW_ATE_signed)\n",
    );
    // `Ty->isMetadataTy()`, anchored at the *type* `parseType` just read.
    let roundtrip =
        format!("{HEADER}  #dbg_value(metadata %a, !5, !DIExpression(), !4)\n{TRAILER}");
    let err = parse_err(&roundtrip);
    assert_eq!(err.to_string(), "invalid metadata-value-metadata roundtrip");
    assert_eq!(
        reported_offset(&err),
        roundtrip
            .find("metadata %a")
            .expect("the fixture writes one")
    );
    // `parseType(Ty, TypeMsg, Loc)` with `TypeMsg = "expected metadata
    // operand"`, anchored at the token that is not a type.
    let not_a_type = format!("{HEADER}  #dbg_value(42, !5, !DIExpression(), !4)\n{TRAILER}");
    let err = parse_err(&not_a_type);
    assert_eq!(err.to_string(), "expected metadata operand");
    assert_eq!(
        reported_offset(&err),
        not_a_type.find("42").expect("the fixture writes one")
    );
}

/// Byte offset a rejection reports, for comparison against a needle's own
/// offset — the same "text alone cannot see the anchor" guard
/// `parser_module_level.rs` spells with line and column.
fn reported_offset(err: &ParseError) -> usize {
    usize::try_from(
        err.loc()
            .expect("a rejection reports a location")
            .span
            .start,
    )
    .expect("span start fits in usize")
}

/// Regression (broad-review Critical): an align-less alloca with attached
/// `!dbg` metadata parses. The metadata comma must not be mis-consumed as an
/// array size (`LLParser::parseAlloc` branches on `MetadataVar` before the
/// size parse). Ubiquitous in debug builds.
#[test]
fn alloca_with_trailing_dbg_metadata_parses() {
    let src = r#"
define void @f() !dbg !3 {
entry:
  %p = alloca i32, !dbg !4
  ret void
}

!0 = !DIFile(filename: "a.c", directory: "/tmp")
!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, producer: "llvmkit")
!2 = !DISubroutineType(types: !{null})
!3 = distinct !DISubprogram(name: "f", file: !0, type: !2, unit: !1)
!4 = !DILocation(line: 1, column: 1, scope: !3)
"#;
    let printed = parse_and_render(src);
    assert!(
        printed.contains("%p = alloca i32, align 4, !dbg !"),
        "{printed}"
    );
}

// ---------------------------------------------------------------------------
// Specialized `DI*` field validation
//
// Ports `llvm/test/Assembler/invalid-di*.ll`. Each fixture is a `not llvm-as`
// FileCheck case whose CHECK line pins the diagnostic text, so these assert on
// the same three messages `LLParser`'s `PARSE_MD_FIELDS` macro emits.
// ---------------------------------------------------------------------------

/// Parse `src` expecting failure, returning the error.
fn parse_err(src: &str) -> ParseError {
    let module = Module::dynamic("parser_debug_metadata_invalid");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("parse must fail")
}

/// Ports `test/Assembler/invalid-dilocation-field-bad.ll` from the vendored
/// copy, whose CHECK line is `error: invalid field 'bad'`. The fixture's
/// `[[@LINE+1]]:18` column pin is asserted by that file's `loc=`/`error=` row
/// in `parser_corpus_manifest.txt`; this test pins the typed variant.
#[test]
fn dilocation_rejects_a_field_its_class_does_not_declare() {
    const FIXTURE: &str =
        include_str!("fixtures/upstream/assembler-corpus/invalid-dilocation-field-bad.ll");

    let err = parse_err(FIXTURE);
    assert!(
        matches!(
            &err,
            ParseError::InvalidMetadataField { kind, field, .. }
                if *kind == "DILocation" && field == "bad"
        ),
        "expected invalid-field error, got: {err:?}"
    );
    assert_eq!(err.to_string(), "invalid field 'bad'");
}

/// llvmkit-specific (no upstream fixture emits it): `parseMDFieldsImpl`'s
/// first statement is `if (Lex.getKind() != lltok::LabelStr) return
/// tokError("expected field label here")`, and a fully-numeric label is
/// `lltok::LabelID`, not `LabelStr` — so `!DIFile(42: 1)` fails on the *label*
/// rather than being reported as an unknown field. llvmkit answered
/// `invalid field '42'` until W14b, because it had no numeric-label token and
/// `42:` arrived as a `LabelStr` named `42`.
#[test]
fn a_numeric_label_is_not_a_metadata_field_name() {
    let err = parse_err("!0 = !DIFile(42: 1)\n");
    assert_eq!(err.to_string(), "expected field label here");
}

/// Ports `test/Assembler/invalid-dilocation-field-twice.ll`, whose CHECK line
/// is `error: field 'line' cannot be specified more than once`.
#[test]
fn dilocation_rejects_a_field_specified_twice() {
    let err = parse_err("!0 = !{}\n!1 = !DILocation(line: 3, scope: !0, line: 3)\n");
    assert!(
        matches!(
            &err,
            ParseError::DuplicateMetadataField { kind, field, .. }
                if *kind == "DILocation" && field == "line"
        ),
        "expected duplicate-field error, got: {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "field 'line' cannot be specified more than once"
    );
}

/// Ports the `missing required field` family from `test/Assembler/`:
/// `invalid-dilocation-missing-scope.ll`, `-missing-scope-2.ll`,
/// `invalid-difile-missing-filename.ll`, `-missing-directory.ll`,
/// `invalid-dienumerator-missing-name.ll`, `-missing-value.ll`,
/// `invalid-disubroutinetype-missing-types.ll`,
/// `invalid-ditemplatetypeparameter-missing-type.ll`,
/// `invalid-ditemplatevalueparameter-missing-value.ll`,
/// `invalid-dicompositetype-missing-tag.ll`,
/// `invalid-diderivedtype-missing-tag.ll`, `-missing-basetype.ll`,
/// `invalid-dilocalvariable-missing-scope.ll` and
/// `invalid-dinamespace-missing-namespace.ll` — each fixture's source line with
/// the field its CHECK line names.
#[test]
fn required_specialized_metadata_fields_are_enforced() {
    for (src, kind, field) in [
        ("!0 = !DILocation()\n", "DILocation", "scope"),
        ("!0 = !DILocation(line: 7)\n", "DILocation", "scope"),
        ("!0 = !DIFile(directory: \"dir\")\n", "DIFile", "filename"),
        ("!0 = !DIFile(filename: \"file\")\n", "DIFile", "directory"),
        ("!0 = !DIEnumerator(value: 7)\n", "DIEnumerator", "name"),
        (
            "!0 = !DIEnumerator(name: \"name\")\n",
            "DIEnumerator",
            "value",
        ),
        (
            "!29 = !DISubroutineType(flags: DIFlagPublic | DIFlagStaticMember)\n",
            "DISubroutineType",
            "types",
        ),
        (
            "!0 = !DITemplateTypeParameter(name: \"param\")\n",
            "DITemplateTypeParameter",
            "type",
        ),
        (
            "!0 = !DITemplateValueParameter(tag: DW_TAG_template_value_parameter,\n                               type: !{})\n",
            "DITemplateValueParameter",
            "value",
        ),
        (
            "!25 = !DICompositeType(name: \"Type\")\n",
            "DICompositeType",
            "tag",
        ),
        (
            "!0 = !DIDerivedType(baseType: !{})\n",
            "DIDerivedType",
            "tag",
        ),
        (
            "!0 = !DIDerivedType(tag: DW_TAG_pointer_type)\n",
            "DIDerivedType",
            "baseType",
        ),
        ("!0 = !DILocalVariable()\n", "DILocalVariable", "scope"),
        (
            "!0 = !DINamespace(name: \"Namespace\")\n",
            "DINamespace",
            "scope",
        ),
    ] {
        let err = parse_err(src);
        assert!(
            matches!(
                &err,
                ParseError::MissingRequiredMetadataField { kind: k, field: f, .. }
                    if *k == kind && *f == field
            ),
            "expected missing required field '{field}' for !{kind}, got: {err:?}"
        );
        assert_eq!(err.to_string(), format!("missing required field '{field}'"));
    }
}

/// llvmkit-specific: no upstream counterpart, but it locks upstream's
/// structure. `DIExpression` is the one specialized node
/// `LLParser::parseSpecializedMDNode` routes away from `PARSE_MD_FIELDS`, to
/// `parseDIExpressionBody` — its body is a positional `DW_OP_*` list, so it
/// declares no fields at all.
///
/// A `name: value` pair inside one is therefore not an *invalid field* but a
/// malformed *element*: upstream sees `lltok::LabelStr`, which is neither
/// `DwarfOp` nor `DwarfAttEncoding` nor `APSInt`, and reports "expected
/// unsigned integer". This pins both halves — the empty tables, and that a
/// field-shaped body is rejected on the element path rather than the field one.
#[test]
fn diexpression_declares_no_named_fields() {
    assert!(
        SpecializedMetadataKind::DiExpression
            .declared_fields()
            .is_empty()
    );
    assert_eq!(
        SpecializedMetadataKind::DiExpression
            .required_fields()
            .count(),
        0
    );
    let err = parse_err("!0 = !DIExpression(line: 1)\n");
    assert_eq!(err.to_string(), "expected unsigned integer");
}

/// llvmkit-specific: no upstream counterpart. This is the weak half of a drift
/// check, and it is worth saying why the strong half is missing.
///
/// `attribute_td_drift.rs` can re-read `Attributes.td` because that `.td` is
/// **vendored and tracked** under `crates/llvmkit-asmparser/tablegen/`. The
/// specialized-`DI*` field lists have no such input: they are per-class
/// `VISIT_MD_FIELDS` macro blocks inside `LLParser.cpp`, which lives only in
/// `orig_cpp/` — a **gitignored** reference tree, so a test that read it would
/// pass locally and fail in CI. Vendoring the file to fix that would mean
/// tracking an 8k-line C++ source to scrape preprocessor text out of, which is
/// a far larger commitment than a `.td`.
///
/// So the tables are hand-ported, and this pins the two halves against each
/// other instead: every name in `required_fields` must also appear in `fields`,
/// for every modelled kind. It catches a typo'd or dropped required field; it
/// cannot catch upstream adding a field.
#[test]
fn required_fields_are_a_subset_of_accepted_fields() {
    for kind in SpecializedMetadataKind::ALL {
        for required in kind.required_fields() {
            assert!(
                kind.accepts_field(required.name()),
                "!{} lists '{}' as required but not as an accepted field",
                kind.name(),
                required.name()
            );
        }
    }
}

/// Ports `test/Assembler/debug-info.ll`'s three `!DISubroutineType` lines
/// (`!28`, `!29`, `!30`) against the two `CHECK-NEXT` lines that answer them:
///
/// ```text
/// ; CHECK-NEXT: !26 = !DISubroutineType(flags: DIFlagPublic | DIFlagStaticMember, types: !25)
/// ; CHECK-NEXT: !27 = !DISubroutineType(types: !25)
/// ```
///
/// Two `Check`s, not one. The disjunction re-emerges through
/// `MDFieldPrinter::printDIFlags` — `DINode::splitFlags` plus
/// `getFlagString`, joined by `ListSeparator(" | ")`. And `flags: 0` prints
/// **nothing at all**, because `printDIFlags` opens `if (!Flags) return;`
/// before it writes the field name — which is why upstream's three input nodes
/// come back as two, `!29` and `!30` being identical once printed.
#[test]
fn debug_info_flag_disjunction_round_trips() {
    let text = parse_and_render(
        "!0 = !{}\n\
         !1 = !DISubroutineType(flags: DIFlagPublic | DIFlagStaticMember, types: !0)\n\
         !2 = !DISubroutineType(flags: 0, types: !0)\n\
         !3 = !DISubroutineType(types: !0)\n",
    );
    assert!(
        text.contains("!DISubroutineType(flags: DIFlagPublic | DIFlagStaticMember, types: !0)"),
        "output:\n{text}"
    );
    assert!(
        !text.contains("flags: 0"),
        "a zero `flags:` field prints nothing at all:\n{text}"
    );
    assert_eq!(
        text.matches("!DISubroutineType(types: !0)").count(),
        2,
        "`flags: 0` and an omitted `flags:` print the same text:\n{text}"
    );
}

/// `LLParser::parseMDField(DIFlagField&)`'s `parseFlag` accepts an unsigned
/// `lltok::APSInt` term anywhere in the `|` chain — it is the first arm, ahead
/// of the `lltok::DIFlag` one — and ORs it into the same bitfield. So all four
/// spellings below are one constant, and
/// `MDFieldPrinter::printDIFlags` prints the one canonical form for it.
///
/// `DIFlagPublic` is `0x3` and `DIFlagStaticMember` is `0x1000`, from
/// `DebugInfoFlags.def`; the mixed forms are those two numbers written out.
///
/// **No upstream `.ll` fixture pins a mixed numeric/keyword term.** Searched at
/// `llvmorg-22.1.4` with `grep -rn -a --include=*.ll -E 'flags: [0-9]+ \||flags:
/// [A-Za-z]+ \| [0-9]' orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/` — the
/// only numeric `flags:` hits are `!DISubroutineType(flags: 0, …)` in
/// `test/Assembler/debug-info.ll` (covered above) and the `^N = flags: <n>`
/// lines of the module summary index, which are a different grammar. The
/// source is therefore `parseFlag` itself.
#[test]
fn debug_info_flags_accept_numeric_terms_in_any_position() {
    for spelling in [
        "DIFlagPublic | DIFlagStaticMember",
        "4099",
        "3 | DIFlagStaticMember",
        "DIFlagPublic | 4096",
    ] {
        let text = parse_and_render(&format!(
            "!0 = !{{}}\n!1 = !DISubroutineType(flags: {spelling}, types: !0)\n"
        ));
        assert!(
            text.contains("!DISubroutineType(flags: DIFlagPublic | DIFlagStaticMember, types: !0)"),
            "`flags: {spelling}` printed:\n{text}"
        );
    }
}

/// `DINode::splitFlags` (`lib/IR/DebugInfoMetadata.cpp`) is why the printed
/// form is canonical rather than an echo. Its first block carries upstream's
/// own comment — "so that, for example, we emit `DIFlagPublic` and not
/// `DIFlagPrivate | DIFlagProtected`" — and the whole routine then walks
/// `HANDLE_DI_FLAG` in `.def` order, so a written order is not preserved and a
/// bit written twice appears once. The trailing `Extra` is what
/// `printDIFlags` emits for the bits no row names.
///
/// **No upstream `.ll` fixture pins any of these**, because they are all
/// non-canonical *inputs* and every checked-in fixture is already canonical:
/// the `grep` in [`debug_info_flags_accept_numeric_terms_in_any_position`]
/// finds no numeric term to combine, and
/// `grep -rn -a --include=*.ll 'DIFlagProtected | DIFlagPrivate'
/// orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/` has no matches. The source
/// is `splitFlags` and `printDIFlags` themselves.
#[test]
fn debug_info_flags_print_canonically_not_as_written() {
    // The accessibility triple collapses to the composite spelling.
    let cases = [
        ("DIFlagProtected | DIFlagPrivate", "DIFlagPublic"),
        // Written order does not survive: `.def` order is bit order.
        (
            "DIFlagStaticMember | DIFlagPublic",
            "DIFlagPublic | DIFlagStaticMember",
        ),
        // A duplicate term is one bit.
        ("DIFlagVector | DIFlagVector", "DIFlagVector"),
        // `FlagPtrToMemberRep`'s triple collapses the same way the
        // accessibility one does.
        (
            "DIFlagSingleInheritance | DIFlagMultipleInheritance",
            "DIFlagVirtualInheritance",
        ),
        // An unnamed bit is the trailing number `printDIFlags` appends.
        ("DIFlagVector | 2097152", "DIFlagVector | 2097152"),
        // A wholly unnamed bitfield prints as the bare number, which is the
        // `SplitFlags.empty()` half of the `if (Extra || SplitFlags.empty())`.
        ("2097152", "2097152"),
    ];
    for (written, printed) in cases {
        let text = parse_and_render(&format!(
            "!0 = !{{}}\n!1 = !DISubroutineType(flags: {written}, types: !0)\n"
        ));
        assert!(
            text.contains(&format!("!DISubroutineType(flags: {printed}, types: !0)")),
            "`flags: {written}` should print `flags: {printed}`:\n{text}"
        );
    }
}

/// `MDFieldPrinter::printDISPFlags` differs from its `printDIFlags` twin in one
/// statement, and carries its own comment for it: "Always print this field,
/// because no flags in the IR at all will be interpreted as old-style
/// isDefinition: true." So a zero `spFlags:` prints `spFlags: 0` where a zero
/// `flags:` prints nothing.
///
/// Ports `test/Assembler/disubprogram.ll`'s
/// `; CHECK: !9 = distinct !DISubprogram(scope: null, spFlags: 0)`.
#[test]
fn disubprogram_zero_sp_flags_prints_zero() {
    let text = parse_and_render("!0 = distinct !DISubprogram(scope: null, spFlags: 0)\n");
    assert!(
        text.contains("distinct !DISubprogram(scope: null, spFlags: 0)"),
        "output:\n{text}"
    );
}

/// `DINode::getFlag` and `DISubprogram::getFlag` are `StringSwitch`es built by
/// including `DebugInfoFlags.def` **without** `DI_FLAG_LARGEST_NEEDED` /
/// `DISP_FLAG_LARGEST_NEEDED` — only `DebugInfoMetadata.h` defines those, to
/// bound the bitmask enums — so neither `DIFlagLargest` nor `DISPFlagLargest`
/// is a spelling either routine matches. Both fall to `.Default(FlagZero)`,
/// and `parseFlag`'s `if (!Val)` then rejects them, exactly as it rejects
/// `DIFlagZero` (which the switch *does* carry, at value zero) and an
/// invented spelling.
///
/// `LLLexer::LexIdentifier` returns `lltok::DIFlag` for any word starting
/// `DIFlag`, so all four reach `parseFlag` as flag tokens rather than as
/// unknown keywords.
///
/// **No upstream `.ll` fixture pins these.** `test/Assembler/invalid-diflag-bad.ll`
/// is the family's only negative and pins an invented spelling; searched at
/// `llvmorg-22.1.4` with `grep -rn -a --include=*.ll 'DIFlagLargest\|DIFlagZero\|DISPFlagLargest'
/// orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/` — no matches. The source is
/// `getFlag`'s two include sites.
#[test]
fn debug_info_flag_names_the_string_switch_lacks_are_rejected() {
    for (field, spelling, message) in [
        ("flags", "DIFlagLargest", "invalid debug info flag"),
        ("flags", "DIFlagZero", "invalid debug info flag"),
        (
            "spFlags",
            "DISPFlagLargest",
            "invalid subprogram debug info flag",
        ),
        (
            "spFlags",
            "DISPFlagZero",
            "invalid subprogram debug info flag",
        ),
    ] {
        let source = format!("!0 = distinct !DISubprogram(scope: null, {field}: {spelling})\n");
        let error = parse_err(&source).to_string();
        assert_eq!(
            error,
            format!("{message} '{spelling}'"),
            "`{field}: {spelling}`"
        );
    }
}

/// `parseFlag`'s first arm is `Lex.getKind() == lltok::APSInt &&
/// !Lex.getAPSIntVal().isSigned()`. A **signed** literal fails that guard and
/// falls into the second, which demands a `lltok::DIFlag` and answers
/// `expected debug info flag` — the same message the `DISPFlagField` overload
/// gives, which does *not* say "subprogram".
///
/// No upstream `.ll` fixture writes a negative `flags:`; searched at
/// `llvmorg-22.1.4` with `grep -rn -a --include=*.ll 'flags: -'
/// orig_cpp/llvm-project-llvmorg-22.1.4/llvm/test/` — no matches. The source is
/// the guard.
#[test]
fn debug_info_flags_reject_a_signed_numeric_term() {
    for field in ["flags", "spFlags"] {
        let error = parse_err(&format!(
            "!0 = distinct !DISubprogram(scope: null, {field}: -1)\n"
        ))
        .to_string();
        assert!(
            error.contains("expected debug info flag"),
            "`{field}: -1` gave: {error}"
        );
    }
}

/// Ports `test/Assembler/diexpression.ll`, whose `CHECK-SAME` lines are
/// byte-identical to its input and are matched against the second `llvm-dis`
/// of `llvm-as | llvm-dis | llvm-as | llvm-dis`; `parse_render_reparse` runs
/// both trips. Covers the empty
/// body, bare ops, op+literal sequences, and the `DW_OP_LLVM_convert` form that
/// mixes in `DW_ATE_*` attribute encodings — the second keyword family
/// `LLParser::parseDIExpressionBody` accepts.
#[test]
fn diexpression_forms_round_trip() {
    const FIXTURE: &str = include_str!("fixtures/upstream/assembler-corpus/diexpression.ll");

    // The fixture's own nine `; CHECK-SAME:` needles.
    const FORMS: [&str; 9] = [
        "!DIExpression()",
        "!DIExpression(DW_OP_deref)",
        "!DIExpression(DW_OP_constu, 3, DW_OP_plus)",
        "!DIExpression(DW_OP_LLVM_fragment, 3, 7)",
        "!DIExpression(DW_OP_deref, DW_OP_plus_uconst, 3, DW_OP_LLVM_fragment, 3, 7)",
        "!DIExpression(DW_OP_constu, 2, DW_OP_swap, DW_OP_xderef)",
        "!DIExpression(DW_OP_plus_uconst, 3)",
        "!DIExpression(DW_OP_LLVM_convert, 16, DW_ATE_unsigned, DW_OP_LLVM_convert, 32, DW_ATE_signed)",
        "!DIExpression(DW_OP_LLVM_tag_offset, 1)",
    ];
    let text = parse_render_reparse(FIXTURE);
    for form in FORMS {
        assert!(text.contains(form), "missing {form} in:\n{text}");
    }
}

/// Ports `test/Assembler/invalid-diexpression-large.ll`: an element of exactly
/// `UINT64_MAX` is accepted (`CHECK-NOT: error:`) and one above it is not,
/// with `parseDIExpressionBody`'s own
/// `CHECK: … error: element too large, limit is 18446744073709551615`.
#[test]
fn diexpression_element_at_the_u64_limit_is_accepted_and_beyond_is_rejected() {
    let text = parse_and_render("!named = !{!0}\n!0 = !DIExpression(18446744073709551615)\n");
    assert!(
        text.contains("!DIExpression(18446744073709551615)"),
        "output:\n{text}"
    );
    assert_eq!(
        parse_err("!0 = !DIExpression(18446744073709551616)\n").to_string(),
        "element too large, limit is 18446744073709551615"
    );
}

/// The 14 specialized classes added on 2026-08-07, closing the modelled set to
/// all 32 `HANDLE_SPECIALIZED_MDNODE_LEAF` entries in `llvm/IR/Metadata.def`.
///
/// Each line is a minimal well-formed node carrying exactly that class's
/// `REQUIRED` fields per its `LLParser::parse*` `VISIT_MD_FIELDS` block, so the
/// case doubles as a check that the required-field tables admit a valid node
/// rather than only rejecting invalid ones. `DILexicalBlock` matters most: it
/// appears in essentially every `-g` build, and before this it did not parse.
#[test]
fn the_remaining_specialized_classes_parse_and_round_trip() {
    let src = r#"
!named = !{!1, !2, !3, !4, !5, !6, !7, !8, !9, !10, !11, !12, !13, !14}
!0 = !{}
!1 = !DILexicalBlock(scope: !0)
!2 = !DILexicalBlockFile(scope: !0, discriminator: 7)
!3 = !DICommonBlock(scope: !0)
!4 = !DIImportedEntity(tag: DW_TAG_imported_module, scope: !0)
!5 = !DILabel(scope: !0, name: "lbl", file: !0, line: 3)
!6 = !DIMacro(type: DW_MACINFO_define, name: "M")
!7 = !DIMacroFile(file: !0)
!8 = !GenericDINode(tag: DW_TAG_variable)
!9 = !DISubrangeType(name: "st")
!10 = !DIGenericSubrange(count: !0)
!11 = !DIFixedPointType(name: "fx")
!12 = !DIStringType(name: "str")
!13 = !DIObjCProperty(name: "prop")
!14 = distinct !DIAssignID()
"#;
    let text = parse_and_render(src);
    for needle in [
        "!DILexicalBlock(",
        "!DILexicalBlockFile(",
        "!DICommonBlock(",
        "!DIImportedEntity(",
        "!DILabel(",
        "!DIMacro(",
        "!DIMacroFile(",
        "!GenericDINode(",
        "!DISubrangeType(",
        "!DIGenericSubrange(",
        "!DIFixedPointType(",
        "!DIStringType(",
        "!DIObjCProperty(",
        "distinct !DIAssignID()",
    ] {
        assert!(text.contains(needle), "missing {needle} in:\n{text}");
    }
}

/// Mirrors `LLParser::parseDIAssignID`, which rejects a uniqued node with
/// "missing 'distinct', required for !DIAssignID()" before reading the parens —
/// the class exists to give an assignment a unique identity.
#[test]
fn diassignid_requires_distinct() {
    // This used to be `let _ = parse_err(...)` — the error was discarded, so
    // the message the doc comment names was never actually checked.
    assert_eq!(
        parse_err("!0 = !DIAssignID()\n").to_string(),
        "missing 'distinct', required for !DIAssignID()"
    );
    // The `distinct` form is the only accepted one.
    let text = parse_and_render("!named = !{!0}\n!0 = distinct !DIAssignID()\n");
    assert!(text.contains("distinct !DIAssignID()"), "output:\n{text}");
}

/// Ports the metadata negatives from `test/Assembler`, each verbatim with its
/// CHECK line: `invalid-mdnode-vector.ll`, `invalid-mdnode-vector2.ll`,
/// `invalid-mdnode-badref.ll`, `invalid-metadata-has-type.ll` and
/// `invalid-metadata-attachment-has-type.ll`.
///
/// The last two are the "common error from old format" pair, as their own
/// comments say: `metadata !{}` and `!{metadata !0}` were once legal, and
/// upstream detects each with a message of its own rather than a generic
/// failure.
#[test]
fn metadata_negative_fixtures_match_upstream_text() {
    // invalid-mdnode-vector.ll
    assert_eq!(parse_err("!0 = !\n").to_string(), "expected '{' here");
    // invalid-mdnode-vector2.ll
    assert_eq!(
        parse_err("!0 = !{\n").to_string(),
        "expected metadata operand"
    );
    // invalid-mdnode-badref.ll
    assert_eq!(
        parse_err("!named = !{!0}\n!0 = !{!0, !1}\n").to_string(),
        "use of undefined metadata '!1'"
    );
    // invalid-metadata-has-type.ll
    assert_eq!(
        parse_err("!0 = metadata !{}\n").to_string(),
        "unexpected type in metadata definition"
    );
    // invalid-metadata-attachment-has-type.ll
    assert_eq!(
        parse_err("define void @foo() {\n  ret void, !bar !{metadata !0}\n}\n!0 = !{}\n")
            .to_string(),
        "invalid metadata-value-metadata roundtrip"
    );
}

/// `parseNamedMetadata`'s two special-cased operand spellings.
///
/// A `!DIExpression(...)` may be written **inline** as a named-metadata
/// operand — upstream's comment: "parse DIExpressions inline as a special
/// case. They are still MDNodes, so they can still appear in named metadata."
/// A `!DIArgList(...)` may not, because it can hold `LocalAsMetadata`
/// arguments that need a function context, and it gets its own message.
///
/// llvmkit's loop accepted only `!N` slot references, so the inline form did
/// not parse and the `DIArgList` rejection had no message of its own.
#[test]
fn named_metadata_operands_special_case_diexpression_and_diarglist() {
    let text = parse_and_render("!named = !{!DIExpression(DW_OP_deref)}\n");
    assert!(
        text.contains("!DIExpression(DW_OP_deref)"),
        "output:\n{text}"
    );

    assert_eq!(
        parse_err("!named = !{!DIArgList()}\n").to_string(),
        "found DIArgList outside of function"
    );
}

/// llvmkit-specific: no upstream counterpart. Pins that the modelled set is the
/// complete `HANDLE_SPECIALIZED_MDNODE_LEAF` list from `llvm/IR/Metadata.def`,
/// so a class cannot be dropped from `ALL` without this failing, and every
/// entry round-trips its own name through `from_name`.
#[test]
fn every_specialized_kind_round_trips_its_name() {
    assert_eq!(SpecializedMetadataKind::ALL.len(), 32);
    for kind in SpecializedMetadataKind::ALL {
        assert_eq!(
            SpecializedMetadataKind::from_name(kind.name()),
            Some(kind),
            "{} did not round-trip through from_name",
            kind.name()
        );
    }
}

// ---------------------------------------------------------------------------
// Per-field-type value validation
//
// Each `LLParser::parseMDField` overload carries its own rejection, and the
// wording below is the overload's own — these are the messages `not llvm-as`
// prints. Field grammars come from `SpecializedMetadataKind::declared_fields`.
// ---------------------------------------------------------------------------

/// Mirrors the keyword-family overloads of `LLParser::parseMDField`
/// (`LLParser.cpp`), each of which rejects a spelling its `Dwarf.def` /
/// `DebugInfoFlags.def` table does not contain.
#[test]
fn keyword_families_reject_a_spelling_upstream_does_not_know() {
    for (src, what, value) in [
        (
            "!0 = !{}\n!1 = !DIDerivedType(tag: DW_TAG_bogus, baseType: !0)\n",
            "DWARF tag",
            "DW_TAG_bogus",
        ),
        (
            "!0 = !DIBasicType(encoding: DW_ATE_bogus)\n",
            "DWARF type attribute encoding",
            "DW_ATE_bogus",
        ),
        (
            "!0 = !{}\n!1 = !DISubprogram(virtuality: DW_VIRTUALITY_bogus)\n",
            "DWARF virtuality code",
            "DW_VIRTUALITY_bogus",
        ),
        (
            "!0 = !{}\n!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_bogus)\n",
            "DWARF language",
            "DW_LANG_bogus",
        ),
        (
            "!0 = !{}\n!1 = !DISubroutineType(types: !0, cc: DW_CC_bogus)\n",
            "DWARF calling convention",
            "DW_CC_bogus",
        ),
        (
            "!0 = !DIMacro(type: DW_MACINFO_bogus, name: \"m\")\n",
            "DWARF macinfo type",
            "DW_MACINFO_bogus",
        ),
        (
            "!0 = !{}\n!1 = !DIDerivedType(tag: DW_TAG_member, baseType: !0, flags: DIFlagBogus)\n",
            "debug info flag",
            "DIFlagBogus",
        ),
        (
            "!0 = !DISubprogram(spFlags: DISPFlagBogus)\n",
            "subprogram debug info flag",
            "DISPFlagBogus",
        ),
        (
            "!0 = !DIFile(filename: \"a\", directory: \"b\", checksumkind: CSK_BOGUS)\n",
            "checksum kind",
            "CSK_BOGUS",
        ),
    ] {
        let err = parse_err(src);
        assert!(
            matches!(
                &err,
                ParseError::InvalidMetadataFieldValue { what: w, value: v, .. }
                    if *w == what && v == value
            ),
            "expected invalid {what} '{value}', got: {err:?}"
        );
        assert_eq!(err.to_string(), format!("invalid {what} '{value}'"));
    }
}

/// A valid keyword from each family still parses — the tables must not be so
/// strict that ordinary debug metadata stops working.
#[test]
fn keyword_families_accept_the_spellings_upstream_knows() {
    let text = parse_and_render(
        r#"
!named = !{!1, !2, !3, !4}
!0 = !{}
!1 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !0, flags: DIFlagPublic | DIFlagStaticMember)
!2 = !DIBasicType(encoding: DW_ATE_signed)
!3 = distinct !DICompileUnit(file: !0, language: DW_LANG_C99, emissionKind: FullDebug, nameTableKind: GNU)
!4 = !DIFile(filename: "a", directory: "b", checksumkind: CSK_MD5, checksum: "abc")
"#,
    );
    for needle in [
        "tag: DW_TAG_pointer_type",
        "flags: DIFlagPublic | DIFlagStaticMember",
        "encoding: DW_ATE_signed",
        "language: DW_LANG_C99",
        "emissionKind: FullDebug",
        "nameTableKind: GNU",
        "checksumkind: CSK_MD5",
    ] {
        assert!(text.contains(needle), "missing {needle} in:\n{text}");
    }
}

/// Mirrors `LLParser::parseMDField(MDUnsignedField&)`, whose limit comes from
/// the field's declared type: `LineField` is `UINT32_MAX` and `ColumnField`
/// `UINT16_MAX`, so a `DILocation` column of 65536 is out of range while the
/// same value in `line` is fine.
#[test]
fn unsigned_metadata_fields_are_range_checked() {
    let err = parse_err("!0 = !{}\n!1 = !DILocation(scope: !0, column: 65536)\n");
    assert!(
        matches!(
            &err,
            ParseError::MetadataFieldValueTooLarge { field, limit, .. }
                if field == "column" && *limit == u64::from(u16::MAX)
        ),
        "expected column out of range, got: {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "value for 'column' too large, limit is 65535"
    );

    let text =
        parse_and_render("!named = !{!1}\n!0 = !{}\n!1 = !DILocation(scope: !0, line: 65536)\n");
    assert!(text.contains("line: 65536"), "output:\n{text}");
}

/// Mirrors `LLParser::parseMDField(MDField&)` with `(/* AllowNull */ false)`:
/// `DILocation`'s `scope` is the canonical case.
#[test]
fn a_non_nullable_metadata_field_rejects_null() {
    let err = parse_err("!0 = !DILocation(scope: null)\n");
    assert!(
        matches!(
            &err,
            ParseError::MetadataFieldCannotBeNull { field, .. } if field == "scope"
        ),
        "expected scope-cannot-be-null, got: {err:?}"
    );
    assert_eq!(err.to_string(), "'scope' cannot be null");
}

/// Mirrors `LLParser::parseMDField(MDStringField&)` with `EmptyIs::Error`:
/// `DIGlobalVariable`'s `name` is declared that way.
#[test]
fn a_non_empty_string_field_rejects_the_empty_string() {
    let err = parse_err("!0 = !DIGlobalVariable(name: \"\")\n");
    assert!(
        matches!(
            &err,
            ParseError::MetadataFieldCannotBeEmpty { field, .. } if field == "name"
        ),
        "expected name-cannot-be-empty, got: {err:?}"
    );
    assert_eq!(err.to_string(), "'name' cannot be empty");
}

/// Mirrors `LLParser::parseMDField(MDBoolField&)`, which accepts only the two
/// keywords.
#[test]
fn a_bool_field_rejects_a_non_boolean() {
    let err = parse_err("!0 = !{}\n!1 = !DISubprogram(scope: !0, isDefinition: 1)\n");
    assert!(
        matches!(&err, ParseError::Expected { expected, .. } if expected.contains("true")),
        "expected a true/false error, got: {err:?}"
    );
}

/// The three families `LLLexer` matches as **exact words** rather than by
/// prefix — `EmissionKind`, `NameTableKind`, `FixedPointKind`
/// (`LLLexer::LexIdentifier`). An unknown spelling never becomes one of those
/// tokens: it falls through to a silent `lltok::Error`, which is what makes
/// the *`expected …`* arm of each `LLParser::parseMDField` overload the one
/// that fires, and the `invalid … kind '…'` arm beside it dead for this input.
///
/// llvmkit used to answer `unknown keyword 'Bogus'` from the lexer instead, so
/// all three messages were unreachable.
#[test]
fn exact_word_kind_families_reject_an_unknown_spelling() {
    for (src, expected) in [
        (
            "!0 = !{}\n!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, emissionKind: Bogus)\n",
            "expected emission kind",
        ),
        (
            "!0 = !{}\n!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, nameTableKind: Bogus)\n",
            "expected nameTable kind",
        ),
        (
            "!0 = !DIFixedPointType(kind: Bogus)\n",
            "expected fixed-point kind",
        ),
    ] {
        assert_eq!(parse_err(src).to_string(), expected, "for {src:?}");
    }
}

/// The same shape for the `DW_*`-prefixed families, whose `parseMDField`
/// overloads open with the identical token-kind check. A word matching no
/// keyword — as opposed to a `DW_TAG_*` spelling the table does not carry,
/// which is the `invalid …` arm — reaches the `expected …` arm.
///
/// Anchored on the `parseMDField` overloads for `DwarfTagField`,
/// `DwarfLangField`, `DwarfAttEncodingField`, `DwarfVirtualityField`,
/// `DwarfCCField` and `DwarfMacinfoTypeField`; no upstream `.ll` covers them
/// (`test/Assembler` writes only the `invalid …` triggers), so the fixtures
/// are llvmkit's and the expected text is upstream's verbatim.
#[test]
fn dwarf_kind_families_reject_a_word_that_is_no_keyword() {
    for (src, expected) in [
        ("!0 = !DIBasicType(tag: Bogus)\n", "expected DWARF tag"),
        (
            "!0 = !DIBasicType(encoding: Bogus)\n",
            "expected DWARF type attribute encoding",
        ),
        (
            "!0 = !{}\n!1 = distinct !DICompileUnit(file: !0, language: Bogus)\n",
            "expected DWARF language",
        ),
        (
            "!0 = !{}\n!1 = distinct !DISubprogram(scope: !0, virtuality: Bogus)\n",
            "expected DWARF virtuality code",
        ),
        (
            "!0 = !DISubroutineType(cc: Bogus, types: !1)\n!1 = !{}\n",
            "expected DWARF calling convention",
        ),
        (
            "!0 = !DIMacro(type: Bogus, line: 1, name: \"x\")\n",
            "expected DWARF macinfo type",
        ),
    ] {
        assert_eq!(parse_err(src).to_string(), expected, "for {src:?}");
    }
}

/// llvmkit-specific: no upstream counterpart, but it guards a bug this port
/// nearly shipped. The parser's `fixed_point_kind` table originally read
/// `Unsigned`/`Signed`/`Rational`, which would have *rejected valid IR* —
/// upstream's spellings are `Binary`/`Decimal`/`Rational`
/// (`DIFixedPointType::FixedPointKind`).
///
/// These three families come from C++ enums, not a `.def`, so
/// `dwarf_def_drift.rs` cannot cover them. The lexer's word lists are the
/// second copy in-tree, so this pins the parser's tables against what the lexer
/// will actually hand it: a spelling the lexer accepts must not be one the
/// parser then calls invalid.
#[test]
fn exact_word_kind_families_accept_every_spelling_the_lexer_produces() {
    let text = parse_and_render(
        r#"
!named = !{!1, !2, !3, !4, !5, !6, !7, !8, !9, !10, !11}
!0 = !{}
!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, emissionKind: NoDebug)
!2 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, emissionKind: FullDebug)
!3 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, emissionKind: LineTablesOnly)
!4 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, emissionKind: DebugDirectivesOnly)
!5 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, nameTableKind: Default)
!6 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, nameTableKind: GNU)
!7 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, nameTableKind: Apple)
!8 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, nameTableKind: None)
!9 = !DIFixedPointType(kind: Binary)
!10 = !DIFixedPointType(kind: Decimal)
!11 = !DIFixedPointType(kind: Rational)
"#,
    );
    for needle in [
        "emissionKind: NoDebug",
        "emissionKind: FullDebug",
        "emissionKind: LineTablesOnly",
        "emissionKind: DebugDirectivesOnly",
        "nameTableKind: Default",
        "nameTableKind: GNU",
        "nameTableKind: Apple",
        "nameTableKind: None",
        "kind: Binary",
        "kind: Decimal",
        "kind: Rational",
    ] {
        assert!(text.contains(needle), "missing {needle} in:\n{text}");
    }
}

// --------------------------------------------------------------------------
// Debug records: `#dbg_*`, `!DIArgList`, and the debug-format intermix guard
// --------------------------------------------------------------------------

const DEBUG_VALUE_LIST: &str =
    include_str!("fixtures/upstream/debug-value-list/debug_value_list.ll");

const DBG_RECORD_INVALID_1: &str =
    include_str!("fixtures/upstream/dbg-record-invalid/dbg-record-invalid-1.ll");
const DBG_RECORD_INVALID_2: &str =
    include_str!("fixtures/upstream/dbg-record-invalid/dbg-record-invalid-2.ll");
const DBG_RECORD_INVALID_3: &str =
    include_str!("fixtures/upstream/dbg-record-invalid/dbg-record-invalid-3.ll");
const DBG_RECORD_INVALID_4: &str =
    include_str!("fixtures/upstream/dbg-record-invalid/dbg-record-invalid-4.ll");
const DBG_RECORD_INVALID_6: &str =
    include_str!("fixtures/upstream/dbg-record-invalid/dbg-record-invalid-6.ll");
const DBG_RECORD_INVALID_7: &str =
    include_str!("fixtures/upstream/dbg-record-invalid/dbg-record-invalid-7.ll");
const DBG_RECORD_INVALID_8: &str =
    include_str!("fixtures/upstream/dbg-record-invalid/dbg-record-invalid-8.ll");

/// Ports `test/Assembler/dbg-record-invalid-1.ll`: a `#dbg_value` record and a
/// `llvm.dbg.value` call in one module, record first.
///
/// Mirrors `LLParser`'s `SeenNewDbgInfoFormat` / `SeenOldDbgInfoFormat` pair,
/// whose whole job is to catch the mixture — upstream asserts the two are never
/// both set by the time `validateEndOfModule` runs.
#[test]
fn a_dbg_intrinsic_after_a_debug_record_is_rejected() {
    assert_eq!(
        parse_err(DBG_RECORD_INVALID_1).to_string(),
        "llvm.dbg intrinsic should not appear in a module using non-intrinsic debug info"
    );
}

/// Ports `test/Assembler/dbg-record-invalid-3.ll`: the same mixture the other
/// way round, call first.
#[test]
fn a_debug_record_after_a_dbg_intrinsic_is_rejected() {
    assert_eq!(
        parse_err(DBG_RECORD_INVALID_3).to_string(),
        "debug record should not appear in a module containing debug info intrinsics"
    );
}

/// Ports `test/Assembler/dbg-record-invalid-4.ll`: `#dbg_invalid` is not one
/// of the five spellings `LLLexer`'s `DBGRECORDTYPEKEYWORD` turns into a
/// `DbgRecordType`, so the word reaches `parseDebugRecord` as a silent
/// `lltok::Error` and its opening check answers `expected debug record type
/// here` — the one lowercase label in an otherwise capital-`E` routine.
#[test]
fn an_unknown_debug_record_type_is_rejected() {
    assert_eq!(
        parse_err(DBG_RECORD_INVALID_4).to_string(),
        "expected debug record type here"
    );
}

/// Ports `test/Assembler/dbg-record-invalid-2.ll` and `-6.ll`: a `#dbg_value`
/// field that should be a metadata node but is a bare value. `parseMDNode`
/// falls through to its `expected '!' here`, whichever field it is.
#[test]
fn a_debug_record_field_that_is_not_a_metadata_node_is_rejected() {
    for source in [DBG_RECORD_INVALID_2, DBG_RECORD_INVALID_6] {
        assert_eq!(parse_err(source).to_string(), "expected '!' here");
    }
}

/// Ports `test/Assembler/dbg-record-invalid-7.ll` and `-8.ll`: a missing
/// separator inside a `#dbg_value`. `parseDebugRecord` labels every one of its
/// commas with a **capital** `E`, a spelling it shares only with
/// `parseNamedMetadata`.
#[test]
fn a_debug_record_missing_a_separator_reports_the_capital_e_label() {
    for source in [DBG_RECORD_INVALID_7, DBG_RECORD_INVALID_8] {
        assert_eq!(parse_err(source).to_string(), "Expected ',' here");
    }
}

/// `!DIArgList(i32 %a, i32 %b)` in a `#dbg_value` record. Its operands are a
/// `ValueAsMetadata` list, so it needs a function state, which is why
/// `parseMetadata` special-cases it ahead of `parseSpecializedMDNode` and why
/// `parseNamedMetadata` refuses one outright. The record is one of its
/// spellings; `metadata !DIArgList(...)` in a call argument, an operand bundle
/// or an exception-argument list is the other, covered by
/// `upstream_debug_value_list_parses_with_di_arg_list_call_arguments` and
/// `di_arg_list_reaches_the_other_parse_metadata_as_value_callers`.
///
/// llvmkit-specific in its assembly, so the round trip is pinned against
/// `AsmWriter::writeDIArgList`, which prints each operand as a typed value.
///
/// `test/Assembler`'s `dbg-record-invalid-*` fixtures do carry `!DIArgList`,
/// but none pins an error raised *inside* the operand list: `-3.ll` and `-4.ll`
/// fail at the record's head before the `(` — at the `#` itself
/// (`LLParser::parseBasicBlock`'s hash loop) and at the record-type name
/// (`LLParser::parseDebugRecord`'s opening check) respectively — while `-0.ll`,
/// `-1.ll` and `-5.ll` parse the list to completion and fail at a token in the
/// next IR line (`}`, the return type of the following `call`, and `}`).
#[test]
fn di_arg_list_round_trips_inside_a_debug_record() {
    let text = parse_and_render(
        r#"
define void @f(i32 %a, i32 %b) !dbg !3 {
entry:
    #dbg_value(!DIArgList(i32 %a, i32 %b), !5, !DIExpression(), !4)
  ret void
}

!0 = !DIFile(filename: "a.c", directory: "/tmp")
!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, producer: "llvmkit")
!2 = !DISubroutineType(types: !{null})
!3 = distinct !DISubprogram(name: "f", file: !0, type: !2, unit: !1)
!4 = !DILocation(line: 1, column: 1, scope: !3)
!5 = !DILocalVariable(name: "x", file: !0, type: !6, scope: !3)
!6 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
"#,
    );
    assert!(
        text.contains("#dbg_value(!DIArgList(i32 %a, i32 %b), !"),
        "output:\n{text}"
    );
}

/// An empty `!DIArgList()` is legal — `parseDIArgList` guards its operand loop
/// with a `rparen` lookahead rather than requiring one.
#[test]
fn an_empty_di_arg_list_is_accepted() {
    let text = parse_and_render(
        r#"
define void @f() !dbg !3 {
entry:
    #dbg_value(!DIArgList(), !5, !DIExpression(), !4)
  ret void
}

!0 = !DIFile(filename: "a.c", directory: "/tmp")
!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, producer: "llvmkit")
!2 = !DISubroutineType(types: !{null})
!3 = distinct !DISubprogram(name: "f", file: !0, type: !2, unit: !1)
!4 = !DILocation(line: 1, column: 1, scope: !3)
!5 = !DILocalVariable(name: "x", file: !0, type: !6, scope: !3)
!6 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
"#,
    );
    assert!(
        text.contains("#dbg_value(!DIArgList(), !"),
        "output:\n{text}"
    );
}

/// Mirrors `LLParser::parseNamedMetadata`'s explicit refusal: "DIArgLists
/// should only appear inline in a function, as they may contain
/// LocalAsMetadata arguments which require a function context."
#[test]
fn a_di_arg_list_outside_a_function_is_rejected() {
    assert_eq!(
        parse_err("!named = !{!DIArgList(i32 0)}\n").to_string(),
        "found DIArgList outside of function"
    );
}

/// Mirrors the four field-interaction rules that live *below* the
/// `PARSE_MD_FIELDS()` macro in their classes' own `parse##CLASS` routines —
/// the reason those routines have a body at all beyond the macro.
#[test]
fn specialized_nodes_enforce_their_field_agreement_rules() {
    for (source, message) in [
        (
            "!0 = !DIFile(filename: \"a\", directory: \"b\")\n\
             !1 = !DICompileUnit(file: !0, language: DW_LANG_C)\n",
            "missing 'distinct', required for !DICompileUnit",
        ),
        (
            "!0 = !DIFile(filename: \"a\", directory: \"b\")\n\
             !1 = distinct !DICompileUnit(file: !0)\n",
            "missing one of 'language' or 'sourceLanguageName', required for !DICompileUnit",
        ),
        (
            "!0 = !DIFile(filename: \"a\", directory: \"b\")\n\
             !1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, sourceLanguageName: DW_LNAME_C)\n",
            "can only specify one of 'language' and 'sourceLanguageName' on !DICompileUnit",
        ),
        (
            "!0 = !DIFile(filename: \"a\", directory: \"b\")\n\
             !1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, sourceLanguageVersion: 1)\n",
            "'sourceLanguageVersion' requires an associated 'sourceLanguageName' on !DICompileUnit",
        ),
        (
            "!0 = !DIFile(filename: \"a\", directory: \"b\", checksumkind: CSK_MD5)\n",
            "'checksumkind' and 'checksum' must be provided together",
        ),
        (
            "!0 = !DIEnumerator(name: \"A\", value: -1, isUnsigned: true)\n",
            "unsigned enumerator with negative value",
        ),
        (
            "!0 = !DISubprogram(name: \"f\", spFlags: DISPFlagDefinition)\n",
            "missing 'distinct', required for !DISubprogram that is a Definition",
        ),
        (
            "!0 = !DISubprogram(name: \"f\", isDefinition: true)\n",
            "missing 'distinct', required for !DISubprogram that is a Definition",
        ),
    ] {
        assert_eq!(parse_err(source).to_string(), message, "source: {source}");
    }
}

/// Mirrors `test/Assembler/debug-info.ll`'s
/// `; CHECK-NEXT: !36 = !DIFile(filename: "file", directory: "dir", source: "int source() { }\0A")`
/// (RUN: `llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s`, so
/// the CHECK line is the *second* `llvm-dis`'s output — `parse_render_reparse`
/// runs both trips). `llvm::printEscapedString`
/// writes `'\\' << hexdigit(C >> 4) << hexdigit(C & 0x0F)`, and `hexdigit`'s
/// `LowerCase` parameter defaults to `false`, so the escape is `\0A` and not
/// `\0a`.
///
/// The obvious fixture for this, `test/Assembler/difile-escaped-chars.ll`
/// (`; CHECK: !0 = !DIFile(filename: "\00\01\02\80\81\82\FD\FE\FF", ...)`),
/// cannot be ported: llvmkit rejects it with `expected UTF-8 string constant`.
/// That is gap **G9** in `docs/fixture-coverage.md`, left on record rather
/// than trimmed.
#[test]
fn metadata_string_hex_escapes_print_uppercase() {
    // The vendored fixture whole, not a hand-typed excerpt: `UPSTREAM.md`'s
    // audit rule requires a `mirror` row over an upstream `.ll` to load a
    // checked-in copy. This is the same file `parser_corpus_manifest.txt`
    // drives at `status=pass`; `!39` and `!40` are the two `source:` nodes.
    const FIXTURE: &str = include_str!("fixtures/upstream/assembler-corpus/debug-info.ll");

    let text = parse_render_reparse(FIXTURE);
    assert!(text.contains(r#"source: "int source() { }\0A""#), "{text}");
}

/// Mirrors `LLParser::parseDIExpressionBody`, which looks each `DW_OP_*` and
/// `DW_ATE_*` spelling up in its own table and rejects one the table does not
/// carry — llvmkit used to store the name as written and print it straight
/// back. The oversized-element case is a message of its own, split from
/// `expected unsigned integer` because the value is read first and measured
/// second.
#[test]
fn di_expression_validates_its_operands() {
    assert_eq!(
        parse_err("!0 = !DIExpression(DW_OP_bogus)\n").to_string(),
        "invalid DWARF op 'DW_OP_bogus'"
    );
    assert_eq!(
        parse_err("!0 = !DIExpression(DW_ATE_bogus)\n").to_string(),
        "invalid DWARF attribute encoding 'DW_ATE_bogus'"
    );
    assert_eq!(
        parse_err("!0 = !DIExpression(18446744073709551616)\n").to_string(),
        "element too large, limit is 18446744073709551615"
    );
}

/// `!DIArgList(metadata %a)` — a `metadata`-typed operand inside a
/// `DIArgList`.
///
/// `LLParser::parseDIArgList` is the second direct caller of
/// `parseValueAsMetadata`, alongside `parseMetadata`'s non-`!` fall-through,
/// and it passes its own `TypeMsg`, `"expected value-as-metadata operand"`.
/// Because it goes through that routine it inherits the
/// `if (Ty->isMetadataTy())` guard, so a `metadata` operand is rejected at the
/// *type* with `invalid metadata-value-metadata roundtrip`. llvmkit had
/// inlined the routine's body here without the guard, so the parse ran on and
/// complained about the value instead.
///
/// **Anchored on the routine, not on a fixture.**
/// `test/Assembler`'s `dbg-record-invalid-*` fixtures do carry `!DIArgList`,
/// but none pins an error raised *inside* the operand list: `-3.ll` and `-4.ll`
/// fail at the record's head before the `(` — at the `#` itself
/// (`LLParser::parseBasicBlock`'s hash loop) and at the record-type name
/// (`LLParser::parseDebugRecord`'s opening check) respectively — while `-0.ll`,
/// `-1.ll` and `-5.ll` parse the list to completion and fail at a token in the
/// next IR line (`}`, the return type of the following `call`, and `}`).
#[test]
fn di_arg_list_rejects_a_metadata_typed_operand() {
    let src = r#"
define void @f(i32 %a) !dbg !3 {
entry:
    #dbg_value(!DIArgList(metadata %a), !5, !DIExpression(), !4)
  ret void
}

!0 = !DIFile(filename: "a.c", directory: "/tmp")
!1 = distinct !DICompileUnit(file: !0, language: DW_LANG_C, producer: "llvmkit")
!2 = !DISubroutineType(types: !{null})
!3 = distinct !DISubprogram(name: "f", file: !0, type: !2, unit: !1)
!4 = !DILocation(line: 1, column: 1, scope: !3)
!5 = !DILocalVariable(name: "x", file: !0, type: !6, scope: !3)
!6 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
"#;
    assert_eq!(
        parse_err(src).to_string(),
        "invalid metadata-value-metadata roundtrip"
    );
}
/// Ports `test/DebugInfo/Generic/debug_value_list.ll`, byte-identical under
/// `fixtures/upstream/debug-value-list/` — three `llvm.dbg.value` calls whose
/// first argument is `metadata !DIArgList(...)`.
///
/// The construct this pins is `LLParser::parseMetadata`'s opening
/// `lltok::MetadataVar` / `"DIArgList"` dispatch into
/// `LLParser::parseDIArgList`, which runs ahead of `parseSpecializedMDNode`
/// and is the reason `LLParser::parseMetadataAsValue` forwards a
/// `PerFunctionState &` at all. llvmkit had hoisted that dispatch into two
/// callers instead, so every site reached through `parse_metadata_as_value`
/// rejected this file with `expected metadata type`.
///
/// **Oracle substitution, and why.** The fixture's `RUN` line is
/// `opt -passes=verify < %s | opt -passes=verify -S | FileCheck %s`, so its
/// `CHECK-COUNT-3: #dbg_value(` block is `opt`'s output *after* the
/// dbg-intrinsic-to-`#dbg_*`-record conversion in `llvm::UpgradeIntrinsicCall`.
/// llvmkit does not port that routine — `crates/llvmkit-ir/src/auto_upgrade.rs`
/// covers `UpgradeModuleFlags`, `UpgradeSectionAttributes` and
/// `UpgradeTBAANode`, and nothing on the intrinsic side, recorded as
/// `docs/divergences.md` entry 19 — so llvmkit re-prints the intrinsic-call
/// spelling. Its metadata slot numbering is not `SlotTracker`'s walk order
/// either (entry 99), which is why the `CHECK-SAME: !16,` directive has no
/// counterpart here.
///
/// So of the fixture's four directives, **two are asserted against upstream's
/// literal text** — `!DIArgList(i32 %a, i32 %b, i32 5)` and the
/// `!DIExpression(DW_OP_LLVM_arg, ...)`, on one line as `CHECK-SAME` demands.
/// `CHECK-COUNT-3: #dbg_value(` is asserted as the three `llvm.dbg.value` calls
/// llvmkit actually prints, and `CHECK-SAME: !16,` is dropped; both are
/// recorded as entries 19 and 99 rather than trimmed. The fixture itself is
/// byte-identical to upstream's.
#[test]
fn upstream_debug_value_list_parses_with_di_arg_list_call_arguments() {
    let module = Module::dynamic("debug_value_list");
    Parser::new(DEBUG_VALUE_LIST.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("test/DebugInfo/Generic/debug_value_list.ll parses");
    let text = format!("{module}");
    module.verify().expect("the fixture verifies");

    // `; CHECK-COUNT-3: #dbg_value(`
    assert_eq!(
        text.matches("call void @llvm.dbg.value(metadata !DIArgList(")
            .count(),
        3,
        "output:\n{text}"
    );
    // `; CHECK-SAME: !DIArgList(i32 %a, i32 %b, i32 5)` and
    // `; CHECK-SAME: !DIExpression(DW_OP_LLVM_arg, 0, ...)`, both on the line
    // the third match sits on.
    let expression = concat!(
        "!DIExpression(DW_OP_LLVM_arg, 0, DW_OP_LLVM_arg, 1, DW_OP_plus, ",
        "DW_OP_LLVM_arg, 2, DW_OP_plus)"
    );
    let third = text
        .lines()
        .find(|line| line.contains("!DIArgList(i32 %a, i32 %b, i32 5)"))
        .unwrap_or_else(|| panic!("no three-operand DIArgList in:\n{text}"));
    assert!(third.contains(expression), "line: {third}");
}

/// `metadata !DIArgList(...)` inside an operand bundle and inside a
/// `cleanuppad` argument list — the two `parseMetadataAsValue` callers that are
/// not `parseParameterList`.
///
/// `LLParser::parseOptionalOperandBundles` and `LLParser::parseExceptionArgs`
/// both call `parseMetadataAsValue(V, PFS)` once the operand type is
/// `metadata`, and that routine is `parseMetadata` plus `MetadataAsValue::get`
/// — so upstream's `DIArgList` dispatch is reachable from both. No upstream
/// fixture spells either shape; this test has no upstream counterpart and
/// exists to hold the dispatch at the call sites the ported fixture above does
/// not reach.
#[test]
fn di_arg_list_reaches_the_other_parse_metadata_as_value_callers() {
    let text = parse_and_render(
        r#"
declare void @g()

define void @f(i32 %a, i32 %b) personality ptr null {
entry:
  call void @g() [ "tag"(metadata !DIArgList(i32 %a, i32 %b)) ]
  invoke void @g() to label %cont unwind label %pad

cont:
  ret void

pad:
  %cp = cleanuppad within none [metadata !DIArgList(i32 %a)]
  cleanupret from %cp unwind to caller
}
"#,
    );
    assert!(
        text.contains(r#"[ "tag"(metadata !DIArgList(i32 %a, i32 %b)) ]"#),
        "output:\n{text}"
    );
    assert!(
        text.contains("cleanuppad within none [metadata !DIArgList(i32 %a)]"),
        "output:\n{text}"
    );
}
