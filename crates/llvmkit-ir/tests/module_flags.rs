//! Typed module-flags API: `Module::{add,set}_module_flag`,
//! `Module::module_flag`, `Module::module_flags`, and the
//! `ModuleFlagBehavior` / `ModuleFlagKey` vocabulary.
//!
//! The `set_module_flag*` tests port the module-flag `TEST(ModuleTest, ...)`
//! blocks of `llvm/unittests/IR/ModuleTest.cpp` verbatim; the rest are
//! llvmkit-specific locks on the vocabulary tables (anchored on
//! `lib/IR/Module.cpp` symbols) and on the storage promise that flags are
//! ordinary `!llvm.module.flags` tuples the printer already knows.

use llvmkit_ir::{IrError, MetadataKind, Module, ModuleFlagBehavior, ModuleFlagKey, module_new};

/// Port of `TEST(ModuleTest, setModuleFlag)`
/// (`llvm/unittests/IR/ModuleTest.cpp`): key `"Key"`, `MDString` values
/// `"Val1"` / `"Val2"`; the flag reads back absent, then `Val1`, then —
/// after a second `setModuleFlag` — `Val2`, not a duplicate. Upstream's
/// `EXPECT_EQ(Val1, M.getModuleFlag(Key))` compares uniqued `Metadata`
/// pointers; `MetadataId` equality is the same comparison here because
/// `metadata_string` interns.
#[test]
fn set_module_flag() -> Result<(), IrError> {
    let m = module_new!("M")?;
    let key = ModuleFlagKey::from("Key");
    let val1 = m.metadata_string("Val1");
    let val2 = m.metadata_string("Val2");
    assert_eq!(m.module_flag(&key), None);
    m.set_module_flag(ModuleFlagBehavior::Error, key.clone(), val1)?;
    assert_eq!(m.module_flag(&key), Some(val1));
    m.set_module_flag(ModuleFlagBehavior::Error, key.clone(), val2)?;
    assert_eq!(m.module_flag(&key), Some(val2));
    Ok(())
}

/// Port of `TEST(ModuleTest, setModuleFlagInt)`
/// (`llvm/unittests/IR/ModuleTest.cpp`): `uint32_t` values 1 and 2, which
/// upstream's `setModuleFlag(.., uint32_t)` overload wraps as `i32`
/// constants; the read-back extracts the integer like upstream's
/// `mdconst::extract_or_null<ConstantInt>` + `getZExtValue`.
#[test]
fn set_module_flag_int() -> Result<(), IrError> {
    let m = module_new!("M")?;
    let key = ModuleFlagKey::from("Key");
    let extract = |id| -> Option<u64> {
        let Some(MetadataKind::Constant(value_id)) = m.metadata_get(id) else {
            return None;
        };
        m.view(value_id).to_const_int()?.try_zext_u64()
    };
    assert_eq!(m.module_flag(&key), None);
    let val1 = m.metadata_constant(m.i32_type().const_int(1_u32))?;
    m.set_module_flag(ModuleFlagBehavior::Error, key.clone(), val1)?;
    let a1 = m.module_flag(&key).and_then(extract);
    assert_eq!(a1, Some(1));
    let val2 = m.metadata_constant(m.i32_type().const_int(2_u32))?;
    m.set_module_flag(ModuleFlagBehavior::Error, key.clone(), val2)?;
    let a2 = m.module_flag(&key).and_then(extract);
    assert_eq!(a2, Some(2));
    Ok(())
}

/// Port of `TEST(ModuleTest, setModuleFlagTwoMod)`
/// (`llvm/unittests/IR/ModuleTest.cpp`): the same key set in two modules is
/// two independent flags — changing `MA`'s value leaves `MB`'s original
/// value in place.
#[test]
fn set_module_flag_two_mod() -> Result<(), IrError> {
    let ma = Module::dynamic("MA");
    let mb = Module::dynamic("MB");
    let key = ModuleFlagKey::from("Key");
    let extract = |m: &llvmkit_ir::Module<llvmkit_ir::DynBrand>| -> Option<u64> {
        let id = m.module_flag(&key)?;
        let Some(MetadataKind::Constant(value_id)) = m.metadata_get(id) else {
            return None;
        };
        m.view(value_id).to_const_int()?.try_zext_u64()
    };

    // Set a flag to MA
    assert_eq!(ma.module_flag(&key), None);
    let a_val1 = ma.metadata_constant(ma.i32_type().const_int(1_u32))?;
    ma.set_module_flag(ModuleFlagBehavior::Error, key.clone(), a_val1)?;
    assert_eq!(extract(&ma), Some(1));

    // Set a flag to MB
    assert_eq!(mb.module_flag(&key), None);
    let b_val1 = mb.metadata_constant(mb.i32_type().const_int(1_u32))?;
    mb.set_module_flag(ModuleFlagBehavior::Error, key.clone(), b_val1)?;
    assert_eq!(extract(&mb), Some(1));

    // Change the flag of MA
    let a_val2 = ma.metadata_constant(ma.i32_type().const_int(2_u32))?;
    ma.set_module_flag(ModuleFlagBehavior::Error, key.clone(), a_val2)?;
    assert_eq!(extract(&ma), Some(2));

    // MB should keep the original flag value
    assert_eq!(extract(&mb), Some(1));
    Ok(())
}

/// llvmkit-specific, no upstream unit test: locks the well-known key
/// strings and the `key`/`from_key` inversion against drift. Every string
/// is verified against its `lib/IR/Module.cpp` accessor —
/// `Module::getDwarfVersion` reads `"Dwarf Version"`, `Module::isDwarf64`
/// `"DWARF64"`, `Module::getCodeViewFlag` `"CodeView"`, and so on — plus
/// the two verifier-only keys (`Verifier::visitModuleFlag`'s
/// `"wchar_size"` and `"CG Profile"`).
#[test]
fn module_flag_key_strings_round_trip() {
    let table: &[(ModuleFlagKey, &str)] = &[
        (ModuleFlagKey::DwarfVersion, "Dwarf Version"),
        (ModuleFlagKey::Dwarf64, "DWARF64"),
        (ModuleFlagKey::CodeView, "CodeView"),
        (ModuleFlagKey::PicLevel, "PIC Level"),
        (ModuleFlagKey::PieLevel, "PIE Level"),
        (ModuleFlagKey::CodeModel, "Code Model"),
        (ModuleFlagKey::LargeDataThreshold, "Large Data Threshold"),
        (ModuleFlagKey::ProfileSummary, "ProfileSummary"),
        (ModuleFlagKey::CsProfileSummary, "CSProfileSummary"),
        (
            ModuleFlagKey::SemanticInterposition,
            "SemanticInterposition",
        ),
        (ModuleFlagKey::RtLibUseGot, "RtLibUseGOT"),
        (
            ModuleFlagKey::DirectAccessExternalData,
            "direct-access-external-data",
        ),
        (ModuleFlagKey::Uwtable, "uwtable"),
        (ModuleFlagKey::FramePointer, "frame-pointer"),
        (ModuleFlagKey::WcharSize, "wchar_size"),
        (
            ModuleFlagKey::NumRegisterParameters,
            "NumRegisterParameters",
        ),
        (ModuleFlagKey::StackProtectorGuard, "stack-protector-guard"),
        (
            ModuleFlagKey::StackProtectorGuardReg,
            "stack-protector-guard-reg",
        ),
        (
            ModuleFlagKey::StackProtectorGuardSymbol,
            "stack-protector-guard-symbol",
        ),
        (
            ModuleFlagKey::StackProtectorGuardOffset,
            "stack-protector-guard-offset",
        ),
        (
            ModuleFlagKey::OverrideStackAlignment,
            "override-stack-alignment",
        ),
        (ModuleFlagKey::MaxTlsAlign, "MaxTLSAlign"),
        (ModuleFlagKey::SdkVersion, "SDK Version"),
        (
            ModuleFlagKey::DarwinTargetVariantTriple,
            "darwin.target_variant.triple",
        ),
        (ModuleFlagKey::TargetAbi, "target-abi"),
        (ModuleFlagKey::CgProfile, "CG Profile"),
        (ModuleFlagKey::WinX64EhUnwindV2, "winx64-eh-unwindv2"),
    ];
    for (variant, spelling) in table {
        assert_eq!(variant.key(), *spelling, "{variant:?}");
        assert_eq!(&ModuleFlagKey::from_key(spelling), variant, "{spelling}");
    }
    assert_eq!(
        ModuleFlagKey::from_key("no-such-key"),
        ModuleFlagKey::Custom("no-such-key".to_owned())
    );
    assert_eq!(ModuleFlagKey::Custom("x".to_owned()).key(), "x");
}

/// llvmkit-specific, no upstream unit test: locks
/// `ModuleFlagKey::default_behavior` to the `lib/IR/Module.cpp` setter
/// pairings — `Module::setPICLevel` uses `Min`; `setPIELevel`,
/// `setRtLibUseGOT`, `setDirectAccessExternalData`, `setUwtable`, and
/// `setFramePointer` use `Max`; `setCodeModel`, `setLargeDataThreshold`,
/// `setProfileSummary` (both keys), `setSemanticInterposition`,
/// `setStackProtectorGuard{,Reg,Symbol,Offset}`, and
/// `setOverrideStackAlignment` use `Error`; `setSDKVersion` (via
/// `addSDKVersionMD`) and `setDarwinTargetVariantTriple` use `Warning`.
/// Keys with no `Module.cpp` setter answer `None`.
#[test]
fn module_flag_key_default_behaviors_match_module_cpp_setters() {
    use ModuleFlagBehavior::{Error, Max, Min, Warning};
    let some: &[(ModuleFlagKey, ModuleFlagBehavior)] = &[
        (ModuleFlagKey::PicLevel, Min),
        (ModuleFlagKey::PieLevel, Max),
        (ModuleFlagKey::RtLibUseGot, Max),
        (ModuleFlagKey::DirectAccessExternalData, Max),
        (ModuleFlagKey::Uwtable, Max),
        (ModuleFlagKey::FramePointer, Max),
        (ModuleFlagKey::CodeModel, Error),
        (ModuleFlagKey::LargeDataThreshold, Error),
        (ModuleFlagKey::ProfileSummary, Error),
        (ModuleFlagKey::CsProfileSummary, Error),
        (ModuleFlagKey::SemanticInterposition, Error),
        (ModuleFlagKey::StackProtectorGuard, Error),
        (ModuleFlagKey::StackProtectorGuardReg, Error),
        (ModuleFlagKey::StackProtectorGuardSymbol, Error),
        (ModuleFlagKey::StackProtectorGuardOffset, Error),
        (ModuleFlagKey::OverrideStackAlignment, Error),
        (ModuleFlagKey::SdkVersion, Warning),
        (ModuleFlagKey::DarwinTargetVariantTriple, Warning),
    ];
    for (key, behavior) in some {
        assert_eq!(key.default_behavior(), Some(*behavior), "{key:?}");
    }
    let none = [
        ModuleFlagKey::DwarfVersion,
        ModuleFlagKey::Dwarf64,
        ModuleFlagKey::CodeView,
        ModuleFlagKey::WcharSize,
        ModuleFlagKey::NumRegisterParameters,
        ModuleFlagKey::MaxTlsAlign,
        ModuleFlagKey::TargetAbi,
        ModuleFlagKey::CgProfile,
        ModuleFlagKey::WinX64EhUnwindV2,
        ModuleFlagKey::Custom("x".to_owned()),
    ];
    for key in none {
        assert_eq!(key.default_behavior(), None, "{key:?}");
    }
}

/// llvmkit-specific, no upstream unit test: locks
/// `ModuleFlagBehavior::from_raw` to the `Module::isValidModFlagBehavior`
/// range (`ModFlagBehaviorFirstVal = Error = 1` through
/// `ModFlagBehaviorLastVal = Min = 8`, `Module.h`) and `raw` to the
/// declared discriminants.
#[test]
fn module_flag_behavior_raw_round_trip() {
    let all = [
        (1, ModuleFlagBehavior::Error),
        (2, ModuleFlagBehavior::Warning),
        (3, ModuleFlagBehavior::Require),
        (4, ModuleFlagBehavior::Override),
        (5, ModuleFlagBehavior::Append),
        (6, ModuleFlagBehavior::AppendUnique),
        (7, ModuleFlagBehavior::Max),
        (8, ModuleFlagBehavior::Min),
    ];
    for (raw, behavior) in all {
        assert_eq!(ModuleFlagBehavior::from_raw(raw), Some(behavior));
        assert_eq!(u64::from(behavior.raw()), raw);
    }
    assert_eq!(ModuleFlagBehavior::from_raw(0), None);
    assert_eq!(ModuleFlagBehavior::from_raw(9), None);
    assert_eq!(ModuleFlagBehavior::from_raw(u64::MAX), None);
}

/// llvmkit-specific, no upstream unit test: the storage promise. A flag
/// added through the typed API is an ordinary
/// `!{i32 behavior, !"key", value}` tuple inside `!llvm.module.flags` —
/// the shape `Module::addModuleFlag` builds (`lib/IR/Module.cpp`) and the
/// only shape the printer ever sees, so `AsmWriter` output is byte-identical
/// to the parsed spelling.
#[test]
fn add_module_flag_prints_as_the_upstream_tuples() -> Result<(), IrError> {
    let m = module_new!("flags")?;
    let four = m.metadata_constant(m.i32_type().const_int(4_u32))?;
    m.add_module_flag(ModuleFlagBehavior::Error, ModuleFlagKey::WcharSize, four)?;
    let two = m.metadata_constant(m.i32_type().const_int(2_u32))?;
    m.add_module_flag(ModuleFlagBehavior::Min, ModuleFlagKey::PicLevel, two)?;
    m.verify_borrowed()?;
    let text = format!("{m}");
    assert!(text.contains("!llvm.module.flags = !{!0, !1}"), "{text}");
    assert!(
        text.contains("!0 = !{i32 1, !\"wchar_size\", i32 4}"),
        "{text}"
    );
    assert!(
        text.contains("!1 = !{i32 8, !\"PIC Level\", i32 2}"),
        "{text}"
    );
    Ok(())
}

/// llvmkit-specific, no upstream unit test (anchor:
/// `Module::getModuleFlagsMetadata`, `lib/IR/Module.cpp`): `module_flags`
/// decodes every well-formed tuple in operand order — behavior, classified
/// key, and the value operand id — and skips a malformed (two-operand)
/// tuple silently, exactly the tolerance of upstream's read path ("The
/// verifier will catch errors, so no need to check them here").
#[test]
fn module_flags_decodes_entries_and_skips_malformed() -> Result<(), IrError> {
    let m = module_new!("flags")?;
    let four = m.metadata_constant(m.i32_type().const_int(4_u32))?;
    m.add_module_flag(ModuleFlagBehavior::Error, ModuleFlagKey::WcharSize, four)?;
    let custom = m.metadata_string("v");
    m.add_module_flag(ModuleFlagBehavior::Warning, "my-flag", custom)?;
    // A malformed two-operand tuple, appended directly to the named node —
    // the read path skips it; `verify` (not exercised here) rejects it.
    let one = m.metadata_constant(m.i32_type().const_int(1_u32))?;
    let malformed = m.metadata_tuple([one, custom])?;
    let flags_node = m.get_or_insert_named_metadata("llvm.module.flags");
    m.named_metadata_add_operand(flags_node, malformed)?;

    let entries = m.module_flags();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].behavior, ModuleFlagBehavior::Error);
    assert_eq!(entries[0].key, ModuleFlagKey::WcharSize);
    assert_eq!(entries[0].value, four);
    assert_eq!(entries[1].behavior, ModuleFlagBehavior::Warning);
    assert_eq!(entries[1].key, ModuleFlagKey::Custom("my-flag".to_owned()));
    assert_eq!(entries[1].value, custom);
    Ok(())
}

/// llvmkit-specific, no upstream unit test (anchor: `Module::addModuleFlag`
/// vs `Module::setModuleFlag`, `lib/IR/Module.cpp`): `add` appends even
/// when the key exists — upstream documents `setModuleFlag` as "Like
/// addModuleFlag but replaces the old module flag if it already exists" —
/// while `set` replaces in place, preserving the flag's position.
#[test]
fn add_appends_but_set_replaces_in_place() -> Result<(), IrError> {
    let m = module_new!("flags")?;
    let a = m.metadata_string("a");
    let b = m.metadata_string("b");
    let c = m.metadata_string("c");
    m.add_module_flag(ModuleFlagBehavior::Warning, "first", a)?;
    m.add_module_flag(ModuleFlagBehavior::Warning, "second", b)?;
    // Replace the *first* flag: position preserved, no duplicate added.
    m.set_module_flag(ModuleFlagBehavior::Warning, "first", c)?;
    let entries = m.module_flags();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].key, ModuleFlagKey::Custom("first".to_owned()));
    assert_eq!(entries[0].value, c);
    assert_eq!(entries[1].key, ModuleFlagKey::Custom("second".to_owned()));
    assert_eq!(entries[1].value, b);
    // `add` on an existing key appends a second tuple (upstream contract),
    // which `module_flag` — a first-match walk like `getModuleFlag` —
    // does not observe.
    m.add_module_flag(ModuleFlagBehavior::Warning, "second", a)?;
    assert_eq!(m.module_flags().len(), 3);
    assert_eq!(m.module_flag(&ModuleFlagKey::from("second")), Some(b));
    Ok(())
}
