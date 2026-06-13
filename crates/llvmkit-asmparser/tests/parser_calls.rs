//! Call / invoke / callbr parser tests.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    let module = Module::new("parser_calls");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    format!("{module}")
}

/// Mirrors `test/Assembler/inline-asm.ll`: plain calls may use inline asm
/// as the callee.
#[test]
fn inline_asm_void_call_round_trips() {
    let text = parse_and_render(
        "define void @f() {\nentry:\n  call void asm sideeffect \"\", \"\"()\n  ret void\n}\n",
    );
    assert!(
        text.contains("call void asm sideeffect \"\", \"\"()"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("call void asm sideeffect \"\", \"\"()"));
}

/// Mirrors `test/Assembler/inline-asm.ll`: inline-asm modifiers preserve
/// canonical keyword order.
#[test]
fn inline_asm_intel_alignstack_unwind_round_trips() {
    let text = parse_and_render(
        "define void @f() {\nentry:\n  call void asm sideeffect alignstack inteldialect unwind \"nop\", \"\"()\n  ret void\n}\n",
    );
    assert!(
        text.contains("call void asm sideeffect alignstack inteldialect unwind \"nop\", \"\"()"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `test/Assembler/invoke.ll` with inline asm as callee.
#[test]
fn inline_asm_invoke_round_trips() {
    let text = parse_and_render(
        "define void @f() {\nentry:\n  invoke void asm sideeffect \"\", \"\"() to label %ok unwind label %bad\nok:\n  ret void\nbad:\n  unreachable\n}\n",
    );
    assert!(
        text.contains("invoke void asm sideeffect \"\", \"\"() to label %ok unwind label %bad"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `test/Assembler/callbr.ll` with inline asm as callee.
#[test]
fn inline_asm_callbr_round_trips() {
    let text = parse_and_render(
        "define void @f() {\nentry:\n  callbr void asm sideeffect \"jmp ${0:l}\", \"!i\"() to label %fallthrough [label %target]\nfallthrough:\n  ret void\ntarget:\n  ret void\n}\n",
    );
    assert!(
        text.contains("callbr void asm sideeffect \"jmp ${0:l}\", \"!i\"() to label %fallthrough [label %target]"),
        "AsmWriter output: {text}"
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
    assert!(
        text.contains("%r = tail call fastcc zeroext i32 @callee(i32 zeroext %x) #0"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `test/Bitcode/operand-bundles.ll`: call/invoke operand-bundle
/// lists are parsed into CallBase storage and printed after call-site attrs.
#[test]
fn operand_bundles_round_trip() {
    let text = parse_and_render(
        "declare void @callee0()\n\
define void @f(i32 %x) {\n\
entry:\n\
  call void @callee0() [ \"foo\"(i32 42, i32 %x), \"bar\"() ]\n\
  invoke void @callee0() [ \"foo\"(i32 %x) ] to label %ok unwind label %bad\n\
ok:\n\
  ret void\n\
bad:\n\
  unreachable\n\
}\n",
    );
    assert!(
        text.contains("call void @callee0() [\"foo\"(i32 42, i32 %x), \"bar\"()]"),
        "AsmWriter output: {text}"
    );
    assert!(
        text.contains(
            "invoke void @callee0() [\"foo\"(i32 %x)]\n          to label %ok unwind label %bad"
        ),
        "AsmWriter output: {text}"
    );
}
