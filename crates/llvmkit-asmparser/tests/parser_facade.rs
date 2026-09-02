//! Public parser facade integration tests.
//!
//! Mirrors upstream `Parser.h` / `Parser.cpp` entry points exercised by
//! `unittests/AsmParser/AsmParserTest.cpp`.

use llvmkit_asmparser::file_loc::{FileLoc, FileLocRange};
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_asmparser::parser;
use llvmkit_asmparser::{
    ParserConfig, parse_dynamic, parse_dynamic_with_config, parse_file_dynamic, parse_into,
};
use llvmkit_ir::{AnyTypeEnum, BrandError, Module, module_new};

const MINIMAL: &str = include_str!("fixtures/facade_minimal.ll");
const INCOMPLETE_IR_DECLARATIONS: &str =
    include_str!("fixtures/upstream/incomplete-ir/declarations.ll");

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

/// llvmkit-specific (**no upstream counterpart**): C++ has no compile-time
/// module identity. Locks the replacement for the deleted `parse_branded`:
/// the caller claims the brand, names the module, and hands both to
/// `parse_into`, so no brand outcome enters `ParseError`.
#[test]
fn a_branded_module_is_claimed_by_the_caller_then_parsed_into() {
    struct ParsedFacade;
    impl llvmkit_ir::ModuleBrand for ParsedFacade {}

    let module = Module::branded::<ParsedFacade, _>("facade.ll").expect("brand is free");
    let module = parse_into(module, MINIMAL).expect("branded parse succeeds");
    assert_eq!(module.name(), "facade.ll");

    // The brand is held, so a second claim is refused — by `BrandError`, which
    // `ParseError` no longer has a variant for.
    assert!(matches!(
        Module::branded::<ParsedFacade, _>("again"),
        Err(BrandError::InUse { .. })
    ));

    let module = module.verify().expect("parsed module verifies");
    assert!(format!("{module}").contains("define i32 @main()"));
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

/// `ASSERT_EQ_LOC` from `unittests/AsmParser/AsmParserTest.cpp`: two ranges
/// are equal when each contains the other.
#[track_caller]
fn assert_eq_loc(left: FileLocRange, right: FileLocRange) {
    assert!(
        left.contains_range(right) && right.contains_range(left),
        "left location: {}:{} - {}:{}\nright location: {}:{} - {}:{}",
        left.start.line,
        left.start.col,
        left.end.line,
        left.end.col,
        right.start.line,
        right.start.col,
        right.end.line,
        right.end.col,
    );
}

/// Ports `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
/// ParserObjectLocations)` — the same source, the same three expected
/// [`FileLocRange`]s, and the same "the point query and the range query name
/// the same object" cross-checks.
///
/// The source is spelled here rather than loaded from a fixture because the
/// upstream test spells it inline too, and the expected columns are counted
/// off exactly those bytes.
#[test]
fn parser_object_locations() {
    let source = concat!(
        "define i32 @main() {\n",
        "entry:\n",
        "    %a = add i32 1, 2\n",
        "    ret i32 %a\n",
        "}\n"
    );
    parser::parse_assembly_with_context(source.as_bytes(), |module, _parsed, context| {
        let main_id = module.function_dyn("main").expect("@main is defined");
        let main_fn = module.view(main_id);

        let main_loc = context
            .function_location(main_fn)
            .expect("@main has a recorded range");
        assert_eq_loc(
            main_loc,
            FileLocRange::new(FileLoc::new(0, 0), FileLoc::new(4, 1)),
        );
        assert_eq!(
            context.function_at(main_loc.start),
            context.function_at_range(main_loc)
        );

        let entry_bb = main_fn
            .basic_blocks()
            .next()
            .expect("@main has an entry block");
        let entry_loc = context
            .block_location(&entry_bb)
            .expect("the entry block has a recorded range");
        assert_eq_loc(
            entry_loc,
            FileLocRange::new(FileLoc::new(1, 0), FileLoc::new(3, 14)),
        );
        assert_eq!(
            context.block_at(entry_loc.start),
            context.block_at_range(entry_loc)
        );

        let instruction_locations = [
            FileLocRange::new(FileLoc::new(2, 4), FileLoc::new(2, 21)),
            FileLocRange::new(FileLoc::new(3, 4), FileLoc::new(3, 14)),
        ];
        for (instruction, expected) in entry_bb.instructions().zip(instruction_locations) {
            let loc = context
                .instruction_location(&instruction)
                .expect("the instruction has a recorded range");
            assert_eq_loc(loc, expected);
            assert_eq!(
                context.instruction_at(loc.start),
                context.instruction_at_range(loc)
            );
        }
    })
    .expect("context parse succeeds");
}

/// llvmkit-specific: the registry is filled from real token spans, so a
/// construct that is *not* at the position a line-scanning heuristic would
/// guess is still found. Closest upstream anchor: the same three
/// `ParserContext->add*Location` calls in `LLParser::parseDefine` /
/// `parseBasicBlock`, which are span-driven by construction and so have
/// nothing to assert about it.
#[test]
fn parser_context_records_several_functions_on_one_line() {
    // Two definitions sharing a line, and a body whose instruction does not
    // start the line it is on.
    let source = "define void @a() { entry: ret void } define void @b() { entry: ret void }\n";
    parser::parse_assembly_with_context(source.as_bytes(), |module, _parsed, context| {
        let a = module.view(module.function_dyn("a").expect("@a is defined"));
        let b = module.view(module.function_dyn("b").expect("@b is defined"));
        let a_loc = context.function_location(a).expect("@a has a range");
        let b_loc = context.function_location(b).expect("@b has a range");
        assert_eq_loc(
            a_loc,
            FileLocRange::new(FileLoc::new(0, 0), FileLoc::new(0, 36)),
        );
        assert_eq_loc(
            b_loc,
            FileLocRange::new(FileLoc::new(0, 37), FileLoc::new(0, 73)),
        );
        // The reverse lookup separates them, which the old whole-line
        // heuristic could not.
        assert_eq!(context.function_at(FileLoc::new(0, 10)), Some(a));
        assert_eq!(context.function_at(FileLoc::new(0, 50)), Some(b));
    })
    .expect("context parse succeeds");
}

/// Ports `test/Assembler/incomplete-ir-declarations.ll`, whose
/// `RUN: opt -S -allow-incomplete-ir` is [`ParserConfig::allow_incomplete_ir`]
/// here. The fixture is checked in verbatim; its six `CHECK` lines are
/// asserted below in its own order.
///
/// **One of the six lines diverges and is asserted as it actually is, not
/// trimmed**, and it is recorded in `docs/divergences.md`: `@fn1`'s
/// declaration prints its parameter *names*.
/// `AssemblyWriter::printFunction` branches on `F->isDeclaration()` and prints
/// only the types there; llvmkit's `fmt_function_header` prints names
/// unconditionally, so the line reads `declare void @fn1(i32 %0)`. Unrelated
/// to this fixture and pre-existing — `crates/llvmkit-ir/tests/
/// builder_call.rs::…` already pins `declare float @llvm.acos.f32(float %0)`.
///
/// The `@fn2` line used to diverge too: llvmkit built a real `declare void
/// @fn2(i32)` at the *first* call site instead of routing a direct callee
/// through a `ForwardRefVals` placeholder, so `GetCommonFunctionType` never
/// got the chance to answer null for its three disagreeing call sites. The
/// callee position goes through `global_forward_ref` now, so the `i8` fallback
/// is reached.
#[test]
fn incomplete_ir_declarations() {
    let config = ParserConfig {
        allow_incomplete_ir: true,
        ..ParserConfig::DEFAULT
    };
    let module = parse_dynamic_with_config(INCOMPLETE_IR_DECLARATIONS, &config)
        .expect("incomplete IR parses");
    let printed = format!("{module}");
    // `@g1`..`@g4` are never callees — an argument, two pointer operands and a
    // return operand — so each takes upstream's dummy `i8` fallback, and so
    // does `@fn2`, whose three call sites disagree.
    for expected in [
        "@fn2 = external global i8",
        "@g1 = external global i8",
        "@g2 = external global i8",
        "@g3 = external global i8",
        "@g4 = external global i8",
        // `@fn1` is called twice at one signature, which is the whole point of
        // `GetCommonFunctionType`. The printer divergence supplies the ` %0`.
        "declare void @fn1(i32 %0)",
    ] {
        assert!(
            printed.contains(expected),
            "missing {expected} in:\n{printed}"
        );
    }
}

/// llvmkit-specific: the same fixture under the **default** configuration,
/// pinning that `allow_incomplete_ir` is off unless asked for. Upstream has no
/// negative fixture for the declaration half — `AllowIncompleteIR` is
/// `cl::init(false)` and every other `test/Assembler` fixture depends on it —
/// so the closest anchor is `LLParser::validateEndOfModule`'s
/// `use of undefined value '@…'` guard.
///
/// The reported name is `@fn1`, upstream's: `validateEndOfModule` reports
/// `ForwardRefVals.begin()`, which for a sorted `std::map` is the
/// lexicographically first leftover, and llvmkit's `forward_ref_globals` is a
/// `BTreeMap` holding every `@`-reference — callee or not — for the same
/// reason. It used to answer `@g1`, because a direct callee lived in a second
/// map swept afterwards.
#[test]
fn incomplete_ir_is_rejected_by_default() {
    let err = parse_dynamic(INCOMPLETE_IR_DECLARATIONS)
        .expect_err("incomplete IR is refused without the option");
    assert_eq!(err.to_string(), "use of undefined value '@fn1'");
}

/// llvmkit-specific: [`ParserConfig::data_layout_callback`] replaces the
/// file's `target datalayout` before it is parsed, and sees the target triple
/// beside it. Upstream ships no unit test for `DataLayoutCallbackTy`; the
/// closest anchor is `LLParser::parseTargetDefinitions`, which holds the
/// string tentative across the whole leading run for exactly this.
#[test]
fn data_layout_callback_overrides_the_files_layout() {
    let seen = std::cell::RefCell::new(Vec::new());
    let callback = |triple: &str, layout: &str| {
        seen.borrow_mut()
            .push((triple.to_owned(), layout.to_owned()));
        Some("e-p:32:32".to_owned())
    };
    let config = ParserConfig {
        data_layout_callback: Some(&callback),
        ..ParserConfig::DEFAULT
    };
    let source = concat!(
        "target datalayout = \"e-p:64:64\"\n",
        "target triple = \"x86_64-pc-linux-gnu\"\n",
    );
    let module = parse_dynamic_with_config(source, &config).expect("override parses");
    // The callback runs once, after the whole leading run, so it sees the
    // triple declared *below* the layout string.
    assert_eq!(
        seen.into_inner(),
        vec![("x86_64-pc-linux-gnu".to_owned(), "e-p:64:64".to_owned())]
    );
    assert!(format!("{module}").contains("target datalayout = \"e-p:32:32\""));
}

/// llvmkit-specific: a callback answering `None` keeps the file's own layout
/// string, which is the default callback upstream installs
/// (`[](StringRef, StringRef) { return std::nullopt; }`).
#[test]
fn data_layout_callback_declining_keeps_the_files_layout() {
    let callback = |_: &str, _: &str| None;
    let config = ParserConfig {
        data_layout_callback: Some(&callback),
        ..ParserConfig::DEFAULT
    };
    let module = parse_dynamic_with_config("target datalayout = \"e-p:64:64\"\n", &config)
        .expect("declined override parses");
    assert!(format!("{module}").contains("target datalayout = \"e-p:64:64\""));
}
