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

/// llvmkit-specific callbr successor subset of `test/Assembler/callbr.ll`.
#[test]
fn callbr_successor_structure_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/callbr/callbr_successor_structure_round_trips.ll");

    let text = parse_and_render_bytes("callbr_successor_structure_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "callbr void @callee(i1 %c)",
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
