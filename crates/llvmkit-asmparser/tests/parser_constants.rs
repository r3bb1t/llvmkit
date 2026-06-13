//! Constant parser tests.
//!
//! Mirrors upstream aggregate-constant parsing from
//! `test/Assembler/aggregate-constant-values.ll`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError, parser};
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    let module = Module::new("parser_constants");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    format!("{module}")
}

/// Mirrors `test/Assembler/aggregate-constant-values.ll`: array constant
/// initializer syntax (`[i32 1, i32 2]`) as a global initializer.
#[test]
fn array_constant_initializer_round_trips() {
    let text = parse_and_render("@arr = global [2 x i32] [i32 1, i32 2]\n");
    assert!(
        text.contains("@arr = global [2 x i32] [i32 1, i32 2]"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `test/Assembler/aggregate-constant-values.ll`: struct constant
/// initializer syntax (`{ i32 1, i32 2 }`) as a global initializer.
#[test]
fn struct_constant_initializer_round_trips() {
    let text = parse_and_render("@pair = global { i32, i32 } { i32 1, i32 2 }\n");
    assert!(
        text.contains("@pair = global { i32, i32 } { i32 1, i32 2 }"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `test/Assembler/getelementptr.ll`: a global initializer may be a
/// `getelementptr` constant expression.
#[test]
fn getelementptr_constant_expr_initializer_round_trips() {
    let text = parse_and_render(
        "@data = global i8 0\n@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)\n",
    );
    assert!(
        text.contains("@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)"),
        "AsmWriter output: {text}"
    );
}
/// Mirrors `test/Assembler/ConstantExprNoFold.ll`: cast constant expressions
/// round-trip through parser and writer without constructing instructions.
#[test]
fn constant_expr_casts_round_trip() {
    let src = "@g = global i32 0\n@p = global ptr bitcast (ptr @g to ptr)\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@p = global ptr bitcast (ptr @g to ptr)"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("@p = global ptr bitcast (ptr @g to ptr)"));
}

/// Mirrors `test/Assembler/ConstantExprNoFold.ll`: supported binary constant
/// expressions print in the canonical `add (ty lhs, ty rhs)` form.
#[test]
fn constant_expr_binary_round_trip() {
    let src = "@sum = global i32 add (i32 1, i32 2)\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@sum = global i32 add (i32 1, i32 2)"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("@sum = global i32 add (i32 1, i32 2)"));
}

/// Mirrors `test/Assembler/getelementptr.ll`: general getelementptr constant
/// expressions use the `ConstantExpr` storage path, not the legacy offset-only
/// compatibility node.
#[test]
fn constant_expr_gep_round_trip() {
    let src =
        "@data = global i8 0\n@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)"));
}

/// Mirrors `test/Assembler/blockaddress.ll`: blockaddress constants name a
/// function and a basic block.
#[test]
fn blockaddress_round_trips() {
    let src =
        "define void @f() {\nentry:\n  ret void\n}\n@addr = global ptr blockaddress(@f, %entry)\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@addr = global ptr blockaddress(@f, %entry)"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("@addr = global ptr blockaddress(@f, %entry)"));
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID`:
/// `dso_local_equivalent @f` is a parser-level constant form.
#[test]
fn dso_local_equivalent_round_trips() {
    let src = "declare void @f()\n@p = global ptr dso_local_equivalent @f\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@p = global ptr dso_local_equivalent @f"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("@p = global ptr dso_local_equivalent @f"));
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID`: `no_cfi`
/// wraps a global function reference as a constant.
#[test]
fn no_cfi_round_trips() {
    let src = "declare void @f()\n@p = global ptr no_cfi @f\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@p = global ptr no_cfi @f"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("@p = global ptr no_cfi @f"));
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID` `kw_none`
/// and `test/Assembler/target-types.ll`: `none` is accepted for token and
/// target extension constants once storage exists.
#[test]
fn token_or_target_none_round_trips() {
    let module = Module::new("parser_constants_none");
    let parsed =
        parser::parse_constant_value(b"none", &module, module.token_type().as_type(), None)
            .expect("token none parses");
    assert_eq!(format!("{}", parsed.as_value()), "token none");
}

/// Mirrors `test/Assembler/ConstantExprNoFold.ll` and
/// `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID`: `ptrtoaddr`
/// is a distinct supported constant expression opcode.
#[test]
fn ptrtoaddr_constant_expr_round_trips() {
    let src = "@g = global i32 0\n@addr = global i64 ptrtoaddr (ptr @g to i64)\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@addr = global i64 ptrtoaddr (ptr @g to i64)"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("@addr = global i64 ptrtoaddr (ptr @g to i64)"));
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID`: LLVM
/// 22 rejects legacy folded constant-expression opcodes with opcode-specific
/// diagnostics.
#[test]
fn unsupported_constant_expr_opcodes_are_rejected() {
    for (opcode, src) in [
        ("fadd", "@x = global double fadd (double 1.0, double 2.0)\n"),
        ("zext", "@x = global i64 zext (i32 1 to i64)\n"),
        ("mul", "@x = global i32 mul (i32 1, i32 2)\n"),
        ("select", "@x = global i32 select (i1 true, i32 1, i32 2)\n"),
        ("icmp", "@x = global i1 icmp (i32 1, i32 2)\n"),
    ] {
        let module = Module::new("parser_constants_unsupported");
        let err = Parser::new(src.as_bytes(), &module)
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
/// and `Constants.cpp::ConstantTargetNone::get`: `none` is token-only.
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

/// Mirrors `test/Assembler/target-types.ll` and `Type.cpp::getTargetTypeInfo`:
/// target-extension zeroinitializer requires the zero-initializable property.
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

/// Mirrors `Constants.cpp::ConstantPtrAuth::get`: ptrauth carries pointer,
/// key, discriminator, address discriminator, and deactivation symbol.
#[test]
fn ptrauth_five_operands_round_trips() {
    let src = "@g = global i8 0\n@signed = global ptr ptrauth (ptr @g, i32 0, i64 1, ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 2 to ptr))\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@signed = global ptr ptrauth (ptr @g, i32 0, i64 1, ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 2 to ptr))"),
        "AsmWriter output: {text}"
    );
    let reparsed = parse_and_render(&text);
    assert!(reparsed.contains("@signed = global ptr ptrauth (ptr @g, i32 0, i64 1, ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 2 to ptr))"));
}

/// Mirrors `Constants.cpp::ConstantPtrAuth::get`: default ptrauth operands
/// are omitted only when all trailing operands are defaults.
#[test]
fn ptrauth_default_operands_are_elided() {
    let src = "declare void @f()\n@signed = global ptr ptrauth (ptr @f, i32 0)\n";
    let text = parse_and_render(src);
    assert!(
        text.contains("@signed = global ptr ptrauth (ptr @f, i32 0)"),
        "AsmWriter output: {text}"
    );
    assert!(
        !text.contains("i64 0") && !text.contains("ptr null"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `test/Assembler/blockaddress.ll`: forward blockaddress references
/// resolve to the later real function signature, not a placeholder `void ()`.
#[test]
fn forward_blockaddress_resolves_later_signature() {
    let src = "@addr = global ptr blockaddress(@f, %entry)\ndefine i32 @f(i32 %x) {\nentry:\n  ret i32 %x\n}\n";
    let text = parse_and_render(src);
    assert!(text.contains("@addr = global ptr blockaddress(@f, %entry)"));
    assert!(
        !text.contains("declare void @f()"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID`: forward
/// dso/no_cfi references resolve to the later real declaration.
#[test]
fn forward_dso_and_no_cfi_resolve_later_signature() {
    let src =
        "@d = global ptr dso_local_equivalent @f\n@n = global ptr no_cfi @f\ndeclare i32 @f(i32)\n";
    let text = parse_and_render(src);
    assert!(text.contains("@d = global ptr dso_local_equivalent @f"));
    assert!(text.contains("@n = global ptr no_cfi @f"));
    assert!(
        text.contains("declare i32 @f(i32"),
        "AsmWriter output: {text}"
    );
    assert!(
        !text.contains("declare void @f()"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID` `kw_splat`:
/// splat constants expand to fixed-vector element lists in storage.
#[test]
fn constant_splat_vector_round_trips() {
    let text = parse_and_render("@v = global <4 x i32> splat (i32 7)\n");
    assert!(
        text.contains("@v = global <4 x i32> <i32 7, i32 7, i32 7, i32 7>"),
        "AsmWriter output: {text}"
    );
}
