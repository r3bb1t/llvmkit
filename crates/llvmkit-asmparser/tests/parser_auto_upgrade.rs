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
/// The fixture's only `CHECK` is
/// `@"OBJC_LABEL_CATEGORY_$" = {{.*}}, section "__DATA,__objc_catlist,regular,no_dead_strip"`,
/// and both halves of it are asserted. The name half used to be skipped:
/// llvmkit printed it unquoted, because its port of
/// `printLLVMNameWithoutPrefix` carried `$` in the allowed set where upstream's
/// is `isalnum || '-' || '.' || '_'`. That divergence is closed and its
/// ledger entry deleted.
/// `{{.*}}` cannot cross a line, so the two halves are asserted against the one
/// line rather than against the whole module.
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
    let line = text
        .lines()
        .find(|line| line.starts_with("@\"OBJC_LABEL_CATEGORY_$\" = "))
        .unwrap_or_else(|| panic!("no `@\"OBJC_LABEL_CATEGORY_$\"` line in:\n{text}"));
    assert!(
        line.contains(", section \"__DATA,__objc_catlist,regular,no_dead_strip\""),
        "{line}"
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

/// Ports `llvm/test/CodeGen/NVPTX/upgrade-nvvm-annotations.ll` whole — all
/// fourteen `!nvvm.annotations` entries and every `CHECK` line it carries.
///
/// The fixture's `RUN` line is `opt < %s -passes=verify -S | FileCheck %s`:
/// `opt` builds the module through `LLParser::Run`, so
/// `validateEndOfModule`'s `UpgradeNVVMAnnotations` call is what produced the
/// checked text, and `-passes=verify` changes nothing. Every `CHECK` is
/// therefore `AsmWriter`'s rendering of the parser's own answer.
///
/// Two spelling differences from the `CHECK` block, neither introduced here:
/// upstream hoists function attributes into numbered `attributes #N = { … }`
/// groups where llvmkit prints them inline on the header
/// (`docs/divergences.md` entries 40 and 58), and llvmkit prints every node in
/// the metadata arena where `opt` prints only the reachable ones — so `!14`
/// and the emptied entries survive. The attribute *contents* the `CHECK`
/// lines name are asserted verbatim.
#[test]
fn nvvm_annotations_become_function_attributes() {
    let src = r#"
define i32 @test_align(i32 %a, i32 %b) {
  ret i32 0
}

define void @test_kernel() {
  ret void
}

define void @test_maxclusterrank() {
  ret void
}

define void @test_cluster_max_blocks() {
  ret void
}

define void @test_minctasm() {
  ret void
}

define void @test_maxnreg() {
  ret void
}

define void @test_maxntid_1() {
  ret void
}

define void @test_maxntid_2() {
  ret void
}

define void @test_maxntid_3() {
  ret void
}

define void @test_maxntid_4() {
  ret void
}

define void @test_reqntid() {
  ret void
}

define void @test_cluster_dim() {
  ret void
}

define void @test_grid_constant(ptr byval(i32) %input1, i32 %input2, ptr byval(i32) %input3) {
  ret void
}

!nvvm.annotations = !{!0, !1, !2, !3, !4, !5, !6, !7, !8, !9, !10, !11, !12, !13}

!0 = !{ptr @test_align, !"align", i32 u0x00000008, !"align", i32 u0x00010008, !"align", i32 u0x00020010}
!1 = !{null, !"align", i32 u0x00000008, !"align", i32 u0x00010008, !"align", i32 u0x00020008}
!2 = !{ptr @test_kernel, !"kernel", i32 1}
!3 = !{ptr @test_maxclusterrank, !"maxclusterrank", i32 2}
!4 = !{ptr @test_cluster_max_blocks, !"cluster_max_blocks", i32 3}
!5 = !{ptr @test_minctasm, !"minctasm", i32 4}
!6 = !{ptr @test_maxnreg, !"maxnreg", i32 5}
!7 = !{ptr @test_maxntid_1, !"maxntidx", i32 50}
!8 = !{ptr @test_maxntid_2, !"maxntidx", i32 11, !"maxntidy", i32 22, !"maxntidz", i32 33}
!9 = !{ptr @test_maxntid_3, !"maxntidz", i32 11, !"maxntidy", i32 22, !"maxntidx", i32 33}
!10 = !{ptr @test_maxntid_4, !"maxntidz", i32 100}
!11 = !{ptr @test_reqntid, !"reqntidx", i32 31, !"reqntidy", i32 32, !"reqntidz", i32 33}
!12 = !{ptr @test_cluster_dim, !"cluster_dim_x", i32 101, !"cluster_dim_y", i32 102, !"cluster_dim_z", i32 103}
!13 = !{ptr @test_grid_constant, !"grid_constant", !14}
!14 = !{i32 1, i32 3}
"#;
    let text = parse_to_text(src);

    // `; CHECK-LABEL: define alignstack(8) i32 @test_align(`
    // `; CHECK-SAME: i32 alignstack(8) [[A:%.*]], i32 alignstack(16) [[B:%.*]]) {`
    assert_contains(
        &text,
        "define alignstack(8) i32 @test_align(i32 alignstack(8) %a, i32 alignstack(16) %b)",
    );
    // `; CHECK-LABEL: define ptx_kernel void @test_kernel() {`
    assert_contains(&text, "define ptx_kernel void @test_kernel()");
    // `; CHECK: attributes #[[ATTR0]] = { "nvvm.maxclusterrank"="2" }`
    assert_contains(
        &text,
        "define void @test_maxclusterrank() \"nvvm.maxclusterrank\"=\"2\"",
    );
    // `; CHECK: attributes #[[ATTR1]] = { "nvvm.maxclusterrank"="3" }`
    assert_contains(
        &text,
        "define void @test_cluster_max_blocks() \"nvvm.maxclusterrank\"=\"3\"",
    );
    // `; CHECK: attributes #[[ATTR2]] = { "nvvm.minctasm"="4" }`
    assert_contains(
        &text,
        "define void @test_minctasm() \"nvvm.minctasm\"=\"4\"",
    );
    // `; CHECK: attributes #[[ATTR3]] = { "nvvm.maxnreg"="5" }`
    assert_contains(&text, "define void @test_maxnreg() \"nvvm.maxnreg\"=\"5\"");
    // `; CHECK: attributes #[[ATTR4]] = { "nvvm.maxntid"="50" }`
    assert_contains(
        &text,
        "define void @test_maxntid_1() \"nvvm.maxntid\"=\"50\"",
    );
    // `; CHECK: attributes #[[ATTR5]] = { "nvvm.maxntid"="11,22,33" }`
    assert_contains(
        &text,
        "define void @test_maxntid_2() \"nvvm.maxntid\"=\"11,22,33\"",
    );
    // `; CHECK: attributes #[[ATTR6]] = { "nvvm.maxntid"="33,22,11" }`
    assert_contains(
        &text,
        "define void @test_maxntid_3() \"nvvm.maxntid\"=\"33,22,11\"",
    );
    // `; CHECK: attributes #[[ATTR7]] = { "nvvm.maxntid"="1,1,100" }`
    assert_contains(
        &text,
        "define void @test_maxntid_4() \"nvvm.maxntid\"=\"1,1,100\"",
    );
    // `; CHECK: attributes #[[ATTR8]] = { "nvvm.reqntid"="31,32,33" }`
    assert_contains(
        &text,
        "define void @test_reqntid() \"nvvm.reqntid\"=\"31,32,33\"",
    );
    // `; CHECK: attributes #[[ATTR9]] = { "nvvm.cluster_dim"="101,102,103" }`
    assert_contains(
        &text,
        "define void @test_cluster_dim() \"nvvm.cluster_dim\"=\"101,102,103\"",
    );
    // `; CHECK-SAME: ptr byval(i32) "nvvm.grid_constant" [[INPUT1:%.*]], i32 [[INPUT2:%.*]],`
    // `; CHECK-SAME: ptr byval(i32) "nvvm.grid_constant" [[INPUT3:%.*]]) {`
    assert_contains(
        &text,
        "define void @test_grid_constant(ptr byval(i32) \"nvvm.grid_constant\" %input1, i32 %input2, ptr byval(i32) \"nvvm.grid_constant\" %input3)",
    );
    // Every entry was consumed, so the named node survives with no operands —
    // the `NamedMD->clearOperands()` half of the routine. `!1`'s `null` global
    // took the `if (!GV) continue` arm and was dropped without being rebuilt.
    assert_contains(&text, "!nvvm.annotations = !{}");
}

/// **No upstream counterpart**, deliberately: every input below reaches an
/// `assert` or a bare `cast<>` inside `llvm::UpgradeNVVMAnnotations` /
/// `upgradeSingleNVVMAnnotation` / `upgradeNVVMFnVectorAttr`, so upstream has
/// no defined answer to port and no fixture that could pin one. llvmkit may
/// not panic in a production path, and the choice made at every one of those
/// sites is *upgrade nothing and keep the entry*; this is what holds that
/// choice in place.
///
/// The sites, in the order the routine reaches them: `cast<MDNode>` on a
/// named-node operand (`!5`, a specialized node — the one non-tuple an
/// `!nvvm.annotations` operand can actually be, since `parseNamedMetadata`
/// only accepts `!N` ids), `MD->getOperand(0)` on an empty node (`!1`),
/// `cast<Function>(GV)` (`!0`), the `(MD->getNumOperands() % 2) == 1`
/// assertion (`!2`), `cast<MDString>(MD->getOperand(j))` on a key that is not
/// a string (`!7`), `mdconst::extract<ConstantInt>` on a non-integer value
/// (`!3` scalar, `!4` through `upgradeNVVMFnVectorAttr` — two different
/// routes to the same `extract`), and `Align(V)`'s
/// `assert(V > 0 && isPowerOf2_64(V))` (`!6`).
#[test]
fn malformed_nvvm_annotations_are_preserved_rather_than_upgraded() {
    let src = r#"
@global = global i32 0

define void @f() {
  ret void
}

!nvvm.annotations = !{!0, !1, !2, !3, !4, !5, !6, !7}

!0 = !{ptr @global, !"kernel", i32 1}
!1 = !{}
!2 = !{ptr @f, !"maxnreg"}
!3 = !{ptr @f, !"maxnreg", !"not an integer"}
!4 = !{ptr @f, !"maxntidx", !"not an integer"}
!5 = !DIBasicType(name: "int")
!6 = !{ptr @f, !"align", i32 u0x00000003}
!7 = !{ptr @f, i32 99, i32 1}
"#;
    let text = parse_to_text(src);

    // `!1` and `!5` take the two `continue` arms and leave the list; `!2`'s
    // unpaired key is dropped by the `j + 1 < je` bound, which empties its
    // rebuilt node down to the global alone and so fails `size() > 1`. The
    // rest are rebuilt verbatim, and print under fresh numbers because llvmkit
    // does not unique tuples (`docs/divergences.md` entry 99).
    assert_contains(&text, "!nvvm.annotations = !{!8, !9, !10, !11, !12}");
    assert_contains(&text, "!8 = !{ptr @global, !\"kernel\", i32 1}");
    assert_contains(&text, "!9 = !{ptr @f, !\"maxnreg\", !\"not an integer\"}");
    assert_contains(&text, "!10 = !{ptr @f, !\"maxntidx\", !\"not an integer\"}");
    assert_contains(&text, "!11 = !{ptr @f, !\"align\", i32 3}");
    assert_contains(&text, "!12 = !{ptr @f, i32 99, i32 1}");
    // Nothing was written to either global.
    assert!(
        !text.contains("ptx_kernel"),
        "an annotation on a global variable upgraded a calling convention:\n{text}"
    );
    assert!(
        !text.contains("nvvm.maxnreg"),
        "a non-integer value produced an attribute:\n{text}"
    );
    assert!(
        !text.contains("nvvm.maxntid"),
        "a non-integer value produced a vector attribute:\n{text}"
    );
    assert!(
        !text.contains("alignstack"),
        "a non-power-of-two alignment produced an attribute:\n{text}"
    );
}

/// **No upstream counterpart** in fixture form: the
/// `SmallPtrSet<const MDNode *, 8> SeenNodes` guard of
/// `llvm::UpgradeNVVMAnnotations` is unpinned upstream. It is observable here
/// because a *repeated* entry that upgrades nothing would otherwise be rebuilt
/// once per occurrence — two operands, not one.
///
/// The surviving operand is `!1`, not `!0`, for the reason above: upstream's
/// `MDNode::get` would hand back `!0` itself (`docs/divergences.md` entry 99).
#[test]
fn a_repeated_nvvm_annotation_entry_is_visited_once() {
    let src = r#"
define void @f() {
  ret void
}

!nvvm.annotations = !{!0, !0}

!0 = !{ptr @f, !"not a known key", i32 1}
"#;
    let text = parse_to_text(src);
    assert_contains(&text, "!nvvm.annotations = !{!1}");
    assert_contains(&text, "!1 = !{ptr @f, !\"not a known key\", i32 1}");
}
