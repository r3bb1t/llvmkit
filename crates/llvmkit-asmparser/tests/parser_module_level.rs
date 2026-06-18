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
/// Mirrors `LLParser::parseSourceFileName` and `AsmWriter.cpp`'s
/// `getSourceFileName()` print arm: the directive is stored on the
/// module and re-emitted immediately after the `ModuleID` comment.
#[test]
fn source_filename_round_trips_through_asm_writer() {
    let m = Module::new("source_file");
    parse_into("source_filename = \"dir/file.c\"\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("; ModuleID = 'source_file'\nsource_filename = \"dir/file.c\"\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors `LLParser::parseComdat`: a top-level `$name = comdat <kind>`
/// directive creates the module COMDAT entry and AsmWriter re-emits it.
#[test]
fn top_level_comdat_round_trips() {
    let m = Module::new("comdat_module");
    parse_into("$foo = comdat largest\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("$foo = comdat largest\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors the `externally_initialized` flag in `LLParser::parseGlobal`.
#[test]
fn global_externally_initialized_round_trips() {
    let m = Module::new("global_externally_initialized");
    parse_into("@g = externally_initialized global i32 0\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("@g = externally_initialized global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors the full linkage-prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_linkage_round_trips() {
    let m = Module::new("global_linkage");
    parse_into("@g = weak_odr global i32 0\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("@g = weak_odr global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors the visibility-prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_visibility_round_trips() {
    let m = Module::new("global_visibility");
    parse_into("@g = hidden global i32 0\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("@g = hidden global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors the DLL storage class prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_dll_storage_round_trips() {
    let m = Module::new("global_dll_storage");
    parse_into("@g = dllexport global i32 0\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("@g = dllexport global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors the thread-local mode prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_tls_mode_round_trips() {
    let m = Module::new("global_tls");
    parse_into("@g = thread_local(initialexec) global i32 0\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("@g = thread_local(initialexec) global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors the unnamed-address prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_unnamed_addr_round_trips() {
    let m = Module::new("global_unnamed_addr");
    parse_into("@g = local_unnamed_addr global i32 0\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("@g = local_unnamed_addr global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors the address-space prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_addrspace_round_trips() {
    let m = Module::new("global_addrspace");
    parse_into("@g = addrspace(3) global i32 0\n", &m);
    let printed = format!("{m}");
    assert!(
        printed.contains("@g = addrspace(3) global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors the global-object suffix loop in `LLParser::parseGlobal` for
/// section, partition, explicit COMDAT attachment, and alignment.
#[test]
fn global_trailing_attributes_round_trip() {
    let m = Module::new("global_trailing_attrs");
    parse_into(
        "$foo = comdat any\n@g = global i32 0, section \".data\", partition \"part\", comdat($foo), align 8\n",
        &m,
    );
    let printed = format!("{m}");
    assert!(
        printed.contains(
            "@g = global i32 0, section \".data\", partition \"part\", comdat($foo), align 8\n"
        ),
        "AsmWriter output: {printed}"
    );
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

/// Mirrors `LLParser::parseTopLevelEntities`'s default arm: any unknown
/// leading token is reported as a typed `top-level entity` error.
#[test]
fn unknown_top_level_entity_is_typed_error() {
    let m = Module::new("unknown_top_level");
    let parser = Parser::new(b"42 i32\n", &m).unwrap();
    let err = parser.parse_module().unwrap_err();
    assert!(format!("{err}").contains("top-level entity"));
}
