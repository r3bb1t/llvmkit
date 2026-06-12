//! Public parser facade integration tests.
//!
//! Mirrors upstream `Parser.h` / `Parser.cpp` entry points exercised by
//! `unittests/AsmParser/AsmParserTest.cpp`.

use llvmkit_asmparser::asm_parser_context::AsmParserContext;
use llvmkit_asmparser::file_loc::FileLoc;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_asmparser::parser;
use llvmkit_ir::{AnyTypeEnum, Module};

const MINIMAL: &str = include_str!("fixtures/facade_minimal.ll");

/// Ports `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
/// ParseAssemblyString)` to the Rust facade.
#[test]
fn parse_assembly_string_into_round_trips_module() {
    let module = Module::new("facade_string");
    parser::parse_assembly_string_into(MINIMAL, &module).expect("facade parse succeeds");
    let printed = format!("{module}");
    assert!(printed.contains("target triple = \"x86_64-pc-linux-gnu\""));
    assert!(printed.contains("define i32 @main()"));
    assert!(printed.contains("ret i32 0"));
}

/// Ports `llvm/lib/AsmParser/Parser.cpp::parseAssemblyFile` file-loading
/// wrapper shape.
#[test]
fn parse_assembly_file_into_reads_file() {
    let module = Module::new("facade_file");
    parser::parse_assembly_file_into(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/facade_minimal.ll"
        ),
        &module,
    )
    .expect("facade file parse succeeds");
    let printed = format!("{module}");
    assert!(printed.contains("define i32 @main()"));
    assert!(printed.contains("ret i32 0"));
}

/// Mirrors `LLParser.cpp::parseTypeAtBeginning`: parsing stops after the
/// first type and reports the consumed byte count.
#[test]
fn parse_type_at_beginning_reports_read_count() {
    let module = Module::new("facade_type_prefix");
    let (ty, consumed) =
        parser::parse_type_at_beginning(b"i32, rest", &module, None).expect("type prefix parses");
    assert_eq!(consumed, 3);
    assert!(matches!(AnyTypeEnum::from(ty), AnyTypeEnum::Int(t) if t.bit_width() == 32));
}

/// Mirrors `LLParser.cpp::parseType`: the standalone facade requires EOF.
#[test]
fn parse_type_requires_end() {
    let module = Module::new("facade_type_eof");
    let err =
        parser::parse_type(b"i32 trailing", &module, None).expect_err("trailing token rejected");
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, "end of string"),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `LLParser.cpp::parseStandaloneConstantValue` through the facade.
#[test]
fn parse_constant_value_uses_slot_mapping() {
    let module = Module::new("facade_constant");
    let constant = parser::parse_constant_value(b"42", &module, module.i32_type().as_type(), None)
        .expect("constant parses");
    assert_eq!(constant.ty(), module.i32_type().as_type());
}

/// Mirrors `AsmParserContext.cpp` source-location recording exposed through
/// `Parser.cpp` parse-with-context entry points.
#[test]
fn parser_context_records_function_block_instruction_locations() {
    let module = Module::new("facade_context");
    let mut context = AsmParserContext::new();
    parser::parse_assembly_into_with_context(MINIMAL.as_bytes(), &module, &mut context)
        .expect("context parse succeeds");
    assert!(context.function_at(FileLoc::new(2, 0)).is_some());
    assert!(context.block_at(FileLoc::new(3, 0)).is_some());
    assert!(context.instruction_at(FileLoc::new(4, 2)).is_some());
}
