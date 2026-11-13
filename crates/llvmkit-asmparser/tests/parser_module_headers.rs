//! Module/header parser integration tests.
//!
//! Mirrors focused top-level/header grammar from upstream `LLParser.cpp`.

pub mod support;

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::module_new;
use llvmkit_ir::{Linkage, Module, Visibility};
use support::{check_directives, read_check_directives};

fn parse_module(src: &str) -> String {
    let module = Module::dynamic("parser_module_headers");
    Parser::new(src.as_bytes(), &module)
        .expect("parser constructs")
        .parse_module()
        .expect("module parses");
    format!("{module}")
}

/// The `dso_local` / visibility / DLL-storage clause run parses in
/// `LLParser::parseOptionalLinkage`'s order, which is also the order
/// `llvm/lib/IR/AsmWriter.cpp::AssemblyWriter::printFunction` emits
/// (`printDSOLocation`, then `printVisibility`, then `printDLLStorageClass`),
/// and prints back as `printDSOLocation` writes it.
///
/// **No upstream fixture combines `dso_local` with a visibility keyword** —
/// `test/Assembler` covers each clause alone — so this is llvmkit-specific,
/// with `parseOptionalLinkage` as the anchor for the order. It is a
/// regression lock twice over. llvmkit read visibility *before* dso-locality
/// until 0.0.5, and its own writer emitted that same wrong order, so it
/// round-tripped itself while rejecting `define dso_local hidden void @f()`.
/// And its writer then printed the keyword back as written, where
/// `printDSOLocation` prints `dso_local` only when `isImplicitDSOLocal` does not
/// already imply it, which a hidden or protected visibility does, and never
/// prints `dso_preemptable`.
#[test]
fn dso_local_precedes_visibility_and_dll_storage() {
    for (src, header) in [
        (
            "define dso_local hidden void @f() { ret void }\n",
            "define hidden void @f()",
        ),
        (
            "declare dso_local protected void @g()\n",
            "declare protected void @g()",
        ),
        (
            "define dso_preemptable dllexport void @h() { ret void }\n",
            "define dllexport void @h()",
        ),
        (
            "@v = dso_local hidden global i32 0\n",
            "@v = hidden global i32 0",
        ),
    ] {
        let printed = parse_module(src);
        assert!(
            printed.contains(header),
            "expected `{header}` in printed module, got:\n{printed}"
        );
    }
}

/// llvmkit-specific: the setters' half of `GlobalValue::isDSOLocal`. The bit
/// `setLinkage` and `setVisibility` set when `isImplicitDSOLocal` holds outlives
/// the linkage or visibility that set it, and only `setDSOLocal(false)` clears
/// it. No upstream unit test isolates it: `rg -n
/// "isDSOLocal|setDSOLocal|isImplicitDSOLocal"
/// orig_cpp/llvm-project-llvmorg-22.1.4/llvm/unittests` prints nothing.
#[test]
fn the_dso_local_bit_outlives_what_set_it() {
    let module = Module::dynamic("dso_local_bit");
    Parser::new(b"@g = global i32 0\n", &module)
        .expect("parser constructs")
        .parse_module()
        .expect("module parses");
    let g = module.view(module.global("g").expect("@g is defined"));
    assert!(!g.is_dso_local());

    g.set_linkage(&module, Linkage::Internal);
    assert!(g.is_dso_local(), "a local linkage sets the bit");
    g.set_linkage(&module, Linkage::External);
    assert!(g.is_dso_local(), "the bit stays when the linkage goes");
    assert!(format!("{module}").contains("@g = dso_local global i32 0"));

    g.set_dso_local(&module, false);
    assert!(!g.is_dso_local());
    g.set_visibility(&module, Visibility::Hidden);
    assert!(g.is_dso_local(), "a hidden visibility sets the bit");
    assert!(format!("{module}").contains("@g = hidden global i32 0"));
    g.set_visibility(&module, Visibility::Default);
    assert!(g.is_dso_local(), "the bit stays when the visibility goes");
    assert!(format!("{module}").contains("@g = dso_local global i32 0"));
}

/// Ports `test/Bitcode/dso_location.ll`, vendored verbatim and checked against
/// its own `CHECK` block. Its RUN line is `llvm-as < %s | llvm-dis`: llvmkit has
/// no bitcode, and the bit round-trips through it unchanged
/// (`BitcodeWriter.cpp` writes `isDSOLocal()`, `BitcodeReader.cpp` reads it
/// back through `setDSOLocal`), so a parse and a print stand for the pipeline.
///
/// The fixture pins `printDSOLocation`: `dso_local default` prints as
/// `dso_local`, `hidden` and `protected` print no `dso_local` though both imply
/// it, and `dso_preemptable` never prints. llvmkit used to print the keyword it
/// was given.
///
/// Its `ret void ()* null` pins `LLParser::parseRet` too, which reads a whole
/// type before it asks `isVoidTy`: llvmkit took the `void` token for
/// `ret void` and refused the file as `value doesn't match function result
/// type 'ptr'`.
#[test]
fn the_dso_location_fixture_prints_as_upstream() {
    const FIXTURE: &str = include_str!("fixtures/upstream/Bitcode/dso_location.ll");

    let printed = parse_module(FIXTURE);
    check_directives(&printed, &read_check_directives(FIXTURE));
}

/// `LLParser::parseOptionalComdat`'s bare form: `comdat` with no parenthesised
/// name borrows the symbol's own name. llvmkit rejected it on globals with
/// `expected explicit comdat($name)` and, on functions, silently built a
/// comdat named `""`.
///
/// `test/Assembler` covers only the failing case
/// (`unnamed-comdat.ll`, ported in `parser_diagnostics.rs`), so the accepting
/// half is anchored on `parseOptionalComdat` itself (D11).
#[test]
fn bare_comdat_borrows_the_symbols_own_name() {
    let printed = parse_module("$v = comdat any\n@v = global i32 0, comdat\n");
    assert!(printed.contains("@v = global i32 0, comdat"), "{printed}");

    let printed = parse_module("$f = comdat any\ndefine void @f() comdat {\n  ret void\n}\n");
    assert!(printed.contains("define void @f() comdat"), "{printed}");
}

/// `LLParser::parseDeclare` collects metadata attachments written *before*
/// the header and applies them to the function once it exists. llvmkit went
/// straight to linkage, so `declare !dbg !0 void @f()` did not parse at all.
/// `define` has no such prefix form — its attachments follow the header.
#[test]
fn declare_accepts_metadata_before_the_header() {
    let printed = parse_module(
        "declare !dbg !0 void @f()\n\
         !0 = distinct !DISubprogram(name: \"f\", spFlags: DISPFlagDefinition)\n",
    );
    assert!(printed.contains("declare !dbg !0 void @f()"), "{printed}");
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
    let err = {
        let module = module_new!("parser_module_headers_invalid").expect("fresh module");
        Parser::new(b"define common void @f() { ret void }\n", &module)
            .expect("parser constructs")
            .parse_module()
            .expect_err("common function linkage rejected")
    };
    assert!(matches!(err, ParseError::Message { .. }));
    assert_eq!(err.to_string(), "invalid function linkage type");
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
