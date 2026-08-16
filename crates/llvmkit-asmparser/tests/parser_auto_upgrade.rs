//! AutoUpgrade at end of module.
//!
//! Each `#[test]` ports an upstream `.ll` fixture that exercises one of the
//! `AutoUpgrade.h` entry points `LLParser::validateEndOfModule` calls.
//! Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_to_text(src: &str) -> String {
    let module = Module::dynamic("test");
    let _ = Parser::new(src.as_bytes(), &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    format!("{module}")
}

fn assert_contains(text: &str, needle: &str) {
    assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
}

/// Ports `llvm/test/Bitcode/upgrade-module-flag.ll`, whose `RUN` line is
/// `llvm-as < %s | llvm-dis`: every `CHECK` is `AsmWriter`'s rendering of what
/// `LLParser::validateEndOfModule` handed back, so the upgrade is the parser's.
///
/// Covers five arms of `llvm::UpgradeModuleFlags` at once — `PIC Level`
/// Error → Min, `PIE Level` Error → Max, the space-stripping of
/// `Objective-C Image Info Section`, the `amdgpu_code_object_version` rename,
/// and the `Objective-C Class Properties` flag synthesised because
/// `Objective-C Image Info Version` is present without it.
#[test]
fn module_flags_are_upgraded() {
    let src = r#"
!llvm.module.flags = !{!0, !1, !2, !3, !4}

!0 = !{i32 1, !"PIC Level", i32 1}
!1 = !{i32 1, !"PIE Level", i32 1}
!2 = !{i32 1, !"Objective-C Image Info Version", i32 0}
!3 = !{i32 1, !"Objective-C Image Info Section", !"__DATA, __objc_imageinfo, regular, no_dead_strip"}
!4 = !{i32 1, !"amdgpu_code_object_version", i32 500}
"#;
    let text = parse_to_text(src);
    assert_contains(&text, "!{i32 8, !\"PIC Level\", i32 1}");
    assert_contains(&text, "!{i32 7, !\"PIE Level\", i32 1}");
    assert_contains(
        &text,
        "!{i32 1, !\"Objective-C Image Info Version\", i32 0}",
    );
    assert_contains(
        &text,
        "!{i32 1, !\"Objective-C Image Info Section\", !\"__DATA,__objc_imageinfo,regular,no_dead_strip\"}",
    );
    assert_contains(&text, "!{i32 1, !\"amdhsa_code_object_version\", i32 500}");
    assert_contains(&text, "!{i32 4, !\"Objective-C Class Properties\", i32 0}");
}

/// Ports `llvm/test/Bitcode/upgrade-garbage-collection-for-objc.ll` — the
/// `Objective-C Garbage Collection` arm of `llvm::UpgradeModuleFlags`, which
/// rewrites an `i32`-typed value to `i8` and forces the behavior to `Error`.
///
/// The fixture's `Objective-C Class Properties` flag is already present, so
/// the synthesised-flag arm stays quiet.
#[test]
fn objc_garbage_collection_flag_narrows_to_i8() {
    let src = r#"
target triple = "x86_64-apple-macosx10.15.0"

!llvm.module.flags = !{!0, !1, !2, !3, !4, !5, !6, !7}
!llvm.ident = !{!8}

!0 = !{i32 2, !"SDK Version", [2 x i32] [i32 10, i32 15]}
!1 = !{i32 1, !"Objective-C Version", i32 2}
!2 = !{i32 1, !"Objective-C Image Info Version", i32 0}
!3 = !{i32 1, !"Objective-C Image Info Section", !"__DATA,__objc_imageinfo,regular,no_dead_strip"}
!4 = !{i32 1, !"Objective-C Garbage Collection", i32 0}
!5 = !{i32 1, !"Objective-C Class Properties", i32 64}
!6 = !{i32 1, !"wchar_size", i32 4}
!7 = !{i32 7, !"PIC Level", i32 2}
!8 = !{!"Apple clang version 11.0.0 (clang-1100.0.33.12)"}
"#;
    let text = parse_to_text(src);
    assert_contains(&text, "!{i32 1, !\"Objective-C Garbage Collection\", i8 0}");
}

/// Ports the module-flag half of
/// `llvm/test/Bitcode/upgrade-garbage-collection-for-swift.ll`: a wide
/// `Objective-C Garbage Collection` value (`83953408` = `0x05010700`) is split
/// into an `i8 0` low byte plus the three synthesised Swift flags, with the
/// ABI version taken from bits 8..16, the major from 24..32 and the minor from
/// 16..24.
///
/// The fixture's function body is dropped: it is `i8**` typed-pointer IR whose
/// only role is to give the module something to hold, and the `CHECK` lines
/// name only module flags.
#[test]
fn objc_garbage_collection_flag_yields_swift_versions() {
    let src = r#"
target triple = "x86_64-apple-macosx10.15.0"

!llvm.module.flags = !{!0, !1, !2, !3, !4, !5, !6, !7, !8}
!swift.module.flags = !{!9}

!0 = !{i32 2, !"SDK Version", [2 x i32] [i32 10, i32 15]}
!1 = !{i32 1, !"Objective-C Version", i32 2}
!2 = !{i32 1, !"Objective-C Image Info Version", i32 0}
!3 = !{i32 1, !"Objective-C Image Info Section", !"__DATA,__objc_imageinfo,regular,no_dead_strip"}
!4 = !{i32 4, !"Objective-C Garbage Collection", i32 83953408}
!5 = !{i32 1, !"Objective-C Class Properties", i32 64}
!6 = !{i32 1, !"wchar_size", i32 4}
!7 = !{i32 7, !"PIC Level", i32 2}
!8 = !{i32 1, !"Swift Version", i32 7}
!9 = !{!"standard-library", i1 false}
"#;
    let text = parse_to_text(src);
    assert_contains(&text, "!{i32 1, !\"Objective-C Garbage Collection\", i8 0}");
    assert_contains(&text, "!{i32 1, !\"Swift ABI Version\", i32 7}");
    assert_contains(&text, "!{i32 1, !\"Swift Major Version\", i8 5}");
    assert_contains(&text, "!{i32 1, !\"Swift Minor Version\", i8 1}");
}

/// Ports `llvm/test/Bitcode/upgrade-section-name.ll` verbatim —
/// `llvm::UpgradeSectionAttributes`, which strips the spaces around the commas
/// of an Objective-C category-list section name.
///
/// The fixture's `CHECK` is
/// `@"OBJC_LABEL_CATEGORY_$" = {{.*}}, section "__DATA,__objc_catlist,regular,no_dead_strip"`.
/// Only the section half is asserted: llvmkit prints the global's name
/// *unquoted*, because its port of `printLLVMNameWithoutPrefix` treats `$` as
/// a plain character where upstream's allowed set is `isalnum || '-' || '.' ||
/// '_'` and quotes anything else. That is a pre-existing printer divergence
/// this fixture happens to walk past, not the behavior under test — see
/// `docs/divergences.md`. `@"\01l_OBJC_$_CATEGORY_I_$_Robot"` still prints
/// quoted, because `\01` is outside the set on both sides.
///
/// The `__DATA, __objc_const` section of `@"\01l_OBJC_$_CATEGORY_I_$_Robot"`
/// is the fixture's own negative case: the guard is a prefix test on
/// `"__DATA, __objc_catlist"`, so that global keeps its spaces.
#[test]
fn objc_catlist_section_name_loses_its_spaces() {
    let src = r#"
%struct._class_t = type { %struct._class_t*, %struct._class_t*, %struct._objc_cache*, i8* (i8*, i8*)**, %struct._class_ro_t* }
%struct._objc_cache = type opaque
%struct._class_ro_t = type { i32, i32, i32, i8*, i8*, %struct.__method_list_t*, %struct._objc_protocol_list*, %struct._ivar_list_t*, i8*, %struct._prop_list_t* }
%struct.__method_list_t = type { i32, i32, [0 x %struct._objc_method] }
%struct._objc_method = type { i8*, i8*, i8* }
%struct._objc_protocol_list = type { i64, [0 x %struct._protocol_t*] }
%struct._protocol_t = type { i8*, i8*, %struct._objc_protocol_list*, %struct.__method_list_t*, %struct.__method_list_t*, %struct.__method_list_t*, %struct.__method_list_t*, %struct._prop_list_t*, i32, i32, i8**, i8*, %struct._prop_list_t* }
%struct._ivar_list_t = type { i32, i32, [0 x %struct._ivar_t] }
%struct._ivar_t = type { i64*, i8*, i8*, i32, i32 }
%struct._prop_list_t = type { i32, i32, [0 x %struct._prop_t] }
%struct._prop_t = type { i8*, i8* }
%struct._category_t = type { i8*, %struct._class_t*, %struct.__method_list_t*, %struct.__method_list_t*, %struct._objc_protocol_list*, %struct._prop_list_t*, %struct._prop_list_t*, i32 }

@OBJC_CLASS_NAME_ = private unnamed_addr constant [6 x i8] c"Robot\00", section "__TEXT,__objc_classname,cstring_literals", align 1
@"OBJC_CLASS_$_I" = external global %struct._class_t
@"\01l_OBJC_$_CATEGORY_I_$_Robot" = private global %struct._category_t { i8* getelementptr inbounds ([6 x i8], [6 x i8]* @OBJC_CLASS_NAME_, i32 0, i32 0), %struct._class_t* @"OBJC_CLASS_$_I", %struct.__method_list_t* null, %struct.__method_list_t* null, %struct._objc_protocol_list* null, %struct._prop_list_t* null, %struct._prop_list_t* null, i32 64 }, section "__DATA, __objc_const", align 8
@"OBJC_LABEL_CATEGORY_$" = private global [1 x i8*] [i8* bitcast (%struct._category_t* @"\01l_OBJC_$_CATEGORY_I_$_Robot" to i8*)], section "__DATA, __objc_catlist, regular, no_dead_strip", align 8
@llvm.compiler.used = appending global [3 x i8*] [i8* bitcast (%struct._category_t* @"\01l_OBJC_$_CATEGORY_I_$_Robot" to i8*), i8* getelementptr inbounds ([6 x i8], [6 x i8]* @OBJC_CLASS_NAME_, i32 0, i32 0), i8* bitcast ([1 x i8*]* @"OBJC_LABEL_CATEGORY_$" to i8*)], section "llvm.metadata"

!llvm.module.flags = !{!0, !1, !2, !3, !4, !5}

!0 = !{i32 1, !"Objective-C Version", i32 2}
!1 = !{i32 1, !"Objective-C Image Info Version", i32 0}
!2 = !{i32 1, !"Objective-C Image Info Section", !"__DATA, __objc_imageinfo, regular, no_dead_strip"}
!3 = !{i32 4, !"Objective-C Garbage Collection", i32 0}
!4 = !{i32 1, !"Objective-C Class Properties", i32 64}
!5 = !{i32 1, !"PIC Level", i32 2}
"#;
    let text = parse_to_text(src);
    assert_contains(
        &text,
        ", section \"__DATA,__objc_catlist,regular,no_dead_strip\"",
    );
    assert_contains(&text, ", section \"__DATA, __objc_const\"");
}

/// Scalar-format TBAA tags are rewritten into the struct-path-aware format.
/// Mirrors `llvm::UpgradeTBAANode` reached from `validateEndOfModule`'s
/// `InstsWithTBAATag` loop.
///
/// The input shape is the pre-struct-path spelling `test/Analysis/TypeBasedAliasAnalysis`
/// fixtures still carry — `!{!"int", !{!"omnipotent char", !{!"Simple C/C++ TBAA"}}}` —
/// which upstream rewrites to `!{tag, tag, i64 0}` because the tag has two
/// operands rather than three.
#[test]
fn scalar_tbaa_tag_becomes_struct_path() {
    let src = r#"
define void @f(ptr %p) {
  store i32 0, ptr %p, !tbaa !0
  ret void
}

!0 = !{!"int", !1}
!1 = !{!"omnipotent char", !2}
!2 = !{!"Simple C/C++ TBAA"}
"#;
    let text = parse_to_text(src);
    assert!(
        !text.contains("!tbaa !0"),
        "the scalar tag should have been replaced:\n{text}"
    );
}

/// A struct-path-aware tag — first operand an `MDNode`, at least three
/// operands — is left alone. Mirrors `UpgradeTBAANode`'s early return, and is
/// the shape every clang since 3.6 emits.
#[test]
fn struct_path_tbaa_tag_is_left_alone() {
    let src = r#"
define void @f(ptr %p) {
  store i32 0, ptr %p, !tbaa !0
  ret void
}

!0 = !{!1, !1, i64 0}
!1 = !{!"int", !2, i64 0}
!2 = !{!"omnipotent char", !3, i64 0}
!3 = !{!"Simple C/C++ TBAA"}
"#;
    let text = parse_to_text(src);
    assert_contains(&text, "!tbaa !0");
    assert_contains(&text, "!0 = !{!1, !1, i64 0}");
}

/// A zero-operand tag is invalid and `UpgradeTBAANode` returns it untouched,
/// leaving the verifier to report it. Mirrors the `NumOperands == 0` guard.
///
/// The input is the one already pinned by
/// `parser_metadata.rs::instruction_multiple_trailing_metadata`, re-read here
/// for the upgrade path rather than the attachment printer.
#[test]
fn empty_tbaa_tag_is_left_alone() {
    let src = r#"
define void @f() {
  ret void, !dbg !0, !tbaa !1
}

!0 = !{}
!1 = !{}
"#;
    let text = parse_to_text(src);
    assert_contains(&text, "ret void, !dbg !0, !tbaa !1");
}

/// A module with no `llvm.module.flags` node is untouched: `UpgradeModuleFlags`
/// opens with `if (!ModFlags) return false;`. llvmkit-specific in shape — there
/// is no upstream fixture for "nothing happens" — but the guard it pins is
/// upstream's.
#[test]
fn a_module_without_flags_is_untouched() {
    let module = Module::dynamic("empty");
    assert!(!llvmkit_ir::auto_upgrade::upgrade_module_flags(&module));
}
