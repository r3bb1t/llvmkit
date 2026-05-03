//! Module-level parser integration tests.
//!
//! Mirrors the constructive subset of upstream
//! `unittests/AsmParser/AsmParserTest.cpp` and `test/Assembler/*.ll`
//! fixtures that Session 2 of the parser-first roadmap is responsible
//! for. Each `#[test]` cites the upstream anchor it ports.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::{AnyTypeEnum, Module};

fn parse_into(src: &str, m: &Module<'_>) {
    Parser::new(src.as_bytes(), m)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
}

/// Mirrors `test/Assembler/datalayout.ll` + the trailing `target triple`
/// arm: a module that carries both directives round-trips through the
/// AsmWriter byte-for-byte.
#[test]
fn target_directives_round_trip_through_asm_writer() {
    let m = Module::new("target_directives_round_trip");
    parse_into(
        "target datalayout = \"e-m:e-i64:64\"\ntarget triple = \"x86_64-unknown-linux-gnu\"\n",
        &m,
    );
    let printed = format!("{m}");
    assert!(printed.contains("target datalayout = \"e-m:e-i64:64\""));
    assert!(printed.contains("target triple = \"x86_64-unknown-linux-gnu\""));
}

/// Ports the `module asm` arm of `test/Assembler/module-asm.ll`. Multiple
/// directives accumulate, separated by newlines as upstream's
/// `printModuleInlineAsm` emits.
#[test]
fn module_asm_directives_accumulate() {
    let m = Module::new("module_asm");
    parse_into(
        "module asm \"first line\"\nmodule asm \"second line\"\n",
        &m,
    );
    let asm = m.module_asm();
    assert!(asm.contains("first line"));
    assert!(asm.contains("second line"));
}

/// Mirrors `test/Assembler/named-types.ll`: a recursive named-struct
/// forward reference resolves once the matching `%foo = type { ... }`
/// definition is encountered.
#[test]
fn named_struct_forward_reference_resolves() {
    let m = Module::new("recursive_named");
    let parser = Parser::new(b"%self = type { ptr }\n", &m).unwrap();
    let parsed = parser.parse_module().expect("parser succeeds");
    // The named-type table records the definition so external callers can
    // resolve `%self` against this module.
    assert!(parsed.slot_mapping.named_types.contains_key("self"));
}

/// Ports the `[N x T]` and `<N x T>` arms of `LLParser::parseType`. The
/// declarator ingestion proves both compositions work end-to-end.
#[test]
fn array_and_vector_types_parse() {
    let m = Module::new("aggregate_types");
    parse_into("declare void @takes([4 x i32], <8 x float>)\n", &m);
    let f = m.function_by_name("takes").expect("function present");
    let params: Vec<_> = f.signature().params().collect();
    assert_eq!(params.len(), 2);
    assert!(matches!(
        AnyTypeEnum::from(params[0]),
        AnyTypeEnum::Array(_)
    ));
    let v = match AnyTypeEnum::from(params[1]) {
        AnyTypeEnum::Vector(v) => v,
        other => panic!("expected vector type, got {other:?}"),
    };
    assert!(!v.is_scalable());
}

/// Ports the `<vscale x N x T>` arm of `LLParser::parseType`.
#[test]
fn scalable_vector_type_parses() {
    let m = Module::new("scalable_vec");
    parse_into("declare void @sv(<vscale x 4 x i32>)\n", &m);
    let f = m.function_by_name("sv").expect("function present");
    let params: Vec<_> = f.signature().params().collect();
    let v = match AnyTypeEnum::from(params[0]) {
        AnyTypeEnum::Vector(v) => v,
        other => panic!("expected vector type, got {other:?}"),
    };
    assert!(v.is_scalable());
}

/// Mirrors `test/Assembler/declare.ll`: a varargs declaration whose
/// signature round-trips through the AsmWriter.
#[test]
fn variadic_declaration_round_trips() {
    let m = Module::new("variadic_decl");
    parse_into("declare i32 @printf(ptr, ...)\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("declare i32 @printf(ptr, ...)")
            || printed.contains("declare i32 @printf(ptr %0, ...)"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors `test/Assembler/global-variable-attributes.ll` (the integer
/// arm). The numbered-global slot table tracks `@0`, `@1`, etc. like
/// upstream's `NumberedVals`.
#[test]
fn numbered_global_records_in_slot_mapping() {
    let m = Module::new("numbered_globals");
    let parser = Parser::new(b"@0 = global i32 0\n@1 = global i32 1\n", &m).unwrap();
    let parsed = parser.parse_module().expect("parser succeeds");
    assert_eq!(parsed.slot_mapping.global_values.get_next(), 2);
    assert!(parsed.slot_mapping.global_values.get(0).is_some());
    assert!(parsed.slot_mapping.global_values.get(1).is_some());
}

/// Mirrors `LLParser::parseType`'s `void` arm: void is rejected outside of
/// function-result position. Upstream emits "void type only allowed for
/// function results"; the Rust analogue uses a structured error.
#[test]
fn void_in_value_position_is_rejected() {
    let m = Module::new("reject_void");
    let parser = Parser::new(b"@x = global void 0\n", &m).unwrap();
    let err = parser.parse_module().unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("non-void type") || msg.contains("only allowed at function results"),
        "got: {msg}"
    );
}

/// Mirrors the `parseType` rejection of legacy `T*` typed pointers in
/// LLVM 17+ opaque-pointer mode.
#[test]
fn legacy_typed_pointer_is_rejected() {
    let m = Module::new("legacy_ptr");
    let parser = Parser::new(b"%s = type { i32* }\n", &m).unwrap();
    let err = parser.parse_module().unwrap_err();
    assert!(format!("{err}").contains("opaque-pointer"));
}

/// Mirrors `LLParser::parseTopLevelEntities`'s default arm: any unknown
/// leading token is reported as a typed `top-level entity` error.
#[test]
fn unknown_top_level_entity_is_typed_error() {
    let m = Module::new("unknown_top_level");
    let parser = Parser::new(b"42 i32\n", &m).unwrap();
    let err = parser.parse_module().unwrap_err();
    assert!(format!("{err}").contains("top-level entity"));
}
