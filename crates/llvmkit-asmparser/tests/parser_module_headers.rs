//! Module/header parser integration tests.
//!
//! Mirrors focused top-level/header grammar from upstream `LLParser.cpp`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_module(src: &str) -> Module<'_> {
    let module = Module::new("parser_module_headers");
    Parser::new(src.as_bytes(), &module)
        .expect("parser constructs")
        .parse_module()
        .expect("module parses");
    module
}

/// Mirrors `LLParser.cpp::parseFunctionHeader`: function definitions accept
/// non-declaration linkage before the return type.
#[test]
fn function_linkage_definition_round_trips() {
    let module = parse_module("define internal void @f() { ret void }\n");
    let printed = format!("{module}");
    assert!(printed.contains("define internal void @f()"));
}

/// Mirrors `LLParser.cpp::parseFunctionHeader` and `test/Assembler/unnamed-addr.ll`:
/// function headers accept the post-argument `local_unnamed_addr` marker.
#[test]
fn function_local_unnamed_addr_round_trips() {
    let module = parse_module("define void @f() local_unnamed_addr { ret void }\n");
    let printed = format!("{module}");
    assert!(printed.contains("define void @f() local_unnamed_addr"));
}

/// Mirrors `LLParser.cpp::parseFunctionHeader`: `extern_weak` is valid on
/// function declarations and is stored with other function header fields.
#[test]
fn extern_weak_declaration_with_unnamed_addr_round_trips() {
    let module = parse_module("declare extern_weak void @hook() unnamed_addr\n");
    let printed = format!("{module}");
    assert!(printed.contains("declare extern_weak void @hook() unnamed_addr"));
}
/// Mirrors `LLParser.cpp::parseAliasOrIFunc` and `test/Assembler/alias-redefinition.ll`:
/// a module-level alias stores its aliasee and prints the canonical `alias` form.
#[test]
fn global_alias_round_trips() {
    let module = parse_module("@target = global i32 0\n@alias = alias i32, ptr @target\n");
    let printed = format!("{module}");
    assert!(printed.contains("@target = global i32 0\n"));
    assert!(printed.contains("@alias = alias i32, ptr @target\n"));
}

/// Mirrors `LLParser.cpp::parseFunctionHeader`'s linkage validation: `common`
/// and `appending` are global-variable linkages, not function linkages.
#[test]
fn common_function_linkage_is_rejected() {
    let module = Module::new("parser_module_headers_invalid");
    let err = Parser::new(b"define common void @f() { ret void }\n", &module)
        .expect("parser constructs")
        .parse_module()
        .expect_err("common function linkage rejected");
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "invalid function linkage type")
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
