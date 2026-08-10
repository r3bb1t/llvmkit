//! Public parser facade integration tests.
//!
//! Mirrors upstream `Parser.h` / `Parser.cpp` entry points exercised by
//! `unittests/AsmParser/AsmParserTest.cpp`.

use llvmkit_asmparser::file_loc::FileLoc;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_asmparser::parser;
use llvmkit_asmparser::{parse_branded, parse_dynamic, parse_file_dynamic, parse_into};
use llvmkit_ir::{AnyTypeEnum, Module, module_new};

const MINIMAL: &str = include_str!("fixtures/facade_minimal.ll");

/// Ports `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
/// ParseAssemblyString)` to the Rust facade.
#[test]
fn parse_assembly_string_round_trips_module() {
    parser::parse_assembly(MINIMAL, |module, _parsed| {
        let printed = format!("{module}");
        assert!(printed.contains("target triple = \"x86_64-pc-linux-gnu\""));
        assert!(printed.contains("define i32 @main()"));
        assert!(printed.contains("ret i32 0"));
    })
    .expect("facade parse succeeds");
}

/// llvmkit-specific (cycle C4): the closure-free entry points hand back the
/// **owned** module, so the caller can `verify()` it — which the closure forms
/// cannot do, because they only lend the module by reference.
#[test]
fn parse_dynamic_returns_an_owned_verifiable_module() {
    let module = parse_dynamic(MINIMAL).expect("owned parse succeeds");
    let module = module.verify().expect("parsed module verifies");
    let printed = format!("{module}");
    assert!(printed.contains("target triple = \"x86_64-pc-linux-gnu\""));
    assert!(printed.contains("define i32 @main()"));
    assert!(printed.contains("ret i32 0"));
}

/// The owned module is a value: it can be pushed into a `Vec` and outlive the
/// call that produced it. `DynBrand` is registry-exempt, so many can be live.
#[test]
fn parse_dynamic_modules_collect_into_a_vec() {
    let sources = [
        "@a = global i32 1
",
        "@b = global i32 2
",
        "@c = global i32 3
",
    ];
    let modules: Vec<_> = sources
        .into_iter()
        .map(|src| parse_dynamic(src).expect("owned parse succeeds"))
        .collect();
    assert_eq!(modules.len(), 3);
    // Distinct modules, separated by the runtime tag.
    assert_ne!(modules[0].id(), modules[1].id());
    assert_ne!(modules[1].id(), modules[2].id());
    assert!(format!("{}", modules[2]).contains("@c = global i32 3"));
}

/// A named brand survives the parse: the returned token carries `B`, so its
/// handles are statically separated from every other module's.
#[test]
fn parse_branded_returns_the_named_brand() {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct ParsedFacade;
    impl llvmkit_ir::ModuleBrand for ParsedFacade {}

    let module: Module<ParsedFacade, _> =
        parse_branded::<ParsedFacade, _>(MINIMAL).expect("branded parse succeeds");
    let module = module.verify().expect("parsed module verifies");
    assert!(format!("{module}").contains("define i32 @main()"));

    // The brand is claimed for as long as the module lives.
    assert!(matches!(
        parse_branded::<ParsedFacade, _>(MINIMAL),
        Err(ParseError::BrandInUse { .. })
    ));
    drop(module);
    // ...and released when it dies.
    assert!(parse_branded::<ParsedFacade, _>(MINIMAL).is_ok());
}

/// `parse_into` lets the caller pick the module — here an unnameable
/// `module_new!` brand — and hands the same token straight back.
#[test]
fn parse_into_fills_a_caller_supplied_module() {
    let module = module_new!("caller-named").expect("fresh brand");
    let id_before = module.id();
    let module = parse_into(module, MINIMAL).expect("parse into succeeds");
    assert_eq!(module.id(), id_before);
    assert_eq!(module.name(), "caller-named");
    let module = module.verify().expect("parsed module verifies");
    assert!(format!("{module}").contains("define i32 @main()"));
}

/// The file entry point names the module after the file and returns it owned.
#[test]
fn parse_file_dynamic_returns_an_owned_module_named_after_the_file() {
    let module = parse_file_dynamic(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/facade_minimal.ll"
    ))
    .expect("owned file parse succeeds");
    assert_eq!(module.name(), "facade_minimal.ll");
    let module = module.verify().expect("parsed module verifies");
    assert!(format!("{module}").contains("define i32 @main()"));
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
    let module = module_new!("facade_type_prefix").expect("fresh module");
    let (ty, consumed) =
        parser::parse_type_at_beginning(b"i32, rest", &module).expect("type prefix parses");
    assert_eq!(consumed, 3);
    assert!(matches!(AnyTypeEnum::from(ty), AnyTypeEnum::Int(t) if t.bit_width() == 32));
}

/// Mirrors `LLParser.cpp::parseType`: the standalone facade requires EOF.
#[test]
fn parse_type_requires_end() {
    let module = module_new!("facade_type_eof").expect("fresh module");
    let err = parser::parse_type(b"i32 trailing", &module).expect_err("trailing token rejected");
    match err {
        ParseError::Expected { expected, .. } => assert_eq!(expected, "end of string"),
        other => panic!("unexpected error variant: {other:?}"),
    }
}

/// Mirrors `LLParser.cpp::parseTargetExtType`: target extension types parse
/// their name plus type and integer parameters.
#[test]
fn parse_target_extension_type() {
    let module = module_new!("facade_target_ext_type").expect("fresh module");
    let ty = parser::parse_type(b"target(\"aarch64.svcount\")", &module)
        .expect("target extension type parses");
    assert_eq!(format!("{ty}"), "target(\"aarch64.svcount\")");
    assert!(matches!(
        AnyTypeEnum::from(ty),
        AnyTypeEnum::TargetExt(t) if t.name() == "aarch64.svcount"
    ));

    let with_params = parser::parse_type(b"target(\"spirv.Image\", i32, 7)", &module)
        .expect("target extension type with parameters parses");
    assert_eq!(format!("{with_params}"), "target(\"spirv.Image\", i32, 7)");
}

/// Mirrors `LLParser.cpp::parseTargetExtType`: type parameters must precede
/// integer parameters in target extension types. The message is upstream's
/// `expected uint32 param`; llvmkit reported a generic production name until
/// the `SeenInt` guard was ported.
#[test]
fn parse_target_extension_rejects_type_after_integer_param() {
    let module = module_new!("facade_target_ext_bad_param_order").expect("fresh module");
    let err = parser::parse_type(b"target(\"spirv.Image\", 7, i32)", &module)
        .expect_err("type parameter after integer parameter is malformed");
    assert_eq!(err.to_string(), "expected uint32 param");
}

/// Mirrors `LLParser.cpp::parseStandaloneConstantValue` through the facade.
#[test]
fn parse_constant_value_uses_slot_mapping() {
    let module = module_new!("facade_constant").expect("fresh module");
    let i32_ty = module.i32_type().as_type();
    let constant = parser::parse_constant_value(b"42", &module, i32_ty).expect("constant parses");
    assert_eq!(constant.ty(), i32_ty);
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
