//! Call / invoke / callbr parser tests.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

pub mod support;

// `canonicalize_horizontal_whitespace` is `FileCheck::CanonicalizeFile`
// (`llvm/lib/FileCheck/FileCheck.cpp`), which is what lets
// `test/Bitcode/operand-bundles.ll`'s `CHECK` text be quoted as written:
// several of its lines spell `float  0.000000e+00` with two spaces, because
// that is how the fixture's *input* spells it, and canonicalization is why
// they still match `llvm-dis`'s single space. That fixture's `RUN` line does
// not pass `--strict-whitespace`, so upstream canonicalizes there too.
use support::{Check, canonicalize_horizontal_whitespace, check_directives};

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

/// `llvm-as < %s | llvm-dis` — both halves. `llvm-as.cpp`'s `main` guards a
/// `verifyModule` call on `if (!DisableVerify)` and exits 1 with
/// `assembly parsed, but does not verify as correct!` when it reports, so a
/// `RUN` line piping through `llvm-as` asserts verification as well as
/// parse-and-print; [`parse_and_render_bytes`] drops that half.
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
/// upstream's text verbatim.
///
/// The fixtures here are llvmkit's minimal shapes; the *upstream* fixture that
/// pins both messages is `test/Verifier/callbr.ll`, ported whole by
/// [`upstream_callbr_label_constraint_fixture_messages_match`] below. This test
/// used to say no upstream fixture reached these two rules, which was a claim
/// about the splits of `inline-asm-constraint-error.ll` only and read as a
/// claim about the tree.
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

/// `llvm/test/Verifier/callbr.ll`, vendored verbatim; its six inline-asm
/// functions cut out and verified one at a time, each against the `CHECK` line
/// the fixture writes for it.
///
/// Upstream's `RUN` line is `not opt -S %s -passes=verify`, so the whole file
/// runs through `Verifier` and its diagnostics are `Check` literals. It is
/// per-function here for the reason
/// [`upstream_musttail_invalid_fixture_messages_match`] is: `verify_borrowed`
/// reports the first failure where upstream's `Verifier` accumulates.
///
/// **The four `llvm.callbr.landingpad` functions of this fixture are not
/// ported.** They belong to `Verifier::visitIntrinsicCall`'s
/// `Intrinsic::callbr_landingpad` arm — `Intrinsic in block must have 1 unique
/// predecessor`, `Intrinsic's corresponding callbr must have intrinsic's
/// parent basic block in indirect destination list`, `No other instructions
/// may proceed intrinsic` — a routine `check_intrinsic_call` does not carry.
/// See `docs/divergences.md`.
#[test]
fn upstream_callbr_label_constraint_fixture_messages_match() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/callbr.ll");
    let cases = [
        (
            "define void @too_few_label_constraints(",
            Some("Number of label constraints does not match number of callbr dests"),
        ),
        ("define void @correct_label_constraints(", None),
        (
            "define void @too_many_label_constraints(",
            Some("Number of label constraints does not match number of callbr dests"),
        ),
        (
            "define void @label_constraint_without_callbr(",
            Some("Label constraints can only be used with callbr"),
        ),
        (
            "define void @callbr_without_label_constraint(",
            Some("Number of label constraints does not match number of callbr dests"),
        ),
        // `;; Ensure you can use the return value of a callbr in indirect
        // targets. No issue!`
        ("define i32 @test4(", None),
    ];
    for (marker, expected) in cases {
        assert_fixture_case_verifies(FIXTURE, "", marker, expected);
    }
}

/// `llvm/test/Verifier/callbr-intrinsic.ll`, vendored verbatim; each of its six
/// functions cut out with the `declare` it needs and verified on its own.
///
/// This is `Verifier::visitCallBrInst`'s whole non-inline-asm arm: the
/// `getCalledFunction` `Check`, the operand-bundle `Check`, the
/// `Intrinsic::amdgcn_kill` case's two `Check`s, and the `default:`
/// `CheckFailed`.
///
/// `@test_callbr_intrinsic_wrong_signature` reaches the same verdict from the
/// **parser**, not the verifier: `parse_callbr` rejects an indirect callee
/// outright because the callbr builder has no indirect-callee form. That is
/// `docs/divergences.md` entry 27, which the verifier half of this port does
/// not close — the `Callbr: indirect function / invalid signature` `Check`
/// now exists in `check_callbr` but nothing in llvmkit can build a `callbr`
/// that reaches it.
#[test]
fn upstream_callbr_intrinsic_fixture_messages_match() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/callbr-intrinsic.ll");
    const KILL: &str = "declare void @llvm.amdgcn.kill(i1)\n";
    const WORKITEM: &str = "declare i32 @llvm.amdgcn.workitem.id.x()\n";
    let cases = [
        (
            KILL,
            "define void @test_callbr_intrinsic_indirect0(",
            "Callbr amdgcn_kill only supports one indirect dest",
        ),
        (
            KILL,
            "define void @test_callbr_intrinsic_indirect2(",
            "Callbr amdgcn_kill only supports one indirect dest",
        ),
        (
            KILL,
            "define void @test_callbr_intrinsic_no_unreachable(",
            "Callbr amdgcn_kill indirect dest needs to be unreachable",
        ),
        (
            WORKITEM,
            "define void @test_callbr_intrinsic_unsupported(",
            "Callbr currently only supports asm-goto and selected intrinsics",
        ),
        (
            KILL,
            "define void @test_callbr_intrinsic_no_operand_bundles(",
            "Callbr for intrinsics currently doesn't support operand bundles",
        ),
    ];
    for (prelude, marker, expected) in cases {
        assert_fixture_case_verifies(FIXTURE, prelude, marker, Some(expected));
    }

    // `@test_callbr_intrinsic_wrong_signature` — rejected by the parser here,
    // by the verifier upstream. Entry 27.
    let source = fixture_define(
        FIXTURE,
        "define void @test_callbr_intrinsic_wrong_signature(",
    );
    let module = Module::dynamic("test_callbr_intrinsic_wrong_signature");
    let err = Parser::new(source.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("llvmkit rejects an indirect callbr at parse time — entry 27");
    assert!(
        err.to_string()
            .contains("expected direct function callee for callbr"),
        "{err}"
    );
}

/// `llvm/test/Verifier/swifterror.ll`, vendored verbatim; its four `define`s
/// cut out and verified one at a time.
///
/// The rules are `Verifier::verifySwiftErrorValue` (reached from
/// `visitFunction`'s argument loop for `@foo` and from `visitAllocaInst` for
/// the rest), `Verifier::verifySwiftErrorCall`, and the `swifterror` loop of
/// `Verifier::visitCallBase`.
///
/// **The fixture's last two lines are `declare`s and are not ported.**
/// `Cannot have multiple 'swifterror' parameters!` is a
/// `Verifier::verifyFunctionAttrs` `Check` and `Attribute 'swifterror'
/// applied to incompatible type!` is one of `Verifier::verifyParameterAttrs`,
/// which that routine calls. Neither routine has a counterpart here —
/// `verifier.rs`'s module header says so ("Per-function attribute coherence
/// rules … are out of scope"). See `docs/divergences.md`.
#[test]
fn upstream_swifterror_fixture_messages_match() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/swifterror.ll");
    let cases = [
        (
            "",
            "define float @foo(",
            "swifterror value can only be loaded and stored from, or as a swifterror argument!",
        ),
        (
            "declare float @foo(ptr swifterror)\n",
            "define float @caller(",
            "swifterror argument for call has mismatched alloca",
        ),
        (
            "",
            "define void @swifterror_alloca_invalid_type(",
            "swifterror alloca must have pointer type",
        ),
        (
            "",
            "define void @swifterror_alloca_array(",
            "swifterror alloca must not be array allocation",
        ),
    ];
    for (prelude, marker, expected) in cases {
        assert_fixture_case_verifies(FIXTURE, prelude, marker, Some(expected));
    }
}

/// Parse `prelude` plus the `define` beginning at `marker`, verify it, and
/// assert either that it verifies clean (`expected` is `None`) or that the
/// failure carries `expected`.
fn assert_fixture_case_verifies(
    fixture: &str,
    prelude: &str,
    marker: &str,
    expected: Option<&str>,
) {
    let source = format!("{prelude}{}", fixture_define(fixture, marker));
    let module = Module::dynamic("fixture_case");
    Parser::new(source.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .unwrap_or_else(|e| panic!("case {marker} parses: {e}"));
    match expected {
        None => module
            .verify_borrowed()
            .unwrap_or_else(|e| panic!("case {marker} carries no CHECK line, so it verifies: {e}")),
        Some(expected) => {
            let err = module
                .verify_borrowed()
                .expect_err("upstream's `RUN` line rejects this module");
            let llvmkit_ir::IrError::VerifierFailure { message, .. } = err else {
                panic!("case {marker}: expected a verifier failure, got {err:?}");
            };
            assert!(
                message.contains(expected),
                "case {marker}: {message:?} does not contain {expected:?}"
            );
        }
    }
}

/// The `define` beginning at `marker`, through its closing brace.
fn fixture_define(fixture: &str, marker: &str) -> String {
    let start = fixture
        .find(marker)
        .unwrap_or_else(|| panic!("missing define marker {marker}"));
    let end = fixture[start..]
        .find("\n}")
        .map(|idx| start + idx + 3)
        .unwrap_or_else(|| panic!("missing define end for {marker}"));
    fixture[start..end].to_owned()
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

/// The **unnamed**-local spelling of
/// [`value_as_metadata_operand_bundle_inputs_round_trip`], which uses `%a`
/// throughout and therefore could not see the printer defect below.
///
/// **Anchored on the routine, not on a fixture**, for the same reason that
/// test gives. The rule is `AsmWriter.cpp`'s
/// `writeAsOperandInternal(raw_ostream &, const Metadata *, AsmWriterContext &,
/// bool)`, whose `ValueAsMetadata` tail is
/// `writeAsOperandInternal(Out, V->getValue(), WriterCtx, /*PrintType=*/true)`
/// — the *same* `AsmWriterContext`, so the same `Machine` numbers `%0` inside
/// the bundle as numbers it outside. llvmkit's metadata sub-printer took no
/// `SlotTracker` and printed the no-slot spelling here, which then failed to
/// re-parse. Both halves are asserted: the bytes, and that they re-parse to the
/// same bytes.
#[test]
fn value_as_metadata_operand_bundle_numbers_an_unnamed_local() {
    let text = parse_and_render(
        "declare void @callee()\n\
define void @f(i32 %a) {\n\
entry:\n\
  %0 = add i32 %a, 1\n\
  call void @callee() [ \"tag\"(metadata i32 %0) ]\n\
  ret void\n\
}\n",
    );
    check_directives(
        &text,
        &[
            Check::Line("%0 = add i32 %a, 1"),
            Check::Next("call void @callee() [ \"tag\"(metadata i32 %0) ]"),
        ],
    );
    assert!(!text.contains("<badref>"), "{text}");
    assert_eq!(
        parse_and_render(&text),
        text,
        "printed module is not round-trip stable"
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

/// `parseValueAsMetadata`'s `TypeMsg` reaches the output in exactly one case,
/// and this pins where the line is drawn.
///
/// `LLParser::parseValueAsMetadata` passes `"expected metadata operand"` to
/// `LLParser::parseType(Type *&Result, const Twine &Msg, bool AllowVoid)`,
/// which reads `Msg` only in the `default:` arm of its leading
/// `switch (Lex.getKind())`. Every later arm, and every nested type routine it
/// calls, raises its own text at its own token. So a `metadata` operand whose
/// type is malformed must report the *type's* complaint, not the operand's —
/// and only a token that begins no type at all gets `expected metadata
/// operand`.
///
/// **Anchored on that policy, not on a fixture.** Upstream pins these message
/// texts elsewhere (`test/Assembler/invalid-opaque-ptr.ll` for `ptr*`), but no
/// `.ll` file was found that reaches them through a `metadata` operand, which
/// is the position this task made reachable.
#[test]
fn a_malformed_metadata_operand_type_keeps_the_type_s_own_message() {
    // (bundle input spelling, expected message)
    const CASES: &[(&str, &str)] = &[
        // `parseType`'s `default:` arm — the one place `TypeMsg` is read.
        ("metadata , ", "expected metadata operand"),
        // `parseStructBody`'s nested `parseType`, on the trailing comma.
        ("metadata { i32, } %x", "expected type"),
        // `parseType`'s suffix loop, `if (!AllowVoid && Result->isVoidTy())`.
        (
            "metadata void %x",
            "void type only allowed for function results",
        ),
        // The `lltok::Type` arm's own `ptr*` guard.
        ("metadata ptr* %x", "ptr* is invalid - use ptr instead"),
        // The suffix loop's `lltok::star` arm, `Result->isLabelTy()`.
        ("metadata label* %x", "basic block pointers are invalid"),
    ];

    for (input, expected) in CASES {
        let src = format!(
            "declare void @callee()\n\
             define void @f(i32 %x) {{\n\
             entry:\n\
               call void @callee() [ \"tag\"({input}) ]\n\
               ret void\n\
             }}\n"
        );
        assert_fixture_rejected("malformed_metadata_operand_type", src.as_bytes(), expected);
    }
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

/// **No upstream counterpart** — no fixture under `test/Assembler`,
/// `test/Verifier` or `test/Feature` pins this diagnostic
/// (`grep -rl "notail call'"` over those three directories of the vendored
/// `llvmorg-22.1.4` tree returns nothing), so the rule is the anchor (D11):
/// `LLParser::parseCall`'s first guard,
/// `if (TCK != CallInst::TCK_None && parseToken(lltok::kw_call, "expected
/// 'tail call', 'musttail call', or 'notail call'")) return true;`. Upstream
/// reaches `parseCall` from `parseInstruction`'s `kw_tail` / `kw_musttail` /
/// `kw_notail` arms with only the tail keyword eaten, so the `call` that
/// follows is mandatory.
#[test]
fn tail_keyword_without_call_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/tail_keyword_without_call_rejected.ll"
    );

    assert_fixture_rejected(
        "tail_keyword_without_call_rejected",
        FIXTURE,
        "expected 'tail call', 'musttail call', or 'notail call'",
    );
}

/// `test/Assembler/callee-type-metadata.ll`, asserting its own `; CHECK:` line.
/// Upstream's `RUN` line is `llvm-as < %s | llvm-dis | FileCheck %s`, so that
/// line is `AsmWriter` output byte for byte, and the `llvm-as` half verifies.
///
/// The fixture is already in `parser_corpus_manifest.txt`, but `parser_corpus.rs`
/// asserts parse / verify / print-reparse-print stability and never reads a
/// fixture's `CHECK` lines — which is how the dropped `signext` survived a
/// `status=pass` row. This test is the missing half.
#[test]
fn indirect_call_parameter_attribute_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/callee-type-metadata.ll");

    let text =
        parse_verify_and_render_bytes("indirect_call_parameter_attribute_round_trips", FIXTURE);
    check_directives(
        &text,
        &[Check::Line(
            "%call = call i32 %fptr(i8 signext %x_val), !callee_type !1",
        )],
    );
}

/// `test/Verifier/kcfi-operand-bundles.ll`, verbatim (the whole file). Every
/// call in it is indirect and every one carries a `"kcfi"` operand bundle.
///
/// Upstream's `RUN` line is `not opt -passes=verify < %s 2>&1 | FileCheck %s`.
/// This is its printed half: each `CHECK-NEXT` line is the offending
/// instruction as `AsmWriter` prints it, asserted as a `Check::Line` because
/// the diagnostic the `-NEXT` counts from is not printed here. The verdict half
/// is [`upstream_kcfi_operand_bundle_fixture_messages_match`].
#[test]
fn indirect_call_kcfi_operand_bundles_round_trip() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/kcfi-operand-bundles.ll");

    let text = parse_and_render_bytes("indirect_call_kcfi_operand_bundles_round_trip", FIXTURE);
    check_directives(
        &text,
        &[
            Check::Line(r#"call void %arg2() [ "kcfi"(i32 42), "kcfi"(i32 42) ]"#),
            Check::Line(r#"call void %arg2() [ "kcfi"(i64 42) ]"#),
            Check::Line(r#"call void %arg2() [ "kcfi"(i32 42) ]"#),
            Check::Line(r#"call void %arg2() [ "kcfi"(i32 42) ]"#),
        ],
    );
}

/// `test/Verifier/ptrauth-operand-bundles.ll`, verbatim (the whole file). Five
/// indirect calls with `"ptrauth"` bundles plus one **direct** call with the
/// same bundle — the direct/indirect contrast in one fixture, which is why it
/// is ported alongside the kcfi one rather than instead of it.
///
/// Upstream's `RUN` line is `not opt -passes=verify < %s 2>&1 | FileCheck %s`,
/// so as with [`indirect_call_kcfi_operand_bundles_round_trip`] this is the
/// parse/print half; the `CHECK-NEXT` lines are `AsmWriter` output of the
/// offending instruction, asserted here as `Check::Line`. The verdict half is
/// [`upstream_ptrauth_operand_bundle_fixture_messages_match`].
#[test]
fn indirect_call_ptrauth_operand_bundles_round_trip() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/ptrauth-operand-bundles.ll");

    let text = parse_and_render_bytes("indirect_call_ptrauth_operand_bundles_round_trip", FIXTURE);
    check_directives(
        &text,
        &[
            Check::Line(
                r#"call void %arg2() [ "ptrauth"(i32 42, i64 100), "ptrauth"(i32 42, i64 %arg0) ]"#,
            ),
            Check::Line(r#"call void %arg2() [ "ptrauth"(i32 %arg1, i64 120) ]"#),
            Check::Line(r#"call void %arg2() [ "ptrauth"(i64 42, i64 120) ]"#),
            Check::Line(r#"call void %arg2() [ "ptrauth"(i32 42, i32 120) ]"#),
            Check::Line(r#"call void @g() [ "ptrauth"(i32 42, i64 120) ]"#),
            Check::Line(r#"call void %arg2() [ "ptrauth"(i32 42, i64 120) ]"#),
            Check::Line(r#"call void %arg2() [ "ptrauth"(i32 42, i64 %arg0) ]"#),
        ],
    );
}

/// The `(line, column)` a diagnostic points at, computed the way
/// `parse_file.rs` computes it for its `<path>:LINE:COL:` prefix — the same
/// coordinates `llvm-as` prints as `<stdin>:LINE:COL:`.
fn error_line_col(src: &str, err: &ParseError) -> (u32, u32) {
    let span = err.loc().expect("diagnostic carries a location").span;
    llvmkit_support::SourceMap::new(src.as_bytes()).line_col(span.start)
}

/// The source text the diagnostic's span opens on, for asserting *which token*
/// an anchor landed on rather than only where it landed.
fn error_token<'a>(src: &'a str, err: &ParseError) -> &'a str {
    let span = err.loc().expect("diagnostic carries a location").span;
    let start = usize::try_from(span.start).expect("offset fits usize");
    let end = usize::try_from(span.end).expect("offset fits usize");
    &src[start..end.min(src.len())]
}

/// **Anchor lock, llvmkit-authored source; no upstream counterpart.**
/// `LLParser::parseCall` raises this one as `error(CallLoc, …)`, not
/// `tokError`, so it must point at `CallLoc` and not at whatever token the
/// parser has reached by the time the return type is known — which, because
/// the guard runs after the whole call has been consumed, is the *next*
/// instruction's first token.
///
/// `CallLoc` is `parseCall`'s `Lex.getLoc()` taken before
/// `EatFastMathFlagsIfPresent()`, and `parseInstruction` has already eaten the
/// `call` keyword, so for a plain `call` it is the first fast-math keyword.
/// The message is `LLParser.cpp`'s, verbatim; no `.ll` under `test/Assembler`,
/// `test/Verifier` or `test/Feature` pins it (`grep -rl "fast-math-flags
/// specified for call"` over those three directories of the vendored
/// `llvmorg-22.1.4` tree returns nothing), so the rule is the anchor (D11).
#[test]
fn fast_math_flags_on_a_non_fp_call_report_at_call_loc() {
    const SRC: &str = "declare void @g(i32)\n\
                       define void @f() {\n  \
                       call nnan void @g(i32 5)\n  \
                       ret void\n\
                       }\n";

    let err = parse_fixture_err(
        "fast_math_flags_on_a_non_fp_call_report_at_call_loc",
        SRC.as_bytes(),
    );
    assert_eq!(
        err.to_string(),
        "fast-math-flags specified for call without floating-point scalar or vector return type"
    );
    assert_eq!(error_line_col(SRC, &err), (3, 8));
    assert_eq!(error_token(SRC, &err), "nnan");
}

/// **Anchor lock, llvmkit-authored source; no upstream counterpart.**
/// `LLParser::parseInstruction` eats one keyword before dispatching, so
/// `parseCall`'s `LocTy CallLoc = Lex.getLoc()` — its last statement before
/// `parseToken(lltok::kw_call, …)` — lands on a *different* token in the two
/// spellings: the token after `call` for a plain call, and the `call` keyword
/// itself after `tail` / `musttail` / `notail`. `error(CallLoc, "not enough
/// parameters specified for call")` is the cheapest of the three diagnostics
/// anchored on it to reach, so it is the one this pins; the same anchor serves
/// the fast-math guard (above) and the `llvm.dbg` guard.
///
/// Both spellings are asserted together because the law is the *relation*
/// between them; a lock on either alone would survive `CallLoc` drifting back
/// to a single uniform token. `musttail` rather than `tail` so the two columns
/// differ.
#[test]
fn call_loc_anchors_at_the_call_keyword_only_for_a_tail_call() {
    const PLAIN: &str = "declare void @g(i32, i32, i32)\n\
                         define void @f() {\n  \
                         call void (i32, i32, i32) @g(i32 1, i32 2)\n  \
                         ret void\n\
                         }\n";
    const MUSTTAIL: &str = "declare void @g(i32, i32, i32)\n\
                            define void @f() {\n  \
                            musttail call void (i32, i32, i32) @g(i32 1, i32 2)\n  \
                            ret void\n\
                            }\n";

    let plain = parse_fixture_err("call_loc_plain", PLAIN.as_bytes());
    assert_eq!(
        plain.to_string(),
        "not enough parameters specified for call"
    );
    assert_eq!(error_line_col(PLAIN, &plain), (3, 8));
    assert_eq!(error_token(PLAIN, &plain), "void");

    let musttail = parse_fixture_err("call_loc_musttail", MUSTTAIL.as_bytes());
    assert_eq!(
        musttail.to_string(),
        "not enough parameters specified for call"
    );
    assert_eq!(error_line_col(MUSTTAIL, &musttail), (3, 12));
    assert_eq!(error_token(MUSTTAIL, &musttail), "call");
}

/// One `test/Verifier` operand-bundle fixture cut into the pieces its `RUN`
/// line pins, all taken from the fixture text rather than retyped.
struct OperandBundleFixture<'a> {
    /// Everything above the `define` — the `declare`s the body refers to.
    preamble: Vec<&'a str>,
    /// The `define` header line.
    define: &'a str,
    /// The terminator and closing brace, kept verbatim so a rebuilt module
    /// ends the way the fixture does.
    tail: Vec<&'a str>,
    /// One `(diagnostic, offending instruction)` per `; CHECK:` directive: the
    /// text FileCheck matches, and the body line that follows it.
    cases: Vec<(&'a str, &'a str)>,
    /// The instructions under `; CHECK-NOT:` — the ones upstream reports
    /// nothing for.
    clean: Vec<&'a str>,
}

/// Split an operand-bundle `test/Verifier` fixture the way its own directives
/// do. Every body line must be claimed by a `; CHECK:` or by the `; CHECK-NOT:`
/// tail; a line that is neither panics rather than being dropped, so a fixture
/// growing a case cannot silently go unasserted.
fn operand_bundle_fixture(fixture: &str) -> OperandBundleFixture<'_> {
    let mut parsed = OperandBundleFixture {
        preamble: Vec::new(),
        define: "",
        tail: Vec::new(),
        cases: Vec::new(),
        clean: Vec::new(),
    };
    let mut pending: Option<&str> = None;
    let mut after_check_not = false;
    for line in fixture.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("; CHECK-NOT:") {
            after_check_not = true;
            continue;
        }
        if trimmed.starts_with("; CHECK-NEXT:") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("; CHECK:") {
            pending = Some(rest.trim());
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with("define ") {
            parsed.define = line;
            continue;
        }
        if parsed.define.is_empty() {
            parsed.preamble.push(line);
            continue;
        }
        if trimmed.starts_with("ret ") || trimmed == "}" {
            parsed.tail.push(line);
            continue;
        }
        match pending.take() {
            Some(expected) => parsed.cases.push((expected, line)),
            None if after_check_not => parsed.clean.push(line),
            None => panic!("fixture body line is pinned by no directive: {line:?}"),
        }
    }
    assert!(!parsed.define.is_empty(), "fixture has no define");
    assert!(!parsed.cases.is_empty(), "fixture has no CHECK cases");
    assert!(!parsed.clean.is_empty(), "fixture has no CHECK-NOT tail");
    parsed
}

/// The fixture's preamble and `define` header wrapped around `body`.
fn operand_bundle_module(fixture: &OperandBundleFixture<'_>, body: &[&str]) -> String {
    let mut out = String::new();
    for line in &fixture.preamble {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(fixture.define);
    out.push('\n');
    for line in body {
        out.push_str(line);
        out.push('\n');
    }
    for line in &fixture.tail {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The verifier's answer for one rebuilt module, `Ok` or the failure message.
fn verify_operand_bundle_module(name: &str, source: &str) -> Result<(), String> {
    let module = Module::dynamic(name);
    Parser::new(source.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .unwrap_or_else(|e| panic!("{name} parses: {e}\n{source}"));
    match module.verify_borrowed() {
        Ok(()) => Ok(()),
        Err(llvmkit_ir::IrError::VerifierFailure { message, .. }) => Err(message),
        Err(other) => panic!("{name}: expected a verifier failure, got {other:?}"),
    }
}

/// Drive one `RUN: not opt -passes=verify` operand-bundle fixture: the whole
/// file must be rejected, each `; CHECK:` directive must be produced by the
/// instruction it precedes, and the `; CHECK-NOT:` tail must verify clean.
///
/// Per-instruction, for the reason
/// [`upstream_musttail_invalid_fixture_messages_match`] is per-function:
/// `Module::verify_borrowed` reports the *first* failure where upstream's
/// `Verifier` keeps walking, so reproducing N `CHECK` lines takes N modules.
/// Within one call site the two agree — upstream's `Check` macro `return`s out
/// of `visitCallBase` on the first failure too.
///
/// `contains` is the comparison because a `CHECK` directive is a substring
/// match, which is FileCheck's own rule.
fn run_operand_bundle_fixture(name: &str, fixture: &str) {
    let parsed = operand_bundle_fixture(fixture);

    let module = Module::dynamic(name);
    Parser::new(fixture.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .unwrap_or_else(|e| panic!("{name} parses: {e}"));
    assert!(
        module.verify_borrowed().is_err(),
        "{name}: upstream's RUN line is `not opt -passes=verify`, so the whole \
         fixture must be rejected"
    );

    for (expected, instruction) in &parsed.cases {
        let source = operand_bundle_module(&parsed, &[instruction]);
        let message = verify_operand_bundle_module(name, &source)
            .expect_err(&format!("{name}: {instruction:?} must be rejected"));
        assert!(
            message.contains(expected),
            "{name}: {message:?} does not contain {expected:?} for {instruction:?}"
        );
    }

    let clean = operand_bundle_module(&parsed, &parsed.clean);
    assert_eq!(
        verify_operand_bundle_module(name, &clean),
        Ok(()),
        "{name}: the `; CHECK-NOT:` tail must verify clean"
    );
}

/// `test/Verifier/kcfi-operand-bundles.ll`, verbatim (the whole file) — the
/// verdict half of the fixture [`indirect_call_kcfi_operand_bundles_round_trip`]
/// ports the printed half of.
///
/// Its two `CHECK:` lines are `Verifier::visitCallBase`'s `"kcfi"` arm:
/// `Multiple kcfi operand bundles` and `Kcfi bundle operand must be an i32
/// constant`.
#[test]
fn upstream_kcfi_operand_bundle_fixture_messages_match() {
    const FIXTURE: &str =
        include_str!("fixtures/upstream/LLParser-parseCall/kcfi-operand-bundles.ll");

    run_operand_bundle_fixture("kcfi-operand-bundles", FIXTURE);
}

/// `test/Verifier/ptrauth-operand-bundles.ll`, verbatim (the whole file) — the
/// verdict half of the fixture
/// [`indirect_call_ptrauth_operand_bundles_round_trip`] ports the printed half
/// of.
///
/// Its four `CHECK:` lines cover `Verifier::visitCallBase`'s `"ptrauth"` arm
/// (`Multiple ptrauth operand bundles`, `Ptrauth bundle key operand must be an
/// i32 constant` for both a non-constant `i32` and an `i64` constant, and
/// `Ptrauth bundle discriminator operand must be an i64`) plus `Direct call
/// cannot have a ptrauth bundle`, the one bundle `Check` raised after the loop.
#[test]
fn upstream_ptrauth_operand_bundle_fixture_messages_match() {
    const FIXTURE: &str =
        include_str!("fixtures/upstream/LLParser-parseCall/ptrauth-operand-bundles.ll");

    run_operand_bundle_fixture("ptrauth-operand-bundles", FIXTURE);
}

/// `test/Verifier/inline-asm-indirect-operand.ll`, verbatim (the whole file).
/// The inline-asm `call`, `invoke` and `callbr` forms of an argument carrying
/// `elementtype(i32)`.
///
/// Upstream's `RUN` line is `not llvm-as < %s -o /dev/null 2>&1 | FileCheck %s`
/// — `llvm-as` runs the verifier, so the fixture has two halves. This test is
/// the parse/print half: the `CHECK-NEXT` lines are the offending instruction
/// as `AsmWriter` prints it, and `@okay`'s call is the positive case with no
/// `CHECK` of its own. The `CHECK` half — the three
/// `Verifier::verifyInlineAsmCall` messages — is
/// [`upstream_inline_asm_indirect_operand_fixture_messages_match`].
#[test]
fn inline_asm_call_elementtype_argument_attribute_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/inline-asm-indirect-operand.ll");

    let text = parse_and_render_bytes(
        "inline_asm_call_elementtype_argument_attribute_round_trips",
        FIXTURE,
    );
    check_directives(
        &text,
        &[
            // `@okay` — the positive case, which carries no upstream `CHECK`
            // line of its own; the attribute must survive here too or the
            // fixture means nothing.
            Check::Line(
                r#"call void asm "addl $1, $0", "=*rm,r"(ptr elementtype(i32) %p, i32 %x)"#,
            ),
            Check::Line(r#"call void asm "addl $1, $0", "=*rm,r"(i32 %p, i32 %x)"#),
            Check::Line(
                r#"call void asm "addl $1, $0", "=*rm,r"(ptr elementtype(i32) %p, ptr elementtype(i32) %x)"#,
            ),
            Check::Line(r#"call void asm "addl $1, $0", "=*rm,r"(ptr %p, i32 %x)"#),
            Check::Line(r#"invoke void asm "addl $1, $0", "=*rm,r"(i32 %p, i32 %x)"#),
            Check::Line(r#"callbr void asm "addl $1, $0", "=*rm,r"(i32 %p, i32 %x)"#),
        ],
    );
}

/// `llvm/test/Verifier/inline-asm-indirect-operand.ll`'s `CHECK` half: each of
/// its six functions cut out and verified on its own, against the message the
/// fixture writes above it.
///
/// These are the three `Check`s of `Verifier::verifyInlineAsmCall`'s
/// per-operand loop, and the `call` / `invoke` / `callbr` spread is the
/// fixture's own point — upstream reaches all three from one routine, and so
/// does llvmkit now. `@okay` carries no `CHECK` line, so it must verify clean.
#[test]
fn upstream_inline_asm_indirect_operand_fixture_messages_match() {
    const FIXTURE: &str =
        include_str!("fixtures/upstream/LLParser-parseCall/inline-asm-indirect-operand.ll");
    let cases = [
        ("define void @okay(", None),
        (
            "define void @not_pointer_arg(",
            Some("Operand for indirect constraint must have pointer type"),
        ),
        (
            "define void @not_indirect(",
            Some("Elementtype attribute can only be applied for indirect constraints"),
        ),
        (
            "define void @missing_elementtype(",
            Some("Operand for indirect constraint must have elementtype attribute"),
        ),
        (
            "define void @not_pointer_arg_invoke(",
            Some("Operand for indirect constraint must have pointer type"),
        ),
        (
            "define void @not_pointer_arg_callbr(",
            Some("Operand for indirect constraint must have pointer type"),
        ),
    ];
    for (marker, expected) in cases {
        assert_fixture_case_verifies(FIXTURE, "", marker, expected);
    }
}

/// **No upstream counterpart.** The rule anchor is `LLParser::parseCall`'s
/// post-construction statements — `CI->setTailCallKind(TCK)`,
/// `CI->setCallingConv(CC)` and `CI->setFastMathFlags(FMF)` — which run on the
/// single `Value *Callee` that `convertValIDToValue` resolved, with no
/// direct/indirect distinction anywhere; and
/// `AssemblyWriter::printInstruction`'s `CallInst` arm, which prints all three
/// without consulting the callee's shape.
///
/// The fixture is llvmkit-authored: it spells a tail-call kind, a calling
/// convention and fast-math flags on a non-`@` callee.
#[test]
fn indirect_call_modifiers_round_trip() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_modifiers_round_trip.ll"
    );

    let text = parse_and_render_bytes("indirect_call_modifiers_round_trip", FIXTURE);
    check_directives(
        &text,
        &[
            Check::Line("tail call void %fp(i32 %v)"),
            Check::Next("notail call void %fp(i32 %v)"),
            Check::Next("call fastcc void %fp(i32 %v)"),
            // `printCallingConv`'s default arm writes `cc99`; the fixture's
            // spaced input is the same token stream after `LLLexer`'s rewind.
            Check::Next("call cc99 void %fp(i32 %v)"),
            Check::Next("%a = call nnan ninf float %fp(float %fv)"),
            Check::Next("%b = call fast float %fp(float %fv)"),
        ],
    );
}

/// **No upstream counterpart** for the `#N` and return-attribute halves — see
/// [`indirect_call_modifiers_round_trip`] for the shape. (The
/// *parameter*-attribute half does have one, ported as
/// [`indirect_call_parameter_attribute_round_trips`].)
///
/// The rule anchor is `LLParser::parseCall`'s `CI->setAttributes(PAL)` and
/// `ForwardRefAttrGroups[CI] = FwdRefAttrGrps`, both of which run on the single
/// resolved `Value *Callee`. A dropped `#0` is doubly visible: the call loses
/// the reference *and* the module keeps an `attributes #0 = { … }` line nothing
/// refers to, which is why the definition is asserted too.
#[test]
fn indirect_call_attributes_round_trip() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_attributes_round_trip.ll"
    );

    let text = parse_and_render_bytes("indirect_call_attributes_round_trip", FIXTURE);
    check_directives(
        &text,
        &[
            Check::Line("call void %fp(i32 noundef %v)"),
            Check::Next("call void %fp(ptr nonnull align 8 %p)"),
            Check::Next("call void %fp(i32 %v) #0"),
            Check::Next("%r = call zeroext i8 %fp(i32 %v)"),
            Check::Line("attributes #0 = { nounwind }"),
        ],
    );
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

/// `LLParser::parseBasicBlock` strips the optional `%name =` **before**
/// `parseInstruction` dispatches, so the token `parseInvoke` reads next is
/// always `parseType`'s return type. A `%`-sigil token in that position is a
/// named struct type, unambiguously — and stays one whether or not the
/// instruction carries a result name.
///
/// **No upstream fixture writes it:** `rg --no-ignore --hidden -a -l "invoke
/// %[A-Za-z_.]"` over `orig_cpp/.../llvm/test/` returns nothing (the only
/// near miss, `test/Assembler/opaque-ptr.ll`, writes `invoke void %p()` — a
/// named *callee*, not a named return type). llvmkit read the return type as
/// a result name and rejected both spellings until this commit.
#[test]
fn invoke_named_struct_return_type_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/invoke_named_struct_return_type.ll");

    let text = parse_and_render_bytes("invoke_named_struct_return_type", FIXTURE);
    assert_check_lines(
        &text,
        &["invoke %struct.S @f()", "%r = invoke %struct.S @f()"],
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

/// Parse with [`ParserConfig::data_layout_callback`] answering `layout`, which
/// is how `llvm-as -data-layout=<layout>` reaches the parser: `llvm-as.cpp`
/// wraps `ClDataLayout` in a `DataLayoutCallbackTy` and hands it to
/// `parseAssemblyFileWithIndex`. The verify half is `llvm-as`'s own — see
/// [`parse_verify_and_render_bytes`].
fn parse_verify_and_render_with_data_layout(src: &[u8], layout: &str) -> String {
    let callback = |_: &str, _: &str| Some(layout.to_owned());
    let config = llvmkit_asmparser::parser::ParserConfig {
        data_layout_callback: Some(&callback),
        ..llvmkit_asmparser::parser::ParserConfig::DEFAULT
    };
    let module = llvmkit_asmparser::parser::parse_dynamic_with_config(src, &config)
        .expect("parser succeeds");
    module
        .verify_borrowed()
        .expect("`llvm-as` verifies this fixture, so llvmkit must too");
    format!("{module}")
}

/// `test/Assembler/call-nonzero-program-addrspace.ll`, first `RUN` line
/// (`not llvm-as %s`): with the file's own — zero — program address space, a
/// callee held in `addrspace(42)` does not match the `ptr` the call site
/// demands. The rule is `LLParser::parseCall`'s
/// `convertValIDToValue(PointerType::get(Context, CallAddrSpace), …)` reaching
/// `PerFunctionState::getVal` and `LLParser::checkValidVariableType`.
///
/// The fixture's `[[@LINE-1]]:25` column pin **is** asserted. It used to be
/// skipped, on the ground that `convert_val_id_to_value` anchored at the token
/// after the ValID rather than at `ID.Loc`; the `ValID::Loc` port closed that,
/// and this comment outlived it. Asserting the column is what keeps the port
/// from regressing silently on a fixture whose whole point is the column.
#[test]
fn call_in_zero_program_addrspace_rejects_a_nonzero_callee() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/call-nonzero-program-addrspace.ll");

    assert_fixture_rejected(
        "call_in_zero_program_addrspace_rejects_a_nonzero_callee",
        FIXTURE,
        "'%fnptr42' defined with type 'ptr addrspace(42)' but expected 'ptr'",
    );

    let src = std::str::from_utf8(FIXTURE).expect("fixture is UTF-8");
    let err = parse_fixture_err(
        "call_in_zero_program_addrspace_rejects_a_nonzero_callee_loc",
        FIXTURE,
    );
    // `; CHECK: …:[[@LINE-1]]:25:` on the line after `%call_no_as = call i8
    // %fnptr42(i32 0)` — upstream's `ID.Loc`, the `%fnptr42` token.
    assert_eq!(error_line_col(src, &err), (10, 25));
    assert_eq!(error_token(src, &err), "%fnptr42");
}

/// `test/Assembler/call-nonzero-program-addrspace.ll`, second `RUN` line
/// (`llvm-as %s -data-layout=P42 | llvm-dis`), asserting its `PROGAS42`
/// prefix. Three rules at once: `parseOptionalProgramAddrSpace` defaulting to
/// the datalayout's program address space, `AssemblyWriter`'s
/// `maybePrintCallAddrSpace` printing `addrspace(0)` because
/// `ForcePrintAddrSpace` is set, and printing `addrspace(42)` because it is
/// non-zero.
///
/// The fixture's `PROGAS42` block is asserted as upstream writes it, `-NEXT`
/// included, through [`check_directives`].
#[test]
fn call_addrspace_round_trips_under_a_nonzero_program_addrspace() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/call-nonzero-program-addrspace.ll");

    let text = parse_verify_and_render_with_data_layout(FIXTURE, "P42");
    check_directives(
        &text,
        &[
            Check::Line("target datalayout = \"P42\""),
            Check::Line("define i8 @test(ptr %fnptr0, ptr addrspace(42) %fnptr42) addrspace(42) {"),
            Check::Next("%explicit_as_0 = call addrspace(0) i8 %fnptr0(i32 0)"),
            Check::Next("%explicit_as_42 = call addrspace(42) i8 %fnptr42(i32 0)"),
            Check::Next("%call_no_as = call addrspace(42) i8 %fnptr42(i32 0)"),
            Check::Next("ret i8 0"),
            Check::Next("}"),
        ],
    );
}

/// `test/Assembler/call-nonzero-program-addrspace-2.ll`, both `RUN` lines. The
/// numbered-value twin of the pair above: `parseValID`'s `t_LocalID` arm
/// reaches `PerFunctionState::getVal(unsigned, …)`, and the printed slot
/// numbers are upstream's (`%0`/`%1` arguments, `%2` the entry block, results
/// from `%3`). Its `PROGAS42` block is asserted with `-NEXT` where upstream
/// writes it, through [`check_directives`].
#[test]
fn numbered_callee_addrspace_matches_upstream_in_both_program_addrspaces() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/call-nonzero-program-addrspace-2.ll");

    assert_fixture_rejected(
        "numbered_callee_addrspace_rejected",
        FIXTURE,
        "'%1' defined with type 'ptr addrspace(42)' but expected 'ptr'",
    );

    let text = parse_verify_and_render_with_data_layout(FIXTURE, "P42");
    check_directives(
        &text,
        &[
            Check::Line("target datalayout = \"P42\""),
            Check::Line("define i8 @test_unnamed(ptr %0, ptr addrspace(42) %1) addrspace(42) {"),
            Check::Next("%3 = call addrspace(0) i8 %0(i32 0)"),
            Check::Next("%4 = call addrspace(42) i8 %1(i32 0)"),
            Check::Next("%5 = call addrspace(42) i8 %1(i32 0)"),
            Check::Next("ret i8 0"),
            Check::Next("}"),
        ],
    );
}

/// `test/Assembler/invoke-nonzero-program-addrspace.ll`, both `RUN` lines.
/// `LLParser::parseInvoke` carries its own `parseOptionalProgramAddrSpace`
/// (upstream's `InvokeAddrSpace`) and `AssemblyWriter`'s `InvokeInst` arm is
/// `maybePrintCallAddrSpace`'s second and last caller.
///
/// Every directive in this fixture's `PROGAS200` block is a plain `PROGAS200:`
/// — upstream writes no `-NEXT` here — so all seven are [`Check::Line`].
#[test]
fn invoke_addrspace_matches_upstream_in_both_program_addrspaces() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/invoke-nonzero-program-addrspace.ll");

    assert_fixture_rejected(
        "invoke_addrspace_rejected",
        FIXTURE,
        "'%fnptr200' defined with type 'ptr addrspace(200)' but expected 'ptr'",
    );

    let text = parse_verify_and_render_with_data_layout(FIXTURE, "P200");
    check_directives(
        &text,
        &[
            Check::Line("target datalayout = \"P200\""),
            Check::Line(
                "define i8 @test_invoke(ptr %fnptr0, ptr addrspace(200) %fnptr200) addrspace(200) personality ptr addrspace(200) @__gxx_personality_v0 {",
            ),
            Check::Line("%explicit_as_0 = invoke addrspace(0) i8 %fnptr0(i32 0)"),
            Check::Line("%explicit_as_42 = invoke addrspace(200) i8 %fnptr200(i32 0)"),
            Check::Line("%no_as = invoke addrspace(200) i8 %fnptr200(i32 0)"),
            Check::Line("ret i8 0"),
            Check::Line("}"),
        ],
    );
}

/// `LLParser::parseCallBr` is the one call-family routine with **no**
/// `parseOptionalProgramAddrSpace` — its `||` chain goes return-attrs ->
/// `parseType` — so `callbr addrspace(1) …` is a syntax error upstream too,
/// and `AssemblyWriter`'s `CallBrInst` arm has no `maybePrintCallAddrSpace`
/// call. LLVM 22.1.4 ships no `.ll` fixture pinning that absence, so the
/// routine is the anchor (D11).
#[test]
fn callbr_does_not_accept_an_address_space() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCallBr/callbr_rejects_addrspace.ll");

    assert_fixture_rejected(
        "callbr_does_not_accept_an_address_space",
        FIXTURE,
        "expected type",
    );
}

/// `test/Assembler/ifunc-program-addrspace.ll`, whole and verbatim, with its
/// own `CHECK` / `CHECK-NEXT` block. Upstream's `RUN` line is
/// `llvm-as < %s | llvm-dis | FileCheck %s`, so [`parse_verify_and_render_bytes`]
/// is the whole pipeline.
///
/// What it pins: `LLParser::convertValIDToValue`'s `t_GlobalName` arm is
/// `getGlobalVal(ID.StrVal, Ty, ID.Loc)`, one lookup in
/// `M->getValueSymbolTable()` that accepts **any** `GlobalValue`. An `ifunc`
/// callee therefore resolves to the ifunc, at the ifunc's own address space —
/// which for `@ifunc_as1` is 1, because `parseAliasOrIFunc` takes
/// `AddrSpace = PTy->getAddressSpace()` from the resolver constant. The call's
/// own `FunctionType` lives on the `CallBase`, so nothing needs the callee to
/// be a `Function`.
///
/// The same fixture also drives the corpus manifest, which asserts what
/// `check_directives` cannot: that the module verifies and that printing it,
/// re-parsing the print and printing again is a fixed point.
#[test]
fn ifunc_callee_resolves_at_the_ifuncs_own_program_address_space() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/ifunc-program-addrspace.ll");

    let text = parse_verify_and_render_bytes(
        "ifunc_callee_resolves_at_the_ifuncs_own_program_address_space",
        FIXTURE,
    );
    check_directives(
        &text,
        &[
            Check::Line("@ifunc_as0 = ifunc void (), ptr @resolver_as0"),
            Check::Line("@ifunc_as1 = ifunc void (), ptr addrspace(1) @resolver_as1"),
            Check::Line("define ptr @resolver_as0() addrspace(0) {"),
            Check::Line("define ptr @resolver_as1() addrspace(1) {"),
            Check::Line("define void @call_ifunc_as0() addrspace(1) {"),
            Check::Next("call addrspace(0) void @ifunc_as0()"),
            Check::Line("define void @call_ifunc_as1() addrspace(1) {"),
            Check::Next("call addrspace(1) void @ifunc_as1()"),
        ],
    );
}

/// `test/Assembler/ifunc-use-list-order.ll`, whole and verbatim. Upstream's
/// `RUN` line is `verify-uselistorder < %s`, which has no `CHECK` block, so
/// what is portable is the half `docs/fixture-coverage.md` maps that tool onto:
/// the module parses and prints. The lines asserted here are the two call sites
/// — one to an ifunc, one to an ordinary function — because they are what the
/// `getGlobalVal` lookup decides.
///
/// This fixture was classified `blocked-model` on the forward-reference gap.
/// It was not blocked on that: `@foo_ifunc` is *defined above* `@bar`, so the
/// callee lookup finds it in the symbol table and the forward-declaration arm
/// is never reached. Its blocker was the narrow callee lookup, the same one
/// [`ifunc_callee_resolves_at_the_ifuncs_own_program_address_space`] pins.
#[test]
fn a_call_to_an_already_defined_ifunc_resolves_to_the_ifunc() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/ifunc-use-list-order.ll");

    let text = parse_verify_and_render_bytes(
        "a_call_to_an_already_defined_ifunc_resolves_to_the_ifunc",
        FIXTURE,
    );
    check_directives(
        &text,
        &[
            Check::Line("@foo_ifunc = ifunc void (), ptr @foo_resolver"),
            Check::Line("define void @bar() {"),
            Check::Line("call void @foo_ifunc()"),
            Check::Line("define void @bar2() {"),
            Check::Line("call void @bar()"),
        ],
    );
}

/// **llvmkit-authored fixture; the rule is the anchor (D11).** No `.ll` in the
/// vendored tree spells a bare call to an alias, a global variable or a
/// numbered global: `rg '= alias '` over `test/Assembler`, `test/Verifier`,
/// `test/Feature` and `test/Bitcode`, intersected with the files that carry a
/// `call`/`invoke`, leaves `test/Feature/aliases.ll` as the only one with a
/// bare alias callee (`%tmp4 = call %FunTy @bar_f()`) — and it is written in
/// typed-pointer syntax LLVM 22.1.4 no longer parses.
///
/// The rule: `LLParser::getGlobalVal`'s lookup is
/// `cast_or_null<GlobalValue>(M->getValueSymbolTable().lookup(Name))`, and its
/// numbered twin reads `NumberedVals`. Both accept **any** `GlobalValue`, and
/// the call's own `FunctionType` lives on the `CallBase`, so the callee never
/// has to be a `Function`. The ifunc arm of that same rule is pinned by two
/// upstream fixtures; these three kinds have none, which is why they are here.
#[test]
fn a_non_function_global_callee_resolves_through_the_symbol_table() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-getGlobalVal/non_function_global_callees.ll");

    let text = parse_verify_and_render_bytes(
        "a_non_function_global_callee_resolves_through_the_symbol_table",
        FIXTURE,
    );
    check_directives(
        &text,
        &[
            Check::Line("call void @a()"),
            Check::Next("call void @gv()"),
            Check::Next("call void @0()"),
        ],
    );
}

/// **llvmkit-authored; the rule is the anchor (D11).** The three arms of
/// `FPMathOperator::isComposedOfHomogeneousFloatingPointTypes` that answer
/// *false* on an aggregate, each of which upstream's `llvm-as` rejects the
/// same way and none of which any `.ll` in the vendored tree spells:
///
/// * a literal struct whose fields are not all one type — `containsHomogeneousTypes`
///   is `!ElementTys.empty() && all_equal(ElementTys)`;
/// * an **identified** struct, homogeneous or not — the routine opens with
///   `if (!StructTy->isLiteral() || …) return false`;
/// * the empty literal struct — the `!ElementTys.empty()` half.
///
/// The positive arms are `test/Bitcode/compatibility.ll`'s
/// `@fastMathFlagsForArrayCalls` / `@fastMathFlagsForStructCalls`, vendored
/// under `fixtures/upstream/compatibility/` and driven by the corpus manifest.
#[test]
fn fast_math_flags_on_a_non_homogeneous_aggregate_call_are_rejected() {
    const MESSAGE: &str =
        "fast-math-flags specified for call without floating-point scalar or vector return type";
    const CASES: &[(&str, &str)] = &[
        (
            "mixed_literal_struct",
            "declare { float, i32 } @m()\n\
             define void @f() {\n  \
             %r = call fast { float, i32 } @m()\n  \
             ret void\n\
             }\n",
        ),
        (
            "identified_struct",
            "%named = type { float, float }\n\
             declare %named @n()\n\
             define void @f() {\n  \
             %r = call fast %named @n()\n  \
             ret void\n\
             }\n",
        ),
        (
            "empty_literal_struct",
            "declare {} @e()\n\
             define void @f() {\n  \
             %r = call fast {} @e()\n  \
             ret void\n\
             }\n",
        ),
    ];

    for (name, source) in CASES {
        let err = parse_fixture_err(name, source.as_bytes());
        assert_eq!(err.to_string(), MESSAGE, "case {name}");
    }
}

/// `llvm/test/Verifier/musttail-invalid.ll`, vendored verbatim; each of its
/// eleven functions cut out with its `declare` and verified on its own, and
/// each asserted against the `CHECK` line the fixture writes for it.
///
/// Upstream's `RUN` line is `not llvm-as %s -o /dev/null 2>&1 | FileCheck %s`,
/// so every module in the file is invalid and the diagnostics are
/// `Verifier::verifyMustTailCall`'s `Check` literals. The fixture is
/// per-function here for the reason
/// `parser_metadata.rs::upstream_invalid_range_metadata_fixture_messages_match`
/// is: `Module::verify_borrowed` reports the *first* failure, where upstream's
/// `Verifier` accumulates, so eleven separate modules is what reproduces
/// eleven separate `CHECK` lines.
///
/// The `CHECK` lines are substrings of the full literal (`mismatched calling
/// conv`, not `cannot guarantee tail call due to mismatched calling conv`),
/// which is FileCheck's own rule, so `contains` is the faithful comparison —
/// the same one `parser_corpus.rs` applies to an `error=` row.
#[test]
fn upstream_musttail_invalid_fixture_messages_match() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/musttail-invalid.ll");
    let cases = [
        ("define void @cc_mismatch(", "mismatched calling conv"),
        ("define void @more_parms(", "mismatched parameter counts"),
        (
            "define void @mismatched_intty(",
            "mismatched parameter types",
        ),
        ("define void @mismatched_vararg(", "mismatched varargs"),
        ("define void @mismatched_retty(", "mismatched return types"),
        (
            "define void @mismatched_byval(",
            "mismatched ABI impacting function attributes",
        ),
        (
            "define void @mismatched_inreg(",
            "mismatched ABI impacting function attributes",
        ),
        (
            "define void @mismatched_sret(",
            "mismatched ABI impacting function attributes",
        ),
        (
            "define void @mismatched_alignment(",
            "mismatched ABI impacting function attributes",
        ),
        (
            "define i32 @not_tail_pos(",
            "musttail call must precede a ret with an optional bitcast",
        ),
        (
            "define void @inline_asm(",
            "cannot use musttail call with inline asm",
        ),
    ];
    for (marker, expected) in cases {
        let source = musttail_fixture_case(FIXTURE, marker);
        let module = Module::dynamic("upstream_musttail_invalid_fixture_messages_match");
        Parser::new(source.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .unwrap_or_else(|e| panic!("case {marker} parses: {e}"));
        let err = module
            .verify_borrowed()
            .expect_err("`llvm-as` rejects every module in this fixture");
        let message = match err {
            llvmkit_ir::IrError::VerifierFailure { message, .. } => message,
            other => panic!("case {marker}: expected a verifier failure, got {other:?}"),
        };
        assert!(
            message.contains(expected),
            "case {marker}: {message:?} does not contain {expected:?}"
        );
    }
}

/// One case of `musttail-invalid.ll`: the `declare` immediately above the
/// marked `define`, plus the `define` through its closing brace.
fn musttail_fixture_case(fixture: &str, define_marker: &str) -> String {
    let define_start = fixture
        .find(define_marker)
        .unwrap_or_else(|| panic!("missing define marker {define_marker}"));
    let define_end = fixture[define_start..]
        .find("\n}")
        .map(|idx| define_start + idx + 3)
        .unwrap_or_else(|| panic!("missing define end for {define_marker}"));
    // `@inline_asm` has no `declare` of its own; every other case is preceded
    // immediately by exactly one.
    let declare = fixture[..define_start]
        .lines()
        .rfind(|line| !line.trim().is_empty() && !line.starts_with(';'))
        .filter(|line| line.starts_with("declare "))
        .unwrap_or_default();
    format!("{declare}\n{}\n", &fixture[define_start..define_end])
}

/// **Regression lock for a closed divergence** (no id: `docs/divergences.md`
/// deletes an entry when it closes and re-uses its number).
/// `LLParser::resolveFunctionType` hardcodes the variadic bit off
/// (`FunctionType::get(RetType, ParamTypes, false)`), so a *short-syntax*
/// `musttail` forwarding call in a varargs function builds a non-vararg
/// call-site type — which `Verifier::verifyMustTailCall`'s
/// `CallerTy->isVarArg() == CalleeTy->isVarArg()` then rejects. llvmkit
/// threaded its own `...` flag in here instead, so the module verified and
/// printed `musttail call void (i32, ...) @f(...)`, a form upstream never
/// produces.
///
/// llvmkit-authored source: no vendored fixture reaches the short syntax
/// (`rg --no-ignore -n -- "musttail call.*\.\.\." orig_cpp/…/llvm/test/`
/// returns only explicit-function-type forms), which is why the divergence
/// survived `musttail-invalid.ll` and `test/Assembler/musttail.ll` alike.
///
/// Both halves are asserted: the module is rejected, *and* the printed bytes
/// carry the short form. The second half is the round-trip claim the entry got
/// wrong — `AsmWriter`'s ellipsis is keyed on the enclosing function's
/// varargs bit, not on the call-site type, so dropping the bit does not drop
/// the `...`.
#[test]
fn short_syntax_musttail_forwarding_call_is_not_vararg() {
    const SRC: &[u8] = b"declare void @f(i32, ...)\n\
                         define void @g(i32 %a, ...) {\n  \
                         musttail call void @f(i32 %a, ...)\n  \
                         ret void\n\
                         }\n";

    let module = Module::dynamic("short_syntax_musttail_forwarding_call_is_not_vararg");
    Parser::new(SRC, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser accepts the short syntax");

    let text = format!("{module}");
    assert!(
        text.contains("musttail call void @f(i32 %a, ...)"),
        "call-site type must print in the short form: {text}"
    );

    let err = module
        .verify_borrowed()
        .expect_err("upstream's llvm-as rejects this module");
    match err {
        llvmkit_ir::IrError::VerifierFailure { rule, message, .. } => {
            assert_eq!(rule, llvmkit_ir::VerifierRule::MustTailCallVarArgsMismatch);
            assert_eq!(
                message,
                "cannot guarantee tail call due to mismatched varargs"
            );
        }
        other => panic!("expected a verifier failure, got {other:?}"),
    }
}

/// `Verifier::verifyMustTailCall`'s returned-value `Check`, all four ways it
/// can hold and the one way it fails:
///
/// ```text
/// Check(!Ret->getReturnValue() || Ret->getReturnValue() == RetVal ||
///           isa<UndefValue>(Ret->getReturnValue()),
///       "musttail call result must be returned", Ret);
/// ```
///
/// `poison` is here because `PoisonValue` derives from `UndefValue`
/// (`llvm/include/llvm/IR/Constants.h`), so `isa<UndefValue>` accepts it —
/// a port that matched only the `undef` constant would reject `ret ptr poison`
/// where upstream accepts it. `test/Verifier/musttail-invalid.ll` reaches this
/// `Check` only through its `not_tail_pos` case, which fails one `Check`
/// earlier, so no vendored fixture separates these five.
///
/// **llvmkit-authored sources**; `llvm/lib/IR/Verifier.cpp::Verifier::verifyMustTailCall`.
#[test]
fn a_musttail_call_result_may_be_returned_as_itself_undef_or_poison() {
    const PROLOGUE: &str = "declare ptr @callee()\ndefine ptr @caller() {\n  \
                            %v = musttail call ptr @callee()\n  ";
    for accepted in [
        "ret ptr %v\n}\n",
        "ret ptr undef\n}\n",
        "ret ptr poison\n}\n",
    ] {
        let source = format!("{PROLOGUE}{accepted}");
        let module = Module::dynamic("musttail_result_returned");
        Parser::new(source.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        module
            .verify_borrowed()
            .unwrap_or_else(|e| panic!("upstream accepts `{accepted}`: {e}"));
    }

    // `!Ret->getReturnValue()` — a void caller, so the `ret` carries nothing.
    let void_source = "declare void @vcallee()\ndefine void @vcaller() {\n  \
                       musttail call void @vcallee()\n  ret void\n}\n";
    let module = Module::dynamic("musttail_result_returned_void");
    Parser::new(void_source.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    module
        .verify_borrowed()
        .expect("upstream accepts `ret void`");

    let rejected = format!("{PROLOGUE}ret ptr null\n}}\n");
    let module = Module::dynamic("musttail_result_not_returned");
    Parser::new(rejected.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    match module.verify_borrowed() {
        Err(llvmkit_ir::IrError::VerifierFailure { rule, message, .. }) => {
            assert_eq!(
                rule,
                llvmkit_ir::VerifierRule::MustTailCallResultNotReturned
            );
            assert_eq!(message, "musttail call result must be returned");
        }
        other => panic!("upstream rejects `ret ptr null` here, got {other:?}"),
    }
}

/// `llvm/test/Verifier/musttail-valid.ll`, vendored verbatim, whole file.
/// Upstream's `RUN` line is `llvm-as %s -o /dev/null` — "Should assemble
/// without error" — so the whole module must parse *and* verify.
///
/// The positive half of `Verifier::verifyMustTailCall`: congruent pointer
/// parameter and return types, matching `x86_thiscallcc` / `x86_fastcallcc`
/// varargs thunks, and a `musttail` whose block has an unreachable successor
/// block after the `ret`.
#[test]
fn upstream_musttail_valid_fixture_verifies() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/Verifier/musttail-valid.ll");

    let module = Module::dynamic("upstream_musttail_valid_fixture_verifies");
    Parser::new(FIXTURE, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    module
        .verify_borrowed()
        .expect("`llvm-as` assembles this fixture without error, so llvmkit must too");
}

/// `llvm/test/Verifier/swifttailcc-musttail-valid.ll`, vendored verbatim, whole
/// file. Upstream's `RUN` line is `opt -passes=verify %s`, with no `not`.
///
/// `@mismatch_parms` is the interesting half: it calls a four-parameter
/// function from a zero-parameter one and still verifies, because
/// `verifyMustTailCall` **returns** out of the `swifttailcc` arm before it
/// reaches the parameter-count `Check`. A port that fell through would reject
/// it.
#[test]
fn upstream_swifttailcc_musttail_valid_fixture_verifies() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/Verifier/swifttailcc-musttail-valid.ll");

    let module = Module::dynamic("upstream_swifttailcc_musttail_valid_fixture_verifies");
    Parser::new(FIXTURE, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    module
        .verify_borrowed()
        .expect("`opt -passes=verify` accepts this fixture, so llvmkit must too");
}

/// `llvm/test/Verifier/tailcc-musttail.ll` and
/// `llvm/test/Verifier/swifttailcc-musttail.ll`, both vendored verbatim; each
/// of their eleven functions cut out with the shared `declare` and verified on
/// its own against the `CHECK` line the fixture writes for it.
///
/// These pin `Verifier::verifyTailCCMustTailAttrs` — the five forbidden
/// ABI-impacting parameter attributes, on the caller and on the callee — plus
/// the `cannot guarantee <cc> tail call for varargs function` `Check` that
/// closes the `tailcc` / `swifttailcc` arm. The two fixtures are the same
/// eleven cases with the calling convention swapped, which is what makes the
/// `CCName` half of the diagnostic worth asserting.
///
/// Per-function for the same reason
/// [`upstream_musttail_invalid_fixture_messages_match`] is: upstream's
/// `Verifier` accumulates and `Module::verify_borrowed` reports the first
/// failure.
#[test]
fn upstream_tailcc_musttail_fixture_messages_match() {
    const TAILCC: &str = include_str!("fixtures/upstream/Verifier/tailcc-musttail.ll");
    const SWIFTTAILCC: &str = include_str!("fixtures/upstream/Verifier/swifttailcc-musttail.ll");
    let markers = [
        (
            "define {CC} void @inreg(",
            "inreg attribute not allowed in {CC} musttail caller",
        ),
        (
            "define {CC} void @inalloca(",
            "inalloca attribute not allowed in {CC} musttail caller",
        ),
        (
            "define {CC} void @swifterror(",
            "swifterror attribute not allowed in {CC} musttail caller",
        ),
        (
            "define {CC} void @preallocated(",
            "preallocated attribute not allowed in {CC} musttail caller",
        ),
        (
            "define {CC} void @byref(",
            "byref attribute not allowed in {CC} musttail caller",
        ),
        (
            "define {CC} void @call_inreg(",
            "inreg attribute not allowed in {CC} musttail callee",
        ),
        (
            "define {CC} void @call_inalloca(",
            "inalloca attribute not allowed in {CC} musttail callee",
        ),
        (
            "define {CC} void @call_swifterror(",
            "swifterror attribute not allowed in {CC} musttail callee",
        ),
        (
            "define {CC} void @call_preallocated(",
            "preallocated attribute not allowed in {CC} musttail callee",
        ),
        (
            "define {CC} void @call_byref(",
            "byref attribute not allowed in {CC} musttail callee",
        ),
        (
            "define {CC} void @call_varargs(",
            "cannot guarantee {CC} tail call for varargs function",
        ),
    ];

    for (cc, fixture) in [("tailcc", TAILCC), ("swifttailcc", SWIFTTAILCC)] {
        // Five of the eleven cases call a function these fixtures spell as a
        // `define` — each of which is itself a failing case, so keeping it
        // whole would report *its* diagnostic first. Each `define` header is
        // therefore reduced to the `declare` it already implies: the signature
        // and its parameter attributes verbatim, without the body. That is all
        // the callee contributes, since `verifyMustTailCall` reads the
        // *call site's* attributes and the call-site function type.
        let declarations: Vec<(String, String)> = fixture
            .lines()
            .filter(|line| line.starts_with("declare ") || line.starts_with("define "))
            .map(|line| {
                let header = line.trim_end().trim_end_matches('{').trim_end();
                let name = header
                    .split('@')
                    .nth(1)
                    .unwrap_or_default()
                    .split('(')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let body = header
                    .strip_prefix("define")
                    .or_else(|| header.strip_prefix("declare"))
                    .unwrap_or(header);
                (name, format!("declare{body}\n"))
            })
            .collect();

        for (marker, expected) in markers {
            let marker = marker.replace("{CC}", cc);
            let expected = expected.replace("{CC}", cc);
            let start = fixture
                .find(&marker)
                .unwrap_or_else(|| panic!("{cc}: missing define marker {marker}"));
            let end = fixture[start..]
                .find("\n}")
                .map(|idx| start + idx + 3)
                .unwrap_or_else(|| panic!("{cc}: missing define end for {marker}"));
            let define = &fixture[start..end];
            let under_test = marker
                .split('@')
                .nth(1)
                .unwrap_or_default()
                .trim_end_matches('(')
                .to_owned();
            let prelude: String = declarations
                .iter()
                .filter(|(name, _)| *name != under_test)
                .map(|(_, text)| text.as_str())
                .collect();
            let source = format!("{prelude}\n{define}\n");

            let module = Module::dynamic("upstream_tailcc_musttail_fixture_messages_match");
            Parser::new(source.as_bytes(), &module)
                .expect("lexer primes")
                .parse_module()
                .unwrap_or_else(|e| panic!("{cc} case {marker} parses: {e}\n{source}"));
            let message = match module.verify_borrowed() {
                Ok(()) => panic!("{cc} case {marker}: upstream rejects this module\n{source}"),
                Err(llvmkit_ir::IrError::VerifierFailure { message, .. }) => message,
                Err(other) => panic!("{cc} case {marker}: expected a verifier failure: {other:?}"),
            };
            assert!(
                message.contains(&expected),
                "{cc} case {marker}: {message:?} does not contain {expected:?}"
            );
        }
    }
}

/// The lines of `fixture` that sit outside every `define … { … }` block: the
/// `%0 = type opaque`, the `declare`s, and the `attributes #0 = { … }` line.
/// A per-case module keeps them so the body it isolates still resolves.
fn top_level_lines(fixture: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in fixture.lines() {
        if inside {
            if line.trim() == "}" {
                inside = false;
            }
            continue;
        }
        if line.trim_start().starts_with("define ") {
            inside = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// One `define` of `fixture`, header through closing brace, taken verbatim.
fn function_text<'a>(fixture: &'a str, header_marker: &str) -> &'a str {
    let start = fixture
        .find(header_marker)
        .unwrap_or_else(|| panic!("missing function {header_marker}"));
    let end = fixture[start..]
        .find("\n}")
        .map(|idx| start + idx + 3)
        .unwrap_or_else(|| panic!("missing end of {header_marker}"));
    &fixture[start..end]
}

/// Parse `source` and report the verifier's answer, or the parse diagnostic.
fn parse_and_verify(name: &str, source: &str) -> Result<Result<(), String>, String> {
    let module = Module::dynamic(name);
    match Parser::new(source.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
    {
        Err(e) => Err(e.to_string()),
        Ok(_) => Ok(match module.verify_borrowed() {
            Ok(()) => Ok(()),
            Err(llvmkit_ir::IrError::VerifierFailure { message, .. }) => Err(message),
            Err(other) => panic!("{name}: expected a verifier failure, got {other:?}"),
        }),
    }
}

/// `test/Verifier/operand-bundles.ll`, vendored verbatim and driven per
/// function — and, for its one multi-diagnostic function, per call — for the
/// reason [`upstream_musttail_invalid_fixture_messages_match`] is driven per
/// function: `Module::verify_borrowed` reports the first failure where
/// upstream's `Verifier` keeps walking.
///
/// This is the fixture behind the `Verifier::visitCallBase` bundle arms the
/// kcfi and ptrauth fixtures do not reach — `"deopt"`, `"gc-transition"`, and
/// `Verifier::verifyAttachedCallBundle` — plus each function's `CHECK-NOT`
/// tail, which must verify clean.
///
/// **Partial in one named place; nothing is trimmed and every line of the
/// fixture is asserted.** Seven of `@f_clang_arc_attachedcall`'s thirteen
/// calls name an intrinsic by address (`ptr @llvm.objc.…`, `ptr @llvm.assume`).
/// Upstream parses those and `Verifier::visitInstruction` exempts an
/// `OB_clang_arc_attachedcall` operand from `Cannot take the address of an
/// intrinsic!` precisely so `verifyAttachedCallBundle` can judge them; llvmkit
/// rejects every `llvm.`-prefixed non-callee reference at *parse* time, which
/// is `docs/divergences.md` entry **37**. Those seven assert that parse
/// rejection with entry 37's message, so this test starts failing the day
/// entry 37 closes and the remaining coverage has to land with it.
/// Consequently the whole-file `RUN` line is asserted as a parse failure
/// rather than a verify failure.
///
/// `@f0` and `@f1` were the second partial place until the verifier carried
/// upstream's `Check` literals: they pin `Instruction does not dominate all
/// uses!`, which llvmkit used to word its own way, and now assert that text
/// like every other function here.
///
/// Every whole-function expectation is **read out of the fixture's own first
/// `; CHECK:` line** rather than repeated in this file, so a re-blessed
/// verifier message that drifts from upstream's cannot be papered over by
/// editing a string here — the same discipline `@f_clang_arc_attachedcall`'s
/// half already used.
#[test]
fn upstream_verifier_operand_bundles_fixture_messages_match() {
    /// `docs/divergences.md` entry 37 — llvmkit's parse-time stand-in for
    /// upstream's `Cannot take the address of an intrinsic!`.
    const ENTRY_37: &str = "intrinsic can only be used as callee";

    const FIXTURE: &str = include_str!("fixtures/upstream/Verifier/operand-bundles.ll");
    let prelude = top_level_lines(FIXTURE);

    // Upstream's `RUN` line is `not opt -passes=verify`; llvmkit stops one
    // layer earlier, on entry 37.
    assert_eq!(
        parse_and_verify("verifier-operand-bundles", FIXTURE),
        Err(ENTRY_37.to_owned())
    );

    // Whole-function cases. The expectation is the function's first `CHECK`
    // directive, taken from the vendored text.
    for marker in [
        "define void @f0(",
        "define void @f1(",
        "define void @f_deopt(",
        "define void @f_gc_transition(",
    ] {
        let text = function_text(FIXTURE, marker);
        let expected = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("; CHECK:"))
            .map(str::trim)
            .unwrap_or_else(|| panic!("{marker}: no `; CHECK:` directive in the fixture"));
        let source = format!("{prelude}\n{text}\n");
        let message = parse_and_verify("verifier-operand-bundles", &source)
            .unwrap_or_else(|e| panic!("{marker} parses: {e}\n{source}"))
            .expect_err(&format!("{marker}: upstream rejects this function"));
        assert!(
            message.contains(expected),
            "{marker}: {message:?} does not contain {expected:?}"
        );
    }

    // `@f_clang_arc_attachedcall` carries eight `CHECK` directives over
    // thirteen calls, so it is driven one call at a time. Each call's
    // expectation is read out of the fixture's own `CHECK` block, which
    // alternates diagnostic, offending instruction, diagnostic, …; the
    // instruction lines are `AsmWriter` output, which for these calls is the
    // source text. A call the block does not name is one upstream reports
    // nothing for.
    let attached = function_text(FIXTURE, "define void @f_clang_arc_attachedcall(");
    let mut expectations: Vec<(&str, &str)> = Vec::new();
    let mut pending: Option<&str> = None;
    let mut body: Vec<&str> = Vec::new();
    let mut header = "";
    for line in attached.lines() {
        let trimmed = line.trim();
        let directive = trimmed
            .strip_prefix("; CHECK-NEXT:")
            .or_else(|| trimmed.strip_prefix("; CHECK:"));
        if let Some(text) = directive {
            let text = text.trim();
            match pending.take() {
                Some(diagnostic) if text.starts_with("call ") => {
                    expectations.push((text, diagnostic));
                }
                // The block strictly alternates; a second diagnostic in a row
                // would otherwise be dropped without an assertion.
                Some(diagnostic) => panic!("unpaired CHECK {diagnostic:?} before {text:?}"),
                None => pending = Some(text),
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with("define ") {
            header = line;
            continue;
        }
        if trimmed.starts_with("ret ") || trimmed == "}" {
            continue;
        }
        body.push(line);
    }
    assert!(pending.is_none(), "trailing CHECK with no instruction");
    assert_eq!(expectations.len(), 8, "fixture CHECK count changed");
    assert_eq!(body.len(), 13, "fixture call count changed");

    let mut blocked = 0;
    for line in &body {
        let source = format!("{prelude}\n{header}\n{line}\n  ret void\n}}\n");
        let answer = parse_and_verify("verifier-operand-bundles", &source);
        if line.contains("ptr @llvm.") {
            blocked += 1;
            assert_eq!(answer, Err(ENTRY_37.to_owned()), "entry 37 blocks {line:?}");
            continue;
        }
        let answer = answer.unwrap_or_else(|e| panic!("{line:?} parses: {e}"));
        match expectations
            .iter()
            .find(|(instruction, _)| *instruction == line.trim())
        {
            Some((_, expected)) => {
                let message =
                    answer.expect_err(&format!("{line:?} must be rejected by the verifier"));
                assert!(
                    message.contains(expected),
                    "{line:?}: {message:?} does not contain {expected:?}"
                );
            }
            None => assert_eq!(
                answer,
                Ok(()),
                "{line:?} is under CHECK-NOT and must verify"
            ),
        }
    }
    assert_eq!(blocked, 7, "entry 37's blocked set changed");
}
