//! Module/header parser integration tests.
//!
//! Mirrors focused top-level/header grammar from upstream `LLParser.cpp`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

fn parse_module(src: &str) -> String {
    Module::with_new("parser_module_headers", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("parser constructs")
            .parse_module()
            .expect("module parses");
        format!("{module}")
    })
}

/// Mirrors `LLParser.cpp::parseFunctionHeader`: function definitions accept
/// non-declaration linkage before the return type.
#[test]
fn function_linkage_definition_round_trips() {
    let printed = parse_module("define internal void @f() { ret void }\n");
    assert!(printed.contains("define internal void @f()"));
}

/// Mirrors `LLParser.cpp::parseFunctionHeader` and `test/Assembler/unnamed-addr.ll`:
/// function headers accept the post-argument `local_unnamed_addr` marker.
#[test]
fn function_local_unnamed_addr_round_trips() {
    let printed = parse_module("define void @f() local_unnamed_addr { ret void }\n");
    assert!(printed.contains("define void @f() local_unnamed_addr"));
}

/// Mirrors `LLParser.cpp::parseFunctionHeader`: `extern_weak` is valid on
/// function declarations and is stored with other function header fields.
#[test]
fn extern_weak_declaration_with_unnamed_addr_round_trips() {
    let printed = parse_module("declare extern_weak void @hook() unnamed_addr\n");
    assert!(printed.contains("declare extern_weak void @hook() unnamed_addr"));
}
/// Mirrors `LLParser.cpp::parseAliasOrIFunc` and `test/Assembler/alias-redefinition.ll`:
/// a module-level alias stores its aliasee and prints the canonical `alias` form.
#[test]
fn global_alias_round_trips() {
    let printed = parse_module("@target = global i32 0\n@alias = alias i32, ptr @target\n");
    assert!(printed.contains("@target = global i32 0\n"));
    assert!(printed.contains("@alias = alias i32, ptr @target\n"));
}

/// Mirrors `LLParser.cpp::parseFunctionHeader`'s linkage validation: `common`
/// and `appending` are global-variable linkages, not function linkages.
#[test]
fn common_function_linkage_is_rejected() {
    let err = Module::with_new("parser_module_headers_invalid", |module| {
        Parser::new(b"define common void @f() { ret void }\n", &module)
            .expect("parser constructs")
            .parse_module()
            .expect_err("common function linkage rejected")
    });
    match err {
        ParseError::Expected { expected, .. } => {
            assert_eq!(expected, "invalid function linkage type")
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

/// Mirrors `LLParser.cpp::parseUnnamedAttrGroup`: attribute groups are parsed
/// once and referenced from declarations by `#N`.
#[test]
fn attribute_group_round_trips() {
    let printed = parse_module(
        "attributes #0 = { nounwind \"frame-pointer\"=\"all\" }\ndeclare void @f() #0\n",
    );
    assert!(
        printed.contains("attributes #0 = { nounwind \"frame-pointer\"=\"all\" }"),
        "output: {printed}"
    );
    assert!(
        printed.contains("declare void @f() #0"),
        "output: {printed}"
    );
}

/// Mirrors `LLParser.cpp::parseFunctionHeader` for stored function header
/// modifiers that llvmkit models today.
#[test]
fn function_full_header_round_trips() {
    let printed = parse_module(
        "attributes #0 = { nounwind }\n\
define hidden dllimport fastcc zeroext i32 @f(i32 zeroext %x) unnamed_addr addrspace(1) #0 section \".text\" partition \"part\" align 4 gc \"statepoint-example\" !dbg !0 {\n\
entry:\n\
  ret i32 %x, !dbg !1\n\
}\n\
!0 = !DISubprogram(name: \"f\")\n\
!1 = !DILocation(line: 1, column: 1, scope: !0)\n",
    );
    assert!(printed.contains("define hidden dllimport fastcc zeroext i32 @f(i32 zeroext %x) unnamed_addr addrspace(1) #0"), "output: {printed}");
    assert!(printed.contains("section \".text\""), "output: {printed}");
    assert!(printed.contains("partition \"part\""), "output: {printed}");
    assert!(printed.contains("align 4"), "output: {printed}");
    assert!(
        printed.contains("gc \"statepoint-example\""),
        "output: {printed}"
    );
    assert!(printed.contains("!dbg !0"), "output: {printed}");
    assert!(printed.contains("ret i32 %x, !dbg !1"), "output: {printed}");
}

/// Mirrors `test/Assembler/function-operand-uselistorder.ll`: function
/// headers preserve comdat plus prefix/prologue/personality operands.
#[test]
fn function_operand_header_fields_round_trip() {
    let printed = parse_module(
        "$foo = comdat any\n\
@g = global i32 0\n\
define void @f() comdat($foo) prefix ptr @g prologue ptr @g personality ptr @g {\n\
entry:\n\
  ret void\n\
}\n",
    );
    assert!(
        printed.contains(
            "define void @f() comdat($foo) prefix ptr @g prologue ptr @g personality ptr @g"
        ),
        "output: {printed}"
    );
}
