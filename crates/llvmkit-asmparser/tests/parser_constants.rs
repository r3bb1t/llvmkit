//! Constant parser tests.
//!
//! Tests either include exact upstream `.ll` excerpts with `include_bytes!` or
//! translate one `LLParser.cpp::parseValID` branch directly. Citations live in
//! `UPSTREAM.md`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError, parser};
use llvmkit_ir::Module;

fn parse_and_render(module_name: &str, src: &[u8]) -> String {
    Module::with_new(module_name, |module| {
        Parser::new(src, &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
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

fn assert_parse_print_parse_stable(text: &str) {
    let module_name = text
        .strip_prefix("; ModuleID = '")
        .and_then(|tail| tail.split_once('\''))
        .map_or("parser_constants_reparse", |(name, _)| name);
    let reparsed = parse_and_render(module_name, text.as_bytes());
    assert_eq!(reparsed, text);
}

fn assert_parse_error(src: &[u8], expected_message: &str) {
    let err = Module::with_new("parser_constants_error", |module| {
        Parser::new(src, &module)
            .expect("lexer primes")
            .parse_module()
            .expect_err("fixture is rejected")
    });
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, expected_message),
        other => panic!("unexpected error variant: {other:?}"),
    }
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

/// Direct port of `LLParser::parseValID`'s integer binary
/// constant-expression branch: the accepted `add (ty lhs, ty rhs)` shape.
#[test]
fn constant_expr_binary_round_trip() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseValID/constant_expr_binary_round_trip.ll");

    let text = parse_and_render("constant_expr_binary_round_trip", FIXTURE);
    assert_check_lines(&text, &["@sum = global i32 add (i32 1, i32 2)"]);
    assert_parse_print_parse_stable(&text);
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
    Module::with_new("parser_constants_none", |module| {
        let parsed =
            parser::parse_constant_value(b"none", &module, module.token_type().as_type(), None)
                .expect("token none parses");
        assert_eq!(format!("{}", parsed.as_value()), "token none");
    });
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
        Module::with_new("parser_constants_unsupported", |module| {
            let err = Parser::new(src, &module)
                .expect("lexer primes")
                .parse_module()
                .expect_err("unsupported constexpr is rejected");
            match err {
                ParseError::Expected { expected, .. } => {
                    assert_eq!(
                        expected,
                        format!("{opcode} constexprs are no longer supported")
                    );
                }
                other => panic!("unexpected error variant: {other:?}"),
            }
        });
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

/// Exact negative constant-GEP fixture from
/// `test/Assembler/constant-getelementptr-scalable_pointee.ll`.
#[test]
fn constant_expr_gep_rejects_scalable_vector_pointee() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/constant-getelementptr-scalable_pointee.ll");

    assert_parse_error(FIXTURE, "invalid base element for constant getelementptr");
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID` `kw_none`
/// and `Constants.cpp::ConstantTargetNone::get`: `none` is token-only in the
/// shipped parser subset.
#[test]
fn none_is_token_only() {
    Module::with_new("parser_constants_none_token", |module| {
        let parsed =
            parser::parse_constant_value(b"none", &module, module.token_type().as_type(), None)
                .expect("token none parses");
        assert_eq!(format!("{}", parsed.as_value()), "token none");

        let target_ty = module
            .target_ext_type(
                "spirv.Image",
                Vec::<llvmkit_ir::Type>::new(),
                Vec::<u32>::new(),
            )
            .as_type();
        let err = parser::parse_constant_value(b"none", &module, target_ty, None)
            .expect_err("target-extension none is rejected");
        match err {
            ParseError::Expected { expected, .. } => {
                assert_eq!(expected, "invalid type for none constant")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    });
}

/// llvmkit-specific subset of `test/Assembler/target-types.ll` and
/// `Type.cpp::getTargetTypeInfo`: target-extension zeroinitializer requires the
/// zero-initializable property.
#[test]
fn target_ext_zeroinitializer_requires_zero_init_property() {
    Module::with_new("parser_constants_target_zero", |module| {
        let zero_ty = module
            .target_ext_type(
                "spirv.foo",
                Vec::<llvmkit_ir::Type>::new(),
                Vec::<u32>::new(),
            )
            .as_type();
        let zero = parser::parse_constant_value(b"zeroinitializer", &module, zero_ty, None)
            .expect("zero-initializable target extension parses");
        assert_eq!(
            format!("{}", zero.as_value()),
            "target(\"spirv.foo\") zeroinitializer"
        );

        let image_ty = module
            .target_ext_type(
                "spirv.Image",
                Vec::<llvmkit_ir::Type>::new(),
                Vec::<u32>::new(),
            )
            .as_type();
        let err = parser::parse_constant_value(b"zeroinitializer", &module, image_ty, None)
            .expect_err("non-zero-initializable target extension is rejected");
        match err {
            ParseError::Expected { expected, .. } => {
                assert_eq!(expected, "invalid type for null constant")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    });
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

/// Exact `LLParser::parseValID` `kw_splat` accepted shape: a scalar splat
/// expands to fixed-vector element storage.
#[test]
fn constant_splat_vector_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/constant_splat_vector_round_trips.ll"
    );

    let text = parse_and_render("constant_splat_vector_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &["@v = global <4 x i32> <i32 7, i32 7, i32 7, i32 7>"],
    );
    assert_parse_print_parse_stable(&text);
}
