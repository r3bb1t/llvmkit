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
/// position ("'%x' defined with type 'i32' but expected 'ptr'"): a
/// non-pointer local cannot be a callee. llvmkit surfaces the rule when
/// converting the parsed callee value to a pointer; no upstream lit
/// coverage of the diagnostic, rule shape is the anchor (D11).
#[test]
fn indirect_call_non_pointer_callee_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/indirect_call_non_pointer_callee_rejected.ll"
    );

    assert_fixture_rejected(
        "indirect_call_non_pointer_callee_rejected",
        FIXTURE,
        "pointer callee: type mismatch: expected pointer, got integer",
    );
}

/// llvmkit-specific GAP lock: upstream `test/Assembler/call-arg-is-callee.ll`
/// `@invoke` accepts an indirect invoke (`parseInvoke` shares `parseCall`'s
/// callee path); llvmkit has no indirect-invoke builder yet, so the parser
/// rejects the resolved indirect callee with a deliberate diagnostic
/// instead of the pre-port generic parse failure.
#[test]
fn invoke_indirect_callee_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/invoke_indirect_callee_rejected.ll");

    assert_fixture_rejected(
        "invoke_indirect_callee_rejected",
        FIXTURE,
        "direct function callee for invoke",
    );
}

/// llvmkit-specific STRICTNESS lock: upstream parses an indirect callbr but
/// `Verifier::visitCallBrInst` rejects it ("Callbr: indirect function /
/// invalid signature"); llvmkit rejects at parse time.
#[test]
fn callbr_indirect_callee_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseCall/callbr_indirect_callee_rejected.ll");

    assert_fixture_rejected(
        "callbr_indirect_callee_rejected",
        FIXTURE,
        "direct function callee for callbr",
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
/// `build_invoke_dyn_with_config`.
#[test]
fn invoke_explicit_type_arg_type_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/invoke_explicit_type_arg_type_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "invoke_explicit_type_arg_type_mismatch_rejected",
        FIXTURE,
        "valid invoke: call argument #0 type mismatch: expected integer, got float",
    );
}

/// Crafted against `parseCallBr`'s argument loop with an explicit
/// call-site type — same rule as
/// [`invoke_explicit_type_arg_type_mismatch_rejected`], surfaced through
/// `build_callbr_with_config`.
#[test]
fn callbr_explicit_type_arg_type_mismatch_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseCall/callbr_explicit_type_arg_type_mismatch_rejected.ll"
    );

    assert_fixture_rejected(
        "callbr_explicit_type_arg_type_mismatch_rejected",
        FIXTURE,
        "valid callbr: call argument #0 type mismatch: expected integer, got float",
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
