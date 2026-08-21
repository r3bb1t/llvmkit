//! Function-body parser integration tests (Session 3).
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::parser;

fn parse_and_print(src: &str) -> String {
    parser::parse_assembly(src, |module, _parsed| format!("{module}")).expect("parse")
}

fn parse_and_verify(src: &str) {
    let verify =
        parser::parse_assembly(src, |module, _parsed| module.verify_borrowed()).expect("parse");
    verify.expect("verify");
}

fn parse_expect_error(src: &str) -> String {
    match parser::parse_assembly(src, |_module, _parsed| ()) {
        Ok(()) => panic!("expected parse to fail, but it succeeded"),
        Err(e) => format!("{e}"),
    }
}

/// Mirrors `LLParser::parseRet`'s `void` arm on the smallest body shape:
/// `define void @f() { ret void }`.
#[test]
fn parses_void_function_body() {
    let printed = parse_and_print("define void @f() {\nentry:\n  ret void\n}\n");
    assert!(printed.contains("define void @f() {\n"));
    assert!(printed.contains("ret void\n"));
}

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, ...)`'s
/// add-then-return shape and the `factorial.rs` example's first block.
#[test]
fn parses_int_add_and_ret() {
    let printed = parse_and_print(
        "define i32 @add(i32 %a, i32 %b) {\nentry:\n  %r = add i32 %a, %b\n  ret i32 %r\n}\n",
    );
    assert!(printed.contains("%r = add i32 %a, %b\n"));
    assert!(printed.contains("ret i32 %r\n"));
}

/// Numbered parameters are valid local names. Mirrors
/// `LLParser::PerFunctionState`'s numbered-value bookkeeping.
#[test]
fn parses_numbered_params() {
    let printed = parse_and_print(
        "define i32 @add(i32, i32) {\nentry:\n  %r = add i32 %0, %1\n  ret i32 %r\n}\n",
    );
    assert!(printed.contains("ret i32 %r\n"));
}

/// `unreachable` terminator. Mirrors `LLParser::parseUnreachable`.
#[test]
fn parses_unreachable_terminator() {
    let printed = parse_and_print("define void @abort() {\nentry:\n  unreachable\n}\n");
    assert!(printed.contains("\n  unreachable\n"));
}

/// Conditional branch with `icmp eq` predicate. Mirrors the entry-block
/// arm of `crates/llvmkit-ir/examples/factorial.rs`.
#[test]
fn parses_icmp_and_cond_br() {
    let printed = parse_and_print(
        "define i32 @abs(i32 %x) {\n\
         entry:\n  \
           %is_zero = icmp eq i32 %x, 0\n  \
           br i1 %is_zero, label %zero_path, label %nonzero\n\
         zero_path:\n  \
           ret i32 0\n\
         nonzero:\n  \
           ret i32 %x\n\
         }\n",
    );
    assert!(printed.contains("%is_zero = icmp eq i32 %x, 0\n"));
    assert!(printed.contains("br i1 %is_zero, label %zero_path, label %nonzero\n"));
}

/// Forward block reference: `br label %later` before `later:` is parsed.
/// Mirrors `LLParser::PerFunctionState::getBB`'s forward-reference path.
#[test]
fn parses_forward_block_reference() {
    let printed = parse_and_print(
        "define void @forward() {\n\
         entry:\n  \
           br label %later\n\
         later:\n  \
           ret void\n\
         }\n",
    );
    assert!(printed.contains("br label %later\n"));
    assert!(printed.contains("ret void\n"));
}

/// Regression distilled from `llvm/test/Verifier/range-2.ll::invoke_all`:
/// `LLParser.cpp::parseBasicBlock` defines unlabeled post-terminator blocks
/// through `PerFunctionState::defineBB(Name.empty())`, consuming the same
/// numbered frontier used by the later `%2 = add`.
#[test]
fn parses_implicit_unnamed_blocks_with_shared_numbering() {
    let src = "define i32 @implicit_slots(i1 %cond, i32 %x) {\n\
               entry:\n  \
                 br i1 %cond, label %0, label %1\n  \
                 br label %1\n  \
                 %2 = add i32 %x, 1\n  \
                 ret i32 %2\n\
               }\n";

    parse_and_verify(src);
    let printed = parse_and_print(src);

    assert!(
        printed.contains("br i1 %cond, label %0, label %1\n"),
        "{printed}"
    );
    // Block `1`'s predecessors read `%0, %entry`: `predecessors(BB)` walks the
    // use list, which `Value::addUse` head-inserts, so the newer user (block
    // `0`'s `br label %1`) comes before the older one (the entry's `br i1`).
    assert!(
        printed.contains(
            "0:                                                ; preds = %entry\n  br label %1\n"
        ),
        "{printed}"
    );
    assert!(
        printed.contains(
            "1:                                                ; preds = %0, %entry\n  %2 = add i32 %x, 1\n  ret i32 %2\n"
        ),
        "{printed}"
    );
}

/// Mirrors `LLParser::setInstName(NameID=-1, NameStr="")`: an unnamed
/// non-void `callbr` result still consumes the next numbered local slot.
#[test]
fn parses_unnamed_non_void_callbr_result_numbering() {
    let src = "declare i32 @callee()\n\
               define i32 @callbr_unnamed_result() {\n\
               entry:\n  \
                 callbr i32 @callee() to label %fallthrough []\n\
               fallthrough:\n  \
                 ret i32 %0\n\
               }\n";

    parse_and_verify(src);
    let printed = parse_and_print(src);
    assert!(printed.contains("callbr i32 @callee()"), "{printed}");
    assert!(printed.contains("ret i32 %0"), "{printed}");
}

/// Mirrors `LLParser::parseBasicBlock`: quoted digit-only labels are textual
/// labels, not numbered-label definitions.
#[test]
fn parses_quoted_numeric_label_as_named_block() {
    let src = "define void @quoted_numeric_label() {\n\
               entry:\n  \
                 br label %\"42\"\n\
               \"42\":\n  \
                 ret void\n\
               }\n";

    parse_and_verify(src);
    let printed = parse_and_print(src);
    assert!(printed.contains("br label %\"42\""), "{printed}");
    assert!(printed.contains("\"42\":"), "{printed}");
}

/// Mirrors `LLParser::PerFunctionState::defineBB`: defining a previously
/// forward-referenced numbered block moves it to the textual definition point.
#[test]
fn parses_forward_numbered_block_in_definition_order() {
    let src = "define i32 @forward_numbered_block_order(i1 %cond, i32 %x) {\n\
               entry:\n  \
                 br i1 %cond, label %1, label %0\n  \
                 ret i32 %x\n\
               1:\n  \
                 %2 = add i32 %x, 1\n  \
                 ret i32 %2\n\
               }\n";

    parse_and_verify(src);
    let printed = parse_and_print(src);
    let zero_pos = printed
        .find("0:                                                ; preds = %entry\n  ret i32 %x")
        .expect("prints block 0");
    let one_pos = printed
        .find(
            "1:                                                ; preds = %entry\n  %2 = add i32 %x, 1",
        )
        .expect("prints block 1");
    assert!(zero_pos < one_pos, "{printed}");
}

/// Mirrors `test/Assembler/callbr.ll`, whose
/// `RUN: llvm-as < %s | llvm-dis | FileCheck %s` makes its CHECK block
/// `AssemblyWriter` output. The source defines `kill` before `cont` but the
/// `callbr` names `cont` first, and upstream's CHECK order is
/// `[[KILL:.*:]]` / `unreachable` / `[[CONT]]:` / `ret void` — definition
/// order, because `LLParser::PerFunctionState::defineBB` ends with
/// `F.splice(F.end(), &F, BB->getIterator())` under the comment "Move the
/// block to the end of the function. Forward ref'd blocks are inserted
/// wherever they happen to be referenced."
#[test]
fn forward_referenced_named_block_prints_in_definition_order() {
    const FIXTURE: &str = include_str!("fixtures/upstream/assembler-corpus/callbr.ll");

    let printed = parse_and_print(FIXTURE);
    let kill = printed.find("\nkill:").expect("prints block kill");
    let cont = printed.find("\ncont:").expect("prints block cont");
    assert!(kill < cont, "{printed}");
}

/// No upstream counterpart: this is llvmkit's own statement that block-list
/// order drives the slot tracker's unnamed-value numbering, so the `defineBB`
/// splice is what keeps a printed slot number equal to the one the source
/// wrote. The rule source is `AsmWriter.cpp`'s `SlotTracker`, which walks
/// `for (auto &BB : F)` and numbers unnamed blocks and unnamed instruction
/// results from one shared counter; the shape here is distilled from
/// `test/Assembler/callbr.ll`'s forward-reference pattern.
#[test]
fn out_of_order_named_blocks_do_not_renumber_unnamed_values() {
    let printed = parse_and_print(
        "define i32 @f(i1 %c) {\n\
         entry:\n  br i1 %c, label %b, label %a\n\
         a:\n  %0 = add i32 1, 2\n  ret i32 %0\n\
         b:\n  %1 = add i32 3, 4\n  ret i32 %1\n\
         }\n",
    );
    assert!(printed.contains("%0 = add i32 1, 2"), "{printed}");
    assert!(printed.contains("%1 = add i32 3, 4"), "{printed}");
}

/// Mirrors `test/Assembler/block-labels.ll::@test2` against the vendored
/// fixture, whose CHECK block is
/// `; CHECK-LABEL: define void @test2(i32 %0, i32 %1) {` followed immediately
/// by `; CHECK-NEXT:    ret void` — an explicitly written `2:` entry label is
/// not printed, because `AssemblyWriter::printBasicBlock` takes the
/// slot-label branch only when `!IsEntryBlock`, and `printFunction` writes
/// `Out << " {"` without a newline so the block owns it.
#[test]
fn an_unnamed_entry_block_prints_no_label() {
    let printed = parse_and_print(BLOCK_LABELS_FIXTURE);
    assert!(
        printed.contains("define void @test2(i32 %0, i32 %1) {\n  ret void\n"),
        "{printed}"
    );
}

/// `test/Assembler/block-labels.ll`, vendored whole and parsed whole. Its RUN
/// line is
/// `llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s --match-full-lines`,
/// so every `; CHECK` line in it is `AssemblyWriter` output and a legitimate
/// byte oracle.
const BLOCK_LABELS_FIXTURE: &str =
    include_str!("fixtures/upstream/assembler-corpus/block-labels.ll");

/// Mirrors `test/Assembler/block-labels.ll::@test1`'s CHECK block against the
/// vendored fixture: **15 of its 17 CHECK lines**, the two exceptions being
/// blocked and named below. Each non-entry block carries `printBasicBlock`'s
/// predecessors comment, and `printLLVMName` re-quotes the label the way it
/// quotes any other name, which is what the `"2"`, `-3` and `-N-` blocks are
/// in the fixture to show: a quoted digit-only label stays quoted, and a
/// name that merely *looks* numeric or contains `-` prints bare.
///
/// FileCheck runs without `--strict-whitespace` and so canonicalizes the run
/// of spaces in `; CHECK:      2:       ; preds = %0`; the column asserted
/// here comes from `Out.PadToColumn(50)` itself, which the CHECK lines cannot
/// pin.
///
/// **Partial, and this is the whole of what is left out.** CHECK lines **13
/// and 14 of 17** — `; CHECK-NEXT:   br label %"$N"` and
/// `; CHECK:      "$N":    ; preds = %-N-` — are not asserted, because
/// llvmkit's `fmt_llvm_name_without_prefix` allows `$` in an unquoted name
/// where `printLLVMNameWithoutPrefix` does not, so llvmkit prints `$N` and
/// `br label %$N`. That is divergence **100** in `docs/divergences.md`, which
/// is queued for its own commit; closing it turns this test into a full byte
/// comparison of the fixture and lets its corpus row carry `expect=`.
///
/// They are lines 13-14, not the last two: lines 15-17 (`%4 = add i32 1, 1`,
/// `ret i32 %4`, `}`) sit *after* the `$N` block, are **not** blocked by
/// divergence 100, and are asserted at the end of this test. Nothing else in
/// `@test1` is skipped.
#[test]
fn non_entry_blocks_print_a_predecessors_comment() {
    let printed = parse_and_print(BLOCK_LABELS_FIXTURE);

    // `; CHECK-LABEL: define i32 @test1(i32 %X) {` / `; CHECK-NEXT:` x2 --
    // the implicit entry label is not printed, and it keeps slot 0.
    assert!(
        printed
            .contains("define i32 @test1(i32 %X) {\n  %1 = alloca i32, align 4\n  br label %2\n"),
        "{printed}"
    );

    // The label / predecessors-comment pairs, each followed by the branch the
    // fixture's `; CHECK-NEXT:` pins.
    for (label_line, next_line) in [
        (
            "2:                                                ; preds = %0\n",
            "  br label %3\n",
        ),
        (
            "3:                                                ; preds = %2\n",
            "  br label %\"2\"\n",
        ),
        (
            "\"2\":                                              ; preds = %3\n",
            "  br label %-3\n",
        ),
        (
            "-3:                                               ; preds = %\"2\"\n",
            "  br label %-N-\n",
        ),
        (
            "-N-:                                              ; preds = %-3\n",
            "",
        ),
    ] {
        let expected = format!("{label_line}{next_line}");
        assert!(
            printed.contains(&expected),
            "missing {expected:?} in:\n{printed}"
        );
    }

    // CHECK lines 15-17: `; CHECK-NEXT:   %4 = add i32 1, 1` /
    // `; CHECK-NEXT:   ret i32 %4` / `; CHECK-NEXT: }`. These follow the `$N`
    // block, so they are not reached by the loop above, and nothing about them
    // is blocked by divergence 100. Two CHECK lines sit between the loop's
    // last assertion and this tail, and both are blocked by that divergence:
    // 13 (`; CHECK-NEXT:   br label %"$N"`, which is why the `-N-` entry above
    // carries an empty `next_line`) and 14 (`; CHECK:      "$N":    ; preds =
    // %-N-`). Asserted as one contiguous run so the gap is provably that
    // two-line `$N` pair and nothing else.
    assert!(
        printed.contains("  %4 = add i32 1, 1\n  ret i32 %4\n}\n"),
        "{printed}"
    );
}

/// Ports `test/Assembler/2002-08-15-ConstantExprProblem.ll` whole, from the
/// vendored copy, as the oracle for `printBasicBlock`'s `pred_empty(BB)` arm
/// and for the use-list order `predecessors(BB)` reads. Its **first** RUN line
/// is `llvm-as %s -o /dev/null`, so its `; preds = %BB2, %0` and
/// `; No predecessors!` comments are hand-written *input* rather than
/// FileCheck-verified output — they document the spelling, and the rule source
/// is `printBasicBlock` itself. The predecessor order is upstream's:
/// `PredIterator` walks the block's use list, which `Use::addToList`
/// head-inserts, so `BB2`'s later `br` prints before the entry block's.
///
/// The fixture has a **second** RUN line, `verify-uselistorder %s`, which has
/// no llvmkit counterpart: nothing here re-materialises a use list from a
/// shuffled `uselistorder` directive and compares. That half is unported, and
/// this test covers the `llvm-as` half only.
#[test]
fn an_unreachable_block_prints_no_predecessors() {
    const FIXTURE: &str =
        include_str!("fixtures/upstream/assembler-corpus/2002-08-15-ConstantExprProblem.ll");

    let printed = parse_and_print(FIXTURE);
    assert!(
        printed.contains("BB1:                                              ; preds = %BB2, %0\n"),
        "{printed}"
    );
    assert!(
        printed.contains("BB2:                                              ; No predecessors!\n"),
        "{printed}"
    );
}

/// Sub / mul arms of `parse_int_binop`. Mirrors the loop body of
/// `crates/llvmkit-ir/examples/factorial.rs` (next_acc / next_i lines).
#[test]
fn parses_sub_and_mul() {
    let printed = parse_and_print(
        "define i32 @poly(i32 %x) {\n\
         entry:\n  \
           %a = sub i32 %x, 1\n  \
           %b = mul i32 %a, %x\n  \
           ret i32 %b\n\
         }\n",
    );
    assert!(printed.contains("%a = sub i32 %x, 1\n"));
    assert!(printed.contains("%b = mul i32 %a, %x\n"));
}

/// Negative test: an unsupported opcode in this session is reported as a
/// typed parse error, not a silent miss. Mirrors `LLParser`'s
/// `tokError("expected instruction opcode")` site for the default arm.
/// Uses `store` (no result) with an LHS `%x =` to trigger the `_` arm.
#[test]
fn unsupported_opcode_is_typed_error() {
    let err = parser::parse_assembly(
        "define i32 @f(i32 %a) {\nentry:\n  %x = store i32 %a, ptr null\n  ret i32 %a\n}\n",
        |_module, _parsed| (),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("instruction opcode supported by this parser"),
        "got: {msg}"
    );
}

/// Ports the udiv / sdiv / urem / srem arms of
/// `LLParser::parseInstruction` (`Instruction::UDiv`, etc.).
#[test]
fn parses_div_and_rem_opcodes() {
    let printed = parse_and_print(
        "define i32 @divrem(i32 %a, i32 %b) {\nentry:\n  \
           %u = udiv i32 %a, %b\n  \
           %s = sdiv i32 %a, %b\n  \
           %ur = urem i32 %a, %b\n  \
           %sr = srem i32 %a, %b\n  \
           ret i32 %sr\n\
         }\n",
    );
    assert!(printed.contains("%u = udiv i32 %a, %b\n"));
    assert!(printed.contains("%s = sdiv i32 %a, %b\n"));
    assert!(printed.contains("%ur = urem i32 %a, %b\n"));
    assert!(printed.contains("%sr = srem i32 %a, %b\n"));
}

/// Ports the bitwise / shift arms of `LLParser::parseInstruction`
/// (`Instruction::Shl` / `LShr` / `AShr` / `And` / `Or` / `Xor`).
#[test]
fn parses_shift_and_bitwise_opcodes() {
    let printed = parse_and_print(
        "define i32 @bits(i32 %a, i32 %b) {\nentry:\n  \
           %s1 = shl i32 %a, 1\n  \
           %s2 = lshr i32 %s1, 1\n  \
           %s3 = ashr i32 %s2, 1\n  \
           %s4 = and i32 %s3, %b\n  \
           %s5 = or i32 %s4, %b\n  \
           %s6 = xor i32 %s5, %b\n  \
           ret i32 %s6\n\
         }\n",
    );
    assert!(printed.contains("%s1 = shl i32 %a, 1\n"));
    assert!(printed.contains("%s2 = lshr i32 %s1, 1\n"));
    assert!(printed.contains("%s3 = ashr i32 %s2, 1\n"));
    assert!(printed.contains("%s4 = and i32 %s3, %b\n"));
    assert!(printed.contains("%s5 = or i32 %s4, %b\n"));
    assert!(printed.contains("%s6 = xor i32 %s5, %b\n"));
}

/// Ports `LLParser::parseCast` integer arm: `trunc` / `zext` / `sext`.
/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`.
#[test]
fn parses_int_casts() {
    let printed = parse_and_print(
        "define i64 @widen(i32 %a) {\nentry:\n  \
           %t = trunc i32 %a to i16\n  \
           %z = zext i16 %t to i32\n  \
           %s = sext i32 %z to i64\n  \
           ret i64 %s\n\
         }\n",
    );
    assert!(printed.contains("%t = trunc i32 %a to i16\n"));
    assert!(printed.contains("%z = zext i16 %t to i32\n"));
    assert!(printed.contains("%s = sext i32 %z to i64\n"));
}

/// Ports `LLParser::parseCast`'s `Instruction::PtrToInt` /
/// `Instruction::IntToPtr` arms.
#[test]
fn parses_ptr_int_casts() {
    let printed = parse_and_print(
        "define i64 @addr(ptr %p) {\nentry:\n  \
           %i = ptrtoint ptr %p to i64\n  \
           %q = inttoptr i64 %i to ptr\n  \
           %j = ptrtoint ptr %q to i64\n  \
           ret i64 %j\n\
         }\n",
    );
    assert!(printed.contains("%i = ptrtoint ptr %p to i64\n"));
    assert!(printed.contains("%q = inttoptr i64 %i to ptr\n"));
}

/// Exact scalar instruction excerpt from `llvm/test/Assembler/ptrtoaddr.ll`.
#[test]
fn parses_ptrtoaddr_instruction_distinct_from_ptrtoint() {
    let printed = parse_and_print(
        "target datalayout = \"p1:64:64:64:32\"\n\
         define i64 @test_as0(ptr %p) {\n\
           %addr = ptrtoaddr ptr %p to i64\n\
           ret i64 %addr\n\
         }\n",
    );
    assert!(printed.contains("%addr = ptrtoaddr ptr %p to i64\n"));
}

/// Exact scalar addrspace(1) instruction excerpt from
/// `llvm/test/Assembler/ptrtoaddr.ll` lines 17-21.
#[test]
fn parses_ptrtoaddr_as1_scalar_instruction() {
    let printed = parse_and_print(
        "target datalayout = \"p1:64:64:64:32\"\n\
         define i32 @test_as1(ptr addrspace(1) %p) {\n\
           %addr = ptrtoaddr ptr addrspace(1) %p to i32\n\
           ret i32 %addr\n\
         }\n",
    );
    assert!(printed.contains("%addr = ptrtoaddr ptr addrspace(1) %p to i32\n"));
}

/// Exact vector addrspace(1) instruction excerpt from
/// `llvm/test/Assembler/ptrtoaddr.ll` lines 23-27.
#[test]
fn parses_ptrtoaddr_as1_vector_instruction() {
    let printed = parse_and_print(
        "target datalayout = \"p1:64:64:64:32\"\n\
         define <2 x i32> @test_vec_as1(<2 x ptr addrspace(1)> %p) {\n\
           %addr = ptrtoaddr <2 x ptr addrspace(1)> %p to <2 x i32>\n\
           ret <2 x i32> %addr\n\
         }\n",
    );
    assert!(printed.contains("%addr = ptrtoaddr <2 x ptr addrspace(1)> %p to <2 x i32>\n"));
}

/// Ports the FP arithmetic arms of `LLParser::parseArithmetic`.
/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// shape (no FMF here).
#[test]
fn parses_fp_arith_opcodes() {
    let printed = parse_and_print(
        "define float @fmath(float %a, float %b) {\nentry:\n  \
           %x = fadd float %a, %b\n  \
           %y = fsub float %x, %a\n  \
           %z = fmul float %y, %b\n  \
           %w = fdiv float %z, %a\n  \
           %r = frem float %w, %b\n  \
           ret float %r\n\
         }\n",
    );
    assert!(printed.contains("%x = fadd float %a, %b\n"));
    assert!(printed.contains("%y = fsub float %x, %a\n"));
    assert!(printed.contains("%z = fmul float %y, %b\n"));
    assert!(printed.contains("%w = fdiv float %z, %a\n"));
    assert!(printed.contains("%r = frem float %w, %b\n"));
}

/// Ports `LLParser::parseUnaryOp` `Instruction::FNeg` arm.
#[test]
fn parses_fneg_opcode() {
    let printed = parse_and_print(
        "define float @neg(float %a) {\nentry:\n  %r = fneg float %a\n  ret float %r\n}\n",
    );
    assert!(printed.contains("%r = fneg float %a\n"));
}

/// Ports `LLParser::parseCompare` FP arm. Predicate spelling matches
/// the LangRef table.
#[test]
fn parses_fcmp_opcodes() {
    let printed = parse_and_print(
        "define i1 @ord(float %a, float %b) {\nentry:\n  \
           %r = fcmp oeq float %a, %b\n  \
           ret i1 %r\n\
         }\n",
    );
    assert!(printed.contains("%r = fcmp oeq float %a, %b\n"));
}

/// Ports the `alloca` / `load` / `store` arms of `LLParser::parseAlloc`
/// / `parseLoad` / `parseStore`.
#[test]
fn parses_alloca_load_store() {
    let printed = parse_and_print(
        "define i32 @rw(i32 %v) {\nentry:\n  \
           %slot = alloca i32\n  \
           store i32 %v, ptr %slot\n  \
           %r = load i32, ptr %slot\n  \
           ret i32 %r\n\
         }\n",
    );
    assert!(printed.contains("%slot = alloca i32, align 4\n"));
    assert!(printed.contains("store i32 %v, ptr %slot, align 4\n"));
    assert!(printed.contains("%r = load i32, ptr %slot, align 4\n"));
}

/// Ports the array-size branch of `LLParser::parseAlloc`
/// (`alloca <ty>, <intty> <size>` and the `, align N` combination).
#[test]
fn parses_array_alloca() {
    let printed = parse_and_print(
        "define void @arr(i32 %n) {\nentry:\n  \
           %a = alloca i32, i32 %n\n  \
           %b = alloca i8, i64 5, align 8\n  \
           ret void\n\
         }\n",
    );
    assert!(
        printed.contains("%a = alloca i32, i32 %n, align 4\n"),
        "{printed}"
    );
    assert!(
        printed.contains("%b = alloca i8, i64 5, align 8\n"),
        "{printed}"
    );
}

/// Ports `LLParser::parseGetElementPtr`'s `getIndexedType` rejection
/// ("invalid getelementptr indices"): a struct field index must be a
/// constant i32 in range. `{i32, i64}` has fields 0 and 1 only.
#[test]
fn gep_struct_index_out_of_range_rejected() {
    let e = parse_expect_error(
        "define ptr @f(ptr %p) {\nentry:\n  \
           %r = getelementptr {i32, i64}, ptr %p, i32 0, i32 5\n  \
           ret ptr %r\n\
         }\n",
    );
    assert!(e.contains("getelementptr indices"), "{e}");
}

/// A struct field index that is not an `i32` (here `i64`) is rejected —
/// `StructType::indexValid` requires i32.
#[test]
fn gep_struct_index_non_i32_rejected() {
    let e = parse_expect_error(
        "define ptr @f(ptr %p) {\nentry:\n  \
           %r = getelementptr {i32, i64}, ptr %p, i32 0, i64 1\n  \
           ret ptr %r\n\
         }\n",
    );
    assert!(e.contains("getelementptr indices"), "{e}");
}

/// A non-constant struct field index is rejected.
#[test]
fn gep_struct_index_non_constant_rejected() {
    let e = parse_expect_error(
        "define ptr @f(ptr %p, i32 %n) {\nentry:\n  \
           %r = getelementptr {i32, i64}, ptr %p, i32 0, i32 %n\n  \
           ret ptr %r\n\
         }\n",
    );
    assert!(e.contains("getelementptr indices"), "{e}");
}

/// A valid nested struct index (field 1 of `{i32, i64}`) still parses and
/// round-trips.
#[test]
fn gep_valid_struct_index_round_trips() {
    let printed = parse_and_print(
        "define ptr @f(ptr %p) {\nentry:\n  \
           %r = getelementptr {i32, i64}, ptr %p, i32 0, i32 1\n  \
           ret ptr %r\n\
         }\n",
    );
    assert!(
        printed.contains("getelementptr { i32, i64 }, ptr %p, i32 0, i32 1"),
        "{printed}"
    );
}

/// Ports the `inalloca` / `swifterror` marker arms of
/// `LLParser::parseAlloc` and AsmWriter's AllocaInst printer.
#[test]
fn parses_alloca_markers() {
    let printed = parse_and_print(
        "define void @m() {\nentry:\n  \
           %i = alloca inalloca i32\n  \
           %e = alloca swifterror ptr\n  \
           ret void\n\
         }\n",
    );
    assert!(
        printed.contains("%i = alloca inalloca i32, align 4\n"),
        "{printed}"
    );
    assert!(
        printed.contains("%e = alloca swifterror ptr, align 8\n"),
        "{printed}"
    );
}

/// Ports `LLParser::parseGetElementPtr` plain + inbounds arms.
/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, GEPIndices)`.
#[test]
fn parses_gep_plain_and_inbounds() {
    let printed = parse_and_print(
        "define ptr @walk(ptr %p, i64 %i) {\nentry:\n  \
           %a = getelementptr i32, ptr %p, i64 %i\n  \
           %b = getelementptr inbounds i32, ptr %p, i64 %i\n  \
           ret ptr %b\n\
         }\n",
    );
    assert!(printed.contains("%a = getelementptr i32, ptr %p, i64 %i\n"));
    assert!(printed.contains("%b = getelementptr inbounds i32, ptr %p, i64 %i\n"));
}

/// Ports `LLParser::parseSelect` for the int / fp / ptr arm categories.
/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, SelectMask)`.
#[test]
fn parses_select_int_fp_ptr() {
    let printed = parse_and_print(
        "define void @sel(i1 %c, i32 %a, i32 %b, float %fa, float %fb, ptr %pa, ptr %pb) {\nentry:\n  \
           %ri = select i1 %c, i32 %a, i32 %b\n  \
           %rf = select i1 %c, float %fa, float %fb\n  \
           %rp = select i1 %c, ptr %pa, ptr %pb\n  \
           ret void\n\
         }\n",
    );
    assert!(printed.contains("%ri = select i1 %c, i32 %a, i32 %b\n"));
    assert!(printed.contains("%rf = select i1 %c, float %fa, float %fb\n"));
    assert!(printed.contains("%rp = select i1 %c, ptr %pa, ptr %pb\n"));
}

/// `select` over aggregate arms, which `LLParser::parseSelect` accepts because
/// it delegates wholly to `SelectInst::areInvalidOperands` — and that names no
/// arm restriction beyond token.
///
/// No upstream unit test covers aggregate arms; the rule is read off
/// `areInvalidOperands` itself. This parser used to decline them, announcing
/// the limitation in its own diagnostic ("select arm category supported by
/// this parser"), so the case is worth pinning.
#[test]
fn parses_select_over_struct_and_array_arms() {
    let printed = parse_and_print(
        "define void @sel(i1 %c, { i32, i32 } %sa, { i32, i32 } %sb, [4 x i8] %aa, [4 x i8] %ab) {\nentry:\n  \
           %rs = select i1 %c, { i32, i32 } %sa, { i32, i32 } %sb\n  \
           %ra = select i1 %c, [4 x i8] %aa, [4 x i8] %ab\n  \
           ret void\n\
         }\n",
    );
    assert!(printed.contains("%rs = select i1 %c, { i32, i32 } %sa, { i32, i32 } %sb\n"));
    assert!(printed.contains("%ra = select i1 %c, [4 x i8] %aa, [4 x i8] %ab\n"));
}

/// Negative regression for `LLParser::parseSelect`: constant-folding must not
/// accept a non-`i1` condition before the select condition type is validated.
#[test]
fn select_constant_non_i1_condition_is_rejected_before_fold() {
    let err = parser::parse_assembly(
        "define i32 @bad() {\nentry:\n  %r = select i32 0, i32 5, i32 5\n  ret i32 %r\n}\n",
        |_module, _parsed| (),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("i1 select condition"), "got: {msg}");
}

/// Negative regression for `LLParser::parseSelect` / `SelectInst` validation:
/// token-typed select arms are invalid even when constant folding could choose
/// either equal arm.
#[test]
fn select_constant_token_arms_are_rejected_before_fold() {
    let err = parser::parse_assembly(
        "define void @bad() {\nentry:\n  %r = select i1 true, token none, token none\n  ret void\n}\n",
        |_module, _parsed| (),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("select arm category supported by this parser") || msg.contains("token"),
        "got: {msg}"
    );
}

/// Ports `LLParser::parseCast` `Instruction::{FPToSI,FPToUI}` arms.
#[test]
fn parses_fp_to_int_casts() {
    let printed = parse_and_print(
        "define void @to_int(float %f) {\nentry:\n  \
           %s = fptosi float %f to i32\n  \
           %u = fptoui float %f to i32\n  \
           ret void\n\
         }\n",
    );
    assert!(printed.contains("%s = fptosi float %f to i32\n"));
    assert!(printed.contains("%u = fptoui float %f to i32\n"));
}

/// Ports `LLParser::parseCast` `Instruction::{SIToFP,UIToFP}` arms.
#[test]
fn parses_int_to_fp_casts() {
    let printed = parse_and_print(
        "define void @to_fp(i32 %i) {\nentry:\n  \
           %s = sitofp i32 %i to float\n  \
           %u = uitofp i32 %i to float\n  \
           ret void\n\
         }\n",
    );
    assert!(printed.contains("%s = sitofp i32 %i to float\n"));
    assert!(printed.contains("%u = uitofp i32 %i to float\n"));
}

/// Ports `LLParser::parseCast` `Instruction::AddrSpaceCast` arm.
#[test]
fn parses_addrspacecast() {
    let printed = parse_and_print(
        "define ptr addrspace(1) @as_cast(ptr %p) {\nentry:\n  \
           %r = addrspacecast ptr %p to ptr addrspace(1)\n  \
           ret ptr addrspace(1) %r\n\
         }\n",
    );
    assert!(printed.contains("%r = addrspacecast ptr %p to ptr addrspace(1)\n"));
}

/// Every clause shape `LLParser::parseAlloc` accepts. Its comma arm branches
/// four ways — `align`, `addrspace`, a metadata attachment, or an element
/// count — and the count arm then repeats the same three-way branch, so
/// `align` is legal both with and without a size.
///
/// `test/Assembler/align-inst.ll` and `alloca-addrspace*.ll` cover single
/// clauses; the combinations are anchored on `parseAlloc` itself (D11).
#[test]
fn alloca_accepts_every_upstream_clause_order() {
    for (src, needle) in [
        ("%p = alloca i32", "alloca i32, align 4"),
        ("%p = alloca i32, align 8", "alloca i32, align 8"),
        (
            "%p = alloca i32, align 8, addrspace(5)",
            "alloca i32, align 8, addrspace(5)",
        ),
        (
            "%p = alloca i32, addrspace(5)",
            "alloca i32, align 4, addrspace(5)",
        ),
        ("%p = alloca i32, i32 %n", "alloca i32, i32 %n, align 4"),
        (
            "%p = alloca i32, i32 %n, align 8",
            "alloca i32, i32 %n, align 8",
        ),
        (
            "%p = alloca i32, i32 %n, align 8, addrspace(5)",
            "alloca i32, i32 %n, align 8, addrspace(5)",
        ),
        (
            "%p = alloca i32, i32 %n, addrspace(5)",
            "alloca i32, i32 %n, align 4, addrspace(5)",
        ),
    ] {
        let printed = parse_and_print(&format!(
            "define void @f(i32 %n) {{\nentry:\n  {src}\n  ret void\n}}\n"
        ));
        assert!(printed.contains(needle), "for `{src}` got:\n{printed}");
    }
}

/// An index list stops at a metadata attachment rather than trying to read
/// it as another index. `LLParser::parseIndexList` breaks out of the loop on
/// `MetadataVar` and reports the comma as already eaten, and
/// `LLParser::parseGetElementPtr` does the same inline.
///
/// llvmkit's three index loops had no such guard, so
/// `getelementptr i32, ptr %p, i64 1, !dbg !0` tried to parse `!dbg` as a
/// type — a shape that appears in essentially every `clang -g` module.
/// `parseAlloc`'s equivalent was already handled; these three were not.
///
/// `test/Assembler` has no fixture pairing these opcodes with `!dbg`, so the
/// two upstream routines are the anchor (D11).
#[test]
fn index_lists_stop_at_trailing_metadata() {
    let printed = parse_and_print(
        "define i32 @f(ptr %p, {i32, i32} %agg) {\nentry:\n  \
           %q = getelementptr i32, ptr %p, i64 1, !dbg !0\n  \
           %e = extractvalue {i32, i32} %agg, 0, !dbg !0\n  \
           %i = insertvalue {i32, i32} %agg, i32 7, 1, !dbg !0\n  \
           ret i32 %e, !dbg !0\n\
         }\n\
         !0 = !DILocation(line: 1, column: 1, scope: !1)\n\
         !1 = distinct !DISubprogram(name: \"f\", unit: !2)\n\
         !2 = distinct !DICompileUnit(language: DW_LANG_C11, file: !3, producer: \"llvmkit\")\n\
         !3 = !DIFile(filename: \"a.c\", directory: \"/tmp\")\n",
    );
    assert!(
        printed.contains("%q = getelementptr i32, ptr %p, i64 1"),
        "{printed}"
    );
    assert!(
        printed.contains("%e = extractvalue { i32, i32 } %agg, 0"),
        "{printed}"
    );
    assert!(
        printed.contains("%i = insertvalue { i32, i32 } %agg, i32 7, 1"),
        "{printed}"
    );
}

/// `LLParser::parseInstruction` eats fast-math flags for `select`, `phi`,
/// `fptrunc` and `fpext` before dispatching, then applies them to the
/// result. llvmkit failed to parse the first three spellings and silently
/// **dropped** the flags on `phi`, so none of these round-tripped.
///
/// Spellings are upstream's own: `select nnan ninf i1 …, float …` from
/// `test/Transforms/InstCombine/clamp-to-minmax.ll`, and the scalar and
/// vector `phi nsz` from
/// `test/Transforms/SROA/propagate-fast-math-flags-on-phi.ll`.
/// `test/Assembler` carries no fixture for any of them, so
/// `parseInstruction`'s FMF arms are the anchor (D11).
#[test]
fn fast_math_flags_round_trip_on_select_phi_and_fp_casts() {
    let printed = parse_and_print(
        "define float @sel(i1 %c, float %a, float %b) {\nentry:\n  \
           %r = select nnan ninf i1 %c, float %a, float %b\n  \
           ret float %r\n\
         }\n",
    );
    assert!(
        printed.contains("%r = select nnan ninf i1 %c, float %a, float %b"),
        "{printed}"
    );

    let printed = parse_and_print(
        "define double @phi_with_nsz(i1 %cmp, double %a, double %b) {\nentry:\n  \
           br i1 %cmp, label %if.then, label %return\n\
         if.then:\n  \
           br label %return\n\
         return:\n  \
           %retval = phi nsz double [ %a, %if.then ], [ %b, %entry ]\n  \
           ret double %retval\n\
         }\n",
    );
    assert!(printed.contains("phi nsz double"), "{printed}");

    let printed = parse_and_print(
        "define <2 x double> @vector_phi_with_nsz(i1 %cmp, <2 x double> %a, <2 x double> %b) {\n\
         entry:\n  \
           br i1 %cmp, label %if.then, label %return\n\
         if.then:\n  \
           br label %return\n\
         return:\n  \
           %r = phi nsz <2 x double> [ %a, %if.then ], [ %b, %entry ]\n  \
           ret <2 x double> %r\n\
         }\n",
    );
    assert!(printed.contains("phi nsz <2 x double>"), "{printed}");

    let printed = parse_and_print(
        "define float @tr(double %x) {\nentry:\n  \
           %r = fptrunc contract double %x to float\n  \
           ret float %r\n\
         }\n",
    );
    assert!(
        printed.contains("%r = fptrunc contract double %x to float"),
        "{printed}"
    );

    let printed = parse_and_print(
        "define double @ex(float %x) {\nentry:\n  \
           %r = fpext reassoc float %x to double\n  \
           ret double %r\n\
         }\n",
    );
    assert!(
        printed.contains("%r = fpext reassoc float %x to double"),
        "{printed}"
    );
}

/// The two rejections `LLParser::parseInstruction` pairs with those arms:
/// flags are only legal when the result is an `FPMathOperator`.
#[test]
fn fast_math_flags_on_non_fp_select_or_phi_are_rejected() {
    assert_eq!(
        parse_expect_error(
            "define i32 @f(i1 %c, i32 %a, i32 %b) {\nentry:\n  \
               %r = select fast i1 %c, i32 %a, i32 %b\n  \
               ret i32 %r\n\
             }\n",
        ),
        "fast-math-flags specified for select without floating-point scalar or vector return type"
    );
    assert_eq!(
        parse_expect_error(
            "define i32 @f(i1 %cmp, i32 %a, i32 %b) {\nentry:\n  \
               br i1 %cmp, label %t, label %r\n\
             t:\n  \
               br label %r\n\
             r:\n  \
               %v = phi fast i32 [ %a, %t ], [ %b, %entry ]\n  \
               ret i32 %v\n\
             }\n",
        ),
        "fast-math-flags specified for phi without floating-point scalar or vector return type"
    );
}

/// `LLParser::parseCompare`'s `ICmp` arm accepts pointer operands:
/// its guard is `!isIntOrIntVectorTy() && !isPtrOrPtrVectorTy()`, so a
/// pointer comparison is ordinary IR, not an extension.
///
/// The addrspace spelling is upstream's own, from
/// `test/Verifier/statepoint.ll::@test2`
/// (`%c = icmp eq ptr addrspace(1) %arg, %arg2`); `test/Assembler` has no
/// dedicated fixture, so the `parseCompare` guard is the anchor (D11).
/// Regression lock: llvmkit narrowed both operands to `IntValue<IntDyn>`
/// on the scalar path, which no pointer satisfies, so every pointer
/// comparison was rejected until 0.0.5.
#[test]
fn icmp_accepts_pointer_operands() {
    let printed = parse_and_print(
        "define i1 @ptr_eq(ptr %a, ptr %b) {\nentry:\n  \
           %c = icmp eq ptr %a, %b\n  \
           ret i1 %c\n\
         }\n",
    );
    assert!(printed.contains("%c = icmp eq ptr %a, %b"), "{printed}");

    let printed = parse_and_print(
        "define i1 @test2(ptr addrspace(1) %arg, ptr addrspace(1) %arg2) {\nentry:\n  \
           %c = icmp eq ptr addrspace(1) %arg, %arg2\n  \
           ret i1 %c\n\
         }\n",
    );
    assert!(
        printed.contains("%c = icmp eq ptr addrspace(1) %arg, %arg2"),
        "{printed}"
    );
}

/// The other half of `LLParser::parseCompare`'s two guards, with upstream's
/// exact wording: `icmp` refuses floating-point operands and `fcmp` refuses
/// integer ones.
#[test]
fn compare_operand_category_is_enforced() {
    assert_eq!(
        parse_expect_error(
            "define i1 @f(float %a, float %b) {\nentry:\n  \
               %c = icmp eq float %a, %b\n  \
               ret i1 %c\n\
             }\n",
        ),
        "icmp requires integer operands"
    );
    assert_eq!(
        parse_expect_error(
            "define i1 @f(i32 %a, i32 %b) {\nentry:\n  \
               %c = fcmp oeq i32 %a, %b\n  \
               ret i1 %c\n\
             }\n",
        ),
        "fcmp requires floating point operands"
    );
}

/// The `alloca <ty>, addrspace(N)` clause round-trips (parse + print),
/// mirroring `LLParser::parseAlloc`'s addrspace branch and AsmWriter's
/// AllocaInst addrspace arm.
#[test]
fn alloca_addrspace_round_trips() {
    let printed = parse_and_print(
        "define void @f() {\nentry:\n  \
           %p = alloca i32, addrspace(5)\n  \
           ret void\n\
         }\n",
    );
    assert!(
        printed.contains("%p = alloca i32, align 4, addrspace(5)"),
        "{printed}"
    );
}

/// Ports `test/Assembler/alloca-invalid-type.ll` and `alloca-invalid-type-2.ll`
/// verbatim — `parseAlloc`'s `Ty->isFunctionTy() ||
/// !PointerType::isValidElementType(Ty)` guard — plus the two sibling checks
/// that follow it, neither of which has an upstream fixture.
///
/// llvmkit had none of the three: the type reached the builder, which
/// answered in its own words or accepted it.
///
/// `Cannot allocate unsized type` carries upstream's capital `C`, and an
/// explicit alignment is what makes an unsized allocation legal — without one
/// the alignment would have to come from a layout the type does not have.
#[test]
fn alloca_validates_its_type_and_element_count() {
    for fixture in [
        include_str!("fixtures/upstream/alloca-invalid-type.ll"),
        include_str!("fixtures/upstream/alloca-invalid-type-2.ll"),
    ] {
        assert_eq!(parse_expect_error(fixture), "invalid type for alloca");
    }

    assert_eq!(
        parse_expect_error(
            "define void @f() {\nentry:\n  %p = alloca i32, ptr null\n  ret void\n}\n"
        ),
        "element count must have integer type"
    );
    assert_eq!(
        parse_expect_error(
            "%t = type opaque\ndefine void @f() {\nentry:\n  %p = alloca %t\n  ret void\n}\n"
        ),
        "Cannot allocate unsized type"
    );
    // An explicit alignment makes the same allocation legal.
    parse_and_print(
        "%t = type opaque\ndefine void @f() {\nentry:\n  %p = alloca %t, align 4\n  ret void\n}\n",
    );
}

/// Ports `test/Assembler/invalid-load-missing-explicit-type.ll` verbatim, and
/// pins the rest of `parseLoad`'s and `parseStore`'s check families — twelve
/// diagnostics llvmkit did not have, leaning on builder errors instead, plus
/// four invented `expected …` labels.
///
/// The *shape* matters as much as the text: upstream reads the `align` clause
/// optionally on both paths and then **diagnoses** its absence on an atomic
/// op. llvmkit demanded a comma and an alignment structurally, so
/// `load atomic i32, ptr %p seq_cst` answered `expected ',' after atomic
/// ordering` — a parse failure standing in for a rule.
#[test]
fn load_and_store_validate_their_operands() {
    assert_eq!(
        parse_expect_error(include_str!(
            "fixtures/upstream/invalid-load-missing-explicit-type.ll"
        )),
        "expected comma after load's type"
    );

    for (body, expected) in [
        (
            "%v = load i32, i32 0",
            "load operand must be a pointer to a first class type",
        ),
        (
            "%v = load atomic i32, ptr %p seq_cst",
            "atomic load must have explicit non-zero alignment",
        ),
        (
            "%v = load atomic i32, ptr %p release, align 4",
            "atomic load cannot use Release ordering",
        ),
        ("store i32 0, i32 0", "store operand must be a pointer"),
        (
            "store atomic i32 0, ptr %p seq_cst",
            "atomic store must have explicit non-zero alignment",
        ),
        (
            "store atomic i32 0, ptr %p acquire, align 4",
            "atomic store cannot use Acquire ordering",
        ),
    ] {
        let src = format!("define void @f(ptr %p) {{\nentry:\n  {body}\n  ret void\n}}\n");
        assert_eq!(parse_expect_error(&src), expected, "{body}");
    }
}

/// Ports the whole upstream `atomicrmw` negative family verbatim —
/// `invalid-atomicrmw-add-must-be-integer-type.ll`,
/// `invalid-atomicrmw-fadd-must-be-fp-type.ll`,
/// `invalid-atomicrmw-fsub-must-be-fp-type.ll`,
/// `invalid-atomicrmw-xchg-fp-vector.ll`, and all five splits of
/// `invalid-atomicrmw-scalable.ll`.
///
/// The family exists because `parseAtomicRMW`'s operand rule is **three-way**,
/// and the operation's own name (`AtomicRMWInst::getOperationName`, the same
/// spelling the AsmWriter prints) is part of every message: `xchg` takes an
/// integer, floating-point *or* pointer; the six FP operations take a
/// floating-point; everything else takes an integer.
///
/// The scalable rule is checked before all three, which is why a
/// `<vscale x 2 x half>` operand of `xchg` reports scalability rather than the
/// xchg type rule — the split fixtures cover both orders deliberately.
///
/// llvmkit had none of these: every rejection came from the builder.
#[test]
fn atomicrmw_validates_its_operand_per_operation() {
    for (fixture, expected) in [
        (
            include_str!("fixtures/upstream/invalid-atomicrmw-add-must-be-integer-type.ll"),
            "atomicrmw add operand must be an integer",
        ),
        (
            include_str!("fixtures/upstream/invalid-atomicrmw-fadd-must-be-fp-type.ll"),
            "atomicrmw fadd operand must be a floating point type",
        ),
        (
            include_str!("fixtures/upstream/invalid-atomicrmw-fsub-must-be-fp-type.ll"),
            "atomicrmw fsub operand must be a floating point type",
        ),
        (
            include_str!("fixtures/upstream/invalid-atomicrmw-xchg-fp-vector.ll"),
            "atomicrmw xchg operand must be an integer, floating point, or pointer type",
        ),
    ] {
        assert_eq!(parse_expect_error(fixture), expected);
    }

    for fixture in [
        include_str!(
            "fixtures/upstream/invalid-atomicrmw-scalable/scalable_fp_vector_atomicrmw_xchg.ll"
        ),
        include_str!(
            "fixtures/upstream/invalid-atomicrmw-scalable/scalable_int_vector_atomicrmw_xchg.ll"
        ),
        include_str!(
            "fixtures/upstream/invalid-atomicrmw-scalable/scalable_ptr_vector_atomicrmw_xchg.ll"
        ),
        include_str!(
            "fixtures/upstream/invalid-atomicrmw-scalable/scalable_fp_vector_atomicrmw_fadd.ll"
        ),
        include_str!(
            "fixtures/upstream/invalid-atomicrmw-scalable/scalable_int_vector_atomicrmw_add.ll"
        ),
    ] {
        assert_eq!(
            parse_expect_error(fixture),
            "atomicrmw operand may not be scalable"
        );
    }

    // The two rules with no upstream fixture, both anchored on the routine.
    assert_eq!(
        parse_expect_error(
            "define void @f(ptr %p) {\nentry:\n  atomicrmw add ptr %p, i32 1 unordered\n  ret void\n}\n"
        ),
        "atomicrmw cannot be unordered"
    );
    // The size rule reads `getTypeStoreSizeInBits`, which rounds up to whole
    // bytes — so `i4` is a *legal* operand (store size 8 bits) and only a
    // non-power-of-two byte count trips it. `i24` is 3 bytes.
    assert_eq!(
        parse_expect_error(
            "define void @f(ptr %p) {\nentry:\n  atomicrmw add ptr %p, i24 1 seq_cst\n  ret void\n}\n"
        ),
        "atomicrmw operand must be power-of-two byte-sized integer"
    );
    parse_and_print(
        "define void @f(ptr %p) {\nentry:\n  atomicrmw add ptr %p, i4 1 seq_cst\n  ret void\n}\n",
    );
}

/// Ports all four of `test/Assembler/cmpxchg-ordering{,-2,-3,-4}.ll`
/// verbatim, which between them cover both of `parseCmpXchg`'s ordering
/// predicates: `AtomicCmpXchgInst::isValidSuccessOrdering` denies `NotAtomic`
/// and `Unordered`, and `isValidFailureOrdering` additionally denies the two
/// orderings that imply a release.
///
/// Both are `tokError` and run *before* the operand types are looked at,
/// which is why llvmkit reached neither — every rejection came from the
/// builder, after the operands.
#[test]
fn cmpxchg_validates_its_orderings_and_operands() {
    assert_eq!(
        parse_expect_error(include_str!("fixtures/upstream/cmpxchg-ordering.ll")),
        "invalid cmpxchg success ordering"
    );
    for fixture in [
        include_str!("fixtures/upstream/cmpxchg-ordering-2.ll"),
        include_str!("fixtures/upstream/cmpxchg-ordering-3.ll"),
        include_str!("fixtures/upstream/cmpxchg-ordering-4.ll"),
    ] {
        assert_eq!(
            parse_expect_error(fixture),
            "invalid cmpxchg failure ordering"
        );
    }

    assert_eq!(
        parse_expect_error(
            "define void @f(ptr %p, i32 %b, i64 %c) {\nentry:\n  \
             %x = cmpxchg ptr %p, i32 %b, i64 %c seq_cst seq_cst\n  ret void\n}\n"
        ),
        "compare value and new value type do not match"
    );
}

/// Ports the instruction-side `getelementptr` negatives W4 deferred here:
/// `getelementptr_struct.ll`, `getelementptr_invalid_ptr.ll`,
/// `invalid-gep-missing-explicit-type.ll`, `getelementptr_vscale_struct.ll`
/// and `getelementptr_vec_struct.ll`, all verbatim.
///
/// The last two have **vector** bases and indices, which llvmkit's builder
/// cannot yet express — but they still reach upstream's answer, because
/// `parseGetElementPtr`'s rules all run before the instruction is built. That
/// ordering is the point: the conversion to the builder's scalar shape sits
/// *after* every check, so a recorded IR gap costs coverage only where the
/// input is otherwise valid.
///
/// The scalable rule here differs from the constant-expression arm's, which
/// W4 landed: an instruction asks only whether the source type is a struct
/// containing a scalable vector, where `ConstantExpr::isSupportedGetElementPtr`
/// refuses any scalable source outright.
#[test]
fn getelementptr_validates_its_base_and_indices() {
    for (fixture, expected) in [
        (
            include_str!("fixtures/upstream/invalid-gep-missing-explicit-type.ll"),
            "expected comma after getelementptr's type",
        ),
        (
            include_str!("fixtures/upstream/getelementptr_invalid_ptr.ll"),
            "base of getelementptr must be a pointer",
        ),
        (
            include_str!("fixtures/upstream/getelementptr_struct.ll"),
            "invalid getelementptr indices",
        ),
        (
            include_str!("fixtures/upstream/getelementptr_vscale_struct.ll"),
            "invalid getelementptr indices",
        ),
        (
            include_str!("fixtures/upstream/getelementptr_vec_struct.ll"),
            "invalid getelementptr indices",
        ),
    ] {
        assert_eq!(parse_expect_error(fixture), expected);
    }

    // The two rules with no upstream fixture on the instruction path.
    assert_eq!(
        parse_expect_error(
            "%t = type opaque\ndefine void @f(ptr %p) {\nentry:\n  \
             %g = getelementptr %t, ptr %p, i32 0\n  ret void\n}\n"
        ),
        "base element of getelementptr must be sized"
    );
    assert_eq!(
        parse_expect_error(
            "define void @f(ptr %p) {\nentry:\n  \
             %g = getelementptr { <vscale x 2 x i32> }, ptr %p, i32 0, i32 0\n  ret void\n}\n"
        ),
        "getelementptr cannot target structure that contains scalable vector type"
    );
    assert_eq!(
        parse_expect_error(
            "define void @f(ptr %p) {\nentry:\n  \
             %g = getelementptr i32, ptr %p, ptr null\n  ret void\n}\n"
        ),
        "getelementptr index must be an integer"
    );
}

/// The four terminator routines' operand rules — `parseRet`, `parseBr`,
/// `parseSwitch` and `parseIndirectBr`. llvmkit had none of the six: every
/// rejection came from the builder, in its own words.
///
/// Two ordering details are upstream's and are pinned here: `parseSwitch` and
/// `parseIndirectBr` both demand the `[` **before** checking the condition or
/// address type, so a malformed table is reported ahead of a bad operand.
///
/// No `test/Assembler` fixture pins any of these, so the routines are the
/// anchors.
#[test]
fn terminators_validate_their_operands() {
    for (src, expected) in [
        (
            "define i32 @f() {\nentry:\n  ret void\n}\n",
            "value doesn't match function result type 'i32'",
        ),
        (
            "define void @f() {\nentry:\n  ret i32 0\n}\n",
            "value doesn't match function result type 'void'",
        ),
        (
            "define void @f(i32 %c) {\nentry:\n  br i32 %c, label %a, label %b\na:\n  ret void\nb:\n  ret void\n}\n",
            "branch condition must have 'i1' type",
        ),
        (
            "define void @f(ptr %p) {\nentry:\n  switch ptr %p, label %d [ ]\nd:\n  ret void\n}\n",
            "switch condition must have integer type",
        ),
        (
            "define void @f(i32 %c) {\nentry:\n  switch i32 %c, label %d [ i32 1, label %d i32 1, label %d ]\nd:\n  ret void\n}\n",
            "duplicate case value in switch",
        ),
        (
            "define void @f(i32 %c, i32 %v) {\nentry:\n  switch i32 %c, label %d [ i32 %v, label %d ]\nd:\n  ret void\n}\n",
            "case value is not a constant integer",
        ),
        (
            "define void @f(i32 %a) {\nentry:\n  indirectbr i32 %a, [ label %d ]\nd:\n  ret void\n}\n",
            "indirectbr address must have pointer type",
        ),
    ] {
        assert_eq!(parse_expect_error(src), expected, "{src}");
    }
}

/// `LLParser::parseIndexList`, which `parseExtractValue` and
/// `parseInsertValue` share, plus the aggregate rules that follow it.
///
/// llvmkit had **two** copies of the index loop (and a third in the GEP path)
/// where upstream has one, and both copies silently produced an *empty* index
/// list — so `extractvalue {i32} %a` with no index at all parsed. Upstream's
/// grammar is `(',' uint32)+`: that first comma is required.
#[test]
fn aggregate_index_lists_and_operands_are_validated() {
    for (src, expected) in [
        (
            "define void @f({i32} %a) {\nentry:\n  %v = extractvalue {i32} %a\n  ret void\n}\n",
            "expected ',' as start of index list",
        ),
        (
            "define void @f(i32 %a) {\nentry:\n  %v = extractvalue i32 %a, 0\n  ret void\n}\n",
            "extractvalue operand must be aggregate type",
        ),
        (
            "define void @f({i32} %a) {\nentry:\n  %v = extractvalue {i32} %a, 3\n  ret void\n}\n",
            "invalid indices for extractvalue",
        ),
        (
            "define void @f({i32} %a) {\nentry:\n  %v = insertvalue {i32} %a, i64 0, 0\n  ret void\n}\n",
            "insertvalue operand and field disagree in type: 'i64' instead of 'i32'",
        ),
    ] {
        assert_eq!(parse_expect_error(src), expected, "{src}");
    }
}

/// The remaining per-instruction operand rules: `parsePHI`, `parseVAArg`,
/// `parseArithmetic`/`parseLogical` (which differ only in wording — upstream
/// routes `and`/`or`/`xor` through the second), `parseCmpPredicate`'s two
/// default arms, and the three `isValidOperands` predicates behind
/// `extractelement` / `insertelement` / `shufflevector`.
///
/// Each `isValidOperands` message covers *every* way its predicate can fail,
/// which is why upstream needs one text where llvmkit had several invented
/// per-operand labels. The `shufflevector` rows spell out that breadth: the
/// operand-type pair, the scalable-mask branch, the mask type's scalability,
/// the `Elem >= V1Size * 2` range clause and a non-constant mask all produce
/// the one message, at the first operand.
#[test]
fn instruction_operand_rules_match_upstream_text() {
    for (src, expected) in [
        (
            "define void @f() {\nentry:\n  br label %m\nm:\n  %p = phi i32 (i32)\n  ret void\n}\n",
            "phi node must have first class type",
        ),
        (
            "define void @f(ptr %p) {\nentry:\n  %v = va_arg ptr %p, void\n  ret void\n}\n",
            "void type only allowed for function results",
        ),
        (
            "define void @f(float %a) {\nentry:\n  %v = add float %a, %a\n  ret void\n}\n",
            "invalid operand type for instruction",
        ),
        (
            "define void @f(float %a) {\nentry:\n  %v = and float %a, %a\n  ret void\n}\n",
            "instruction requires integer or integer vector operands",
        ),
        // The trigger is `oeq`, a real *fcmp* predicate in an icmp. A
        // misspelling reaches the same arm now that a word matching no
        // keyword arrives as `Token::Error`.
        (
            "define void @f(i32 %a) {\nentry:\n  %v = icmp oeq i32 %a, %a\n  ret void\n}\n",
            "expected icmp predicate (e.g. 'eq')",
        ),
        (
            "define void @f(i32 %a) {\nentry:\n  %v = extractelement i32 %a, i32 0\n  ret void\n}\n",
            "invalid extractelement operands",
        ),
        (
            "define void @f(<2 x i32> %a) {\nentry:\n  %v = insertelement <2 x i32> %a, i64 0, i32 0\n  ret void\n}\n",
            "invalid insertelement operands",
        ),
        (
            "define void @f(<2 x i32> %a, <4 x i32> %b) {\nentry:\n  \
             %v = shufflevector <2 x i32> %a, <4 x i32> %b, <2 x i32> zeroinitializer\n  ret void\n}\n",
            "invalid shufflevector operands",
        ),
        // `isValidOperands`' scalable branch: a scalable operand admits only
        // an all-zero or all-poison mask. `splat (i32 1)` is neither.
        (
            "define void @f(<vscale x 4 x i32> %a, <vscale x 4 x i32> %b) {\nentry:\n  \
             %v = shufflevector <vscale x 4 x i32> %a, <vscale x 4 x i32> %b, <vscale x 4 x i32> splat (i32 1)\n  ret void\n}\n",
            "invalid shufflevector operands",
        ),
        // `isa<ScalableVectorType>(MaskTy) != isa<ScalableVectorType>(V1->getType())`.
        (
            "define void @f(<vscale x 4 x i32> %a, <vscale x 4 x i32> %b) {\nentry:\n  \
             %v = shufflevector <vscale x 4 x i32> %a, <vscale x 4 x i32> %b, <4 x i32> zeroinitializer\n  ret void\n}\n",
            "invalid shufflevector operands",
        ),
        // `Elem >= V1Size * 2` — a lane naming neither source.
        (
            "define void @f(<2 x i32> %a, <2 x i32> %b) {\nentry:\n  \
             %v = shufflevector <2 x i32> %a, <2 x i32> %b, <4 x i32> <i32 0, i32 99, i32 2, i32 3>\n  ret void\n}\n",
            "invalid shufflevector operands",
        ),
        // The routine's closing `return false`: a mask that is not a constant
        // at all. Upstream reads it with `parseTypeAndValue`, so it parses
        // cleanly and `isValidOperands` is what refuses it.
        (
            "define void @f(<4 x i32> %a, <4 x i32> %b, <4 x i32> %m) {\nentry:\n  \
             %v = shufflevector <4 x i32> %a, <4 x i32> %b, <4 x i32> %m\n  ret void\n}\n",
            "invalid shufflevector operands",
        ),
    ] {
        assert_eq!(parse_expect_error(src), expected, "{src}");
    }
}

/// `parseLandingPad`'s clause rules, whose asymmetry is the point: a `catch`
/// clause takes a **non**-array constant and a `filter` clause takes an array
/// one, with `clause argument must be a constant` shared by both.
///
/// And the three EH pads' scope guard, which each run immediately after their
/// `within` so a bad scope token gets its own message rather than whatever
/// reading a value would have said. Their `'within'` labels were wrong too:
/// upstream says `after`, llvmkit said `in`.
#[test]
fn eh_clause_and_scope_rules_match_upstream_text() {
    let prelude = "declare i32 @__gxx_personality_v0(...)\n";
    for (body, expected) in [
        (
            "  %lp = landingpad i32 catch [1 x ptr] zeroinitializer\n",
            "'catch' clause has an invalid type",
        ),
        (
            "  %lp = landingpad i32 filter ptr null\n",
            "'filter' clause has an invalid type",
        ),
    ] {
        let src = format!(
            "{prelude}define void @f() personality ptr @__gxx_personality_v0 {{\nentry:\n{body}  ret void\n}}\n"
        );
        assert_eq!(parse_expect_error(&src), expected, "{body}");
    }

    for (body, expected) in [
        (
            "  %cp = cleanuppad within 3 []\n  ret void\n",
            "expected scope value for cleanuppad",
        ),
        (
            "  %cp = catchpad within 3 []\n  ret void\n",
            "expected scope value for catchpad",
        ),
        // `catchswitch` is a *terminator*, so it ends its block. Written in
        // upstream's own spelling (`test/Verifier/invalid-eh.ll` uses `%cs =`
        // throughout): the scope guard runs after `within`, so the result name
        // is orthogonal to the rule under test — and writing it named keeps
        // this a regression guard for the named-form dispatch.
        (
            "  %cs = catchswitch within 3 [label %entry] unwind to caller\n",
            "expected scope value for catchswitch",
        ),
    ] {
        let src = format!("define void @f() {{\nentry:\n{body}}}\n");
        assert_eq!(parse_expect_error(&src), expected, "{body}");
    }
}

/// `parseFence`'s two rules, neither of which has an upstream fixture. Both
/// are `tokError`, so both anchor after the ordering keyword.
#[test]
fn fence_rejects_unordered_and_monotonic() {
    for (ordering, expected) in [
        ("unordered", "fence cannot be unordered"),
        ("monotonic", "fence cannot be monotonic"),
    ] {
        let src = format!("define void @f() {{\nentry:\n  fence {ordering}\n  ret void\n}}\n");
        assert_eq!(parse_expect_error(&src), expected, "{ordering}");
    }
}
