//! Parser integration tests for remaining S3.2 opcodes.
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_fixture(src: &[u8]) -> String {
    Module::with_new("test", |module| {
        let _ = Parser::new(src, &module)
            .expect("parse constructor")
            .parse_module()
            .expect("parse succeeded");
        format!("{module}")
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

// ── Cast ops ─────────────────────────────────────────────────────────────────

/// `bitcast` parser coverage from `test/Assembler/2006-12-09-Cast-To-Bool.ll`.
#[test]
fn bitcast_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/2006-12-09-Cast-To-Bool/bitcast_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["bitcast"]);
}

/// `fptrunc double %src to float` from `test/Bitcode/conversionInstructions.3.2.ll`.
#[test]
fn fptrunc_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/conversionInstructions.3.2/fptrunc_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["%res1 = fptrunc double %src to float"]);
}

/// `fpext float %src to double` from `test/Bitcode/conversionInstructions.3.2.ll`.
#[test]
fn fpext_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/conversionInstructions.3.2/fpext_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["%res1 = fpext float %src to double"]);
}

// ── Vector ops ────────────────────────────────────────────────────────────────

/// `extractelement` parser coverage from `test/Bitcode/vectorInstructions.3.2.ll`.
#[test]
fn extractelement_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/vectorInstructions.3.2/extractelement_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["%res1 = extractelement <2 x i8> %x1, i32 0"]);
}

/// `insertelement` parser coverage from `test/Bitcode/vectorInstructions.3.2.ll`.
#[test]
fn insertelement_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/vectorInstructions.3.2/insertelement_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["%res1 = insertelement <2 x i8> %x1, i8 0, i32 0"]);
}

/// `shufflevector` parser coverage from `test/Bitcode/vectorInstructions.3.2.ll`.
#[test]
fn shufflevector_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/vectorInstructions.3.2/shufflevector_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(
        &text,
        &[
            "%res1 = shufflevector <2 x i8> %x1, <2 x i8> %x1, <2 x i32> <i32 0, i32 1>",
            "%res2 = shufflevector <2 x i8> %x1, <2 x i8> undef, <2 x i32> <i32 0, i32 1>",
        ],
    );
}

/// `shufflevector` typed `zeroinitializer` mask operand from
/// `test/Bitcode/compatibility.ll` line 1539.
#[test]
fn shufflevector_zeroinitializer_mask_operand_round_trips() {
    let text = parse_fixture(
        b"define <2 x i8> @shuffle_zero_mask(<2 x i8> %x, <2 x i8> %y) {\n\
entry:\n\
  %res = shufflevector <2 x i8> %x, <2 x i8> %y, <2 x i32> zeroinitializer\n\
  ret <2 x i8> %res\n\
}\n",
    );
    assert_check_lines(
        &text,
        &["%res = shufflevector <2 x i8> %x, <2 x i8> %y, <2 x i32> zeroinitializer"],
    );
}

// ── Aggregate ops ─────────────────────────────────────────────────────────────

/// llvmkit-specific opaque-pointer subset of `test/Assembler/insertextractvalue.ll`.
#[test]
fn extractvalue_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/insertextractvalue/extractvalue_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(
        &text,
        &[
            "@foo",
            "load",
            "extractvalue",
            "insertvalue",
            "store",
            "ret",
        ],
    );
}

/// llvmkit-specific opaque-pointer subset of `test/Assembler/insertextractvalue.ll`.
#[test]
fn insertvalue_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/insertextractvalue/insertvalue_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(
        &text,
        &[
            "@foo",
            "load",
            "extractvalue",
            "insertvalue",
            "store",
            "ret",
        ],
    );
}

// ── SSA/control-flow ──────────────────────────────────────────────────────────

/// Zero-input `phi i32` from `test/Assembler/zero-input-phi.ll`.
#[test]
fn phi_int_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/zero-input-phi/phi_int_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(
        &text,
        &[
            "define void @dead_phi()",
            "entry:",
            "ret void",
            "return:",
            "%r = phi i32",
            "ret void",
        ],
    );
}

/// llvmkit-specific direct-call subset of `test/Bitcode/miscInstructions.3.2.ll`.
#[test]
fn call_function_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/miscInstructions.3.2/call_function_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["%res1 = call i32 @test(i32 %x)"]);
}

/// llvmkit-specific named-result subset of `test/Bitcode/compatibility.ll`.
#[test]
fn freeze_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/compatibility/freeze_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["freeze i32 %op1"]);
}

// ── Terminators ───────────────────────────────────────────────────────────────

/// Minimal empty switch from `test/Assembler/2003-05-15-SwitchBug.ll`.
#[test]
fn switch_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/2003-05-15-SwitchBug/switch_round_trips.ll");
    const EXPECTED: &str = "\
; ModuleID = 'test'
define void @test(i32 %X) {
0:
  switch i32 %X, label %dest [
  ]

dest:
  ret void
}
";

    let text = parse_fixture(FIXTURE);
    assert_eq!(text, EXPECTED);
}

/// llvmkit-specific opaque-pointer subset of `test/Bitcode/terminatorInstructions.3.2.ll`.
#[test]
fn indirectbr_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/terminatorInstructions.3.2/indirectbr_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["indirectbr ptr %Addr, [label %bb1, label %bb2]"]);
}

// ── Atomic ops ────────────────────────────────────────────────────────────────

/// `fence` syncscope/order cases from `test/Assembler/atomic.ll`.
#[test]
fn fence_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/atomic/fence_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(
        &text,
        &[
            "fence syncscope(\"singlethread\") release",
            "fence seq_cst",
            "fence syncscope(\"device\") seq_cst",
        ],
    );
}

/// `cmpxchg` opaque-pointer case from `test/Assembler/opaque-ptr.ll`.
#[test]
fn cmpxchg_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/opaque-ptr/cmpxchg_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(
        &text,
        &[
            "define void @cmpxchg(ptr %p, i32 %a, i32 %b)",
            "%val_success = cmpxchg ptr %p, i32 %a, i32 %b acq_rel monotonic",
            "ret void",
        ],
    );
}

/// Forward-reference `atomicrmw` case from `test/Assembler/atomicrmw.ll`.
#[test]
fn atomicrmw_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/atomicrmw/atomicrmw_round_trips.ll");

    let text = parse_fixture(FIXTURE);
    assert_check_lines(&text, &["@f", "atomicrmw"]);
}
