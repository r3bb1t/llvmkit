//! Verifier rules for `!llvm.module.flags` — the port of
//! `Verifier::visitModuleFlags` / `visitModuleFlag` /
//! `visitModuleFlagCGProfileEntry` (`llvm/lib/IR/Verifier.cpp`).
//!
//! Each negative test ports one numbered metadata line of an upstream
//! `llvm/test/Verifier/module-flags-*.ll` fixture: the flag tuple is built
//! programmatically with the same operands the fixture spells, and the
//! asserted message is the fixture's `CHECK:` line. They live here rather
//! than in the parser corpus because upstream runs these fixtures through
//! `not llvm-as`, i.e. the check under test is the verifier's, not the
//! parser's.

use llvmkit_ir::{
    IrError, Linkage, MetadataId, MetadataKind, Module, ModuleBrand, NamedMetadataName,
    VerifierRule, module_new,
};

/// Append one pre-built flag tuple to `!llvm.module.flags`.
fn add_flag<B: ModuleBrand>(m: &Module<B>, operands: &[MetadataId<B>]) -> Result<(), IrError> {
    let tuple = m.metadata_tuple(operands)?;
    let flags = m.get_or_insert_named_metadata(NamedMetadataName::ModuleFlags);
    m.named_metadata_add_operand(flags, tuple)
}

/// `i32 <v>` as a metadata operand.
fn int32<B: ModuleBrand>(m: &Module<B>, v: i32) -> Result<MetadataId<B>, IrError> {
    m.metadata_constant(m.i32_type().const_int(v))
}

fn assert_flag_error(err: &IrError, rule: VerifierRule, fragment: &str) {
    match err {
        IrError::VerifierFailure {
            rule: actual,
            message,
            ..
        } => {
            assert_eq!(*actual, rule, "{err:?}");
            assert!(
                message.contains(fragment),
                "message {message:?} lacks {fragment:?}"
            );
        }
        other => panic!("expected a VerifierFailure, got {other:?}"),
    }
}

/// Port of `llvm/test/Verifier/module-flags-1.ll` `!0 = !{i32 1}` —
/// `CHECK: incorrect number of operands in module flag`.
#[test]
fn incorrect_number_of_operands_in_module_flag() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    add_flag(&m, &[one])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidOperandCount,
        "incorrect number of operands in module flag",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!1 = !{!"foo", !"foo", i32 42}` —
/// `CHECK: invalid behavior operand in module flag (expected constant
/// integer)`.
#[test]
fn behavior_operand_must_be_a_constant_integer() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let foo = m.metadata_string("foo");
    let forty_two = int32(&m, 42)?;
    add_flag(&m, &[foo, foo, forty_two])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidBehavior,
        "invalid behavior operand in module flag (expected constant integer)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!2 = !{i32 999, !"foo", i32 43}` —
/// `CHECK: invalid behavior operand in module flag (unexpected constant)`.
#[test]
fn behavior_operand_constant_out_of_range() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let bad = int32(&m, 999)?;
    let foo = m.metadata_string("foo");
    let forty_three = int32(&m, 43)?;
    add_flag(&m, &[bad, foo, forty_three])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidBehavior,
        "invalid behavior operand in module flag (unexpected constant)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!3 = !{i32 1, i32 1, i32 44}` —
/// `CHECK: invalid ID operand in module flag (expected metadata string)`.
#[test]
fn id_operand_must_be_a_metadata_string() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let forty_four = int32(&m, 44)?;
    add_flag(&m, &[one, one, forty_four])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidId,
        "invalid ID operand in module flag (expected metadata string)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!4 = !{i32 3, !"bla", i32 45}` —
/// `CHECK: invalid value for 'require' module flag (expected metadata
/// pair)`.
#[test]
fn require_value_must_be_a_metadata_pair() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let three = int32(&m, 3)?;
    let bla = m.metadata_string("bla");
    let forty_five = int32(&m, 45)?;
    add_flag(&m, &[three, bla, forty_five])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "invalid value for 'require' module flag (expected metadata pair)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!5 = !{i32 3, !"bla", !{i32 46}}` —
/// `CHECK: invalid value for 'require' module flag (expected metadata
/// pair)` (a node, but not a two-element pair).
#[test]
fn require_value_pair_must_have_two_operands() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let three = int32(&m, 3)?;
    let bla = m.metadata_string("bla");
    let forty_six = int32(&m, 46)?;
    let not_a_pair = m.metadata_tuple([forty_six])?;
    add_flag(&m, &[three, bla, not_a_pair])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "invalid value for 'require' module flag (expected metadata pair)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!6 = !{i32 3, !"bla", !{i32 47, i32 48}}` —
/// `CHECK: invalid value for 'require' module flag (first value operand
/// should be a string)`.
#[test]
fn require_pair_first_operand_must_be_a_string() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let three = int32(&m, 3)?;
    let bla = m.metadata_string("bla");
    let forty_seven = int32(&m, 47)?;
    let forty_eight = int32(&m, 48)?;
    let pair = m.metadata_tuple([forty_seven, forty_eight])?;
    add_flag(&m, &[three, bla, pair])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "invalid value for 'require' module flag (first value operand should be a string)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!7 = !{i32 1, !"foo", i32 49}` /
/// `!8 = !{i32 2, !"foo", i32 50}` — `CHECK: module flag identifiers must
/// be unique (or of 'require' type)`.
#[test]
fn flag_identifiers_must_be_unique() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let foo = m.metadata_string("foo");
    let one = int32(&m, 1)?;
    let two = int32(&m, 2)?;
    let forty_nine = int32(&m, 49)?;
    let fifty = int32(&m, 50)?;
    add_flag(&m, &[one, foo, forty_nine])?;
    add_flag(&m, &[two, foo, fifty])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagDuplicateId,
        "module flag identifiers must be unique (or of 'require' type)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!9 = !{i32 2, !"bar", i32 51}` /
/// `!10 = !{i32 3, !"bar", !{!"bar", i32 51}}` — the `CHECK-NOT` half: a
/// `require` flag may share its ID with the flag it restricts, and a
/// requirement whose value matches verifies.
#[test]
fn distinct_ids_and_a_satisfied_requirement_verify() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let bar = m.metadata_string("bar");
    let two = int32(&m, 2)?;
    let three = int32(&m, 3)?;
    let fifty_one = int32(&m, 51)?;
    add_flag(&m, &[two, bar, fifty_one])?;
    let requirement = m.metadata_tuple([bar, fifty_one])?;
    add_flag(&m, &[three, bar, requirement])?;
    m.verify_borrowed()?;
    Ok(())
}

/// Port of `module-flags-1.ll` `!16 = !{i32 5, !"flag-2", i32 56}` (and its
/// twin `!17`) — `CHECK: invalid value for 'append'-type module flag
/// (expected a metadata node)`.
#[test]
fn append_value_must_be_a_metadata_node() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let five = int32(&m, 5)?;
    let name = m.metadata_string("flag-2");
    let fifty_six = int32(&m, 56)?;
    add_flag(&m, &[five, name, fifty_six])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "invalid value for 'append'-type module flag (expected a metadata node)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!18 = !{i32 5, !"flag-4", !{i32 57}}` —
/// the `CHECK-NOT` half: an `append` flag with a node value verifies.
#[test]
fn append_with_a_node_value_verifies() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let five = int32(&m, 5)?;
    let name = m.metadata_string("flag-4");
    let fifty_seven = int32(&m, 57)?;
    let node = m.metadata_tuple([fifty_seven])?;
    add_flag(&m, &[five, name, node])?;
    m.verify_borrowed()?;
    Ok(())
}

/// Port of `module-flags-1.ll` `!19 = !{i32 7, !"max", !"max"}` —
/// `CHECK: invalid value for 'max' module flag (expected constant
/// integer)`.
#[test]
fn max_value_must_be_a_constant_integer() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let seven = int32(&m, 7)?;
    let max = m.metadata_string("max");
    add_flag(&m, &[seven, max, max])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "invalid value for 'max' module flag (expected constant integer)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!20 = !{i32 8, !"min", !"min"}` —
/// `CHECK: invalid value for 'min' module flag (expected constant
/// non-negative integer)`.
#[test]
fn min_value_must_be_a_constant_integer() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let eight = int32(&m, 8)?;
    let min = m.metadata_string("min");
    add_flag(&m, &[eight, min, min])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "invalid value for 'min' module flag (expected constant non-negative integer)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!21 = !{i32 8, !"min", i32 -1}` —
/// `CHECK: invalid value for 'min' module flag (expected constant
/// non-negative integer)`.
#[test]
fn min_value_must_be_non_negative() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let eight = int32(&m, 8)?;
    let min = m.metadata_string("min");
    let minus_one = int32(&m, -1)?;
    add_flag(&m, &[eight, min, minus_one])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "invalid value for 'min' module flag (expected constant non-negative integer)",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!11 = !{i32 3, !"bar", !{!"no-such-flag",
/// i32 52}}` — `CHECK: invalid requirement on flag, flag is not present in
/// module`.
#[test]
fn requirement_on_an_absent_flag_is_invalid() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let three = int32(&m, 3)?;
    let bar = m.metadata_string("bar");
    let missing = m.metadata_string("no-such-flag");
    let fifty_two = int32(&m, 52)?;
    let requirement = m.metadata_tuple([missing, fifty_two])?;
    add_flag(&m, &[three, bar, requirement])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidRequirement,
        "invalid requirement on flag, flag is not present in module",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!12 = !{i32 1, !"flag-0", i32 53}` /
/// `!13 = !{i32 3, !"bar", !{!"flag-0", i32 54}}` — `CHECK: invalid
/// requirement on flag, flag does not have the required value`.
#[test]
fn requirement_with_a_different_value_is_invalid() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let three = int32(&m, 3)?;
    let flag_zero = m.metadata_string("flag-0");
    let bar = m.metadata_string("bar");
    let fifty_three = int32(&m, 53)?;
    let fifty_four = int32(&m, 54)?;
    add_flag(&m, &[one, flag_zero, fifty_three])?;
    let requirement = m.metadata_tuple([flag_zero, fifty_four])?;
    add_flag(&m, &[three, bar, requirement])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidRequirement,
        "invalid requirement on flag, flag does not have the required value",
    );
    Ok(())
}

/// Port of `module-flags-1.ll` `!14 = !{i32 1, !"flag-1", i32 55}` /
/// `!15 = !{i32 3, !"bar", !{!"flag-1", i32 55}}` — the `CHECK-NOT` half:
/// a requirement whose value matches the flag's verifies. Upstream's
/// comparison is uniqued-pointer identity; the two `i32 55` operands here
/// are built as separate constants so the port also covers llvmkit's
/// structural comparison standing in for uniquing.
#[test]
fn requirement_with_the_required_value_verifies() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let three = int32(&m, 3)?;
    let flag_one = m.metadata_string("flag-1");
    let bar = m.metadata_string("bar");
    let flag_value = int32(&m, 55)?;
    add_flag(&m, &[one, flag_one, flag_value])?;
    let required_value = int32(&m, 55)?;
    let requirement = m.metadata_tuple([flag_one, required_value])?;
    add_flag(&m, &[three, bar, requirement])?;
    m.verify_borrowed()?;
    Ok(())
}

/// Port of `llvm/test/Verifier/module-flags-2.ll`
/// (`!0 = !{null, null, null}`) — `CHECK: invalid behavior operand in
/// module flag (expected constant integer)`.
#[test]
fn null_behavior_operand_is_rejected() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let null = m.metadata_node(MetadataKind::Null)?;
    add_flag(&m, &[null, null, null])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidBehavior,
        "invalid behavior operand in module flag (expected constant integer)",
    );
    Ok(())
}

/// Port of `llvm/test/Verifier/module-flags-3.ll`
/// (`!0 = !{i32 1, null, null}`) — `CHECK: invalid ID operand in module
/// flag (expected metadata string)`.
#[test]
fn null_id_operand_is_rejected() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let null = m.metadata_node(MetadataKind::Null)?;
    add_flag(&m, &[one, null, null])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidId,
        "invalid ID operand in module flag (expected metadata string)",
    );
    Ok(())
}

/// Port of `llvm/test/Verifier/module-flags-semantic-interposition.ll`
/// (`!0 = !{i32 1, !"SemanticInterposition", float 1.}`) —
/// `CHECK: SemanticInterposition metadata requires constant integer
/// argument`.
#[test]
fn semantic_interposition_requires_a_constant_integer() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let key = m.metadata_string("SemanticInterposition");
    let float_one = m.metadata_constant(m.f32_type().const_float(1.0))?;
    add_flag(&m, &[one, key, float_one])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "SemanticInterposition metadata requires constant integer argument",
    );
    Ok(())
}

/// llvmkit-specific spelling of the same rule for `wchar_size` — the check
/// is `Verifier::visitModuleFlag`'s (`CHECK`-message
/// "wchar_size metadata requires constant integer argument"), but upstream
/// ships no dedicated `.ll` fixture for it; the non-integer value mirrors
/// the SemanticInterposition sibling fixture's `float 1.`.
#[test]
fn wchar_size_requires_a_constant_integer() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let key = m.metadata_string("wchar_size");
    let float_one = m.metadata_constant(m.f32_type().const_float(1.0))?;
    add_flag(&m, &[one, key, float_one])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagInvalidValue,
        "wchar_size metadata requires constant integer argument",
    );
    Ok(())
}

/// llvmkit-specific spelling of `Verifier::visitModuleFlag`'s
/// `"Linker Options"` arm ("'Linker Options' named metadata no longer
/// supported") — no dedicated upstream `.ll` fixture; the flag counts as
/// upgraded (and verifies) exactly when the `llvm.linker.options` named
/// metadata exists, per the upstream comment about the bitcode reader.
#[test]
fn linker_options_flag_requires_the_upgraded_named_metadata() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let six = int32(&m, 6)?;
    let key = m.metadata_string("Linker Options");
    let empty = m.metadata_tuple([])?;
    add_flag(&m, &[six, key, empty])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagLinkerOptionsUnsupported,
        "'Linker Options' named metadata no longer supported",
    );
    // The upgraded module — same flag plus the named metadata — verifies.
    m.get_or_insert_named_metadata(NamedMetadataName::LinkerOptions);
    m.verify_borrowed()?;
    Ok(())
}

/// Port of `llvm/test/Verifier/module-flags-note-gnu-property-elf-pauthabi.ll`
/// `err1.ll` (platform without version) — `CHECK: either both or no
/// 'aarch64-elf-pauthabi-platform' and 'aarch64-elf-pauthabi-version'
/// module flags must be present`.
#[test]
fn pauthabi_platform_without_version_is_rejected() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let key = m.metadata_string("aarch64-elf-pauthabi-platform");
    let two = int32(&m, 2)?;
    add_flag(&m, &[one, key, two])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagPauthAbiPairing,
        "either both or no 'aarch64-elf-pauthabi-platform' and 'aarch64-elf-pauthabi-version' \
         module flags must be present",
    );
    Ok(())
}

/// Port of the same fixture's `err2.ll` (version without platform) — same
/// `CHECK` line.
#[test]
fn pauthabi_version_without_platform_is_rejected() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let key = m.metadata_string("aarch64-elf-pauthabi-version");
    let thirty_one = int32(&m, 31)?;
    add_flag(&m, &[one, key, thirty_one])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagPauthAbiPairing,
        "either both or no 'aarch64-elf-pauthabi-platform' and 'aarch64-elf-pauthabi-version' \
         module flags must be present",
    );
    Ok(())
}

/// The complement of the two `pauthabi_*` error ports: both flags present
/// verifies. No upstream `CHECK` line spells this — it is the rule's stated
/// valid half ("either both or no ..."), pinned so the pairing predicate
/// cannot silently become "never".
#[test]
fn pauthabi_pair_verifies() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let one = int32(&m, 1)?;
    let platform_key = m.metadata_string("aarch64-elf-pauthabi-platform");
    let version_key = m.metadata_string("aarch64-elf-pauthabi-version");
    let two = int32(&m, 2)?;
    let thirty_one = int32(&m, 31)?;
    add_flag(&m, &[one, platform_key, two])?;
    add_flag(&m, &[one, version_key, thirty_one])?;
    m.verify_borrowed()?;
    Ok(())
}

// --------------------------------------------------------------------------
// CG Profile — port of `llvm/test/Verifier/module-flags-cgprofile.ll`
// --------------------------------------------------------------------------

/// The fixture's scaffolding: `declare void @a()` / `declare void @b()` as
/// `ptr`-typed metadata constants, and a `!{i32 5, !"CG Profile", !1}` flag
/// whose `!1` holds exactly `entries`.
fn add_cg_profile_flag<B: ModuleBrand>(
    m: &Module<B>,
    entries: &[MetadataId<B>],
) -> Result<(MetadataId<B>, MetadataId<B>), IrError> {
    let void_fn = m.function_type_no_parameters(m.void_type());
    let a = m.add_function_dyn("a", void_fn, Linkage::External)?;
    let b = m.add_function_dyn("b", void_fn, Linkage::External)?;
    let a_ptr = m.metadata_constant(m.view(a).as_global_constant_ptr())?;
    let b_ptr = m.metadata_constant(m.view(b).as_global_constant_ptr())?;
    let list = m.metadata_tuple(entries)?;
    let five = int32(m, 5)?;
    let key = m.metadata_string("CG Profile");
    add_flag(m, &[five, key, list])?;
    Ok((a_ptr, b_ptr))
}

/// Port of `module-flags-cgprofile.ll` `!2 = !{ptr @a, ptr @b, i64 32}` —
/// the fixture's one well-formed entry (no `CHECK` names it): a
/// `(function, function, count)` triple verifies.
#[test]
fn cg_profile_valid_entry_verifies() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let void_fn = m.function_type_no_parameters(m.void_type());
    let a = m.add_function_dyn("a", void_fn, Linkage::External)?;
    let b = m.add_function_dyn("b", void_fn, Linkage::External)?;
    let a_ptr = m.metadata_constant(m.view(a).as_global_constant_ptr())?;
    let b_ptr = m.metadata_constant(m.view(b).as_global_constant_ptr())?;
    let count = m.metadata_constant(m.i64_type().const_int(32_i64))?;
    let entry = m.metadata_tuple([a_ptr, b_ptr, count])?;
    let list = m.metadata_tuple([entry])?;
    let five = int32(&m, 5)?;
    let key = m.metadata_string("CG Profile");
    add_flag(&m, &[five, key, list])?;
    m.verify_borrowed()?;
    Ok(())
}

/// Port of `module-flags-cgprofile.ll` `!3 = !{ptr @a, ptr @b}` —
/// `CHECK: expected a MDNode triple`.
#[test]
fn cg_profile_entry_must_have_three_operands() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let void_fn = m.function_type_no_parameters(m.void_type());
    let a = m.add_function_dyn("a", void_fn, Linkage::External)?;
    let b = m.add_function_dyn("b", void_fn, Linkage::External)?;
    let a_ptr = m.metadata_constant(m.view(a).as_global_constant_ptr())?;
    let b_ptr = m.metadata_constant(m.view(b).as_global_constant_ptr())?;
    let entry = m.metadata_tuple([a_ptr, b_ptr])?;
    let list = m.metadata_tuple([entry])?;
    let five = int32(&m, 5)?;
    let key = m.metadata_string("CG Profile");
    add_flag(&m, &[five, key, list])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagCgProfileMalformed,
        "expected a MDNode triple",
    );
    Ok(())
}

/// Port of `module-flags-cgprofile.ll`'s `!""` list entry —
/// `CHECK: expected a MDNode triple` (a string is not a node).
#[test]
fn cg_profile_string_entry_is_rejected() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let empty_string = m.metadata_string("");
    add_cg_profile_flag(&m, &[empty_string])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagCgProfileMalformed,
        "expected a MDNode triple",
    );
    Ok(())
}

/// Port of `module-flags-cgprofile.ll`
/// `!4 = !{ptr @a, ptr @b, i64 32, i64 32}` — `CHECK: expected a MDNode
/// triple` (four operands).
#[test]
fn cg_profile_four_operand_entry_is_rejected() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let void_fn = m.function_type_no_parameters(m.void_type());
    let a = m.add_function_dyn("a", void_fn, Linkage::External)?;
    let b = m.add_function_dyn("b", void_fn, Linkage::External)?;
    let a_ptr = m.metadata_constant(m.view(a).as_global_constant_ptr())?;
    let b_ptr = m.metadata_constant(m.view(b).as_global_constant_ptr())?;
    let count = m.metadata_constant(m.i64_type().const_int(32_i64))?;
    let entry = m.metadata_tuple([a_ptr, b_ptr, count, count])?;
    let list = m.metadata_tuple([entry])?;
    let five = int32(&m, 5)?;
    let key = m.metadata_string("CG Profile");
    add_flag(&m, &[five, key, list])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagCgProfileMalformed,
        "expected a MDNode triple",
    );
    Ok(())
}

/// Port of `module-flags-cgprofile.ll` `!5 = !{!"a", ptr @b, i64 32}` —
/// `CHECK: expected a Function or null` (the caller operand).
#[test]
fn cg_profile_caller_must_be_a_function_or_null() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let void_fn = m.function_type_no_parameters(m.void_type());
    let b = m.add_function_dyn("b", void_fn, Linkage::External)?;
    let b_ptr = m.metadata_constant(m.view(b).as_global_constant_ptr())?;
    let a_string = m.metadata_string("a");
    let count = m.metadata_constant(m.i64_type().const_int(32_i64))?;
    let entry = m.metadata_tuple([a_string, b_ptr, count])?;
    let list = m.metadata_tuple([entry])?;
    let five = int32(&m, 5)?;
    let key = m.metadata_string("CG Profile");
    add_flag(&m, &[five, key, list])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagCgProfileMalformed,
        "expected a Function or null",
    );
    Ok(())
}

/// Port of `module-flags-cgprofile.ll` `!6 = !{ptr @a, !"b", i64 32}` —
/// `CHECK: expected a Function or null` (the callee operand).
#[test]
fn cg_profile_callee_must_be_a_function_or_null() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let void_fn = m.function_type_no_parameters(m.void_type());
    let a = m.add_function_dyn("a", void_fn, Linkage::External)?;
    let a_ptr = m.metadata_constant(m.view(a).as_global_constant_ptr())?;
    let b_string = m.metadata_string("b");
    let count = m.metadata_constant(m.i64_type().const_int(32_i64))?;
    let entry = m.metadata_tuple([a_ptr, b_string, count])?;
    let list = m.metadata_tuple([entry])?;
    let five = int32(&m, 5)?;
    let key = m.metadata_string("CG Profile");
    add_flag(&m, &[five, key, list])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagCgProfileMalformed,
        "expected a Function or null",
    );
    Ok(())
}

/// Port of `module-flags-cgprofile.ll` `!7 = !{ptr @a, ptr @b, !""}` —
/// `CHECK: expected an integer constant`.
#[test]
fn cg_profile_count_must_be_an_integer_constant() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let void_fn = m.function_type_no_parameters(m.void_type());
    let a = m.add_function_dyn("a", void_fn, Linkage::External)?;
    let b = m.add_function_dyn("b", void_fn, Linkage::External)?;
    let a_ptr = m.metadata_constant(m.view(a).as_global_constant_ptr())?;
    let b_ptr = m.metadata_constant(m.view(b).as_global_constant_ptr())?;
    let not_a_count = m.metadata_string("");
    let entry = m.metadata_tuple([a_ptr, b_ptr, not_a_count])?;
    let list = m.metadata_tuple([entry])?;
    let five = int32(&m, 5)?;
    let key = m.metadata_string("CG Profile");
    add_flag(&m, &[five, key, list])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagCgProfileMalformed,
        "expected an integer constant",
    );
    Ok(())
}

/// Port of `module-flags-cgprofile.ll` `!8 = !{ptr @a, ptr @b, null}` —
/// `CHECK: expected an integer constant` (a null count; null callers and
/// callees are fine, a null count is not).
#[test]
fn cg_profile_null_count_is_rejected() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let void_fn = m.function_type_no_parameters(m.void_type());
    let a = m.add_function_dyn("a", void_fn, Linkage::External)?;
    let b = m.add_function_dyn("b", void_fn, Linkage::External)?;
    let a_ptr = m.metadata_constant(m.view(a).as_global_constant_ptr())?;
    let b_ptr = m.metadata_constant(m.view(b).as_global_constant_ptr())?;
    let null = m.metadata_node(MetadataKind::Null)?;
    let entry = m.metadata_tuple([a_ptr, b_ptr, null])?;
    let list = m.metadata_tuple([entry])?;
    let five = int32(&m, 5)?;
    let key = m.metadata_string("CG Profile");
    add_flag(&m, &[five, key, list])?;
    let err = m.verify_borrowed().unwrap_err();
    assert_flag_error(
        &err,
        VerifierRule::ModuleFlagCgProfileMalformed,
        "expected an integer constant",
    );
    Ok(())
}
