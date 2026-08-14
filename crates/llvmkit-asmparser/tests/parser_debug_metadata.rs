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

const DEBUG_MODULE: &str = r#"
@g = global i32 0, !dbg !15

define i32 @f() !dbg !3 {
entry:
  ret i32 0, !dbg !4
}

!0 = !DIFile(filename: "a.c", directory: "/tmp")
!1 = !DICompileUnit(file: !0, language: DW_LANG_C, producer: "llvmkit")
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
!4 = !DICompileUnit(language: DW_LANG_C11, file: !6, producer: "llvmkit")
!5 = !DIBasicType(name: "double", size: 64, encoding: DW_ATE_float)
!6 = !DIFile(filename: "test.c", directory: "/tmp")
"#,
    );
    assert!(
        text.contains("#dbg_declare_value(double %x, !1, !DIExpression(), !2)"),
        "output:\n{text}"
    );
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
!1 = !DICompileUnit(file: !0, language: DW_LANG_C, producer: "llvmkit")
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

/// Ports `test/Assembler/invalid-dilocation-field-bad.ll`, whose CHECK line is
/// `error: invalid field 'bad'`.
#[test]
fn dilocation_rejects_a_field_its_class_does_not_declare() {
    let err = parse_err("!0 = !DILocation(bad: 0)\n");
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

/// Ports `test/Assembler/debug-info.ll`'s `DISubroutineType` flags case, whose
/// `CHECK-NEXT` line pins the round-tripped text as byte-identical to the
/// input: `!DISubroutineType(flags: DIFlagPublic | DIFlagStaticMember, types:
/// !25)`. `AsmWriter.cpp::printDIFlags` joins with `ListSeparator(" | ")`, so
/// the joined source text llvmkit stores prints back unchanged.
#[test]
fn debug_info_flag_disjunction_round_trips() {
    let text = parse_and_render(
        "!0 = !{}\n!1 = !DISubroutineType(flags: DIFlagPublic | DIFlagStaticMember, types: !0)\n",
    );
    assert!(
        text.contains("!DISubroutineType(flags: DIFlagPublic | DIFlagStaticMember, types: !0)"),
        "output:\n{text}"
    );
}

/// Ports `test/Assembler/diexpression.ll`, an `llvm-as | llvm-dis` round-trip
/// whose `CHECK-SAME` lines are byte-identical to its input. Covers the empty
/// body, bare ops, op+literal sequences, and the `DW_OP_LLVM_convert` form that
/// mixes in `DW_ATE_*` attribute encodings — the second keyword family
/// `LLParser::parseDIExpressionBody` accepts.
#[test]
fn diexpression_forms_round_trip() {
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
    let mut src = String::from("!named = !{!0, !1, !2, !3, !4, !5, !6, !7, !8}\n");
    for (i, form) in FORMS.iter().enumerate() {
        src.push_str(&format!("!{i} = {form}\n"));
    }
    let text = parse_and_render(&src);
    for form in FORMS {
        assert!(text.contains(form), "missing {form} in:\n{text}");
    }
}

/// Ports `test/Assembler/invalid-diexpression-large.ll`: an element of exactly
/// `UINT64_MAX` is accepted (`CHECK-NOT: error:`) and one above it is not.
///
/// Same logic as upstream, different diagnostic: upstream reports "element too
/// large, limit is 18446744073709551615" from `parseDIExpressionBody`, while
/// llvmkit reports the structured `Expected` error its parser uses throughout,
/// so this asserts on the accept/reject behaviour rather than on message text.
#[test]
fn diexpression_element_at_the_u64_limit_is_accepted_and_beyond_is_rejected() {
    let text = parse_and_render("!named = !{!0}\n!0 = !DIExpression(18446744073709551615)\n");
    assert!(
        text.contains("!DIExpression(18446744073709551615)"),
        "output:\n{text}"
    );
    let _ = parse_err("!0 = !DIExpression(18446744073709551616)\n");
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
            "!0 = !{}\n!1 = !DICompileUnit(file: !0, language: DW_LANG_bogus)\n",
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
!3 = !DICompileUnit(file: !0, language: DW_LANG_C99, emissionKind: FullDebug, nameTableKind: GNU)
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
/// (`LLLexer.cpp::LexIdentifier`). An unknown spelling never becomes one of
/// those tokens in either implementation, so the parser's `invalid ... kind`
/// arm is unreachable for it and the rejection happens a layer earlier.
///
/// Same accept/reject verdict as upstream, different diagnostic: `llvm-as`
/// reaches `expected emission kind` from `parseMDField`, while llvmkit's lexer
/// rejects the unknown keyword outright. Recorded rather than papered over.
#[test]
fn exact_word_kind_families_reject_an_unknown_spelling() {
    for src in [
        "!0 = !{}\n!1 = !DICompileUnit(file: !0, emissionKind: Bogus)\n",
        "!0 = !{}\n!1 = !DICompileUnit(file: !0, nameTableKind: Bogus)\n",
        "!0 = !DIFixedPointType(kind: Bogus)\n",
    ] {
        let _ = parse_err(src);
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
!1 = !DICompileUnit(file: !0, emissionKind: NoDebug)
!2 = !DICompileUnit(file: !0, emissionKind: FullDebug)
!3 = !DICompileUnit(file: !0, emissionKind: LineTablesOnly)
!4 = !DICompileUnit(file: !0, emissionKind: DebugDirectivesOnly)
!5 = !DICompileUnit(file: !0, nameTableKind: Default)
!6 = !DICompileUnit(file: !0, nameTableKind: GNU)
!7 = !DICompileUnit(file: !0, nameTableKind: Apple)
!8 = !DICompileUnit(file: !0, nameTableKind: None)
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
