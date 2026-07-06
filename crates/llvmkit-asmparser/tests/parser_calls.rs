//! Call / invoke / callbr parser tests.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    parse_and_render_bytes("parser_calls", src.as_bytes())
}

fn parse_and_render_bytes(module_name: &str, src: &[u8]) -> String {
    Module::with_new(module_name, |module| {
        Parser::new(src, &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        format!("{module}")
    })
}

fn parse_fixture_err(module_name: &str, src: &[u8]) -> ParseError {
    Module::with_new(module_name, |module| {
        Parser::new(src, &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("parser rejects malformed input")
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

/// llvmkit-specific subset of `test/Assembler/inline-asm-constraint-error.ll`:
/// non-callbr inline asm may not carry label constraints.
#[test]
fn inline_asm_call_label_constraint_subset() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/inline-asm-constraint-error/inline_asm_call_label_constraint_subset.ll"
    );

    let err = parse_fixture_err("inline_asm_call_label_constraint_subset", FIXTURE);
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "inline asm call without label constraints")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// llvmkit-specific subset of `test/Assembler/inline-asm-constraint-error.ll`:
/// callbr inline asm must provide one label constraint per indirect label.
#[test]
fn inline_asm_callbr_label_constraints_subset() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/inline-asm-constraint-error/inline_asm_callbr_label_constraints_subset.ll"
    );

    let err = parse_fixture_err("inline_asm_callbr_label_constraints_subset", FIXTURE);
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(
            expected,
            "inline asm callbr label constraint count matches indirect labels"
        ),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `test/Assembler/callbr.ll` successor structure with the upstream
/// `@llvm.amdgcn.kill` intrinsic callee.
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
            "cont:",
            "ret void",
            "kill:",
            "unreachable",
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

/// llvmkit-specific subset of `test/Bitcode/operand-bundles.ll`: call/invoke
/// operand-bundle lists are parsed into CallBase storage and printed after call-site attrs.
#[test]
fn operand_bundles_round_trip() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/operand-bundles/operand_bundles_round_trip.ll");

    let text = parse_and_render_bytes("operand_bundles_round_trip", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "call void @callee0() [\"foo\"(i32 42, i32 %x), \"bar\"()]",
            "invoke void @callee0() [\"foo\"(i32 %x)]\n          to label %ok unwind label %bad",
        ],
    );
}

fn assert_fixture_rejected(module_name: &str, src: &[u8], expected_message: &str) {
    let err = parse_fixture_err(module_name, src);
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, expected_message),
        other => panic!("unexpected error variant: {other:?}"),
    }
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
        "valid call: call argument #0 type mismatch: expected integer, got float",
    );
}

/// Crafted against the same `parseCall` argument-loop rule as
/// [`call_explicit_type_arg_type_mismatch_rejected`]: the comparison is type
/// IDENTITY, so i8-vs-i32 is rejected even though both sides render as the
/// `integer` kind label in the diagnostic.
#[test]
fn call_explicit_type_arg_width_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_explicit_type_arg_width_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "call_explicit_type_arg_width_mismatch_rejected",
        FIXTURE,
        "valid call: call argument #0 type mismatch: expected integer, got integer",
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
        "valid call: call argument count mismatch: expected 2, got 1",
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
        "valid call: call argument count mismatch: expected 0, got 1",
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
        "valid call: call argument count mismatch: expected 1, got 0",
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
/// `build_indirect_call_dyn`'s `validate_call_site_args` gate.
#[test]
fn indirect_call_arg_type_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_arg_type_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "indirect_call_arg_type_mismatch_rejected",
        FIXTURE,
        "valid indirect call: call argument #0 type mismatch: expected integer, got float",
    );
}

/// llvmkit-specific STRICTNESS lock: upstream `parseCall` infers the
/// call-site type from the argument list and ACCEPTS a direct call whose
/// shape disagrees with the declaration (legal-but-UB IR under opaque
/// pointers). llvmkit's `resolve_direct_callee` rejects it at parse time.
/// Complements `parser_forward_refs.rs::
/// forward_global_reference_signature_mismatch_is_rejected`, which locks
/// the same diagnostic for return-type drift between forward references.
#[test]
fn call_inferred_signature_arg_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/call_inferred_signature_arg_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "call_inferred_signature_arg_mismatch_rejected",
        FIXTURE,
        "function callee signature mismatch",
    );
}

/// llvmkit-specific STRICTNESS lock, invoke form: `parseInvoke` always
/// infers the call-site type from the argument list (upstream accepts the
/// mismatch; llvmkit's `resolve_direct_callee` rejects at parse time).
#[test]
fn invoke_inferred_signature_arg_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/invoke_inferred_signature_arg_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "invoke_inferred_signature_arg_mismatch_rejected",
        FIXTURE,
        "function callee signature mismatch",
    );
}

/// llvmkit-specific STRICTNESS lock, callbr form: `parseCallBr` always
/// infers the call-site type from the argument list (upstream accepts the
/// mismatch at parse time; llvmkit's `resolve_direct_callee` rejects).
#[test]
fn callbr_inferred_signature_arg_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/callbr_inferred_signature_arg_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "callbr_inferred_signature_arg_mismatch_rejected",
        FIXTURE,
        "function callee signature mismatch",
    );
}
