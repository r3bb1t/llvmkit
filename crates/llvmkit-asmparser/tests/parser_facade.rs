//! Public parser facade integration tests.
//!
//! Mirrors upstream `Parser.h` / `Parser.cpp` entry points exercised by
//! `unittests/AsmParser/AsmParserTest.cpp`.

use llvmkit_asmparser::file_loc::FileLoc;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_asmparser::parser;
use llvmkit_ir::{AnyTypeEnum, Module};

const MINIMAL: &str = include_str!("fixtures/facade_minimal.ll");

/// Ports `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
/// ParseAssemblyString)` to the Rust facade.
#[test]
fn parse_assembly_string_round_trips_module() {
    parser::parse_assembly_string(MINIMAL, |module, _parsed| {
        let printed = format!("{module}");
        assert!(printed.contains("target triple = \"x86_64-pc-linux-gnu\""));
        assert!(printed.contains("define i32 @main()"));
        assert!(printed.contains("ret i32 0"));
    })
    .expect("facade parse succeeds");
}

/// Ports `llvm/lib/AsmParser/Parser.cpp::parseAssemblyFile` file-loading
/// wrapper shape.
#[test]
fn parse_assembly_file_reads_file() {
    parser::parse_assembly_file(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/facade_minimal.ll"
        ),
        |module, _parsed| {
            let printed = format!("{module}");
            assert!(printed.contains("define i32 @main()"));
            assert!(printed.contains("ret i32 0"));
        },
    )
    .expect("facade file parse succeeds");
}

/// Mirrors `LLParser.cpp::parseTypeAtBeginning`: parsing stops after the
/// first type and reports the consumed byte count.
#[test]
fn parse_type_at_beginning_reports_read_count() {
    Module::with_new::<_, _, _>("facade_type_prefix", |module| {
        let (ty, consumed) = parser::parse_type_at_beginning(b"i32, rest", &module, None)
            .expect("type prefix parses");
        assert_eq!(consumed, 3);
        assert!(matches!(AnyTypeEnum::from(ty), AnyTypeEnum::Int(t) if t.bit_width() == 32));
    });
}

/// Mirrors `LLParser.cpp::parseType`: the standalone facade requires EOF.
#[test]
fn parse_type_requires_end() {
    Module::with_new::<_, _, _>("facade_type_eof", |module| {
        let err = parser::parse_type(b"i32 trailing", &module, None)
            .expect_err("trailing token rejected");
        match err {
            ParseError::Expected { expected, .. } => assert_eq!(expected, "end of string"),
            other => panic!("unexpected error variant: {other:?}"),
        }
    });
}

/// Mirrors `LLParser.cpp::parseTargetExtType`: target extension types parse
/// their name plus type and integer parameters.
#[test]
fn parse_target_extension_type() {
    Module::with_new::<_, _, _>("facade_target_ext_type", |module| {
        let ty = parser::parse_type(b"target(\"aarch64.svcount\")", &module, None)
            .expect("target extension type parses");
        assert_eq!(format!("{ty}"), "target(\"aarch64.svcount\")");
        assert!(matches!(
            AnyTypeEnum::from(ty),
            AnyTypeEnum::TargetExt(t) if t.name() == "aarch64.svcount"
        ));

        let with_params = parser::parse_type(b"target(\"spirv.Image\", i32, 7)", &module, None)
            .expect("target extension type with parameters parses");
        assert_eq!(format!("{with_params}"), "target(\"spirv.Image\", i32, 7)");
    });
}

/// Mirrors `LLParser.cpp::parseTargetExtType`: type parameters must precede
/// integer parameters in target extension types.
#[test]
fn parse_target_extension_rejects_type_after_integer_param() {
    Module::with_new::<_, _, _>("facade_target_ext_bad_param_order", |module| {
        let err = parser::parse_type(b"target(\"spirv.Image\", 7, i32)", &module, None)
            .expect_err("type parameter after integer parameter is malformed");
        match err {
            ParseError::Expected { expected, .. } => assert_eq!(expected, "target extension type"),
            other => panic!("unexpected error variant: {other:?}"),
        }
    });
}

/// Mirrors `LLParser.cpp::parseStandaloneConstantValue` through the facade.
#[test]
fn parse_constant_value_uses_slot_mapping() {
    Module::with_new::<_, _, _>("facade_constant", |module| {
        let i32_ty = module.i32_type().as_type();
        let constant =
            parser::parse_constant_value(b"42", &module, i32_ty, None).expect("constant parses");
        assert_eq!(constant.ty(), i32_ty);
    });
}

/// Mirrors `AsmParserContext.cpp` source-location recording exposed through
/// `Parser.cpp` parse-with-context entry points.
#[test]
fn parser_context_records_function_block_instruction_locations() {
    parser::parse_assembly_with_context(MINIMAL.as_bytes(), |_module, _parsed, context| {
        assert!(context.function_at(FileLoc::new(2, 0)).is_some());
        assert!(context.block_at(FileLoc::new(3, 0)).is_some());
        assert!(context.instruction_at(FileLoc::new(4, 2)).is_some());
    })
    .expect("context parse succeeds");
}
