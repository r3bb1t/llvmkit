//! Typed module-level flags. Mirrors `Module::ModFlagBehavior` /
//! `Module::ModuleFlagEntry` (`llvm/include/llvm/IR/Module.h`) and the
//! module-flag helpers in `llvm/lib/IR/Module.cpp`
//! (`Module::addModuleFlag`, `Module::setModuleFlag`, `Module::getModuleFlag`,
//! `Module::getModuleFlagsMetadata`).
//!
//! Module flags carry no storage of their own: exactly as upstream, each flag
//! is a three-operand metadata tuple `!{i32 behavior, !"key", value}` inside
//! the `llvm.module.flags` named metadata node
//! ([`NamedMetadataName::ModuleFlags`](crate::NamedMetadataName::ModuleFlags)),
//! so parsed IR, the printer, and the round-trip contract are untouched. This
//! module supplies the typed vocabulary — [`ModuleFlagBehavior`],
//! [`ModuleFlagKey`], [`ModuleFlagEntry`] — and
//! [`Module`](crate::Module) carries the accessors
//! (`add_module_flag` / `set_module_flag` / `module_flag` / `module_flags`).

use crate::Branded;
use crate::metadata::{MetadataId, MetadataKind, MetadataSlot, MetadataStore, StoredBrand};
use crate::module::ModuleBrand;

// --------------------------------------------------------------------------
// Behavior
// --------------------------------------------------------------------------

/// Merge behavior of a module flag when two modules are linked. Mirrors
/// `Module::ModFlagBehavior` (`llvm/include/llvm/IR/Module.h`), including the
/// discriminant values, which are what the `i32` behavior operand of a
/// `!llvm.module.flags` tuple stores.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleFlagBehavior {
    /// Emits an error if two values disagree, otherwise the resulting value
    /// is that of the operands.
    Error = 1,
    /// Emits a warning if two values disagree. The result value will be the
    /// operand for the flag from the first module being linked.
    Warning = 2,
    /// Adds a requirement that another module flag be present and have a
    /// specified value after linking is performed. The value must be a
    /// metadata pair: the ID of the restricted flag, and the value it must
    /// have.
    Require = 3,
    /// Uses the specified value, regardless of the behavior or value of the
    /// other module. If both modules specify `Override` but the values
    /// differ, an error is emitted.
    Override = 4,
    /// Appends the two values, which are required to be metadata nodes.
    Append = 5,
    /// Appends the two values, which are required to be metadata nodes.
    /// Duplicate entries in the second list are dropped during the append.
    AppendUnique = 6,
    /// Takes the max of the two values, which are required to be integers.
    Max = 7,
    /// Takes the min of the two values, which are required to be integers.
    Min = 8,
}

impl ModuleFlagBehavior {
    /// Classify a raw behavior operand value. `None` outside
    /// `ModFlagBehaviorFirstVal..=ModFlagBehaviorLastVal` (`1..=8`) — the
    /// range check of `Module::isValidModFlagBehavior`
    /// (`llvm/lib/IR/Module.cpp`).
    pub fn from_raw(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::Error),
            2 => Some(Self::Warning),
            3 => Some(Self::Require),
            4 => Some(Self::Override),
            5 => Some(Self::Append),
            6 => Some(Self::AppendUnique),
            7 => Some(Self::Max),
            8 => Some(Self::Min),
            _ => None,
        }
    }

    /// The discriminant as stored in the `i32` behavior operand. (A `match`,
    /// not an `as` cast — the repo bans `as`.)
    pub const fn raw(self) -> u32 {
        match self {
            Self::Error => 1,
            Self::Warning => 2,
            Self::Require => 3,
            Self::Override => 4,
            Self::Append => 5,
            Self::AppendUnique => 6,
            Self::Max => 7,
            Self::Min => 8,
        }
    }
}

// --------------------------------------------------------------------------
// Key
// --------------------------------------------------------------------------

/// The identifier of a module flag, with the well-known upstream spellings as
/// variants.
///
/// The fixed set is collected from the keys `lib/IR/Module.cpp` reads and
/// writes through `Module::getModuleFlag` / `Module::addModuleFlag` (the
/// `getDwarfVersion` / `setPICLevel` / ... accessor family), plus the keys the
/// verifier special-cases (`Verifier::visitModuleFlag`,
/// `lib/IR/Verifier.cpp`: `wchar_size`, `CG Profile`). The namespace is open —
/// any other spelling is valid IR and stays [`Custom`](Self::Custom).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModuleFlagKey {
    /// `"Dwarf Version"` — read by `Module::getDwarfVersion`.
    DwarfVersion,
    /// `"DWARF64"` — read by `Module::isDwarf64`.
    Dwarf64,
    /// `"CodeView"` — read by `Module::getCodeViewFlag`.
    CodeView,
    /// `"PIC Level"` — `Module::getPICLevel` / `setPICLevel`.
    PicLevel,
    /// `"PIE Level"` — `Module::getPIELevel` / `setPIELevel`.
    PieLevel,
    /// `"Code Model"` — `Module::getCodeModel` / `setCodeModel`.
    CodeModel,
    /// `"Large Data Threshold"` — `Module::getLargeDataThreshold` /
    /// `setLargeDataThreshold`.
    LargeDataThreshold,
    /// `"ProfileSummary"` — `Module::getProfileSummary` / `setProfileSummary`.
    ProfileSummary,
    /// `"CSProfileSummary"` — the context-sensitive arm of
    /// `Module::setProfileSummary`.
    CsProfileSummary,
    /// `"SemanticInterposition"` — `Module::getSemanticInterposition` /
    /// `setSemanticInterposition`.
    SemanticInterposition,
    /// `"RtLibUseGOT"` — `Module::getRtLibUseGOT` / `setRtLibUseGOT`.
    RtLibUseGot,
    /// `"direct-access-external-data"` —
    /// `Module::getDirectAccessExternalData` / `setDirectAccessExternalData`.
    DirectAccessExternalData,
    /// `"uwtable"` — `Module::getUwtable` / `setUwtable`.
    Uwtable,
    /// `"frame-pointer"` — `Module::getFramePointer` / `setFramePointer`.
    FramePointer,
    /// `"wchar_size"` — checked by `Verifier::visitModuleFlag` and read by
    /// `TargetLibraryInfo` (`lib/Analysis/TargetLibraryInfo.cpp`); its
    /// producer (clang's CodeGen) is outside the vendored LLVM tree.
    WcharSize,
    /// `"NumRegisterParameters"` — read by
    /// `Module::getNumberRegisterParameters`.
    NumRegisterParameters,
    /// `"stack-protector-guard"` — `Module::getStackProtectorGuard` /
    /// `setStackProtectorGuard`.
    StackProtectorGuard,
    /// `"stack-protector-guard-reg"` — `Module::getStackProtectorGuardReg` /
    /// `setStackProtectorGuardReg`.
    StackProtectorGuardReg,
    /// `"stack-protector-guard-symbol"` —
    /// `Module::getStackProtectorGuardSymbol` / `setStackProtectorGuardSymbol`.
    StackProtectorGuardSymbol,
    /// `"stack-protector-guard-offset"` —
    /// `Module::getStackProtectorGuardOffset` / `setStackProtectorGuardOffset`.
    StackProtectorGuardOffset,
    /// `"override-stack-alignment"` — `Module::getOverrideStackAlignment` /
    /// `setOverrideStackAlignment`.
    OverrideStackAlignment,
    /// `"MaxTLSAlign"` — read by `Module::getMaxTLSAlignment`.
    MaxTlsAlign,
    /// `"SDK Version"` — `Module::getSDKVersion` / `setSDKVersion`.
    SdkVersion,
    /// `"darwin.target_variant.triple"` —
    /// `Module::getDarwinTargetVariantTriple` / `setDarwinTargetVariantTriple`.
    DarwinTargetVariantTriple,
    /// `"target-abi"` — read by `Module::getTargetABIFromMD`.
    TargetAbi,
    /// `"CG Profile"` — written by the `CGProfilePass`
    /// (`lib/Transforms/Instrumentation/CGProfile.cpp`) and checked by
    /// `Verifier::visitModuleFlagCGProfileEntry`.
    CgProfile,
    /// `"winx64-eh-unwindv2"` — read by `Module::getWinX64EHUnwindV2Mode`.
    WinX64EhUnwindV2,
    /// Any other spelling — the namespace is open, so an unknown key is
    /// valid IR, not an error.
    Custom(String),
}

impl ModuleFlagKey {
    /// The key string exactly as it appears in the flag tuple's `!"key"`
    /// operand — each spelling verified against `lib/IR/Module.cpp`.
    pub fn key(&self) -> &str {
        match self {
            Self::DwarfVersion => "Dwarf Version",
            Self::Dwarf64 => "DWARF64",
            Self::CodeView => "CodeView",
            Self::PicLevel => "PIC Level",
            Self::PieLevel => "PIE Level",
            Self::CodeModel => "Code Model",
            Self::LargeDataThreshold => "Large Data Threshold",
            Self::ProfileSummary => "ProfileSummary",
            Self::CsProfileSummary => "CSProfileSummary",
            Self::SemanticInterposition => "SemanticInterposition",
            Self::RtLibUseGot => "RtLibUseGOT",
            Self::DirectAccessExternalData => "direct-access-external-data",
            Self::Uwtable => "uwtable",
            Self::FramePointer => "frame-pointer",
            Self::WcharSize => "wchar_size",
            Self::NumRegisterParameters => "NumRegisterParameters",
            Self::StackProtectorGuard => "stack-protector-guard",
            Self::StackProtectorGuardReg => "stack-protector-guard-reg",
            Self::StackProtectorGuardSymbol => "stack-protector-guard-symbol",
            Self::StackProtectorGuardOffset => "stack-protector-guard-offset",
            Self::OverrideStackAlignment => "override-stack-alignment",
            Self::MaxTlsAlign => "MaxTLSAlign",
            Self::SdkVersion => "SDK Version",
            Self::DarwinTargetVariantTriple => "darwin.target_variant.triple",
            Self::TargetAbi => "target-abi",
            Self::CgProfile => "CG Profile",
            Self::WinX64EhUnwindV2 => "winx64-eh-unwindv2",
            Self::Custom(key) => key,
        }
    }

    /// Classify a textual key: the well-known spellings map to their
    /// variants, anything else to [`Custom`](Self::Custom).
    pub fn from_key(key: &str) -> Self {
        match key {
            "Dwarf Version" => Self::DwarfVersion,
            "DWARF64" => Self::Dwarf64,
            "CodeView" => Self::CodeView,
            "PIC Level" => Self::PicLevel,
            "PIE Level" => Self::PieLevel,
            "Code Model" => Self::CodeModel,
            "Large Data Threshold" => Self::LargeDataThreshold,
            "ProfileSummary" => Self::ProfileSummary,
            "CSProfileSummary" => Self::CsProfileSummary,
            "SemanticInterposition" => Self::SemanticInterposition,
            "RtLibUseGOT" => Self::RtLibUseGot,
            "direct-access-external-data" => Self::DirectAccessExternalData,
            "uwtable" => Self::Uwtable,
            "frame-pointer" => Self::FramePointer,
            "wchar_size" => Self::WcharSize,
            "NumRegisterParameters" => Self::NumRegisterParameters,
            "stack-protector-guard" => Self::StackProtectorGuard,
            "stack-protector-guard-reg" => Self::StackProtectorGuardReg,
            "stack-protector-guard-symbol" => Self::StackProtectorGuardSymbol,
            "stack-protector-guard-offset" => Self::StackProtectorGuardOffset,
            "override-stack-alignment" => Self::OverrideStackAlignment,
            "MaxTLSAlign" => Self::MaxTlsAlign,
            "SDK Version" => Self::SdkVersion,
            "darwin.target_variant.triple" => Self::DarwinTargetVariantTriple,
            "target-abi" => Self::TargetAbi,
            "CG Profile" => Self::CgProfile,
            "winx64-eh-unwindv2" => Self::WinX64EhUnwindV2,
            _ => Self::Custom(key.to_owned()),
        }
    }

    /// The behavior the canonical `lib/IR/Module.cpp` setter for this key
    /// pairs it with, or `None` when no setter exists there.
    ///
    /// The rule is deliberately narrow: `Some` only where a
    /// `Module::set*` member in `lib/IR/Module.cpp` names the behavior —
    /// `setPICLevel` uses `Min`, `setPIELevel` / `setUwtable` /
    /// `setFramePointer` / `setRtLibUseGOT` / `setDirectAccessExternalData`
    /// use `Max`, `setCodeModel` / `setLargeDataThreshold` /
    /// `setProfileSummary` / `setSemanticInterposition` /
    /// `setStackProtectorGuard{,Reg,Symbol,Offset}` /
    /// `setOverrideStackAlignment` use `Error`, and `setSDKVersion` (via
    /// `addSDKVersionMD`) / `setDarwinTargetVariantTriple` use `Warning`.
    /// Keys that `Module.cpp` only reads (`Dwarf Version`, `DWARF64`,
    /// `CodeView`, `NumRegisterParameters`, `MaxTLSAlign`, `target-abi`,
    /// `winx64-eh-unwindv2`), keys produced outside `lib/IR/Module.cpp`
    /// (`wchar_size` by clang; `CG Profile` by `CGProfile.cpp`, with
    /// `Append`), and [`Custom`](Self::Custom) keys answer `None`.
    pub const fn default_behavior(&self) -> Option<ModuleFlagBehavior> {
        match self {
            Self::PicLevel => Some(ModuleFlagBehavior::Min),
            Self::PieLevel
            | Self::RtLibUseGot
            | Self::DirectAccessExternalData
            | Self::Uwtable
            | Self::FramePointer => Some(ModuleFlagBehavior::Max),
            Self::CodeModel
            | Self::LargeDataThreshold
            | Self::ProfileSummary
            | Self::CsProfileSummary
            | Self::SemanticInterposition
            | Self::StackProtectorGuard
            | Self::StackProtectorGuardReg
            | Self::StackProtectorGuardSymbol
            | Self::StackProtectorGuardOffset
            | Self::OverrideStackAlignment => Some(ModuleFlagBehavior::Error),
            Self::SdkVersion | Self::DarwinTargetVariantTriple => Some(ModuleFlagBehavior::Warning),
            Self::DwarfVersion
            | Self::Dwarf64
            | Self::CodeView
            | Self::WcharSize
            | Self::NumRegisterParameters
            | Self::MaxTlsAlign
            | Self::TargetAbi
            | Self::CgProfile
            | Self::WinX64EhUnwindV2
            | Self::Custom(_) => None,
        }
    }
}

impl From<&str> for ModuleFlagKey {
    fn from(key: &str) -> Self {
        Self::from_key(key)
    }
}

impl From<String> for ModuleFlagKey {
    fn from(key: String) -> Self {
        match Self::from_key(&key) {
            // Reuse the caller's allocation for a custom spelling.
            Self::Custom(_) => Self::Custom(key),
            fixed => fixed,
        }
    }
}

// --------------------------------------------------------------------------
// Entry
// --------------------------------------------------------------------------

/// One decoded module flag. Mirrors `Module::ModuleFlagEntry`
/// (`llvm/include/llvm/IR/Module.h`): the merge behavior, the key, and the
/// value operand as a metadata id.
///
/// Brand-generic because `value` is the tagged metadata currency; read a
/// module's entries with [`Module::module_flags`](crate::Module::module_flags).
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct ModuleFlagEntry<B: ModuleBrand> {
    /// The merge behavior from the flag tuple's first operand.
    pub behavior: ModuleFlagBehavior,
    /// The key from the flag tuple's `!"key"` operand.
    pub key: ModuleFlagKey,
    /// The value operand — resolve it with
    /// [`Module::metadata_get`](crate::Module::metadata_get).
    pub value: MetadataId<B>,
}

impl<B: ModuleBrand> ModuleFlagEntry<B> {
    /// Crate-internal: retag a stored entry into the caller's brand — the
    /// clone-out half of [`Module::module_flags`](crate::Module::module_flags).
    /// Mirrors `NamedMetadataNode::from_stored`.
    pub(crate) fn from_stored(stored: ModuleFlagEntry<StoredBrand>) -> Self {
        Self {
            behavior: stored.behavior,
            key: stored.key,
            value: MetadataId::from_stored(stored.value),
        }
    }
}

// --------------------------------------------------------------------------
// Crate-internal decode helpers (shared by the Module read path and the
// verifier)
// --------------------------------------------------------------------------

/// Resolve a metadata slot through [`MetadataKind::Ref`] chains to the
/// underlying node.
///
/// Upstream has no analogue: a C++ `Metadata *` *is* the node, while
/// `MetadataKind::Ref` is llvmkit's arena spelling of "the same node again",
/// so flag decoding normalizes through it. The walk is bounded by the store
/// size — a longer chain can only mean a `metadata_reserve`/`metadata_set`
/// cycle, which resolves to no underlying node, so the answer is `None`
/// rather than a hang.
pub(crate) fn resolve_metadata_ref(
    store: &MetadataStore,
    slot: MetadataSlot,
) -> Option<MetadataSlot> {
    let mut current = slot;
    for _ in 0..=store.len() {
        match store.get(current)? {
            MetadataKind::Ref(id) => current = id.slot(),
            _ => return Some(current),
        }
    }
    None
}

/// Decode a `llvm.module.flags` operand into its three tuple operand ids, or
/// `None` when the operand is not a three-element tuple. The shape check of
/// `Verifier::visitModuleFlag` ("incorrect number of operands in module
/// flag"); the read paths use it to skip what the verifier would reject.
pub(crate) fn module_flag_tuple(
    store: &MetadataStore,
    op: MetadataId<StoredBrand>,
) -> Option<[MetadataId<StoredBrand>; 3]> {
    let slot = resolve_metadata_ref(store, op.slot())?;
    let MetadataKind::Tuple { operands, .. } = store.get(slot)? else {
        return None;
    };
    match operands.as_slice() {
        &[behavior, key, value] => Some([behavior, key, value]),
        _ => None,
    }
}
