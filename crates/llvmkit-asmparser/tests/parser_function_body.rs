//! Function-body parser integration tests (Session 3).
//!
//! Each `#[test]` mirrors a constructive `.ll` fixture or unit-test case
//! from upstream LLVM. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::parser;

fn parse_and_print(src: &str) -> String {
    parser::parse_assembly_string(src, |module, _parsed| format!("{module}")).expect("parse")
}

fn parse_and_verify(src: &str) {
    let verify = parser::parse_assembly_string(src, |module, _parsed| module.verify_borrowed())
        .expect("parse");
    verify.expect("verify");
}

fn parse_expect_error(src: &str) -> String {
    match parser::parse_assembly_string(src, |_module, _parsed| ()) {
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
    assert!(printed.contains("0:\n  br label %1\n"), "{printed}");
    assert!(
        printed.contains("1:\n  %2 = add i32 %x, 1\n  ret i32 %2\n"),
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
    let zero_pos = printed.find("0:\n  ret i32 %x").expect("prints block 0");
    let one_pos = printed
        .find("1:\n  %2 = add i32 %x, 1")
        .expect("prints block 1");
    assert!(zero_pos < one_pos, "{printed}");
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
    let err = parser::parse_assembly_string(
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

/// Negative regression for `LLParser::parseSelect`: constant-folding must not
/// accept a non-`i1` condition before the select condition type is validated.
#[test]
fn select_constant_non_i1_condition_is_rejected_before_fold() {
    let err = parser::parse_assembly_string(
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
    let err = parser::parse_assembly_string(
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
