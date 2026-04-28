//! Top-level globals + comdats round-trip / format tests.
//!
//! ## Upstream provenance
//!
//! Every fixture below is anchored on `test/Bitcode/compatibility.ll`
//! (the canonical IR-backwards-compat suite). Each test cites the
//! relevant `; CHECK:` line range. Aggregate-constant tests cite
//! `unittests/IR/ConstantsTest.cpp`. Verifier negatives cite
//! `Verifier::visitGlobalVariable` in `lib/IR/Verifier.cpp`.

use llvmkit_ir::comdat::SelectionKind;
use llvmkit_ir::global_value::{DllStorageClass, ThreadLocalMode, Visibility};
use llvmkit_ir::{Align, IrError, Linkage, MaybeAlign, Module, UnnamedAddr, VerifierRule};

fn module_text(m: &Module<'_>) -> String {
    format!("{m}")
}

// ---------------------------------------------------------------------------
// Simple globals
// ---------------------------------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 88-89:
/// `@g1 = global i32 0` -- a default-linkage `global` with i32 zero
/// initializer.
#[test]
fn simple_global_i32_zero() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.add_global("g1", i32_ty.as_type(), zero).expect("add");
    assert!(
        module_text(&m).contains("@g1 = global i32 0\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 90-91:
/// `@g2 = constant i32 0` -- a `constant` (immutable) global.
#[test]
fn simple_global_constant_i32_zero() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.add_global_constant("g2", i32_ty.as_type(), zero)
        .expect("add");
    assert!(
        module_text(&m).contains("@g2 = constant i32 0\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 114-115:
/// `@g.external = external global i32` -- a declaration-only global
/// with explicit `external` keyword.
#[test]
fn external_declaration_global() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    m.add_external_global("g.external", i32_ty.as_type())
        .expect("add");
    assert!(
        module_text(&m).contains("@g.external = external global i32\n"),
        "got:\n{}",
        module_text(&m)
    );
}

// ---------------------------------------------------------------------------
// Linkage
// ---------------------------------------------------------------------------

fn linkage_text(linkage: Linkage) -> String {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g", i32_ty.as_type())
        .linkage(linkage)
        .initializer(zero)
        .build()
        .expect("build");
    module_text(&m)
}

/// Mirrors `test/Bitcode/compatibility.ll` line 94-95.
#[test]
fn linkage_private() {
    assert!(
        linkage_text(Linkage::Private).contains("@g = private global i32 0\n"),
        "got:\n{}",
        linkage_text(Linkage::Private)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 96-97.
#[test]
fn linkage_internal() {
    assert!(linkage_text(Linkage::Internal).contains("@g = internal global i32 0\n"));
}

/// Mirrors `test/Bitcode/compatibility.ll` line 98-99.
#[test]
fn linkage_available_externally() {
    assert!(
        linkage_text(Linkage::AvailableExternally)
            .contains("@g = available_externally global i32 0\n")
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 100-101.
#[test]
fn linkage_linkonce() {
    assert!(linkage_text(Linkage::LinkOnceAny).contains("@g = linkonce global i32 0\n"));
}

/// Mirrors `test/Bitcode/compatibility.ll` line 110-111.
#[test]
fn linkage_linkonce_odr() {
    assert!(linkage_text(Linkage::LinkOnceODR).contains("@g = linkonce_odr global i32 0\n"));
}

/// Mirrors `test/Bitcode/compatibility.ll` line 102-103.
#[test]
fn linkage_weak() {
    assert!(linkage_text(Linkage::WeakAny).contains("@g = weak global i32 0\n"));
}

/// Mirrors `test/Bitcode/compatibility.ll` line 112-113.
#[test]
fn linkage_weak_odr() {
    assert!(linkage_text(Linkage::WeakODR).contains("@g = weak_odr global i32 0\n"));
}

/// Mirrors `test/Bitcode/compatibility.ll` line 108-109:
/// `@g.extern_weak = extern_weak global i32` -- declaration-only.
#[test]
fn linkage_extern_weak_declaration() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    m.global_builder("g.extern_weak", i32_ty.as_type())
        .linkage(Linkage::ExternalWeak)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.extern_weak = extern_weak global i32\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 104-105:
/// `@g.common = common global i32 0` -- common linkage requires a
/// zero initializer.
#[test]
fn linkage_common_zero_init() {
    assert!(
        linkage_text(Linkage::Common).contains("@g = common global i32 0\n"),
        "got:\n{}",
        linkage_text(Linkage::Common)
    );
}

// ---------------------------------------------------------------------------
// Visibility
// ---------------------------------------------------------------------------

fn visibility_text(vis: Visibility) -> String {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g", i32_ty.as_type())
        .visibility(vis)
        .initializer(zero)
        .build()
        .expect("build");
    module_text(&m)
}

/// Mirrors `test/Bitcode/compatibility.ll` line 120-121.
#[test]
fn visibility_hidden() {
    assert!(
        visibility_text(Visibility::Hidden).contains("@g = hidden global i32 0\n"),
        "got:\n{}",
        visibility_text(Visibility::Hidden)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 122-123.
#[test]
fn visibility_protected() {
    assert!(visibility_text(Visibility::Protected).contains("@g = protected global i32 0\n"));
}

// ---------------------------------------------------------------------------
// DLL storage class
// ---------------------------------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 130-131.
#[test]
fn dll_export() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g.dllexport", i32_ty.as_type())
        .dll_storage_class(DllStorageClass::DllExport)
        .initializer(zero)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.dllexport = dllexport global i32 0\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 128-129:
/// `@g.dllimport = external dllimport global i32`.
#[test]
fn dll_import_declaration() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    m.global_builder("g.dllimport", i32_ty.as_type())
        .dll_storage_class(DllStorageClass::DllImport)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.dllimport = external dllimport global i32\n"),
        "got:\n{}",
        module_text(&m)
    );
}

// ---------------------------------------------------------------------------
// Thread-local
// ---------------------------------------------------------------------------

fn tls_text(mode: ThreadLocalMode) -> String {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g", i32_ty.as_type())
        .thread_local_mode(mode)
        .initializer(zero)
        .build()
        .expect("build");
    module_text(&m)
}

/// Mirrors `test/Bitcode/compatibility.ll` line 136-137:
/// `@g.generaldynamic = thread_local global i32 0`.
#[test]
fn tls_general_dynamic() {
    assert!(tls_text(ThreadLocalMode::GeneralDynamic).contains("@g = thread_local global i32 0\n"));
}

/// Mirrors `test/Bitcode/compatibility.ll` line 138-139:
/// `@g.localdynamic = thread_local(localdynamic) global i32 0`.
#[test]
fn tls_local_dynamic() {
    assert!(
        tls_text(ThreadLocalMode::LocalDynamic)
            .contains("@g = thread_local(localdynamic) global i32 0\n")
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 140-141:
/// `@g.initialexec = thread_local(initialexec) global i32 0`.
#[test]
fn tls_initial_exec() {
    assert!(
        tls_text(ThreadLocalMode::InitialExec)
            .contains("@g = thread_local(initialexec) global i32 0\n")
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 142-143:
/// `@g.localexec = thread_local(localexec) global i32 0`.
#[test]
fn tls_local_exec() {
    assert!(
        tls_text(ThreadLocalMode::LocalExec)
            .contains("@g = thread_local(localexec) global i32 0\n")
    );
}

// ---------------------------------------------------------------------------
// unnamed_addr
// ---------------------------------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 146-147:
/// `@g.unnamed_addr = unnamed_addr global i32 0`.
#[test]
fn unnamed_addr_global() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g.unnamed_addr", i32_ty.as_type())
        .unnamed_addr(UnnamedAddr::Global)
        .initializer(zero)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.unnamed_addr = unnamed_addr global i32 0\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 148-149:
/// `@g.local_unnamed_addr = local_unnamed_addr global i32 0`.
#[test]
fn unnamed_addr_local() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g.local_unnamed_addr", i32_ty.as_type())
        .unnamed_addr(UnnamedAddr::Local)
        .initializer(zero)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.local_unnamed_addr = local_unnamed_addr global i32 0\n"),
        "got:\n{}",
        module_text(&m)
    );
}

// ---------------------------------------------------------------------------
// Address space
// ---------------------------------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 152-153:
/// `@g.addrspace = addrspace(1) global i32 0`.
#[test]
fn address_space_one() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g.addrspace", i32_ty.as_type())
        .address_space(1)
        .initializer(zero)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.addrspace = addrspace(1) global i32 0\n"),
        "got:\n{}",
        module_text(&m)
    );
}

// ---------------------------------------------------------------------------
// externally_initialized
// ---------------------------------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 156-157:
/// `@g.externally_initialized = external externally_initialized global i32`.
#[test]
fn externally_initialized_declaration() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    m.global_builder("g.externally_initialized", i32_ty.as_type())
        .externally_initialized(true)
        .build()
        .expect("build");
    assert!(
        module_text(&m)
            .contains("@g.externally_initialized = external externally_initialized global i32\n"),
        "got:\n{}",
        module_text(&m)
    );
}

// ---------------------------------------------------------------------------
// section + partition + align
// ---------------------------------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 160-161:
/// `@g.section = global i32 0, section "_DATA"`.
#[test]
fn section_attribute() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g.section", i32_ty.as_type())
        .section("_DATA")
        .initializer(zero)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.section = global i32 0, section \"_DATA\"\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 164-165:
/// `@g.partition = global i32 0, partition "part"`.
#[test]
fn partition_attribute() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g.partition", i32_ty.as_type())
        .partition("part")
        .initializer(zero)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.partition = global i32 0, partition \"part\"\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 188-189:
/// `@g.align = global i32 0, align 4`.
#[test]
fn align_attribute() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("g.align", i32_ty.as_type())
        .align(MaybeAlign::from(Align::new(4).expect("align")))
        .initializer(zero)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@g.align = global i32 0, align 4\n"),
        "got:\n{}",
        module_text(&m)
    );
}

// ---------------------------------------------------------------------------
// Comdat
// ---------------------------------------------------------------------------

/// Mirrors `test/Bitcode/compatibility.ll` line 22-23:
/// `$comdat.any = comdat any`.
#[test]
fn comdat_any_emission() {
    let m = Module::new("m");
    m.get_or_insert_comdat("comdat.any");
    assert!(
        module_text(&m).contains("$comdat.any = comdat any\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 24-25 / 26-27 / 28-29 / 30-31:
/// every selection kind round-trips.
#[test]
fn comdat_all_selection_kinds() {
    let m = Module::new("m");
    m.get_or_insert_comdat("comdat.any");
    m.get_or_insert_comdat("comdat.exactmatch")
        .set_selection_kind(SelectionKind::ExactMatch);
    m.get_or_insert_comdat("comdat.largest")
        .set_selection_kind(SelectionKind::Largest);
    m.get_or_insert_comdat("comdat.noduplicates")
        .set_selection_kind(SelectionKind::NoDeduplicate);
    m.get_or_insert_comdat("comdat.samesize")
        .set_selection_kind(SelectionKind::SameSize);
    let text = module_text(&m);
    assert!(text.contains("$comdat.any = comdat any\n"), "got:\n{text}");
    assert!(
        text.contains("$comdat.exactmatch = comdat exactmatch\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("$comdat.largest = comdat largest\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("$comdat.noduplicates = comdat nodeduplicate\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("$comdat.samesize = comdat samesize\n"),
        "got:\n{text}"
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 168-169:
/// `@comdat.any = global i32 0, comdat` -- comdat name implicit
/// (matches the global's name).
#[test]
fn comdat_attached_implicit_name() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let c = m.get_or_insert_comdat("comdat.any");
    m.global_builder("comdat.any", i32_ty.as_type())
        .initializer(zero)
        .comdat(c)
        .build()
        .expect("build");
    assert!(
        module_text(&m).contains("@comdat.any = global i32 0, comdat\n"),
        "got:\n{}",
        module_text(&m)
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 182-185:
/// `@g.comdat1 = global i32 0, section "SharedSection", comdat($comdat1)`
/// -- comdat name explicit (differs from the global's name).
#[test]
fn comdat_attached_explicit_name_with_section() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let c = m.get_or_insert_comdat("comdat1");
    m.global_builder("g.comdat1", i32_ty.as_type())
        .initializer(zero)
        .section("SharedSection")
        .comdat(c)
        .build()
        .expect("build");
    assert!(
        module_text(&m)
            .contains("@g.comdat1 = global i32 0, section \"SharedSection\", comdat($comdat1)\n"),
        "got:\n{}",
        module_text(&m)
    );
}

// ---------------------------------------------------------------------------
// Aggregate constants in initializers (mirrors compatibility.ll constants
// section, lines 33-79)
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, AsInstructionsTest)`
/// (struct-constant construction) and
/// `test/Bitcode/compatibility.ll` line 47:
/// `@const.struct = constant %const.struct.type { i32 -1, i8 undef, i64 poison }`.
#[test]
fn const_struct_initializer() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let i8_ty = m.i8_type();
    let i64_ty = m.i64_type();
    let st = m.struct_type([i32_ty.as_type(), i8_ty.as_type(), i64_ty.as_type()], false);
    let neg_one = i32_ty.const_int(-1i32);
    let undef_i8 = i8_ty.as_type().get_undef();
    let poison_i64 = i64_ty.as_type().get_poison();
    let s = st
        .const_struct::<llvmkit_ir::Constant<'_>, _>([
            neg_one.as_constant(),
            undef_i8.into(),
            poison_i64.into(),
        ])
        .expect("struct");
    m.add_global_constant("c", st.as_type(), s).expect("add");
    let text = module_text(&m);
    assert!(
        text.contains("@c = constant { i32, i8, i64 } { i32 -1, i8 undef, i64 poison }\n"),
        "got:\n{text}"
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 53-58: `[3 x i16]` /
/// `[3 x i32]` / `[3 x i64]` -- non-i8 array elements print element-wise.
#[test]
fn const_array_i32_initializer() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let arr = m.array_type(i32_ty.as_type(), 3);
    let zero = i32_ty.const_int(0i32);
    let one = i32_ty.const_int(1i32);
    let a = arr
        .const_array::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([zero, one, zero])
        .expect("array");
    m.add_global_constant("constant.array.i32", arr.as_type(), a)
        .expect("add");
    let text = module_text(&m);
    assert!(
        text.contains("@constant.array.i32 = constant [3 x i32] [i32 0, i32 1, i32 0]\n"),
        "got:\n{text}"
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 51:
/// `[3 x i8] c"\00\01\00"` -- i8 arrays print as c-strings via
/// `ConstantDataArray::isString` in `lib/IR/AsmWriter.cpp`.
#[test]
fn const_array_i8_prints_as_cstring() {
    let m = Module::new("m");
    let i8_ty = m.i8_type();
    let arr = m.array_type(i8_ty.as_type(), 3);
    let zero = i8_ty.const_int(0i8);
    let one = i8_ty.const_int(1i8);
    let a = arr
        .const_array::<llvmkit_ir::ConstantIntValue<'_, i8>, _>([zero, one, zero])
        .expect("array");
    m.add_global_constant("constant.array.i8", arr.as_type(), a)
        .expect("add");
    let text = module_text(&m);
    assert!(
        text.contains("@constant.array.i8 = constant [3 x i8] c\"\\00\\01\\00\"\n"),
        "got:\n{text}"
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 106-107:
/// `@g.appending = appending global [4 x i8] c"test"` -- appending
/// linkage with an i8-array initializer using the c-string form.
#[test]
fn appending_global_cstring() {
    let m = Module::new("m");
    let i8_ty = m.i8_type();
    let arr = m.array_type(i8_ty.as_type(), 4);
    let bytes: [llvmkit_ir::ConstantIntValue<'_, i8>; 4] = [
        i8_ty.const_int(b't' as i8),
        i8_ty.const_int(b'e' as i8),
        i8_ty.const_int(b's' as i8),
        i8_ty.const_int(b't' as i8),
    ];
    let a = arr
        .const_array::<llvmkit_ir::ConstantIntValue<'_, i8>, _>(bytes)
        .expect("array");
    m.global_builder("g.appending", arr.as_type())
        .linkage(Linkage::Appending)
        .initializer(a)
        .build()
        .expect("build");
    let text = module_text(&m);
    assert!(
        text.contains("@g.appending = appending global [4 x i8] c\"test\"\n"),
        "got:\n{text}"
    );
}

/// Mirrors `test/Bitcode/compatibility.ll` line 70-71: `<3 x i32>`
/// vector constant prints with angle-bracket syntax.
#[test]
fn const_vector_initializer() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let vec_ty = m.vector_type(i32_ty.as_type(), 3, false);
    let zero = i32_ty.const_int(0i32);
    let one = i32_ty.const_int(1i32);
    let v = vec_ty
        .const_vector::<llvmkit_ir::ConstantIntValue<'_, i32>, _>([zero, one, zero])
        .expect("vector");
    m.add_global_constant("constant.vector.i32", vec_ty.as_type(), v)
        .expect("add");
    let text = module_text(&m);
    assert!(
        text.contains("@constant.vector.i32 = constant <3 x i32> <i32 0, i32 1, i32 0>\n"),
        "got:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Verifier negatives
// ---------------------------------------------------------------------------

/// Mirrors `Verifier::visitGlobalVariable` -- the
/// "Global variable initializer type does not match global variable
/// type!" check. Construction-time checks fire before the verifier;
/// this test exercises the construction-time error path.
#[test]
fn initializer_type_mismatch_rejected_at_construction() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let zero64 = i64_ty.const_int(0i64);
    let err = m
        .add_global("g", i32_ty.as_type(), zero64)
        .expect_err("expected mismatch");
    assert!(matches!(err, IrError::TypeMismatch { .. }), "got: {err:?}");
}

/// Mirrors `Verifier::visitGlobalVariable` -- the `hasCommonLinkage`
/// arm: a common-linkage global with a non-zero initializer is
/// invalid.
#[test]
fn common_linkage_nonzero_initializer_rejected() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let one = i32_ty.const_int(1i32);
    m.global_builder("c", i32_ty.as_type())
        .linkage(Linkage::Common)
        .initializer(one)
        .build()
        .expect("build accepts at construction time");
    let err = m.verify_borrowed().expect_err("verifier rejects");
    assert!(
        matches!(
            err,
            IrError::VerifierFailure {
                rule: VerifierRule::CommonLinkageInvariantViolated,
                ..
            }
        ),
        "got: {err:?}"
    );
}

/// Mirrors `Verifier::visitGlobalVariable` -- the `hasCommonLinkage`
/// arm: a common-linkage `constant` is invalid.
#[test]
fn common_linkage_constant_rejected() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.global_builder("c", i32_ty.as_type())
        .linkage(Linkage::Common)
        .constant(true)
        .initializer(zero)
        .build()
        .expect("build");
    let err = m.verify_borrowed().expect_err("verifier rejects");
    assert!(
        matches!(
            err,
            IrError::VerifierFailure {
                rule: VerifierRule::CommonLinkageInvariantViolated,
                ..
            }
        ),
        "got: {err:?}"
    );
}

/// Mirrors `Verifier::visitGlobalVariable` -- the
/// "Globals cannot contain scalable types" check.
#[test]
fn scalable_vector_global_rejected() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let scalable = m.vector_type(i32_ty.as_type(), 4, true);
    m.global_builder("s", scalable.as_type())
        .build()
        .expect("build");
    let err = m.verify_borrowed().expect_err("verifier rejects");
    assert!(
        matches!(
            err,
            IrError::VerifierFailure {
                rule: VerifierRule::GlobalScalableType,
                ..
            }
        ),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Module-level lookup / iteration
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/ModuleTest.cpp::TEST(ModuleTest, GlobalList)`
/// (the `M->getNamedValue("GV")` round-trip and the
/// `M->global_size()` increment-after-insert pattern). Our equivalent
/// API is `Module::get_global(name)`.
#[test]
fn module_named_global_lookup_round_trip() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let g = m.add_global("foo", i32_ty.as_type(), zero).expect("add");
    let looked_up = m.get_global("foo").expect("found");
    assert_eq!(g, looked_up);
    assert!(m.get_global("missing").is_none());
}

/// Mirrors `unittests/IR/ModuleTest.cpp::TEST(ModuleTest, GlobalList)`
/// (the `Range.begin()` / `std::next(Range.begin())` walk that
/// asserts globals iterate in declaration order).
#[test]
fn module_iter_globals_preserves_order() {
    let m = Module::new("m");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    m.add_global("a", i32_ty.as_type(), zero).expect("a");
    m.add_global("b", i32_ty.as_type(), zero).expect("b");
    m.add_global("c", i32_ty.as_type(), zero).expect("c");
    let names: Vec<&str> = m.iter_globals().map(|g| g.name()).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
}

/// Mirrors `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, ComdatUserTracking)`
/// -- the second `M.getOrInsertComdat("comdat")` call returns the
/// same `Comdat *` (the test then attaches it to a global without
/// observing duplicates).
#[test]
fn comdat_get_or_insert_is_idempotent() {
    let m = Module::new("m");
    let a = m.get_or_insert_comdat("c1");
    let b = m.get_or_insert_comdat("c1");
    assert_eq!(a.id(), b.id());
    let c = m.get_or_insert_comdat("c2");
    assert_ne!(a.id(), c.id());
}
