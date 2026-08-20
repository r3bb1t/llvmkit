//! Call / invoke / callbr parser tests.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    parse_and_render_bytes("parser_calls", src.as_bytes())
}

fn parse_and_render_bytes(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    format!("{module}")
}

/// `llvm-as < %s | llvm-dis` — both halves. `llvm-as` runs `verifyModule` on
/// the parsed module unless `-disable-verify` is passed, so a `RUN` line that
/// pipes through it asserts verification as well as parse-and-print;
/// [`parse_and_render_bytes`] drops that half. Mirrors
/// `llvm/tools/llvm-as/llvm-as.cpp`'s `if (!DisableVerify && verifyModule(*M, &errs()))`.
fn parse_verify_and_render_bytes(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    module
        .verify_borrowed()
        .expect("`llvm-as` verifies this fixture, so llvmkit must too");
    format!("{module}")
}

fn parse_fixture_err(module_name: &str, src: &[u8]) -> ParseError {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("parser rejects malformed input")
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

/// Inline-asm `sideeffect alignstack` spelling from `test/Assembler/alignstack.ll`.
#[test]
fn inline_asm_sideeffect_alignstack_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/alignstack/inline_asm_sideeffect_alignstack_round_trips.ll"
    );

    let text = parse_and_render_bytes("inline_asm_sideeffect_alignstack_round_trips", FIXTURE);
    assert_check_lines(&text, &["@test2", "sideeffect alignstack", "ret void"]);
}

/// llvmkit-specific subset of `test/Bindings/llvm-c/echo.ll` inline-asm
/// `inteldialect` / `unwind` spelling, using named values and ordinary calls.
#[test]
fn inline_asm_inteldialect_unwind_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/echo/inline_asm_inteldialect_unwind_round_trips.ll");

    let text = parse_and_render_bytes("inline_asm_inteldialect_unwind_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "%intel = call i32 asm inteldialect unwind \"mov $0, $1\", \"=r,r,~{dirflag},~{fpsr},~{flags}\"(i32 %x)",
            "%att = call i32 asm alignstack unwind \"mov $1, $0\", \"=r,r,~{dirflag},~{fpsr},~{flags}\"(i32 %intel)",
        ],
    );
}

/// Every split of `test/Assembler/inline-asm-constraint-error.ll`, each
/// asserting its own `FileCheck` line. The nine splits are exactly the nine
/// messages `InlineAsm::verify` can produce, and
/// `LLParser::convertValIDToValue` prints each verbatim.
#[test]
fn inline_asm_constraint_errors_match_upstream_text() {
    const SPLITS: &[(&str, &[u8], &str)] = &[
        (
            "parse-fail",
            include_bytes!("fixtures/upstream/inline-asm-constraint-error/parse-fail.ll"),
            "failed to parse constraints",
        ),
        (
            "input-before-output",
            include_bytes!("fixtures/upstream/inline-asm-constraint-error/input-before-output.ll"),
            "output constraint occurs after input, clobber or label constraint",
        ),
        (
            "input-after-clobber",
            include_bytes!("fixtures/upstream/inline-asm-constraint-error/input-after-clobber.ll"),
            "input constraint occurs after clobber constraint",
        ),
        (
            "must-return-void",
            include_bytes!("fixtures/upstream/inline-asm-constraint-error/must-return-void.ll"),
            "inline asm without outputs must return void",
        ),
        (
            "cannot-be-struct",
            include_bytes!("fixtures/upstream/inline-asm-constraint-error/cannot-be-struct.ll"),
            "inline asm with one output cannot return struct",
        ),
        (
            "incorrect-struct-elements",
            include_bytes!(
                "fixtures/upstream/inline-asm-constraint-error/incorrect-struct-elements.ll"
            ),
            "number of output constraints does not match number of return struct elements",
        ),
        (
            "incorrect-arg-num",
            include_bytes!("fixtures/upstream/inline-asm-constraint-error/incorrect-arg-num.ll"),
            "number of input constraints does not match number of parameters",
        ),
        (
            "label-after-clobber",
            include_bytes!("fixtures/upstream/inline-asm-constraint-error/label-after-clobber.ll"),
            "label constraint occurs after clobber constraint",
        ),
        (
            "output-after-label",
            include_bytes!("fixtures/upstream/inline-asm-constraint-error/output-after-label.ll"),
            "output constraint occurs after input, clobber or label constraint",
        ),
    ];

    for (name, fixture, expected) in SPLITS {
        let err = parse_fixture_err(name, fixture);
        assert_eq!(err.to_string(), *expected, "split {name}");
    }
}

/// `test/Assembler/invalid-inline-constraint.ll` (LLVM bug 24646), fixture
/// verbatim — including the stray `0x1C` byte upstream's corrupted body
/// carries. It reaches the same `failed to parse constraints` as the
/// `parse-fail` split above, but through wreckage rather than a tidy bad
/// clobber: the deliberate garbage after the call (`ounwi`, `ounwindret`)
/// lexes as `Token::Error`, which the attribute loop ends on, so
/// `InlineAsm::verify` still runs on the constraint string. llvmkit's lexer
/// used to fail on `unknown keyword 'ounwi'` first.
#[test]
fn a_corrupted_inline_asm_body_still_reports_the_constraint_failure() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/inline-asm-constraint-error/invalid-inline-constraint.ll"
    );

    let err = parse_fixture_err("invalid-inline-constraint", FIXTURE);
    assert_eq!(err.to_string(), "failed to parse constraints");
}

/// `test/Assembler/invalid-untyped-metadata.ll` (LLVM bug 24645), fixture
/// verbatim: inline asm outside a call callee has no function type to check
/// the constraint string against.
#[test]
fn inline_asm_without_a_function_type_is_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/inline-asm-constraint-error/invalid-untyped-metadata.ll");

    let err = parse_fixture_err("invalid-untyped-metadata", FIXTURE);
    assert_eq!(
        err.to_string(),
        "invalid type for inline asm constraint string"
    );
}

/// `Verifier::verifyInlineAsmCall`'s two label rules, asserted at the layer
/// upstream puts them: the parser accepts both shapes and `verify()` reports.
///
/// Both used to be parse-time rejections with llvmkit wordings, which shadowed
/// the verifier rule llvmkit already had — the ordinary-call one carries
/// upstream's text verbatim. The fixtures are llvmkit's, because upstream's
/// own splits of `inline-asm-constraint-error.ll` all stop at
/// `InlineAsm::verify` and never reach these two.
#[test]
fn inline_asm_label_constraint_rules_are_verifier_rules() {
    const CALL: &[u8] = include_bytes!(
        "fixtures/upstream/inline-asm-constraint-error/inline_asm_call_label_constraint_subset.ll"
    );
    const CALLBR: &[u8] = include_bytes!(
        "fixtures/upstream/inline-asm-constraint-error/inline_asm_callbr_label_constraints_subset.ll"
    );

    for (name, fixture, expected) in [
        (
            "inline_asm_call_label_constraint_subset",
            CALL,
            "Label constraints can only be used with callbr",
        ),
        (
            "inline_asm_callbr_label_constraints_subset",
            CALLBR,
            "Number of label constraints does not match number of callbr dests",
        ),
    ] {
        let module = Module::dynamic(name);
        Parser::new(fixture, &module)
            .expect("lexer primes")
            .parse_module()
            .expect("the parser accepts what upstream's parser accepts");
        let err = module
            .verify_borrowed()
            .expect_err("the verifier rejects it");
        assert!(
            err.to_string().contains(expected),
            "{name}: unexpected error: {err}"
        );
    }
}

/// Mirrors `test/Assembler/callbr.ll` successor structure with the upstream
/// `@llvm.amdgcn.kill` intrinsic callee.
///
/// The needles are matched in order, and they now read in upstream's printed
/// order — `[[KILL:.*:]]` / `unreachable` / `[[CONT]]:` / `ret void` — for the
/// first time: the source defines `kill` before `cont` while the `callbr`
/// names `cont` first, and `LLParser::PerFunctionState::defineBB` ends with
/// `F.splice(F.end(), &F, BB->getIterator())`, so the printed order is
/// definition order.
#[test]
fn callbr_successor_structure_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/callbr/callbr_successor_structure_round_trips.ll");

    let text = parse_and_render_bytes("callbr_successor_structure_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "callbr void @llvm.amdgcn.kill(i1 %c)",
            "to label %cont [label %kill]",
            "kill:",
            "unreachable",
            "cont:",
            "ret void",
        ],
    );
}

/// Mirrors `LLParser.cpp::parseCall` call-site modifiers that llvmkit models
/// today: tail kind, calling convention, return/param attrs, and attr-group refs.
#[test]
fn call_modifiers_round_trip() {
    let text = parse_and_render(
        "attributes #0 = { nounwind }\n\
declare fastcc zeroext i32 @callee(i32 zeroext)\n\
define i32 @f(i32 %x) {\n\
entry:\n\
  %r = tail call fastcc zeroext i32 @callee(i32 zeroext %x) #0\n\
  ret i32 %r\n\
}\n",
    );
    assert_check_lines(
        &text,
        &["%r = tail call fastcc zeroext i32 @callee(i32 zeroext %x) #0"],
    );
}

/// Mirrors `test/Assembler/musttail.ll`: a musttail call in a varargs
/// function forwards the varargs with a trailing `...`, which the printer
/// re-emits (AsmWriter's CallInst arm).
#[test]
fn musttail_varargs_forwarding_round_trips() {
    let text = parse_and_render(
        "declare ptr @f(ptr, ...)\n\
         define ptr @thunk(ptr %this, ...) {\n\
         entry:\n\
           %rv = musttail call ptr (ptr, ...) @f(ptr %this, ...)\n\
           ret ptr %rv\n\
         }\n",
    );
    assert_check_lines(
        &text,
        &["%rv = musttail call ptr (ptr, ...) @f(ptr %this, ...)"],
    );
}

/// `LLParser::parseParameterList`: `...` in a non-musttail call's argument
/// list is rejected.
#[test]
fn ellipsis_in_non_musttail_call_rejected() {
    let src = "declare void @f(...)\n\
               define void @g() {\n\
               entry:\n\
                 call void (...) @f(i32 1, ...)\n\
                 ret void\n\
               }\n";
    assert_fixture_rejected(
        "ellipsis_non_musttail",
        src.as_bytes(),
        "expected unexpected ellipsis in argument list for non-musttail call",
    );
}

/// `LLParser::parseParameterList`: a musttail `...` is rejected when the
/// enclosing function is not varargs.
#[test]
fn musttail_ellipsis_in_non_varargs_function_rejected() {
    let src = "declare void @f(...)\n\
               define void @g() {\n\
               entry:\n\
                 musttail call void (...) @f(...)\n\
                 ret void\n\
               }\n";
    assert_fixture_rejected(
        "musttail_non_varargs",
        src.as_bytes(),
        "expected unexpected ellipsis in argument list for musttail call in non-varargs function",
    );
}

/// `LLParser::parseParameterList`'s reciprocal rule: a musttail call in a
/// varargs function must forward the varargs with a trailing `...`.
#[test]
fn musttail_in_varargs_without_ellipsis_rejected() {
    let src = "declare void @f(...)\n\
               define void @g(...) {\n\
               entry:\n\
                 musttail call void (...) @f()\n\
                 ret void\n\
               }\n";
    assert_fixture_rejected(
        "musttail_missing_ellipsis",
        src.as_bytes(),
        "expected '...' at end of argument list for musttail call in varargs function",
    );
}

/// Mirrors `llvm/test/Assembler/amdgcn-intrinsic-attributes.ll` range
/// attribute spelling on call return values.
#[test]
fn call_return_range_attribute_round_trips() {
    let text = parse_and_render(
        "declare range(i8 0, 64) i8 @callee()\n\
define i8 @f() {\n\
entry:\n\
  %r = call range(i8 0, 64) i8 @callee()\n\
  ret i8 %r\n\
}\n",
    );
    assert_check_lines(
        &text,
        &[
            "declare range(i8 0, 64) i8 @callee()",
            "%r = call range(i8 0, 64) i8 @callee()",
        ],
    );
}

/// `FileCheck::CanonicalizeFile` (`llvm/lib/FileCheck/FileCheck.cpp`), both
/// halves of its loop body: drop the `\r` of a `\r\n` pair, then collapse each
/// run of ' ' / '\t' to a single ' '. FileCheck applies it to the check file
/// *and* the input file unless `--strict-whitespace` is given, and
/// `test/Bitcode/operand-bundles.ll`'s `RUN` line does not give it. Porting it
/// is what lets that fixture's `CHECK` text be quoted as written: several of
/// its lines spell `float  0.000000e+00` with two spaces, because that is how
/// the fixture's *input* spells it, and canonicalization is why they still
/// match `llvm-dis`'s single space.
///
/// This is a second copy of the routine in
/// `crates/llvmkit-asmparser/tests/parser_eh_funclet.rs`; items do not cross
/// integration-test binaries, and the `tests/support/` refactor that would
/// remove both copies is recorded in `docs/future-work.md`.
fn canonicalize_horizontal_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        // Eliminate trailing dosish `\r`.
        if c == '\r' && chars.peek() == Some(&'\n') {
            continue;
        }
        if c != ' ' && c != '\t' {
            out.push(c);
            continue;
        }
        // Otherwise, add one space and advance over neighboring space.
        out.push(' ');
        while let Some(' ' | '\t') = chars.peek() {
            chars.next();
        }
    }
    out
}

/// `test/Bitcode/operand-bundles.ll`, vendored whole, asserting its `CHECK`
/// directives in order.
///
/// Its `RUN` line is `llvm-as < %s | llvm-dis | FileCheck %s`, so the `CHECK`
/// text is `AssemblyWriter` output with no bitcode round-trip loss in between
/// — specifically `AssemblyWriter::writeOperandBundles`, whose `Out << " [ "`
/// and `Out << " ]"` are the spaces this fixture pins. The fixture's
/// typed-pointer spelling (`i32* %ptr`) parses and prints as `ptr %ptr`,
/// exactly as `llvm-dis` does, and no directive pins it. The `llvm-as` half of
/// that pipeline verifies the module — no `-disable-verify` here — so
/// [`parse_verify_and_render_bytes`] is the oracle rather than parse-and-print.
///
/// **Harness gap, stated rather than papered over.** This binary's
/// `assert_check_lines` is ordered fixed-substring matching over a byte
/// cursor. That renders `CHECK:` faithfully, and with
/// [`canonicalize_horizontal_whitespace`] applied to both buffers it renders
/// FileCheck's whitespace handling too, so a directive can be quoted from the
/// fixture as written rather than hand-collapsed. What it does **not** render
/// is `CHECK-LABEL`'s block partitioning or `CHECK-NEXT`'s line-adjacency
/// rule: those directives are asserted below as ordered `CHECK`es, which is
/// weaker than FileCheck, not stricter. Nor can it evaluate a regex — `{{$}}`
/// is rendered as a trailing `\n`, which is what end-of-line means for a fixed
/// substring.
/// `crates/llvmkit-asmparser/tests/parser_eh_funclet.rs::check_directives` is a
/// faithful `CHECK`/`CHECK-NEXT` port, but items do not cross integration-test
/// binaries; the `tests/support/` refactor that would let this test use it is
/// recorded in `docs/future-work.md`.
///
/// This replaces a hand-trimmed subset whose header said llvmkit could not
/// express the rest of the file. That premise was stale: the whole fixture
/// parses.
#[test]
fn operand_bundles_ll_matches_upstream_check_lines() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/operand-bundles/operand-bundles.ll");

    // The fixture's own directives, in order, quoted from its `; CHECK…:`
    // lines; `{{$}}` is rendered as the newline it stands for.
    const DIRECTIVES: &[&str] = &[
        // @f0
        "@f0(",
        "call void @callee0() [ \"foo\"(i32 42, i64 100, i32 %x), \"bar\"(float  0.000000e+00, i64 100, i32 %l) ]",
        // @f1 --- one CHECK with `{{$}}`, then two CHECK-NEXT.
        "@f1(",
        "@callee0()\n",
        "call void @callee0() [ \"foo\"() ]",
        "call void @callee0() [ \"foo\"(i32 42, i64 100, i32 %x), \"bar\"(float  0.000000e+00, i64 100, i32 %l) ]",
        // @f2
        "@f2(",
        "call void @callee0() [ \"foo\"() ]",
        // @f3
        "@f3(",
        "call void @callee0() [ \"foo\"(i32 42, i64 100, i32 %x), \"foo\"(i32 42, float  0.000000e+00, i32 %l) ]",
        // @f4
        "@f4(",
        "call void @callee1(i32 10, i32 %x) [ \"foo\"(i32 42, i64 100, i32 %x), \"foo\"(i32 42, float  0.000000e+00, i32 %l) ]",
        // @f5 --- the metadata-string bundle form, which llvmkit already accepted.
        "call void @callee1(i32 10, i32 %x) [ \"foo\"(i32 42, metadata !\"abc\"), \"bar\"(metadata !\"abcde\", metadata !\"qwerty\") ]",
        // @g0 --- the invoke twins of the above.
        "@g0(",
        "invoke void @callee0() [ \"foo\"(i32 42, i64 100, i32 %x), \"bar\"(float  0.000000e+00, i64 100, i32 %l) ]",
        // @g1
        "@g1(",
        "invoke void @callee0()\n",
        "invoke void @callee0() [ \"foo\"() ]",
        "invoke void @callee0() [ \"foo\"(i32 42, i64 100, i32 %x), \"foo\"(i32 42, float  0.000000e+00, i32 %l) ]",
        // @g2
        "@g2(",
        "invoke void @callee0() [ \"foo\"() ]",
        // @g3
        "@g3(",
        "invoke void @callee0() [ \"foo\"(i32 42, i64 100, i32 %x), \"foo\"(i32 42, float  0.000000e+00, i32 %l) ]",
        // @g4
        "@g4(",
        "invoke void @callee1(i32 10, i32 %x) [ \"foo\"(i32 42, i64 100, i32 %x), \"foo\"(i32 42, float  0.000000e+00, i32 %l) ]",
        // @g5
        "invoke void @callee1(i32 10, i32 %x) [ \"foo\"(i32 42, metadata !\"abc\"), \"bar\"(metadata !\"abcde\", metadata !\"qwerty\") ]",
    ];

    let text = parse_verify_and_render_bytes("operand_bundles_ll", FIXTURE);
    let canonical_text = canonicalize_horizontal_whitespace(&text);
    let canonical_directives: Vec<String> = DIRECTIVES
        .iter()
        .map(|directive| canonicalize_horizontal_whitespace(directive))
        .collect();
    let needles: Vec<&str> = canonical_directives.iter().map(String::as_str).collect();
    assert_check_lines(&canonical_text, &needles);
}

/// A `ValueAsMetadata` operand-bundle input — `metadata i32 %a`,
/// `metadata i32 42`, `metadata ptr @g`.
///
/// **Anchored on the routine, not on a fixture.** No `.ll` file was found to
/// port this from — the metadata bundle inputs in the fixture vendored
/// alongside it (`test/Bitcode/operand-bundles.ll` `@f5` and `@g5`) spell only
/// the `metadata !"..."` form, which llvmkit already accepted. The rule is
/// `LLParser::parseOptionalOperandBundles`, which routes a `metadata`-typed
/// input through `parseMetadataAsValue` -> `parseMetadata`, whose non-`!`
/// fall-through is `parseValueAsMetadata`, documented with exactly this
/// grammar:
///
/// ```text
/// /// parseValueAsMetadata
/// ///  ::= i32 %local
/// ///  ::= i32 @global
/// ///  ::= i32 7
/// ```
///
/// One input of each of the three spellings, plus the `!`-led forms in the
/// same bundle set to show the branch did not regress them.
#[test]
fn value_as_metadata_operand_bundle_inputs_round_trip() {
    let text = parse_and_render(
        "@g = external global i8\n\
declare void @callee()\n\
define void @f(i32 %a) {\n\
entry:\n\
  call void @callee() [ \"tag\"(metadata i32 %a, metadata i32 7, metadata ptr @g) ]\n\
  call void @callee() [ \"tag\"(metadata !0, metadata !\"abc\") ]\n\
  ret void\n\
}\n\
!0 = !{i32 1}\n",
    );
    assert_check_lines(
        &text,
        &[
            "call void @callee() [ \"tag\"(metadata i32 %a, metadata i32 7, metadata ptr @g) ]",
            "call void @callee() [ \"tag\"(metadata !0, metadata !\"abc\") ]",
        ],
    );
}

/// A `ValueAsMetadata` argument in a `cleanuppad` argument list.
///
/// **Anchored on the routine, not on a fixture**, for the same reason as
/// [`value_as_metadata_operand_bundle_inputs_round_trip`]. The rule is
/// `LLParser::parseExceptionArgs`, whose loop carries the same
/// `if (ArgTy->isMetadataTy()) { parseMetadataAsValue } else { parseValue }`
/// branch as `parseParameterList` and `parseOptionalOperandBundles`. Before
/// the branch existed here, `metadata !0` parsed (through `parseValID`'s own
/// metadata arms) and `metadata i32 %a` did not. The asserted printed form was
/// pinned from a run.
#[test]
fn value_as_metadata_pad_arguments_round_trip() {
    let text = parse_and_render(
        "declare i32 @__gxx_personality_v0(...)\n\
define void @f(i32 %a) personality ptr @__gxx_personality_v0 {\n\
entry:\n\
  ret void\n\
cleanup:\n\
  %cp = cleanuppad within none [metadata !0, metadata i32 %a]\n\
  ret void\n\
}\n\
!0 = !{i32 1}\n",
    );
    assert_check_lines(
        &text,
        &["%cp = cleanuppad within none [metadata !0, metadata i32 %a]"],
    );
}

/// `metadata metadata %x` — a metadata-typed inner type inside a
/// `metadata`-typed operand.
///
/// `LLParser::parseValueAsMetadata` rejects it before it ever calls
/// `parseValue`: `if (Ty->isMetadataTy()) return error(Loc, "invalid
/// metadata-value-metadata roundtrip");`, anchored at the inner type. llvmkit
/// carried that guard only on the `parseMDNodeVector` path
/// (`test/Assembler/invalid-metadata-attachment-has-type.ll`, pinned by
/// `parser_debug_metadata.rs`); this pins the `parseMetadataAsValue` path,
/// which the operand-bundle metadata branch newly reaches. No `.ll` file was
/// found spelling the operand form, so the rule is the anchor.
#[test]
fn metadata_value_metadata_roundtrip_in_an_operand_bundle_is_rejected() {
    let src = "declare void @callee()\n\
               define void @f(i32 %x) {\n\
               entry:\n\
                 call void @callee() [ \"tag\"(metadata metadata %x) ]\n\
                 ret void\n\
               }\n";
    assert_fixture_rejected(
        "metadata_value_metadata_roundtrip_bundle",
        src.as_bytes(),
        "invalid metadata-value-metadata roundtrip",
    );
}

/// llvmkit-specific subset of
/// `test/Transforms/PreISelIntrinsicLowering/protected-field-pointer.ll`
/// (the `NOPAUTH`-lowered call shape): the `"deactivation-symbol"` operand
/// bundle keeps upstream's tag spelling through a parse/print round trip.
/// The tag is registered as `LLVMContext::OB_deactivation_symbol` and
/// spelled by `knownBundleName` in `lib/IR/LLVMContext.cpp` — llvmkit
/// printed it as `"deactivation"` until this test.
#[test]
fn deactivation_symbol_bundle_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/deactivation-symbol/deactivation_symbol_bundle_round_trip.ll"
    );

    let text = parse_and_render_bytes("deactivation_symbol_bundle_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["call i64 @__emupac_autda(i64 %val, i64 1) [ \"deactivation-symbol\"(ptr @ds1) ]"],
    );
}

/// Assert rejection with upstream's message, **rendered** — see the note on
/// `parser_constants.rs::assert_parse_error` for why the comparison is
/// against `to_string()` and not a variant field.
fn assert_fixture_rejected(module_name: &str, src: &[u8], expected_message: &str) {
    let err = parse_fixture_err(module_name, src);
    assert_eq!(err.to_string(), expected_message);
}

/// Crafted against `llvm/lib/AsmParser/LLParser.cpp::parseCall`'s argument
/// loop ("argument is not of expected type"); LLVM 22.1.4 ships no lit or
/// unittest coverage for that diagnostic, so the rule is the anchor (D11).
/// The parser surfaces the same `validate_call_site_args` gate that
/// `builder_call.rs` locks at the builder level.
#[test]
fn call_explicit_type_arg_type_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_explicit_type_arg_type_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "call_explicit_type_arg_type_mismatch_rejected",
        FIXTURE,
        "argument is not of expected type 'i32'",
    );
}

/// Crafted against the same `parseCall` argument-loop rule as
/// [`call_explicit_type_arg_type_mismatch_rejected`]: the comparison is type
/// IDENTITY, so i8-vs-i32 is rejected, and the diagnostic spells the
/// concrete widths (`expected i32, got i8`) the way upstream's
/// "argument is not of expected type 'i32'" spells the full expected type.
#[test]
fn call_explicit_type_arg_width_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_explicit_type_arg_width_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "call_explicit_type_arg_width_mismatch_rejected",
        FIXTURE,
        "argument is not of expected type 'i32'",
    );
}

/// Crafted against `llvm/lib/AsmParser/LLParser.cpp::parseCall`'s post-loop
/// parameter check ("not enough parameters specified for call"); no upstream
/// lit or unittest coverage exists, so the rule is the anchor (D11).
#[test]
fn call_explicit_type_too_few_args_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_explicit_type_too_few_args_rejected.ll"
    );

    assert_fixture_rejected(
        "call_explicit_type_too_few_args_rejected",
        FIXTURE,
        "not enough parameters specified for call",
    );
}

/// Crafted against `llvm/lib/AsmParser/LLParser.cpp::parseCall`'s argument
/// loop non-vararg overflow arm ("too many arguments specified"); no
/// upstream lit or unittest coverage exists, so the rule is the anchor (D11).
#[test]
fn call_explicit_type_too_many_args_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_explicit_type_too_many_args_rejected.ll"
    );

    assert_fixture_rejected(
        "call_explicit_type_too_many_args_rejected",
        FIXTURE,
        "too many arguments specified",
    );
}

/// Crafted against `llvm/lib/AsmParser/LLParser.cpp::parseCall`'s post-loop
/// parameter check: a vararg callee still requires every fixed parameter
/// ("not enough parameters specified for call").
#[test]
fn call_vararg_missing_fixed_arg_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_vararg_missing_fixed_arg_rejected.ll"
    );

    assert_fixture_rejected(
        "call_vararg_missing_fixed_arg_rejected",
        FIXTURE,
        "not enough parameters specified for call",
    );
}

/// Positive guard for `parseCall`'s vararg arm: arguments past the fixed
/// parameters are legal, so the negative fixtures beside this one cannot
/// come from over-rejection. Printed form matches AsmWriter's explicit
/// vararg call-site type.
#[test]
fn call_vararg_extra_args_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_vararg_extra_args_round_trips.ll"
    );

    let text = parse_and_render_bytes("call_vararg_extra_args_round_trips", FIXTURE);
    assert_check_lines(&text, &["call void (i32, ...) @f(i32 1, i8 2, i32 3)"]);
}

/// Crafted against `llvm/lib/AsmParser/LLParser.cpp::parseCall`'s argument
/// loop, reached through an indirect (undef) callee so validation runs
/// against the explicit call-site function type alone —
/// `indirect_call_dyn`'s `validate_call_site_args` gate.
#[test]
fn indirect_call_arg_type_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_arg_type_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "indirect_call_arg_type_mismatch_rejected",
        FIXTURE,
        "argument is not of expected type 'i32'",
    );
}

/// Mirrors `LLParser::parseCall`: with no explicit call-site type, the
/// call type is inferred from the argument list and the callee resolves
/// as a bare pointer, so a direct call carries its own function type
/// independent of the declaration (`CallBase`). The call-vs-declaration
/// check belongs to the verifier, not the parser — so llvmkit parses and
/// re-prints it in AsmWriter's short form. (Genuinely malformed calls —
/// args not matching the call-site type itself — are still rejected; see
/// the `*_arg_type_mismatch_rejected` and `*_too_{few,many}_args_rejected`
/// locks below. Return-type drift between two forward *declarations* of the
/// same symbol also still errors: `parser_forward_refs.rs::
/// forward_global_reference_signature_mismatch_is_rejected`.)
#[test]
fn call_inferred_signature_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_inferred_signature_round_trips.ll"
    );

    let text = parse_and_render_bytes("call_inferred_signature_round_trips", FIXTURE);
    assert_check_lines(&text, &["call void @f(i32 1)"]);
}

/// Invoke form of [`call_inferred_signature_round_trips`]: `parseInvoke`
/// infers the call-site type from the argument list and the callee
/// resolves as a bare pointer, so the mismatched-declaration invoke parses
/// and re-prints in short form.
#[test]
fn invoke_inferred_signature_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/invoke_inferred_signature_round_trips.ll"
    );

    let text = parse_and_render_bytes("invoke_inferred_signature_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "invoke void @f(float 0.000000e+00)",
            "to label %ok unwind label %lp",
        ],
    );
}

/// Callbr form of [`call_inferred_signature_round_trips`]: `parseCallBr`
/// infers the call-site type from the argument list and the callee
/// resolves as a bare pointer, so the mismatched-declaration callbr parses
/// and re-prints in short form.
#[test]
fn callbr_inferred_signature_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/callbr_inferred_signature_round_trips.ll"
    );

    let text = parse_and_render_bytes("callbr_inferred_signature_round_trips", FIXTURE);
    assert_check_lines(&text, &["callbr void @f(float 0.000000e+00)"]);
}

/// Mirrors `LLParser::parseCall`'s explicit-type branch: an explicitly
/// written call-site type IS the call's function type, independent of the
/// callee's declaration. `call i32 (i32) @f(...)` through a `void (float)`
/// declaration parses (callee resolved as a bare pointer) and re-prints in
/// AsmWriter's short form, `call i32 @f(i32 1)`.
#[test]
fn call_explicit_type_signature_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_explicit_type_signature_round_trips.ll"
    );

    let text = parse_and_render_bytes("call_explicit_type_signature_round_trips", FIXTURE);
    assert_check_lines(&text, &["%r = call i32 @f(i32 1)"]);
}

/// Mirrors `test/Feature/indirectcall.ll`'s `call i64 %fibfunc(...)`: a
/// callee may be any pointer-typed value, parsed through `parseCall`'s
/// `parseValID` + `convertValIDToValue(PointerType)` path. Non-vararg
/// indirect calls print in AsmWriter's short form.
#[test]
fn indirect_call_local_fn_ptr_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_local_fn_ptr_round_trips.ll"
    );

    let text = parse_and_render_bytes("indirect_call_local_fn_ptr_round_trips", FIXTURE);
    assert_check_lines(&text, &["call void %fp(i32 1)", "ret void"]);
}

/// Mirrors `test/Assembler/call-arg-is-callee.ll` `@call`: an explicit
/// vararg call-site type through a local function pointer exercises
/// `resolveFunctionType`'s FunctionType branch together with the
/// indirect-callee path; vararg call sites keep the long-form type.
#[test]
fn indirect_call_vararg_fn_ptr_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_vararg_fn_ptr_round_trips.ll"
    );

    let text = parse_and_render_bytes("indirect_call_vararg_fn_ptr_round_trips", FIXTURE);
    assert_check_lines(&text, &["call void (i32, ...) %fp(i32 1, i8 2)"]);
}

/// Crafted against `convertValIDToValue`'s `t_Null` arm with a pointer
/// target type: `null` is a legal (if degenerate) callee upstream; no
/// upstream lit coverage of the spelling, rule shape is the anchor (D11).
#[test]
fn indirect_call_null_callee_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_null_callee_round_trips.ll"
    );

    let text = parse_and_render_bytes("indirect_call_null_callee_round_trips", FIXTURE);
    assert_check_lines(&text, &["call void null()"]);
}

/// Positive guard for the retired dedicated `undef`-callee arm: `undef`
/// callees ride the generic value path (`convertValIDToValue` `t_Undef`)
/// and must keep parsing after the special case's removal.
#[test]
fn indirect_call_undef_callee_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_undef_callee_round_trips.ll"
    );

    let text = parse_and_render_bytes("indirect_call_undef_callee_round_trips", FIXTURE);
    assert_check_lines(&text, &["call void undef()"]);
}

/// Mirrors `LLParser::PerFunctionState::getVal`'s type check at the callee
/// position: a non-pointer local cannot be a callee, because
/// `convertValIDToValue` asks `getVal` for the name at pointer type and
/// `checkValidVariableType` refuses. No upstream lit coverage of the
/// diagnostic, so the rule shape is the anchor (D11).
#[test]
fn indirect_call_non_pointer_callee_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_non_pointer_callee_rejected.ll"
    );

    assert_fixture_rejected(
        "indirect_call_non_pointer_callee_rejected",
        FIXTURE,
        "'%x' defined with type 'i32' but expected 'ptr'",
    );
}

/// llvmkit-specific GAP lock: upstream `test/Assembler/call-arg-is-callee.ll`
/// Mirrors `test/Assembler/call-arg-is-callee.ll`'s `@invoke`: an invoke
/// through a function-pointer value. `parseInvoke` shares `parseCall`'s
/// callee path (`convertValIDToValue(PointerType)`), and an indirect invoke
/// is valid IR. The varargs call-site type re-prints in full.
#[test]
fn invoke_indirect_callee_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/invoke_indirect_callee_round_trips.ll"
    );

    let text = parse_and_render_bytes("invoke_indirect_callee_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "invoke void (...) %p(ptr %p)",
            "to label %ok unwind label %lp",
        ],
    );
}

/// A non-inline-asm callbr with an indirect callee is invalid IR upstream —
/// `Verifier::visitCallBrInst` requires a direct callee ("Callbr: indirect
/// function / invalid signature"). llvmkit rejects it at parse time, which
/// reaches the same overall verdict (the module is rejected either way).
#[test]
fn callbr_indirect_callee_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/callbr_indirect_callee_rejected.ll");

    assert_fixture_rejected(
        "callbr_indirect_callee_rejected",
        FIXTURE,
        "expected direct function callee for callbr",
    );
}

/// Mirrors `parseInvoke`'s use of `resolveFunctionType`: a written
/// FunctionType IS the call-site type (no inference from the argument
/// list); the non-vararg invoke prints back in short form.
#[test]
fn invoke_explicit_type_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/invoke_explicit_type_round_trips.ll");

    let text = parse_and_render_bytes("invoke_explicit_type_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["invoke void @f(i32 1)", "to label %ok unwind label %lp"],
    );
}

/// Crafted against `resolveFunctionType`'s FunctionType branch reached
/// from `parseInvoke`: vararg invokes are only expressible through the
/// explicit call-site type (upstream shape: the vararg statepoint invoke
/// in `test/Assembler/opaque-ptr-intrinsic-remangling.ll`).
#[test]
fn invoke_explicit_type_vararg_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/invoke_explicit_type_vararg_round_trips.ll"
    );

    let text = parse_and_render_bytes("invoke_explicit_type_vararg_round_trips", FIXTURE);
    assert_check_lines(&text, &["invoke void (ptr, ...) @vf(ptr %p, i32 7)"]);
}

/// Crafted against `resolveFunctionType`'s FunctionType branch reached
/// from `parseCallBr`; no upstream lit coverage of the explicit spelling
/// on callbr, rule shape is the anchor (D11).
#[test]
fn callbr_explicit_type_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/callbr_explicit_type_round_trips.ll");

    let text = parse_and_render_bytes("callbr_explicit_type_round_trips", FIXTURE);
    assert_check_lines(&text, &["callbr void @g(i32 1)", "to label %cont []"]);
}

/// Vararg form of [`callbr_explicit_type_round_trips`]: only expressible
/// through the explicit type, printed back in long form. Parse-level
/// mirror; upstream's verifier additionally restricts non-asm callbr to
/// direct intrinsic callees.
#[test]
fn callbr_explicit_type_vararg_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/callbr_explicit_type_vararg_round_trips.ll"
    );

    let text = parse_and_render_bytes("callbr_explicit_type_vararg_round_trips", FIXTURE);
    assert_check_lines(&text, &["callbr void (i32, ...) @g(i32 1, i8 2)"]);
}

/// Crafted against `parseInvoke`'s argument loop ("argument is not of
/// expected type") with an explicit call-site type; no upstream lit or
/// unittest coverage, rule shape is the anchor (D11). llvmkit routes the
/// check through `validate_call_site_args` in
/// `invoke_dyn_with_config`.
#[test]
fn invoke_explicit_type_arg_type_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/invoke_explicit_type_arg_type_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "invoke_explicit_type_arg_type_mismatch_rejected",
        FIXTURE,
        "argument is not of expected type 'i32'",
    );
}

/// Crafted against `parseCallBr`'s argument loop with an explicit
/// call-site type — same rule as
/// [`invoke_explicit_type_arg_type_mismatch_rejected`], surfaced through
/// `callbr_with_config`.
#[test]
fn callbr_explicit_type_arg_type_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/callbr_explicit_type_arg_type_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "callbr_explicit_type_arg_type_mismatch_rejected",
        FIXTURE,
        "argument is not of expected type 'i32'",
    );
}

/// Mirrors `LLParser::parseInvoke`'s `resolveFunctionType`: an explicitly
/// written call-site type IS the invoke's function type, independent of the
/// declaration. `invoke void (i8) @f(...)` through a `void (i32)`
/// declaration parses (callee resolved as a bare pointer) and re-prints in
/// AsmWriter's short form.
#[test]
fn invoke_explicit_type_signature_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/invoke_explicit_type_signature_round_trips.ll"
    );

    let text = parse_and_render_bytes("invoke_explicit_type_signature_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["invoke void @f(i8 1)", "to label %ok unwind label %lp"],
    );
}

/// `LLParser::parseOptionalOperandBundles` checks emptiness *before* it eats
/// the `]`, and reports at the `[` — so an absent bundle set is fine while a
/// written-but-empty one is an error. llvmkit accepted `[]`.
///
/// No `test/Assembler` fixture pins it, so the routine is the anchor.
#[test]
fn an_empty_operand_bundle_set_is_rejected() {
    let src = "declare void @g()\n\
               define void @f() {\nentry:\n  call void @g() []\n  ret void\n}\n";
    assert_fixture_rejected(
        "empty_operand_bundle_set",
        src.as_bytes(),
        "operand bundle set must not be empty",
    );

    // Absent is fine.
    parse_and_render(
        "declare void @g()\ndefine void @f() {\nentry:\n  call void @g()\n  ret void\n}\n",
    );
}

/// `parseCallBr` ends its `||` chain with `parseToken(lltok::lsquare,
/// "expected '[' in callbr")`, so the indirect-destination list is
/// **mandatory** and no comma precedes it. llvmkit made the whole list
/// optional and tolerated a leading comma, accepting both
/// `callbr void @g() to label %x` and `... to label %x, [...]`.
///
/// An *empty* list is still legal — upstream only requires the brackets.
#[test]
fn a_callbr_indirect_destination_list_is_mandatory() {
    for src in [
        "declare void @g()\n\
         define void @f() {\nentry:\n  callbr void @g() to label %ok\nok:\n  ret void\n}\n",
        "declare void @g()\n\
         define void @f() {\nentry:\n  callbr void @g() to label %ok, [label %ok]\nok:\n  ret void\n}\n",
    ] {
        assert_fixture_rejected(
            "callbr_missing_bracket",
            src.as_bytes(),
            "expected '[' in callbr",
        );
    }

    // The empty-bracket form is what upstream requires, and it round-trips.
    let text = parse_and_render(
        "declare void @g()\n\
         define void @f() {\nentry:\n  callbr void @g() to label %ok []\nok:\n  ret void\n}\n",
    );
    assert_check_lines(&text, &["callbr void @g()", "to label %ok []"]);
}
