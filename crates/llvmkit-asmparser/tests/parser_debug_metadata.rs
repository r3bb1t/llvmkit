//! Specialized debug metadata parser tests.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    Module::with_new("parser_debug_metadata", |module| {
        Parser::new(src.as_bytes(), &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        format!("{module}")
    })
}

const DEBUG_MODULE: &str = r#"
@g = global i32 0, !dbg !15

define i32 @f(), !dbg !3 {
entry:
  ret i32 0, !dbg !4
}

!0 = !DIFile(filename: "a.c", directory: "/tmp")
!1 = !DICompileUnit(file: !0, language: DW_LANG_C, producer: "llvmkit")
!2 = !DISubroutineType(types: !{!7, !7})
!3 = distinct !DISubprogram(name: "f", file: !0, type: !2, unit: !1)
!4 = !DILocation(line: 1, column: 2, scope: !3)
!5 = !DILocalVariable(name: "x", file: !0, type: !7, scope: !3)
!6 = !DIExpression()
!7 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
!8 = !DIDerivedType(name: "ptr", baseType: !7, size: 64)
!9 = !DISubrange(count: 4)
!10 = !DICompositeType(name: "arr", baseType: !7, elements: !{!9})
!11 = !DINamespace(name: "ns", scope: !3)
!12 = !DIEnumerator(name: "A", value: 1)
!13 = !DIModule(name: "m", scope: !11)
!14 = !DIGlobalVariable(name: "g", file: !0, type: !7, scope: !13)
!15 = !DIGlobalVariableExpression(var: !14, expr: !6)
!16 = !DITemplateTypeParameter(name: "T", type: !7)
!17 = !DITemplateValueParameter(name: "N", type: !7, value: 7)
"#;

/// Mirrors `LLParser.cpp::parseSpecializedMDNode` for the core DI node set.
#[test]
fn specialized_debug_nodes_round_trip() {
    let text = parse_and_render(DEBUG_MODULE);
    for needle in [
        "!DIFile(",
        "!DICompileUnit(",
        "distinct !DISubprogram(",
        "!DILocation(",
        "!DILocalVariable(",
        "!DIBasicType(",
        "!DIDerivedType(",
        "!DICompositeType(",
        "!DISubrange(",
        "!DINamespace(",
        "!DIExpression(",
        "!DIGlobalVariable(",
        "!DIGlobalVariableExpression(",
        "!DISubroutineType(",
        "!DIEnumerator(",
        "!DIModule(",
        "!DITemplateTypeParameter(",
        "!DITemplateValueParameter(",
    ] {
        assert!(text.contains(needle), "missing {needle} in:\n{text}");
    }
}

/// Mirrors `AsmWriter.cpp::SlotTracker::CreateMetadataSlot`: debug metadata
/// attachments are printed with canonical dense slots, not literal source ids.
#[test]
fn function_and_global_debug_attachments_round_trip() {
    let text = parse_and_render(DEBUG_MODULE);
    assert!(
        text.contains("@g = global i32 0, !dbg !0"),
        "output:\n{text}"
    );
    assert!(text.contains("define i32 @f(), !dbg !1"), "output:\n{text}");
    assert!(text.contains("ret i32 0, !dbg !2"), "output:\n{text}");
}

/// Mirrors `test/Assembler/dbg_declare_value.ll`.
#[test]
fn dbg_declare_value_record_round_trip() {
    let text = parse_and_render(
        r#"
define void @foo(double %x) !dbg !0 {
entry:
  #dbg_declare_value(double %x, !1, !DIExpression(), !2)
  ret void, !dbg !2
}

!0 = distinct !DISubprogram(name: "foo", type: !3, unit: !4)
!1 = !DILocalVariable(name: "x", scope: !0, type: !5)
!2 = !DILocation(line: 1, column: 17, scope: !0)
!3 = !DISubroutineType(types: !{null, !5})
!4 = !DICompileUnit(language: DW_LANG_C11, producer: "llvmkit")
!5 = !DIBasicType(name: "double", size: 64, encoding: DW_ATE_float)
"#,
    );
    assert!(
        text.contains("#dbg_declare_value(double %x, !1, !DIExpression(), !2)"),
        "output:\n{text}"
    );
}
