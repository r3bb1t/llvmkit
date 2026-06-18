//! Constant parser tests.
//!
//! Tests either include exact upstream `.ll` excerpts with `include_bytes!` or
//! translate one `LLParser.cpp::parseValID` branch directly. Citations live in
//! `UPSTREAM.md`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError, parser};
use llvmkit_ir::Module;

fn parse_and_render(module_name: &str, src: &[u8]) -> String {
    let module = Module::new(module_name);
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

/// llvmkit-specific subset of `LLParser::parseValID`'s `getelementptr`
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

/// llvmkit-specific subset of `LLParser::parseValID`'s integer binary
/// constant-expression branch: the accepted `add (ty lhs, ty rhs)` shape.
#[test]
fn constant_expr_binary_round_trip() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseValID/constant_expr_binary_round_trip.ll");

    let text = parse_and_render("constant_expr_binary_round_trip", FIXTURE);
    assert_check_lines(&text, &["@sum = global i32 add (i32 1, i32 2)"]);
    assert_parse_print_parse_stable(&text);
}

/// llvmkit-specific subset of `LLParser::parseValID`'s general
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

/// llvmkit-specific subset of `LLParser::parseValID`'s `blockaddress` branch:
/// the accepted `blockaddress(@function, %block)` shape.
#[test]
fn blockaddress_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseValID/blockaddress_round_trips.ll");

    let text = parse_and_render("blockaddress_round_trips", FIXTURE);
    assert_check_lines(&text, &["@addr = global ptr blockaddress(@f, %entry)"]);
    assert_parse_print_parse_stable(&text);
}

/// llvmkit-specific subset of `LLParser::parseValID`'s
/// `dso_local_equivalent` branch: the accepted global-initializer shape.
#[test]
fn dso_local_equivalent_round_trips() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/LLParser-parseValID/dso_local_equivalent_round_trips.ll");

    let text = parse_and_render("dso_local_equivalent_round_trips", FIXTURE);
    assert_check_lines(&text, &["@p = global ptr dso_local_equivalent @f"]);
    assert_parse_print_parse_stable(&text);
}

/// llvmkit-specific subset of `LLParser::parseValID`'s `no_cfi` branch: the
/// accepted global-initializer shape.
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
    let module = Module::new("parser_constants_none");
    let parsed =
        parser::parse_constant_value(b"none", &module, module.token_type().as_type(), None)
            .expect("token none parses");
    assert_eq!(format!("{}", parsed.as_value()), "token none");
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

/// llvmkit-specific subset of `LLParser::parseValID`'s unsupported legacy
/// constant-expression diagnostics.
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
        let module = Module::new("parser_constants_unsupported");
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
    }
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID` `kw_none`
/// and `Constants.cpp::ConstantTargetNone::get`: `none` is token-only in the
/// shipped parser subset.
#[test]
fn none_is_token_only() {
    let module = Module::new("parser_constants_none_token");
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
}

/// llvmkit-specific subset of `test/Assembler/target-types.ll` and
/// `Type.cpp::getTargetTypeInfo`: target-extension zeroinitializer requires the
/// zero-initializable property.
#[test]
fn target_ext_zeroinitializer_requires_zero_init_property() {
    let module = Module::new("parser_constants_target_zero");
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
}

/// llvmkit-specific subset of `LLParser::parseValID`'s `ptrauth` branch: the
/// five-operand shape accepted by llvmkit.
#[test]
fn ptrauth_five_operands_round_trips() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/LLParser-parseValID/ptrauth_five_operands_round_trips.ll"
    );

    let text = parse_and_render("ptrauth_five_operands_round_trips", FIXTURE);
    assert_check_lines(
        &text,
        &[
            "@signed = global ptr ptrauth (ptr @g, i32 0, i64 1, ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 2 to ptr))",
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

/// llvmkit-specific subset of `LLParser::parseValID`'s forward blockaddress
/// placeholder resolution.
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

/// llvmkit-specific subset of `LLParser::parseValID`'s forward
/// `dso_local_equivalent` / `no_cfi` placeholder resolution.
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
