//! Constant parser tests.
//!
//! Tests either include exact upstream `.ll` excerpts with `include_bytes!` or
//! translate one `LLParser.cpp::parseValID` branch directly. Citations live in
//! `UPSTREAM.md`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError, parser};
use llvmkit_ir::Module;
use llvmkit_ir::module_new;

pub mod support;

fn parse_and_render(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    format!("{module}")
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

fn assert_parse_print_parse_stable(text: &str) {
    let module_name = text
        .strip_prefix("; ModuleID = '")
        .and_then(|tail| tail.split_once('\''))
        .map_or("parser_constants_reparse", |(name, _)| name);
    let reparsed = parse_and_render(module_name, text.as_bytes());
    assert_eq!(reparsed, text);
}

/// Assert the fixture is rejected with upstream's message, **rendered**.
///
/// Comparing `err.to_string()` rather than a variant's payload field is the
/// point: the `FileCheck` line in each upstream fixture pins the text a user
/// sees, so that is what has to match. Matching a field instead let
/// `ParseError::Expected`'s `expected ` prefix silently prepend itself to
/// messages that upstream prints bare.
fn assert_parse_error(src: &[u8], expected_message: &str) {
    let err = {
        let module = Module::dynamic("parser_constants_error");
        Parser::new(src, &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("fixture is rejected")
    };
    assert_eq!(err.to_string(), expected_message);
}

/// Exact struct aggregate store from `test/Assembler/aggregate-constant-values.ll`.
#[test]
fn struct_constant_initializer_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/aggregate-constant-values/struct_constant_initializer_round_trips.ll"
    );

    let text = parse_and_render("struct_constant_initializer_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["@foo", "store { i32, i32 } { i32 7, i32 9 }, ptr %x", "ret"],
    );
}

/// Exact array aggregate store from `test/Assembler/aggregate-constant-values.ll`.
#[test]
fn array_constant_initializer_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/aggregate-constant-values/array_constant_initializer_round_trips.ll"
    );

    let text = parse_and_render("array_constant_initializer_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["@bar", "store [2 x i32] [i32 7, i32 9], ptr %x", "ret"],
    );
}

/// Mirrors `LLParser.cpp::ValID::t_ConstantSplat` lines 6617-6625:
/// scalable vector splats are valid constants and must parse after AsmWriter
/// emits `splat (...)`.
#[test]
fn scalable_vector_splat_constant_round_trips() {
    let text = parse_and_render(
        "scalable_vector_splat_constant_round_trips",
        b"@v = global <vscale x 2 x i32> splat (i32 7)\n",
    );
    assert_check_lines(&text, &["@v = global <vscale x 2 x i32> splat (i32 7)"]);
    assert_parse_print_parse_stable(&text);
}
/// Direct port of `LLParser::parseValID`'s `getelementptr`
/// global-initializer shape.
#[test]
fn getelementptr_constant_expr_initializer_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/getelementptr_constant_expr_initializer_round_trips.ll"
    );

    let text = parse_and_render(
        "getelementptr_constant_expr_initializer_round_trips",
        FIXTURE,
    );
    assert_check_lines(
        &text,
        &["@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact `addrspacecast` constant expression from `test/Assembler/ConstantExprNoFold.ll`.
#[test]
fn constant_expr_casts_round_trip() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/ConstantExprNoFold/constant_expr_casts_round_trip.ll");

    let text = parse_and_render("constant_expr_casts_round_trip", FIXTURE);
    assert_check_lines(
        &text,
        &["@E = global ptr addrspace(1) addrspacecast (ptr @A to ptr addrspace(1))"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact constant-expression folding excerpt from `test/Assembler/ConstantExprFold.ll`
/// lines 9-50, including vector GEP and vector bitcast FileCheck assertions.
#[test]
fn constant_expr_fold_full_vector_gep_and_bitcast_fixture() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/ConstantExprFold/constant_expr_fold_full_vector_gep_and_bitcast_fixture.ll"
    );

    let text = parse_and_render(
        "constant_expr_fold_full_vector_gep_and_bitcast_fixture",
        FIXTURE,
    );
    assert_check_lines(
        &text,
        &[
            "@A = global i64 0",
            "@add = global ptr inttoptr (i64 ptrtoint (ptr @A to i64) to ptr)",
            "@sub = global ptr inttoptr (i64 ptrtoint (ptr @A to i64) to ptr)",
            "@xor = global ptr inttoptr (i64 ptrtoint (ptr @A to i64) to ptr)",
            "@B = external global %Ty",
            "@cons = weak global i32 0, align 8",
            "@gep1 = global <2 x ptr> undef",
            "@gep2 = global <2 x ptr> undef",
            "@gep3 = global <2 x ptr> zeroinitializer",
            "@gep4 = global <2 x ptr> zeroinitializer",
            "@bitcast1 = global <2 x i32> splat (i32 -1)",
            "@bitcast2 = global <4 x i16> splat (i16 -1)",
            "define void @dummy()",
            "ret void",
        ],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact constant-expression cast folding excerpt from
/// `test/Assembler/ConstantExprFoldCast.ll` lines 11-29.
#[test]
fn constant_expr_fold_cast_fixture_matches_upstream() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/ConstantExprFoldCast/constant_expr_fold_cast_fixture_matches_upstream.ll"
    );

    let text = parse_and_render("constant_expr_fold_cast_fixture_matches_upstream", FIXTURE);
    assert_parse_print_parse_stable(&text);
    assert!(!text.contains("bitcast"), "{text}");
    assert!(!text.contains("trunc"), "{text}");
    assert_eq!(text.matches("addrspacecast").count(), 2, "{text}");
    assert!(text.contains("@K = global ptr @J"), "{text}");
}

/// `test/Assembler/ConstantExprFoldSelect.ll` body, parsed rather than
/// optimized: an all-constant `select` survives the parser as an
/// instruction.
///
/// The fixture's own RUN line is `opt -S -passes=instsimplify`, so the
/// folding its CHECK line expects is a **pass** result, not a parse result —
/// `LLParser::parseSelect` ends in an unconditional `SelectInst::Create`.
/// This test used to assert the folded vector from a bare parse, which made
/// llvmkit's parser do in one step what upstream splits across two; the
/// folding half is covered directly through the API in
/// `llvmkit-ir/tests/constant_fold.rs`.
///
/// LLVM 22 also removed `select` constexprs outright, so there is no
/// constant form for a parser-side fold to produce.
#[test]
fn constant_select_survives_parsing_as_an_instruction() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/ConstantExprFoldSelect/constant_expr_fold_select_vector_fixture_matches_upstream.ll"
    );

    let text = parse_and_render("constant_select_survives_parsing", FIXTURE);
    assert_parse_print_parse_stable(&text);
    assert!(text.contains("%s = select <4 x i1>"), "{text}");
    assert!(
        !text.contains("<i16 undef, i16 -2, i16 -3, i16 4>"),
        "{text}"
    );
}

/// Direct port of `LLParser::parseValID`'s general
/// `getelementptr` constant-expression shape.
#[test]
fn constant_expr_gep_round_trip() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseValID/constant_expr_gep_round_trip.ll");

    let text = parse_and_render("constant_expr_gep_round_trip", FIXTURE);
    assert_check_lines(
        &text,
        &["@ptr = global ptr getelementptr (i8, ptr @data, i64 1)"],
    );
    assert_parse_print_parse_stable(&text);
}
/// Exact scalar-pointer/vector-index constant-expression GEP from
/// `test/Assembler/opaque-ptr.ll`.
#[test]
fn constant_expr_vector_gep_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/opaque-ptr/constexpr_vector_gep_round_trips.ll");

    let text = parse_and_render("constant_expr_vector_gep_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["ret <2 x ptr> getelementptr (i16, ptr null, <2 x i32> <i32 3, i32 4>)"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact constant-expression GEP flag forms from `test/Assembler/flags.ll`.
#[test]
fn constant_expr_gep_flags_match_upstream_flags_fixture() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/flags/constant_expr_gep_flags.ll");

    let text = parse_and_render("constant_expr_gep_flags", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "ret ptr getelementptr nuw (i8, ptr @addr, i64 100)",
            "ret ptr getelementptr inbounds nuw (i8, ptr @addr, i64 100)",
            "ret ptr getelementptr nusw (i8, ptr @addr, i64 100)",
            "ret ptr getelementptr inbounds (i8, ptr @addr, i64 100)",
            "ret ptr getelementptr nusw nuw (i8, ptr @addr, i64 100)",
            "ret ptr getelementptr inbounds nuw (i8, ptr @addr, i64 100)",
            "ret ptr getelementptr inbounds nuw (i8, ptr @addr, i64 100)",
            "ret ptr getelementptr nuw inrange(-8, 16) (i8, ptr @addr, i64 100)",
        ],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact addrspace(1) constant-expression GEP flag form from
/// `test/Assembler/flags.ll`.
#[test]
fn constant_expr_gep_flags_addrspace_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/flags/constant_expr_gep_flags_addrspace.ll");

    let text = parse_and_render("constant_expr_gep_flags_addrspace", FIXTURE);
    assert_check_lines(
        &text,
        &["ret ptr addrspace(1) getelementptr nusw nuw (i8, ptr addrspace(1) @addr_as1, i64 100)"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Direct port of `LLParser::parseValID`'s constant GEP `inrange` APInt
/// truncation branch: endpoints are parsed before DataLayout index-width
/// truncation.
#[test]
fn constant_expr_gep_inrange_apint_bounds_truncate_to_index_width() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/constant_expr_gep_inrange_apint_trunc.ll"
    );

    let text = parse_and_render("constant_expr_gep_inrange_apint_trunc", FIXTURE);
    assert_check_lines(
        &text,
        &["ret ptr getelementptr inrange(0, 1) (i8, ptr @addr, i64 100)"],
    );
    assert_parse_print_parse_stable(&text);
}
/// Direct port of `LLParser::parseValID`'s constant GEP `inrange` APSInt
/// branch: endpoints accept `s0x` / `u0x` hexadecimal APSInt tokens.
#[test]
fn constant_expr_gep_inrange_hex_apsint_bounds_round_trip() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/constant_expr_gep_inrange_hex_apsint.ll"
    );

    let text = parse_and_render("constant_expr_gep_inrange_hex_apsint", FIXTURE);
    assert_check_lines(
        &text,
        &["ret ptr getelementptr inrange(0, 1) (i8, ptr @addr, i64 100)"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Direct port of `LLLexer` hexadecimal APSInt active-bit truncation: `s0x1`
/// is a one-bit signed APSInt and therefore sign-extends to `-1`, so the
/// half-open range is empty after `LLParser::parseValID` index-width extension.
#[test]
fn constant_expr_gep_inrange_signed_hex_active_bits_are_preserved() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/constant_expr_gep_inrange_signed_hex_active_bits_invalid.ll"
    );

    assert_parse_error(FIXTURE, "expected end to be larger than start");
}

/// Direct port of `LLParser::parseValID`'s `blockaddress` branch:
/// the accepted `blockaddress(@function, %block)` shape.
#[test]
fn blockaddress_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseValID/blockaddress_round_trips.ll");

    let text = parse_and_render("blockaddress_round_trips", FIXTURE);
    assert_check_lines(&text, &["@addr = global ptr blockaddress(@f, %entry)"]);
    assert_parse_print_parse_stable(&text);
}

/// Direct port of `LLParser::parseValID`'s `dso_local_equivalent` branch:
/// the accepted global-initializer shape.
#[test]
fn dso_local_equivalent_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseValID/dso_local_equivalent_round_trips.ll");

    let text = parse_and_render("dso_local_equivalent_round_trips", FIXTURE);
    assert_check_lines(&text, &["@p = global ptr dso_local_equivalent @f"]);
    assert_parse_print_parse_stable(&text);
}

/// Direct port of `LLParser::parseValID`'s `no_cfi` branch: the accepted
/// global-initializer shape.
#[test]
fn no_cfi_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseValID/no_cfi_round_trips.ll");

    let text = parse_and_render("no_cfi_round_trips", FIXTURE);
    assert_check_lines(&text, &["@p = global ptr no_cfi @f"]);
    assert_parse_print_parse_stable(&text);
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID` `kw_none`:
/// `none` is accepted for token constants.
#[test]
fn token_none_round_trips() {
    let module = module_new!("parser_constants_none").expect("fresh module");
    let parsed = parser::parse_constant_value(b"none", &module, module.token_type().as_type())
        .expect("token none parses");
    assert_eq!(format!("{}", parsed.as_erased()), "token none");
}

/// **llvmkit-authored source; no upstream `.ll` counterpart.** `grep -rlna
/// "token zeroinitializer" test/ unittests/ lib/` over the vendored
/// `llvmorg-22.1.4` tree returns nothing, so the routine is the anchor (D11):
/// `Constant::getNullValue`'s `case Type::TokenTyID` returns
/// `ConstantTokenNone::get`, the very constant the `token none` spelling
/// builds, and `convertValIDToValue`'s `t_Zero` arm reaches it because a token
/// type is first-class and is neither a label nor a `TargetExtType`.
///
/// Uniquing is asserted alongside the text: upstream's two spellings are one
/// `ConstantTokenNone`, not two constants that happen to print alike.
#[test]
fn token_zeroinitializer_is_the_token_none_constant() {
    let module = module_new!("parser_constants_token_zero").expect("fresh module");
    let token_ty = module.token_type().as_type();

    let zero = parser::parse_constant_value(b"zeroinitializer", &module, token_ty)
        .expect("token zeroinitializer parses");
    assert_eq!(format!("{}", zero.as_erased()), "token none");

    let none = parser::parse_constant_value(b"none", &module, token_ty).expect("token none parses");
    assert_eq!(zero.as_erased().id(), none.as_erased().id());
}

/// Exact `ptrtoaddr` constant expression from `test/Assembler/ptrtoaddr.ll`.
#[test]
fn ptrtoaddr_constant_expr_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/ptrtoaddr/ptrtoaddr_constant_expr_round_trips.ll");

    let text = parse_and_render("ptrtoaddr_constant_expr_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["@global_cast_as0 = global i64 ptrtoaddr (ptr @i_as0 to i64)"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact addrspace(1) `ptrtoaddr` constant expression from
/// `llvm/test/Assembler/ptrtoaddr.ll` lines 7-9.
#[test]
fn ptrtoaddr_as1_constant_expr_round_trips() {
    const FIXTURE: &[u8] = br#"target datalayout = "p1:64:64:64:32"
@i_as1 = addrspace(1) global i32 0
@global_cast_as1 = global i32 ptrtoaddr (ptr addrspace(1) @i_as1 to i32)
"#;

    let text = parse_and_render("ptrtoaddr_as1_constant_expr_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["@global_cast_as1 = global i32 ptrtoaddr (ptr addrspace(1) @i_as1 to i32)"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Direct port of `LLParser::parseValID`'s unsupported legacy
/// constant-expression diagnostics for the listed upstream parser branches.
#[test]
fn unsupported_constant_expr_opcodes_are_rejected() {
    for (opcode, src) in [
        (
            "fadd",
            include_bytes!(
                "fixtures/upstream/LLParser-parseValID/unsupported_constant_expr_fadd.ll"
            )
            .as_slice(),
        ),
        (
            "zext",
            include_bytes!(
                "fixtures/upstream/LLParser-parseValID/unsupported_constant_expr_zext.ll"
            )
            .as_slice(),
        ),
        (
            "mul",
            include_bytes!(
                "fixtures/upstream/LLParser-parseValID/unsupported_constant_expr_mul.ll"
            )
            .as_slice(),
        ),
        (
            "select",
            include_bytes!(
                "fixtures/upstream/LLParser-parseValID/unsupported_constant_expr_select.ll"
            )
            .as_slice(),
        ),
        (
            "icmp",
            include_bytes!(
                "fixtures/upstream/LLParser-parseValID/unsupported_constant_expr_icmp.ll"
            )
            .as_slice(),
        ),
    ] {
        let module = module_new!("parser_constants_unsupported").expect("fresh module");
        let err = Parser::new(src, &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("unsupported constexpr is rejected");
        assert!(matches!(err, ParseError::Message { .. }));
        assert_eq!(
            err.to_string(),
            format!("{opcode} constexprs are no longer supported")
        );
    }
}

/// Ports `ShuffleVectorInst::isValidOperands`: constant-expression shuffle
/// masks must have i32 elements, not just any integer element type.
#[test]
fn constant_expr_shufflevector_rejects_non_i32_mask() {
    assert_parse_error(
        b"define <2 x i32> @bad() {\n  ret <2 x i32> shufflevector (<2 x i32> <i32 1, i32 2>, <2 x i32> <i32 3, i32 4>, <2 x i64> <i64 0, i64 1>)\n}\n",
        "invalid operands to shufflevector",
    );
}

/// Ports `ShuffleVectorInst::isValidOperands`: fixed-vector mask elements
/// greater than or equal to `2 * V1Size` are rejected by `parseValID`.
#[test]
fn constant_expr_shufflevector_rejects_out_of_range_mask() {
    assert_parse_error(
        b"define <2 x i32> @bad() {\n  ret <2 x i32> shufflevector (<2 x i32> <i32 1, i32 2>, <2 x i32> <i32 3, i32 4>, <2 x i32> <i32 0, i32 4>)\n}\n",
        "invalid operands to shufflevector",
    );
}

/// `test/Bitcode/vscale-round-trip.ll`, the whole file, asserting each of its
/// four `CHECK-LABEL` / `CHECK` pairs in file order.
///
/// Upstream's RUN line is `llvm-as < %s | llvm-dis | FileCheck %s`, so the
/// CHECK lines are what the printer emits after a bitcode round trip. Three of
/// the four functions are constant-expression cases;
/// `@non_const_shufflevector` is the instruction form, which needs
/// `ShuffleVectorInst::isValidOperands`' scalable branch —
/// `(Mask[0] != 0 && Mask[0] != PoisonMaskElem) || !all_equal(Mask)` — to
/// admit an all-zero mask, and the `ShuffleVectorInst(Value *, Value *,
/// ArrayRef<int>, ...)` constructor's
/// `VectorType::get(EltTy, Mask.size(), isa<ScalableVectorType>(V1->getType()))`
/// to give it a scalable result type.
#[test]
fn vscale_round_trip_fixture_matches_upstream() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/vscale-round-trip.ll");

    let text = parse_and_render("vscale_round_trip_fixture_matches_upstream", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "define <vscale x 4 x i32> @const_shufflevector(",
            "<vscale x 4 x i32> zeroinitializer",
            "define <vscale x 4 x i32> @const_shufflevector_ex()",
            "<vscale x 4 x i32> zeroinitializer",
            "define <vscale x 4 x i32> @non_const_shufflevector(",
            "%res = shufflevector <vscale x 4 x i32>",
            "define <vscale x 4 x i32> @const_select()",
            "select <vscale x 4 x i1>",
        ],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact negative constant-GEP fixture from
/// `test/Assembler/constant-getelementptr-scalable_pointee.ll`.
#[test]
fn constant_expr_gep_rejects_scalable_vector_pointee() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/constant-getelementptr-scalable_pointee.ll");

    assert_parse_error(FIXTURE, "invalid base element for constant getelementptr");
}

/// Exact negative constant-GEP fixture from
/// `test/Assembler/getelementptr_vec_ce2.ll`: two vector indices whose lane
/// counts disagree. The first vector index is what fixes the width, so the
/// second one is the offender even though the base pointer is a scalar —
/// `LLParser::parseValID`'s "GEPWidth may have been unknown" comment.
#[test]
fn constant_expr_gep_rejects_disagreeing_vector_index_widths() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/getelementptr-vec/getelementptr_vec_ce2.ll");

    assert_parse_error(
        FIXTURE,
        "getelementptr vector index has a wrong number of elements",
    );
}

/// No upstream `.ll` pins these three in their *constant-expression* form —
/// `test/Assembler/getelementptr_struct.ll` and friends are all instruction
/// GEPs — so this anchors the guards by symbol instead:
/// `LLParser::parseValID`'s `getelementptr` arm, in the order it runs them.
///
/// The order is the point. A struct that holds a scalable vector is both
/// unsized and unsupported as a constant-GEP base, and upstream reports it
/// unsized; make the body homogeneous and `StructType::isSized`'s
/// `containsHomogeneousScalableVectorTypes` exception makes it sized, so the
/// *next* check — `ConstantExpr::isSupportedGetElementPtr` — is the one that
/// fires.
#[test]
fn constant_expr_gep_checks_run_in_upstream_order() {
    assert_parse_error(
        b"%opaque = type opaque\n\
          @g = external global i8\n\
          @p = global ptr getelementptr (%opaque, ptr @g, i32 0)\n",
        "base element of getelementptr must be sized",
    );
    assert_parse_error(
        b"%mixed = type { <vscale x 2 x i32>, i32 }\n\
          @g = external global i8\n\
          @p = global ptr getelementptr (%mixed, ptr @g, i32 0)\n",
        "base element of getelementptr must be sized",
    );
    assert_parse_error(
        b"%homogeneous = type { <vscale x 2 x i32>, <vscale x 2 x i32> }\n\
          @g = external global i8\n\
          @p = global ptr getelementptr (%homogeneous, ptr @g, i32 0)\n",
        "invalid base element for constant getelementptr",
    );
}

/// `GetElementPtrInst::getIndexedType` returning null, reached from
/// `LLParser::parseValID` rather than `parseGetElementPtr`. The shape is
/// `test/Verifier/2002-11-05-GetelementptrPointers.ll`'s — indexing *into* a
/// pointer held inside a struct — written as a constant expression, which
/// upstream's test tree does not cover.
#[test]
fn constant_expr_gep_rejects_indices_that_do_not_index_the_source() {
    assert_parse_error(
        b"@g = external global i8\n\
          @p = global ptr getelementptr ({ i32, ptr }, ptr @g, i32 0, i32 1, i32 0)\n",
        "invalid getelementptr indices",
    );
}

/// `LLParser::parseGlobalValueVector`'s empty-list early return: a closing
/// paren yields no operands rather than a diagnostic of its own, so the
/// `Elts.size() == 0` half of upstream's base-pointer check is what reports.
#[test]
fn constant_expr_gep_with_no_operands_reports_the_missing_base() {
    assert_parse_error(
        b"@p = global ptr getelementptr (i8, )\n",
        "base of getelementptr must be a pointer",
    );
}

/// `LLParser::parseConstantValue`'s tail switches on `ValID::Kind` and accepts
/// a fixed set; everything outside it is `expected a constant value`. The
/// message was unreachable while llvmkit's standalone entry point converted
/// whatever `parseValID` returned, which also made it accept `@g` — a
/// `t_GlobalName`, and not in upstream's set.
///
/// `null` is the one kind handled outside the conversion: upstream takes
/// `Constant::getNullValue(Ty)` directly, so at a non-pointer type it is that
/// type's zero rather than `null must be a pointer type`.
///
/// No upstream `.ll` pins any of this — `parseStandaloneConstantValue` is a
/// C++ entry point with no textual test — so the guards are anchored by
/// symbol.
#[test]
fn a_standalone_constant_value_accepts_only_upstreams_kinds() {
    let module = module_new!("parser_constants_standalone").expect("fresh module");
    let i32_ty = module.i32_type().as_type();

    let parsed = parser::parse_constant_value(b"7", &module, i32_ty).expect("an integer parses");
    assert_eq!(format!("{}", parsed.as_erased()), "i32 7");

    // `t_Null` at a non-pointer type is `getNullValue`, not a diagnostic.
    let parsed = parser::parse_constant_value(b"null", &module, i32_ty).expect("null parses");
    assert_eq!(format!("{}", parsed.as_erased()), "i32 0");

    // `t_GlobalName` is not in the accepted set, so the kind switch rejects it
    // before anything tries to resolve the name.
    let err = parser::parse_constant_value(b"@g", &module, module.ptr_type(0).as_type())
        .expect_err("a global name is not a constant value here");
    assert_eq!(err.to_string(), "expected a constant value");

    // Nor is `t_EmptyArray`, at any type.
    let empty_array = module.array_type(module.i32_type(), 0).as_type();
    let err = parser::parse_constant_value(b"[]", &module, empty_array)
        .expect_err("an empty array is not a constant value here");
    assert_eq!(err.to_string(), "expected a constant value");
}

/// Every split of `test/Assembler/constant-splat-diagnostics.ll`, each
/// asserting its own `FileCheck` line. Four pin `convertValIDToValue`'s
/// `t_ConstantSplat` arm; the fifth pins the instruction dispatch, because
/// `splat` is a constant form and never an opcode.
#[test]
fn constant_splat_diagnostics_match_upstream_text() {
    const SPLITS: &[(&str, &[u8], &str)] = &[
        (
            "not_a_scalar",
            include_bytes!("fixtures/upstream/constant-splat-diagnostics/not_a_sclar.ll"),
            "constant expression type mismatch: got type '<1 x i32>' but expected 'i32'",
        ),
        (
            "not_a_vector",
            include_bytes!("fixtures/upstream/constant-splat-diagnostics/not_a_vector.ll"),
            "vector constant must have vector type",
        ),
        (
            "wrong_explicit_type",
            include_bytes!("fixtures/upstream/constant-splat-diagnostics/wrong_explicit_type.ll"),
            "constant expression type mismatch: got type 'i8' but expected 'i32'",
        ),
        (
            "wrong_implicit_type",
            include_bytes!("fixtures/upstream/constant-splat-diagnostics/wrong_implicit_type.ll"),
            "constant expression type mismatch: got type 'i8' but expected 'i32'",
        ),
        (
            "not_a_constant",
            include_bytes!("fixtures/upstream/constant-splat-diagnostics/not_a_constant.ll"),
            "expected instruction opcode",
        ),
    ];

    for (name, fixture, expected) in SPLITS {
        let module = Module::dynamic(*name);
        let err = Parser::new(fixture, &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("split is rejected");
        assert_eq!(err.to_string(), *expected, "split {name}");
    }
}

/// `c"..."` is type-free upstream: `ConstantDataArray::getString` always
/// builds `[N x i8]`, and agreement with the demanded type is
/// `convertValIDToValue`'s job. Deriving the array type from the *expected*
/// type instead accepted this silently. No upstream `.ll` pins it, so the
/// guard is anchored by symbol.
#[test]
fn a_c_string_is_always_an_i8_array() {
    assert_parse_error(
        b"@g = global [4 x i32] c\"abcd\"
",
        "constant expression type mismatch: got type '[4 x i8]' but expected '[4 x i32]'",
    );
}

/// `LLParser::parseValID`'s `kw_dso_local_equivalent` arm rejects a referent
/// whose value type is not a function type. Upstream's only
/// `dso_local_equivalent` fixture is the positive round-trip
/// `test/Assembler/dso_local_equivalent.ll`, so this anchors the guard by
/// symbol; the text is upstream's.
#[test]
fn dso_local_equivalent_requires_a_function_referent() {
    assert_parse_error(
        b"@g = global i32 0
@p = global ptr dso_local_equivalent @g
",
        "expected a function, alias to function, or ifunc in dso_local_equivalent",
    );
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID` `kw_none`
/// and `Constants.cpp::ConstantTargetNone::get`: `none` is token-only in the
/// shipped parser subset.
#[test]
fn none_is_token_only() {
    let module = module_new!("parser_constants_none_token").expect("fresh module");
    let parsed = parser::parse_constant_value(b"none", &module, module.token_type().as_type())
        .expect("token none parses");
    assert_eq!(format!("{}", parsed.as_erased()), "token none");

    let target_ty = module
        .target_ext_type(
            "spirv.Image",
            Vec::<llvmkit_ir::Type<'_, _>>::new(),
            Vec::<u32>::new(),
        )
        .as_type();
    let err = parser::parse_constant_value(b"none", &module, target_ty)
        .expect_err("target-extension none is rejected");
    assert!(matches!(err, ParseError::Message { .. }));
    assert_eq!(err.to_string(), "invalid type for none constant");
}

/// llvmkit-specific subset of `test/Assembler/target-types.ll` and
/// `Type.cpp::getTargetTypeInfo`: target-extension zeroinitializer requires the
/// zero-initializable property.
///
/// The rejection is `convertValIDToValue`'s second `t_Zero` guard,
/// `error(ID.Loc, "invalid type for null constant")` — the *bare* sentence.
/// It used to travel in `ParseError::Expected`, which renders `expected ` in
/// front of it, and `test/Assembler/target-type-properties.ll`'s
/// `zeroinit-error.ll` split stayed green throughout because the corpus driver
/// compares an `error=` pin with `contains`, which a wrapper that only adds
/// text satisfies. Asserting the *variant* is what pins the absence of the
/// prefix; the same guard's other arms are covered by
/// [`zeroinitializer_of_an_unzeroable_type_is_an_invalid_null_constant`].
#[test]
fn target_ext_zeroinitializer_requires_zero_init_property() {
    let module = module_new!("parser_constants_target_zero").expect("fresh module");
    let zero_ty = module
        .target_ext_type(
            "spirv.foo",
            Vec::<llvmkit_ir::Type<'_, _>>::new(),
            Vec::<u32>::new(),
        )
        .as_type();
    let zero = parser::parse_constant_value(b"zeroinitializer", &module, zero_ty)
        .expect("zero-initializable target extension parses");
    assert_eq!(
        format!("{}", zero.as_erased()),
        "target(\"spirv.foo\") zeroinitializer"
    );

    let image_ty = module
        .target_ext_type(
            "spirv.Image",
            Vec::<llvmkit_ir::Type<'_, _>>::new(),
            Vec::<u32>::new(),
        )
        .as_type();
    let err = parser::parse_constant_value(b"zeroinitializer", &module, image_ty)
        .expect_err("non-zero-initializable target extension is rejected");
    match err {
        ParseError::Message { message, .. } => {
            assert_eq!(message, "invalid type for null constant")
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// `convertValIDToValue`'s `case ValID::t_Zero:` opens
/// `if (!Ty->isFirstClassType() || Ty->isLabelTy()) return error(ID.Loc,
/// "invalid type for null constant");` — a guard `parseConstantValue` reaches
/// too, because it routes `t_Zero` through the same routine with
/// `PFS = nullptr`. llvmkit ran it on the value path only, so a global
/// initializer of `label` type skipped it.
///
/// Past that guard, upstream's `Constant::getNullValue` ends in
/// `default: llvm_unreachable("Cannot create a null constant of that type!")`.
/// `metadata`, `x86_amx` and `exnref` are first-class, are not labels and have
/// no `getNullValue` case, so they are what reaches it. Rejecting rather than
/// trapping is hardening; the message is the enclosing guard's, because
/// upstream associates no other text with `t_Zero`. That arm invented
/// `expected zeroinitializer for a zeroable type` instead.
///
/// The remaining two rejecting arms of `Constant::getNullValue`'s llvmkit
/// counterpart are covered here as well, so that one test pins every way out
/// of `t_Zero`: the opaque struct (`!Ty->isFirstClassType()`) and the
/// target-extension type without `HasZeroInit`.
///
/// `test/Assembler/2004-11-28-InvalidTypeCrash.ll` and
/// `target-type-properties.ll`'s `zeroinit-error.ll` split pin those two by
/// message and are driven by the corpus manifest, but on `contains`, and
/// neither sets a column — which is how `expected invalid type for null
/// constant` stayed green. No upstream fixture writes the other four types
/// with `zeroinitializer` at all; the routine is the anchor.
#[test]
fn zeroinitializer_of_an_unzeroable_type_is_an_invalid_null_constant() {
    for source in [
        // `parseConstantValue`'s path — a global initializer.
        "@g = global label zeroinitializer\n",
        "@g = global metadata zeroinitializer\n",
        "@g = global x86_amx zeroinitializer\n",
        "@g = global exnref zeroinitializer\n",
        "%s = type opaque\n@g = global %s zeroinitializer\n",
        // `convertValIDToValue`'s path — an instruction operand.
        "define void @f() {\nentry:\n  %v = freeze label zeroinitializer\n  ret void\n}\n",
        "define void @f() {\nentry:\n  %v = freeze metadata zeroinitializer\n  ret void\n}\n",
        "define void @f() {\nentry:\n  %v = freeze x86_amx zeroinitializer\n  ret void\n}\n",
        "define void @f() {\nentry:\n  %v = freeze exnref zeroinitializer\n  ret void\n}\n",
        "define void @f() {\nentry:\n  %v = freeze target(\"unknown_target_type\") zeroinitializer\n  ret void\n}\n",
    ] {
        let module = module_new!("parser_constants_unzeroable").expect("fresh module");
        let err = Parser::new(source.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("an unzeroable type is rejected");
        match &err {
            ParseError::Message { message, .. } => {
                assert_eq!(message, "invalid type for null constant", "for {source:?}")
            }
            other => panic!("unexpected error variant for {source:?}: {other:?}"),
        }
        // `error(ID.Loc, …)`, and `parseValID`'s first statement is
        // `ID.Loc = Lex.getLoc()` — the `zeroinitializer` token, not the type
        // in front of it and not the lookahead behind it. Derived from those
        // two routines; the fixtures that pin this message pin text only.
        let start = usize::try_from(
            err.loc()
                .expect("a rejection reports a location")
                .span
                .start,
        )
        .expect("span start fits in usize");
        assert_eq!(
            support::line_and_column(source.as_bytes(), start),
            support::line_and_column(
                source.as_bytes(),
                source
                    .find("zeroinitializer")
                    .expect("the fixture writes one"),
            ),
            "for {source:?}"
        );
    }
}

/// Direct port of `LLParser::parseValID`'s `ptrauth` branch: the five-operand
/// shape accepted by upstream.
#[test]
fn ptrauth_five_operands_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/ptrauth_five_operands_round_trips.ll"
    );

    let text = parse_and_render("ptrauth_five_operands_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "@signed = global ptr ptrauth (ptr @g, i32 0, i64 1, ptr inttoptr (i64 1 to ptr), ptr @g)",
        ],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact default ptrauth operand elision from `test/Assembler/ptrauth-const.ll`.
#[test]
fn ptrauth_default_operands_are_elided() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/ptrauth-const/ptrauth_default_operands_are_elided.ll");

    let text = parse_and_render("ptrauth_default_operands_are_elided", FIXTURE);
    assert_check_lines(&text, &["@basic = global ptr ptrauth (ptr @var, i32 0)"]);
    assert_parse_print_parse_stable(&text);
}

/// Exact ptrauth validation diagnostics from `test/Assembler/invalid-ptrauth-const*.ll`.
#[test]
fn ptrauth_invalid_operands_match_upstream_diagnostics() {
    for (fixture, expected) in [
        (
            include_bytes!("fixtures/upstream/ptrauth-const/invalid_ptrauth_base_pointer.ll")
                .as_slice(),
            "constant ptrauth base pointer must be a pointer",
        ),
        (
            include_bytes!("fixtures/upstream/ptrauth-const/invalid_ptrauth_key.ll").as_slice(),
            "constant ptrauth key must be i32 constant",
        ),
        (
            include_bytes!("fixtures/upstream/ptrauth-const/invalid_ptrauth_addr_disc.ll")
                .as_slice(),
            "constant ptrauth address discriminator must be a pointer",
        ),
        (
            include_bytes!("fixtures/upstream/ptrauth-const/invalid_ptrauth_disc_expr.ll")
                .as_slice(),
            "constant ptrauth integer discriminator must be i64 constant",
        ),
        (
            include_bytes!("fixtures/upstream/ptrauth-const/invalid_ptrauth_disc_type.ll")
                .as_slice(),
            "constant ptrauth integer discriminator must be i64 constant",
        ),
        (
            include_bytes!("fixtures/upstream/ptrauth-const/invalid_ptrauth_deactivation.ll")
                .as_slice(),
            "constant ptrauth deactivation symbol must be a pointer",
        ),
    ] {
        assert_parse_error(fixture, expected);
    }
}

/// Direct port of `LLParser::parseValID`'s forward blockaddress placeholder
/// resolution.
#[test]
fn forward_blockaddress_resolves_later_signature() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/forward_blockaddress_resolves_later_signature.ll"
    );

    let text = parse_and_render("forward_blockaddress_resolves_later_signature", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "@addr = global ptr blockaddress(@f, %entry)",
            "define i32 @f(i32 %x)",
        ],
    );
    assert_eq!(text.matches("declare void @f()").count(), 0);
    assert_parse_print_parse_stable(&text);
}

/// Direct port of `LLParser::parseValID`'s forward blockaddress placeholder
/// resolution in a nested aggregate constant.
#[test]
fn nested_forward_blockaddress_resolves_later_signature() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/nested_forward_blockaddress_resolves_later_signature.ll"
    );

    let text = parse_and_render(
        "nested_forward_blockaddress_resolves_later_signature",
        FIXTURE,
    );
    assert_check_lines(
        &text,
        &[
            "@addrs = global [1 x ptr] [ptr blockaddress(@f, %entry)]",
            "define i32 @f(i32 %x)",
        ],
    );
    assert_eq!(text.matches("declare void @f()").count(), 0);
    assert_parse_print_parse_stable(&text);
}

/// A `blockaddress` naming the function it appears in resolves through that
/// function's own state — upstream's `BlockAddressPFS` route in
/// `LLParser::parseValID`'s `kw_blockaddress` arm — even when the label is
/// below the reference.
///
/// The IR shape is `test/Bitcode/blockaddress-addrspace.ll::return-self-good.ll`
/// with its address spaces dropped; that fixture itself is still blocked on
/// the *program* address space (`target datalayout = "P2"` reaching a function
/// that declares none), which is W3 work.
#[test]
fn same_function_forward_blockaddress_resolves_by_name() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/blockaddress-self/self_named_label.ll");

    let text = parse_and_render("same_function_forward_blockaddress_by_name", FIXTURE);
    assert_check_lines(&text, &["ret ptr blockaddress(@take_self_named, %L3)"]);
    assert_parse_print_parse_stable(&text);
}

/// The same rule for a *numbered* label, which is the half that could never
/// have worked: llvmkit stringified the slot id and looked for a block
/// literally named `"2"`, and no unnamed block is. Upstream keeps the two
/// spellings apart as `ValID::t_LocalID` / `t_LocalName` precisely because
/// they resolve through different tables.
///
/// No upstream `.ll` fixture isolates this; the rule is
/// `PerFunctionState::getBB(unsigned)` reached through `BlockAddressPFS`.
#[test]
fn same_function_forward_blockaddress_resolves_by_number() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/blockaddress-self/self_numbered_label.ll");

    let text = parse_and_render("same_function_forward_blockaddress_by_number", FIXTURE);
    assert_check_lines(&text, &["ret ptr blockaddress(@take_self_numbered, %2)"]);
    assert_parse_print_parse_stable(&text);
}

/// Once a function's body is closed its label numbering is gone, so a numeric
/// label can no longer be looked up. Mirrors the `else` arm of
/// `LLParser::parseValID`'s `kw_blockaddress` block lookup, whose wording this
/// pins. A *named* label stays resolvable, because names survive in the
/// function's value symbol table.
#[test]
fn numeric_blockaddress_label_after_the_function_is_defined_is_rejected() {
    assert_parse_error(
        include_bytes!("fixtures/upstream/blockaddress-self/numeric_label_after_definition.ll"),
        "cannot take address of numeric label after the function is defined",
    );
}

/// Direct port of `LLParser::parseValID`'s forward `dso_local_equivalent` /
/// `no_cfi` placeholder resolution.
#[test]
fn forward_dso_and_no_cfi_resolve_later_signature() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/forward_dso_and_no_cfi_resolve_later_signature.ll"
    );

    let text = parse_and_render("forward_dso_and_no_cfi_resolve_later_signature", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "@d = global ptr dso_local_equivalent @f",
            "@n = global ptr no_cfi @f",
            "declare i32 @f(i32 %0)",
        ],
    );
    assert_eq!(text.matches("declare void @f()").count(), 0);
    assert_parse_print_parse_stable(&text);
}

/// Direct port of `LLParser::parseValID`'s `ForwardRefBlockAddresses` path:
/// a function-body constant can name a block in a later-defined function.
#[test]
fn function_body_forward_blockaddress_resolves_later_signature() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/function_body_forward_blockaddress_resolves_later_signature.ll"
    );

    let text = parse_and_render("function_body_forward_blockaddress", FIXTURE);
    assert_check_lines(&text, &["ret ptr blockaddress(@f, %entry)"]);
    assert_parse_print_parse_stable(&text);
}

/// Mirrors `LLParser::ForwardRefBlockAddresses`: RAUW must update constant
/// aggregate users, not only direct instruction operands.
#[test]
fn function_body_forward_aggregate_blockaddress_resolves_later_signature() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/function_body_forward_aggregate_blockaddress_resolves_later_signature.ll"
    );

    let text = parse_and_render("function_body_forward_aggregate_blockaddress", FIXTURE);
    assert_check_lines(
        &text,
        &["call void @sink([1 x ptr] [ptr blockaddress(@f, %entry)])"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Mirrors `LLParser::parseValID`: numbered global IDs share the forward
/// `blockaddress` placeholder path with named functions.
#[test]
fn function_body_forward_numbered_blockaddress_resolves_later_signature() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/function_body_forward_numbered_blockaddress_resolves_later_signature.ll"
    );

    let text = parse_and_render("function_body_forward_numbered_blockaddress", FIXTURE);
    assert_check_lines(&text, &["ret ptr blockaddress(@0, %entry)"]);
    assert_parse_print_parse_stable(&text);
}

/// Exact `return-fwddecl-good.ll` address-space case from
/// `test/Bitcode/blockaddress-addrspace.ll`.
#[test]
fn forward_blockaddress_preserves_function_address_space() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/blockaddress-addrspace/return_fwddecl_good.ll");

    let text = parse_and_render("forward_blockaddress_addrspace", FIXTURE);
    assert_check_lines(
        &text,
        &["ret ptr addrspace(2) blockaddress(@fwddecl_as2, %bb)"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Direct port of `LLParser::parseValID`'s forward `dso_local_equivalent` /
/// `no_cfi` placeholder resolution in nested aggregate constants.
#[test]
fn nested_forward_dso_and_no_cfi_resolve_later_signature() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/nested_forward_dso_and_no_cfi_resolve_later_signature.ll"
    );

    let text = parse_and_render(
        "nested_forward_dso_and_no_cfi_resolve_later_signature",
        FIXTURE,
    );
    assert_check_lines(
        &text,
        &[
            "@d = global [1 x ptr] [ptr dso_local_equivalent @f]",
            "@n = global [1 x ptr] [ptr no_cfi @f]",
            "declare i32 @f(i32 %0)",
        ],
    );
    assert_eq!(text.matches("declare void @f()").count(), 0);
    assert_parse_print_parse_stable(&text);
}

/// Exact `global-fwddecl-good.ll` case from
/// `test/Bitcode/blockaddress-addrspace.ll`: a *global initializer* may name a
/// block in a function that has not been seen yet, and the placeholder takes
/// the address space of the demanded type (`FwdDeclAS = ExpectedTy->
/// getPointerAddressSpace()` in `LLParser::parseValID`'s `kw_blockaddress`
/// arm).
#[test]
fn global_initializer_forward_blockaddress_takes_the_demanded_address_space() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/blockaddress-addrspace/global_fwddecl_good.ll");

    let text = parse_and_render("global_fwddecl_good", FIXTURE);
    assert_check_lines(
        &text,
        &["@global = constant ptr addrspace(2) blockaddress(@fwddecl_in_prog_as, %bb)"],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact `global-fwddecl-bad.ll` case from
/// `test/Bitcode/blockaddress-addrspace.ll`, whose CHECK line pins the
/// wording. A *forward* `blockaddress` is retired by
/// `PerFunctionState::resolveForwardRefBlockAddresses` through
/// `checkValidVariableType`, not by `convertValIDToValue` — so the sentence is
/// `'bb' defined with type … but expected …`, quoting the label with no `%`.
#[test]
fn global_initializer_forward_blockaddress_address_space_mismatch_is_rejected() {
    assert_parse_error(
        include_bytes!("fixtures/upstream/blockaddress-addrspace/global_fwddecl_bad.ll"),
        "'bb' defined with type 'ptr addrspace(1)' but expected 'ptr addrspace(2)'",
    );
}

/// Exact `global-use-bad.ll` case from
/// `test/Bitcode/blockaddress-addrspace.ll`: with the function already
/// defined the `blockaddress` types itself, so the disagreement is
/// `convertValIDToValue`'s `t_Constant` arm instead.
#[test]
fn global_initializer_blockaddress_address_space_mismatch_is_rejected() {
    assert_parse_error(
        include_bytes!("fixtures/upstream/blockaddress-addrspace/global_use_bad.ll"),
        "constant expression type mismatch: got type 'ptr addrspace(1)' but expected 'ptr addrspace(2)'",
    );
}

/// Exact `bad-type-not-ptr.ll` case from
/// `test/Bitcode/blockaddress-addrspace.ll`. The check lives inside
/// `kw_blockaddress`'s `if (!F)` branch, because only there does the demanded
/// type have to supply an address space.
#[test]
fn a_non_pointer_type_for_a_forward_blockaddress_is_rejected() {
    assert_parse_error(
        include_bytes!("fixtures/upstream/blockaddress-addrspace/bad_type_not_ptr.ll"),
        "type of blockaddress must be a pointer and not 'i8'",
    );
}

/// Exact `bad-type-not-i8-ptr.ll` case from
/// `test/Bitcode/blockaddress-addrspace.ll`: the function is never defined, so
/// the placeholder survives to `validateEndOfModule`'s
/// `ForwardRefBlockAddresses` guard.
#[test]
fn a_global_initializer_blockaddress_naming_an_undefined_function_is_rejected() {
    assert_parse_error(
        include_bytes!("fixtures/upstream/blockaddress-addrspace/bad_type_not_i8_ptr.ll"),
        "expected function name in blockaddress",
    );
}

/// Exact module from `test/Assembler/pr119818.ll`: a global initializer whose
/// aggregate names two blocks of a function defined later, one of them by
/// *number*. Only `PerFunctionState::resolveForwardRefBlockAddresses` can
/// resolve `%0`, since the numbering exists solely while that body is open.
#[test]
fn global_initializer_forward_blockaddress_resolves_a_numbered_label() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/pr119818.ll");

    let text = parse_and_render("pr119818", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "@vm_exec_core.insns_address_table = internal constant [2 x ptr] \
             [ptr blockaddress(@vm_exec_core, %0), ptr blockaddress(@vm_exec_core, %block)], align 16",
        ],
    );
    assert_parse_print_parse_stable(&text);
}

/// Exact `undefined_func.ll` case from
/// `test/CodeGen/X86/dso_local_equivalent_errors.ll`, whose CHECK line pins the
/// wording of `validateEndOfModule`'s `ResolveForwardRefDSOLocalEquivalents`.
#[test]
fn dso_local_equivalent_naming_an_undefined_function_is_rejected() {
    assert_parse_error(
        include_bytes!("fixtures/upstream/dso_local_equivalent_errors/undefined_func.ll"),
        "unknown function 'undefined_func' referenced by dso_local_equivalent",
    );
}

/// Exact `invalid_arg.ll` case from
/// `test/CodeGen/X86/dso_local_equivalent_errors.ll`. `@glob` is defined
/// *after* the reference, so the referent is a forward reference at the use
/// and the value-type check is the one
/// `ResolveForwardRefDSOLocalEquivalents` makes, not `parseValID`'s.
#[test]
fn a_forward_dso_local_equivalent_referent_must_still_be_a_function() {
    assert_parse_error(
        include_bytes!("fixtures/upstream/dso_local_equivalent_errors/invalid_arg.ll"),
        "expected a function, alias to function, or ifunc in dso_local_equivalent",
    );
}

/// `ResolveForwardRefDSOLocalEquivalents` interpolates `GVRef.StrVal` into its
/// message whatever the `ValID`'s kind, and a `t_GlobalID` leaves that string
/// empty — so the numbered spelling really does report an empty name. No
/// upstream `.ll` covers it; the quirk is anchored at the lambda itself.
#[test]
fn a_numbered_dso_local_equivalent_referent_reports_an_empty_name() {
    assert_parse_error(
        b"@p = global ptr dso_local_equivalent @0
",
        "unknown function '' referenced by dso_local_equivalent",
    );
}

/// `no_cfi` has no forward-reference map of its own: `parseValID`'s
/// `kw_no_cfi` arm only sets `ValID::NoCFI`, and `convertValIDToValue` resolves
/// the operand with `getGlobalVal` like any other `@name`. So an operand that
/// is never defined is reported by `validateEndOfModule`'s `ForwardRefVals`
/// guard, in that guard's words. No upstream `.ll` isolates it; the rule is
/// anchored at those two symbols.
#[test]
fn no_cfi_naming_an_undefined_global_is_rejected() {
    assert_parse_error(
        b"@p = global ptr no_cfi @never
",
        "use of undefined value '@never'",
    );
}

/// Exact `LLParser::parseValID` `kw_splat` accepted shape plus
/// `AsmWriter.cpp::writeConstantInternal` splat spelling: a scalar splat
/// expands to fixed-vector element storage and prints as `splat (T C)`.
#[test]
fn constant_splat_vector_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/constant_splat_vector_round_trips.ll"
    );

    let text = parse_and_render("constant_splat_vector_round_trips", FIXTURE);
    assert_check_lines(&text, &["@v = global <4 x i32> splat (i32 7)"]);
    assert_parse_print_parse_stable(&text);
}
