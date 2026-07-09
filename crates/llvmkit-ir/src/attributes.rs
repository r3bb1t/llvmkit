//! Attributes. Mirrors `llvm/include/llvm/IR/Attributes.{h,td}` and
//! `llvm/lib/IR/Attributes.cpp`.
//!
//! Phase B subset per the IR foundation plan: enum kind, integer attrs
//! (`align`, `dereferenceable`), range attrs, string attrs, type attrs.
//! The deeper attribute-set machinery (FoldingSet-keyed `AttributeSetNode`) is
//! deferred to a later revision.
//!
//! Type-state shape:
//!
//! - [`AttrKind`] - closed enum of every well-known LLVM attribute kind
//!   (drawn from `Attributes.td`). Marked `#[non_exhaustive]` so future
//!   additions are non-breaking.
//! - [`Attribute`] - sum type where each variant carries the payload its
//!   kind requires. `Attribute::Int` only ever carries an integer kind,
//!   `Attribute::Type` only ever carries a type kind, etc. Wrong-shape
//!   construction is caught by the constructor functions.
//! - [`AttributeSet`] / [`AttributeList`] - keyed by [`AttrIndex`]. The
//!   list maps function / return / per-parameter slots to attribute
//!   sets.

use std::fmt;

use super::ApInt;
use super::module::{Brand, ModuleBrand};
use super::r#type::{Type, TypeId, TypeKind};

/// Whether an operation references memory, modifies memory, both, or neither.
/// Mirrors `llvm::ModRefInfo` in `llvm/include/llvm/Support/ModRef.h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModRefInfo {
    NoModRef,
    Ref,
    Mod,
    ModRef,
}

impl ModRefInfo {
    const fn bits(self) -> u32 {
        match self {
            Self::NoModRef => 0,
            Self::Ref => 1,
            Self::Mod => 2,
            Self::ModRef => 3,
        }
    }

    const fn from_bits(bits: u32) -> Self {
        match bits {
            0 => Self::NoModRef,
            1 => Self::Ref,
            2 => Self::Mod,
            _ => Self::ModRef,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::NoModRef => "none",
            Self::Ref => "read",
            Self::Mod => "write",
            Self::ModRef => "readwrite",
        }
    }
}

/// Memory location class used by the `memory(...)` function attribute.
/// Mirrors `llvm::IRMemLocation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemoryLocation {
    ArgMem,
    InaccessibleMem,
    ErrnoMem,
    Other,
    TargetMem0,
    TargetMem1,
}

impl MemoryLocation {
    const ALL: [Self; 6] = [
        Self::ArgMem,
        Self::InaccessibleMem,
        Self::ErrnoMem,
        Self::Other,
        Self::TargetMem0,
        Self::TargetMem1,
    ];

    const fn shift(self) -> u32 {
        match self {
            Self::ArgMem => 0,
            Self::InaccessibleMem => 2,
            Self::ErrnoMem => 4,
            Self::Other => 6,
            Self::TargetMem0 => 8,
            Self::TargetMem1 => 10,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::ArgMem => "argmem",
            Self::InaccessibleMem => "inaccessiblemem",
            Self::ErrnoMem => "errnomem",
            Self::Other => "other",
            Self::TargetMem0 => "target_mem0",
            Self::TargetMem1 => "target_mem1",
        }
    }
}

/// Encoded memory effects for `memory(...)`. Two bits are stored per
/// [`MemoryLocation`], exactly matching `MemoryEffectsBase` in `ModRef.h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemoryEffects {
    data: u32,
}

impl MemoryEffects {
    const LOC_MASK: u32 = 3;

    pub const fn unknown() -> Self {
        Self::from_mod_ref(ModRefInfo::ModRef)
    }

    pub const fn none() -> Self {
        Self::from_mod_ref(ModRefInfo::NoModRef)
    }

    pub const fn read_only() -> Self {
        Self::from_mod_ref(ModRefInfo::Ref)
    }

    pub const fn write_only() -> Self {
        Self::from_mod_ref(ModRefInfo::Mod)
    }

    pub const fn arg_mem_only() -> Self {
        Self::none().with_mod_ref(MemoryLocation::ArgMem, ModRefInfo::ModRef)
    }

    pub const fn inaccessible_mem_only() -> Self {
        Self::none().with_mod_ref(MemoryLocation::InaccessibleMem, ModRefInfo::ModRef)
    }

    pub const fn inaccessible_or_arg_mem_only() -> Self {
        Self::none()
            .with_mod_ref(MemoryLocation::ArgMem, ModRefInfo::ModRef)
            .with_mod_ref(MemoryLocation::InaccessibleMem, ModRefInfo::ModRef)
    }

    pub const fn create_from_int_value(data: u32) -> Self {
        Self { data }
    }

    pub const fn to_int_value(self) -> u32 {
        self.data
    }

    pub const fn get_mod_ref(self, location: MemoryLocation) -> ModRefInfo {
        ModRefInfo::from_bits((self.data >> location.shift()) & Self::LOC_MASK)
    }

    pub const fn with_mod_ref(self, location: MemoryLocation, mod_ref: ModRefInfo) -> Self {
        let shift = location.shift();
        Self {
            data: (self.data & !(Self::LOC_MASK << shift)) | (mod_ref.bits() << shift),
        }
    }

    pub(crate) const fn from_mod_ref(mod_ref: ModRefInfo) -> Self {
        let bits = mod_ref.bits();
        Self {
            data: bits | (bits << 2) | (bits << 4) | (bits << 6) | (bits << 8) | (bits << 10),
        }
    }

    fn aggregate_mod_ref(self) -> ModRefInfo {
        let mut bits = 0;
        for location in MemoryLocation::ALL {
            bits |= self.get_mod_ref(location).bits();
        }
        ModRefInfo::from_bits(bits)
    }
}

impl fmt::Display for MemoryEffects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let effects = *self;
        let other = effects.get_mod_ref(MemoryLocation::Other);
        let aggregate = effects.aggregate_mod_ref();
        let mut first = true;

        f.write_str("memory(")?;
        if other != ModRefInfo::NoModRef || aggregate == other {
            f.write_str(other.name())?;
            first = false;
        }

        for location in MemoryLocation::ALL {
            let mod_ref = effects.get_mod_ref(location);
            if mod_ref == other || location == MemoryLocation::Other {
                continue;
            }
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            write!(f, "{}: {}", location.name(), mod_ref.name())?;
        }

        f.write_str(")")
    }
}

// --------------------------------------------------------------------------
// AttrKind
// --------------------------------------------------------------------------

/// Discriminator for attribute kinds. Mirrors the `def Foo : ...Attr<...>`
/// declarations in `Attributes.td`.
///
/// Marked `#[non_exhaustive]` so we can add LLVM additions without a
/// breaking change. Variants are organised by payload category for
/// readability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AttrKind {
    // ---- Enum (flag) attributes ----
    AlwaysInline,
    Builtin,
    Cold,
    Convergent,
    DisableSanitizerInstrumentation,
    Hot,
    HybridPatchable,
    ImmArg,
    InReg,
    InlineHint,
    JumpTable,
    MinSize,
    MustProgress,
    Naked,
    Nest,
    NoAlias,
    NoBuiltin,
    NoCallback,
    NoCreateUndefOrPoison,
    NoCapture,
    NoCfCheck,
    NoDuplicate,
    NoFree,
    NoImplicitFloat,
    NoInline,
    NoMerge,
    NoProfile,
    NoRecurse,
    NoRedZone,
    NoReturn,
    NoSanitizeBounds,
    NoSanitizeCoverage,
    NoSync,
    NoUndef,
    NoUnwind,
    NonLazyBind,
    NonNull,
    NullPointerIsValid,
    OptForFuzzing,
    OptimizeForSize,
    OptimizeNone,
    PresplitCoroutine,
    ReadNone,
    ReadOnly,
    Returned,
    ReturnsTwice,
    SExt,
    SafeStack,
    SanitizeAddress,
    SanitizeHwAddress,
    SanitizeMemTag,
    SanitizeMemory,
    SanitizeThread,
    ShadowCallStack,
    SkipProfile,
    Speculatable,
    SpeculativeLoadHardening,
    StackProtect,
    StackProtectReq,
    StackProtectStrong,
    StrictFP,
    SwiftAsync,
    SwiftError,
    SwiftSelf,
    WillReturn,
    Writable,
    WriteOnly,
    ZExt,

    // ---- Integer-valued attributes ----
    Alignment,
    AllocKind,
    AllocSize,
    Dereferenceable,
    DereferenceableOrNull,
    StackAlignment,
    UWTable,
    VScaleRange,
    Range,
    Memory,

    // ---- Type-valued attributes ----
    ByRef,
    ByVal,
    ElementType,
    InAlloca,
    Preallocated,
    StructRet,
}

impl AttrKind {
    /// Mnemonic spelling used in `.ll` syntax. Mirrors the `def` name in
    /// `Attributes.td` (lowercased / underscored where appropriate).
    pub const fn name(self) -> &'static str {
        match self {
            // Enum / flag
            Self::AlwaysInline => "alwaysinline",
            Self::Builtin => "builtin",
            Self::Cold => "cold",
            Self::Convergent => "convergent",
            Self::DisableSanitizerInstrumentation => "disable_sanitizer_instrumentation",
            Self::Hot => "hot",
            Self::HybridPatchable => "hybrid_patchable",
            Self::ImmArg => "immarg",
            Self::InReg => "inreg",
            Self::InlineHint => "inlinehint",
            Self::JumpTable => "jumptable",
            Self::MinSize => "minsize",
            Self::MustProgress => "mustprogress",
            Self::Naked => "naked",
            Self::Nest => "nest",
            Self::NoAlias => "noalias",
            Self::NoBuiltin => "nobuiltin",
            Self::NoCallback => "nocallback",
            Self::NoCreateUndefOrPoison => "nocreateundeforpoison",
            Self::NoCapture => "nocapture",
            Self::NoCfCheck => "nocf_check",
            Self::NoDuplicate => "noduplicate",
            Self::NoFree => "nofree",
            Self::NoImplicitFloat => "noimplicitfloat",
            Self::NoInline => "noinline",
            Self::NoMerge => "nomerge",
            Self::NoProfile => "noprofile",
            Self::NoRecurse => "norecurse",
            Self::NoRedZone => "noredzone",
            Self::NoReturn => "noreturn",
            Self::NoSanitizeBounds => "nosanitize_bounds",
            Self::NoSanitizeCoverage => "nosanitize_coverage",
            Self::NoSync => "nosync",
            Self::NoUndef => "noundef",
            Self::NoUnwind => "nounwind",
            Self::NonLazyBind => "nonlazybind",
            Self::NonNull => "nonnull",
            Self::NullPointerIsValid => "null_pointer_is_valid",
            Self::OptForFuzzing => "optforfuzzing",
            Self::OptimizeForSize => "optsize",
            Self::OptimizeNone => "optnone",
            Self::PresplitCoroutine => "presplitcoroutine",
            Self::ReadNone => "readnone",
            Self::ReadOnly => "readonly",
            Self::Returned => "returned",
            Self::ReturnsTwice => "returns_twice",
            Self::SExt => "signext",
            Self::SafeStack => "safestack",
            Self::SanitizeAddress => "sanitize_address",
            Self::SanitizeHwAddress => "sanitize_hwaddress",
            Self::SanitizeMemTag => "sanitize_memtag",
            Self::SanitizeMemory => "sanitize_memory",
            Self::SanitizeThread => "sanitize_thread",
            Self::ShadowCallStack => "shadowcallstack",
            Self::SkipProfile => "skipprofile",
            Self::Speculatable => "speculatable",
            Self::SpeculativeLoadHardening => "speculative_load_hardening",
            Self::StackProtect => "ssp",
            Self::StackProtectReq => "sspreq",
            Self::StackProtectStrong => "sspstrong",
            Self::StrictFP => "strictfp",
            Self::SwiftAsync => "swiftasync",
            Self::SwiftError => "swifterror",
            Self::SwiftSelf => "swiftself",
            Self::WillReturn => "willreturn",
            Self::Writable => "writable",
            Self::WriteOnly => "writeonly",
            Self::ZExt => "zeroext",
            // Integer
            Self::Alignment => "align",
            Self::AllocKind => "allockind",
            Self::AllocSize => "allocsize",
            Self::Dereferenceable => "dereferenceable",
            Self::DereferenceableOrNull => "dereferenceable_or_null",
            Self::StackAlignment => "alignstack",
            Self::UWTable => "uwtable",
            Self::VScaleRange => "vscale_range",
            Self::Range => "range",
            Self::Memory => "memory",
            // Type
            Self::ByRef => "byref",
            Self::ByVal => "byval",
            Self::ElementType => "elementtype",
            Self::InAlloca => "inalloca",
            Self::Preallocated => "preallocated",
            Self::StructRet => "sret",
        }
    }

    /// `true` for kinds whose payload is a single `u64`.
    pub const fn is_int_kind(self) -> bool {
        matches!(
            self,
            Self::Alignment
                | Self::AllocKind
                | Self::AllocSize
                | Self::Dereferenceable
                | Self::DereferenceableOrNull
                | Self::StackAlignment
                | Self::UWTable
                | Self::VScaleRange
        )
    }

    /// `true` for kinds whose payload is a `Type<'ctx>`.
    pub const fn is_type_kind(self) -> bool {
        matches!(
            self,
            Self::ByRef
                | Self::ByVal
                | Self::ElementType
                | Self::InAlloca
                | Self::Preallocated
                | Self::StructRet
        )
    }

    /// `true` for kinds whose payload is an integer constant range.
    #[inline]
    pub const fn is_range_kind(self) -> bool {
        matches!(self, Self::Range)
    }

    /// `true` for the exact memory-effects payload attribute.
    #[inline]
    pub const fn is_memory_kind(self) -> bool {
        matches!(self, Self::Memory)
    }
    /// `true` for plain enum / flag kinds (no payload).
    #[inline]
    pub const fn is_enum_kind(self) -> bool {
        !self.is_int_kind()
            && !self.is_type_kind()
            && !self.is_range_kind()
            && !self.is_memory_kind()
    }
}

impl fmt::Display for AttrKind {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// --------------------------------------------------------------------------
// Attribute
// --------------------------------------------------------------------------

/// A single attribute.
///
/// One variant per payload category. Constructors at module level
/// (`Attribute::int`, `Attribute::type_attr`, etc.) refuse mismatched
/// kinds at runtime; in practice consumers should use the convenience
/// builders instead of constructing variants directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Attribute<'ctx, B: ModuleBrand = Brand<'ctx>> {
    /// Flag-only attribute (`AlwaysInline`, `NoReturn`, ...).
    Enum(AttrKind),
    /// Integer-valued attribute (`align(8)`, `dereferenceable(N)`, ...).
    Int(AttrKind, u64),
    /// Type-valued attribute (`byval(T)`, `sret(T)`, ...).
    Type(AttrKind, Type<'ctx, B>),
    /// Integer constant-range attribute (`range(i32 0, 8)`).
    Range {
        ty: Type<'ctx, B>,
        lower: ApInt,
        upper: ApInt,
    },
    /// Exact function memory effects (`memory(read)`, `memory(argmem: read)`).
    Memory(MemoryEffects),
    /// Free-form key=value string attribute. Used for target-dependent
    /// attributes (`"target-features"`, `"frame-pointer"`, ...).
    String { key: String, value: String },
}

impl<'ctx> Attribute<'ctx> {
    /// Construct an enum-flavored attribute with the default module brand.
    /// Returns `None` if `kind` expects a payload.
    pub fn enum_attr(kind: AttrKind) -> Option<Self> {
        Self::enum_attr_for_brand(kind)
    }

    /// Construct an integer-valued attribute with the default module brand.
    /// Returns `None` if `kind` is not an integer-flavored kind.
    pub fn int(kind: AttrKind, value: u64) -> Option<Self> {
        Self::int_for_brand(kind, value)
    }

    /// Construct a memory-effects attribute with the default module brand.
    pub fn memory(effects: MemoryEffects) -> Self {
        Self::memory_for_brand(effects)
    }

    /// Construct a string key=value attribute with the default module brand.
    /// Always valid.
    pub fn string<Key, ValueText>(key: Key, value: ValueText) -> Self
    where
        Key: Into<String>,
        ValueText: Into<String>,
    {
        Self::string_for_brand(key, value)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Attribute<'ctx, B> {
    /// Construct an enum-flavored attribute for an explicitly branded module.
    /// Returns `None` if `kind` expects a payload.
    pub fn enum_attr_for_brand(kind: AttrKind) -> Option<Self> {
        if kind.is_enum_kind() {
            Some(Self::Enum(kind))
        } else {
            None
        }
    }

    /// Construct an integer-valued attribute for an explicitly branded module.
    /// Returns `None` if `kind` is not an integer-flavored kind.
    pub fn int_for_brand(kind: AttrKind, value: u64) -> Option<Self> {
        if kind.is_int_kind() {
            Some(Self::Int(kind, value))
        } else {
            None
        }
    }

    /// Construct a memory-effects attribute for an explicitly branded module.
    pub fn memory_for_brand(effects: MemoryEffects) -> Self {
        Self::Memory(effects)
    }

    /// Construct a type-valued attribute. Returns `None` if `kind` is
    /// not a type-flavored kind.
    pub fn type_attr(kind: AttrKind, ty: Type<'ctx, B>) -> Option<Self> {
        if kind.is_type_kind() {
            Some(Self::Type(kind, ty))
        } else {
            None
        }
    }

    /// Construct an integer range attribute. Returns `None` when `ty` is not an
    /// integer type, when the bounds have the wrong bit width, or when the
    /// range spells an empty set other than LLVM's canonical full-set form
    /// `range(T 0, 0)`.
    pub fn range(ty: Type<'ctx, B>, lower: ApInt, upper: ApInt) -> Option<Self> {
        let TypeKind::Integer { bits } = ty.kind() else {
            return None;
        };
        if lower.bit_width() != bits || upper.bit_width() != bits {
            return None;
        }
        if lower.eq_ap_int(&upper) && !lower.is_zero() {
            return None;
        }
        Some(Self::Range { ty, lower, upper })
    }

    /// Construct a string key=value attribute for an explicitly branded module.
    /// Always valid.
    pub fn string_for_brand<Key, ValueText>(key: Key, value: ValueText) -> Self
    where
        Key: Into<String>,
        ValueText: Into<String>,
    {
        Self::String {
            key: key.into(),
            value: value.into(),
        }
    }

    /// `true` for string-flavored attributes.
    #[inline]
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String { .. })
    }

    /// Enum kind, or `None` for string attributes.
    pub fn kind(&self) -> Option<AttrKind> {
        match self {
            Self::Enum(k) | Self::Int(k, _) | Self::Type(k, _) => Some(*k),
            Self::Range { .. } => Some(AttrKind::Range),
            Self::Memory(_) => Some(AttrKind::Memory),
            Self::String { .. } => None,
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> fmt::Display for Attribute<'ctx, B> {
    /// Render in `.ll` syntax (`alwaysinline`, `align(8)`, `byval(i32)`,
    /// `"key"="value"`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enum(k) => f.write_str(k.name()),
            Self::Int(k, v) => write!(f, "{}({v})", k.name()),
            Self::Type(k, t) => write!(f, "{}({t})", k.name()),
            Self::Range { ty, lower, upper } => write!(
                f,
                "range({ty} {}, {})",
                lower.to_string_radix(10, crate::ApIntSignedness::Signed),
                upper.to_string_radix(10, crate::ApIntSignedness::Signed)
            ),
            Self::Memory(effects) => write!(f, "{effects}"),
            Self::String { key, value } if value.is_empty() => write!(f, "\"{key}\""),
            Self::String { key, value } => write!(f, "\"{key}\"=\"{value}\""),
        }
    }
}

// --------------------------------------------------------------------------
// AttrIndex
// --------------------------------------------------------------------------

/// Index into an [`AttributeList`]. Mirrors `AttributeList::AttrIndex`
/// (`Attributes.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttrIndex {
    /// Attributes that apply to the function as a whole.
    Function,
    /// Attributes that apply to the return value.
    Return,
    /// Attributes that apply to a single parameter (0-based).
    Param(u32),
}

impl AttrIndex {
    pub const FIRST_ARG_INDEX: u32 = 0;
}

// --------------------------------------------------------------------------
// AttributeSet
// --------------------------------------------------------------------------

/// A collection of attributes that all apply to the same index. Mirrors
/// `AttributeSet` (`Attributes.h`); the storage shape here is a flat
/// `Vec` rather than the upstream `FoldingSet`-uniqued node, which is
/// fine for the foundation.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct AttributeSet<'ctx, B: ModuleBrand = Brand<'ctx>> {
    attrs: Vec<Attribute<'ctx, B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> AttributeSet<'ctx, B> {
    pub fn new() -> Self {
        Self { attrs: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.attrs.len()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Attribute<'ctx, B>> {
        self.attrs.iter()
    }

    /// Add `attr` if not already present (by `==`). No-op otherwise.
    pub fn add(&mut self, attr: Attribute<'ctx, B>) {
        if !self.attrs.contains(&attr) {
            self.attrs.push(attr);
        }
    }

    /// Remove every attribute matching the given enum/int/type kind.
    /// String attributes are unaffected.
    pub fn remove_kind(&mut self, kind: AttrKind) {
        self.attrs.retain(|a| a.kind() != Some(kind));
    }

    /// Remove every string attribute matching `key`.
    pub fn remove_string(&mut self, key: &str) {
        self.attrs.retain(|a| match a {
            Attribute::String { key: k, .. } => k != key,
            _ => true,
        });
    }

    /// `true` if any attribute in the set matches `kind`.
    pub fn has_kind(&self, kind: AttrKind) -> bool {
        self.attrs.iter().any(|a| a.kind() == Some(kind))
    }

    /// Look up the integer payload for an integer-flavored kind.
    pub fn int_value(&self, kind: AttrKind) -> Option<u64> {
        self.attrs.iter().find_map(|a| match a {
            Attribute::Int(k, v) if *k == kind => Some(*v),
            _ => None,
        })
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FromIterator<Attribute<'ctx, B>> for AttributeSet<'ctx, B> {
    fn from_iter<I: IntoIterator<Item = Attribute<'ctx, B>>>(iter: I) -> Self {
        Self {
            attrs: iter.into_iter().collect(),
        }
    }
}

// --------------------------------------------------------------------------
// AttributeList
// --------------------------------------------------------------------------

/// Per-index attribute table. Mirrors `AttributeList` (`Attributes.h`)
/// in shape; storage is flat (a small `Vec<(AttrIndex, AttributeSet)>`)
/// instead of the upstream FoldingSet, which is fine for the foundation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeList<'ctx, B: ModuleBrand = Brand<'ctx>> {
    entries: Vec<(AttrIndex, AttributeSet<'ctx, B>)>,
}

impl<'ctx, B: ModuleBrand + 'ctx> Default for AttributeList<'ctx, B> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> AttributeList<'ctx, B> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only iterator over `(index, set)` pairs.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (AttrIndex, &AttributeSet<'ctx, B>)> {
        self.entries.iter().map(|(i, s)| (*i, s))
    }

    /// Borrow the attribute set at `index`, or `None` if no entry is
    /// present.
    pub fn get(&self, index: AttrIndex) -> Option<&AttributeSet<'ctx, B>> {
        self.entries
            .iter()
            .find_map(|(i, s)| (*i == index).then_some(s))
    }

    /// Mutably borrow the set at `index`, creating an empty entry if
    /// none exists.
    pub fn get_mut_or_default(&mut self, index: AttrIndex) -> &mut AttributeSet<'ctx, B> {
        if let Some(pos) = self.entries.iter().position(|(i, _)| *i == index) {
            return &mut self.entries[pos].1;
        }
        // `push` followed by indexing the last slot is the panic-free
        // equivalent of `last_mut().unwrap()`.
        let pos = self.entries.len();
        self.entries.push((index, AttributeSet::new()));
        &mut self.entries[pos].1
    }

    /// Add `attr` at `index`. Convenience wrapper around
    /// [`get_mut_or_default`](Self::get_mut_or_default).
    pub fn add(&mut self, index: AttrIndex, attr: Attribute<'ctx, B>) {
        self.get_mut_or_default(index).add(attr);
    }

    /// `true` if no index has any attributes.
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|(_, s)| s.is_empty())
    }
}

// --------------------------------------------------------------------------
// Lifetime-free storage shape
// --------------------------------------------------------------------------

/// Storage form of [`Attribute`] used inside the value arena. Each
/// `Type<'ctx>` payload is collapsed to a [`TypeId`] so the enum is
/// lifetime-free and can be embedded in [`crate::value::ValueData`].
///
/// Conversions are total in both directions when paired with a
/// `Module<'ctx>`: `Attribute<'ctx> <-> AttributeStored`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum AttributeStored {
    Enum(AttrKind),
    Int(AttrKind, u64),
    Type(AttrKind, TypeId),
    Range {
        ty: TypeId,
        lower: ApInt,
        upper: ApInt,
    },
    Memory(MemoryEffects),
    String {
        key: String,
        value: String,
    },
}

impl AttributeStored {
    /// Build storage form from a public [`Attribute`].
    pub(super) fn from_attribute<B: ModuleBrand>(attr: Attribute<'_, B>) -> Self {
        match attr {
            Attribute::Enum(k) => Self::Enum(k),
            Attribute::Int(k, v) => Self::Int(k, v),
            Attribute::Type(k, t) => Self::Type(k, t.id()),
            Attribute::Range { ty, lower, upper } => Self::Range {
                ty: ty.id(),
                lower,
                upper,
            },
            Attribute::Memory(effects) => Self::Memory(effects),
            Attribute::String { key, value } => Self::String { key, value },
        }
    }
}
impl fmt::Display for AttributeStored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enum(k) => f.write_str(k.name()),
            Self::Int(k, v) => write!(f, "{}({v})", k.name()),
            Self::Type(_, _) | Self::Range { .. } => {
                unreachable!("typed attributes need a module context to print")
            }
            Self::Memory(effects) => write!(f, "{effects}"),
            Self::String { key, value } if value.is_empty() => write!(f, "\"{key}\""),
            Self::String { key, value } => write!(f, "\"{key}\"=\"{value}\""),
        }
    }
}

/// Lifetime-free counterpart of [`AttributeList`] used inside the
/// value arena for function payloads.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct AttributeStorage {
    entries: Vec<(AttrIndex, Vec<AttributeStored>)>,
}

impl AttributeStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert `attr` at `index`. De-duplicates by structural
    /// equality.
    pub fn add<B: ModuleBrand>(&mut self, index: AttrIndex, attr: Attribute<'_, B>) {
        self.add_stored(index, AttributeStored::from_attribute(attr));
    }

    pub(super) fn add_stored(&mut self, index: AttrIndex, stored: AttributeStored) {
        if let Some(pos) = self.entries.iter().position(|(i, _)| *i == index) {
            let set = &mut self.entries[pos].1;
            if !set.contains(&stored) {
                set.push(stored);
            }
            return;
        }
        self.entries.push((index, vec![stored]));
    }

    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|(_, attrs)| attrs.is_empty())
    }

    /// `true` if every stored attribute here is also present in `other`.
    pub fn is_subset_of(&self, other: &Self) -> bool {
        self.entries.iter().all(|(index, attrs)| {
            other
                .get(*index)
                .is_some_and(|other_attrs| attrs.iter().all(|attr| other_attrs.contains(attr)))
        })
    }

    /// Merge every attribute from `other` into this storage, preserving
    /// per-index de-duplication.
    pub fn merge_from(&mut self, other: &Self) {
        for (index, attrs) in &other.entries {
            for attr in attrs {
                self.add_stored(*index, attr.clone());
            }
        }
    }

    /// Set equality over attribute contents. Entry and attribute insertion
    /// order are intentionally ignored; attributes are already de-duplicated
    /// within each index by [`Self::add`].
    pub fn has_same_attributes(&self, other: &Self) -> bool {
        self.is_subset_of(other) && other.is_subset_of(self)
    }

    /// Set equality for one attribute index. Attribute storage at other
    /// indexes is ignored.
    pub fn index_has_same_attributes(&self, other: &Self, index: AttrIndex) -> bool {
        self.index_is_subset_of(other, index) && other.index_is_subset_of(self, index)
    }

    /// `true` when every non-empty entry in this storage is at `index`, and
    /// that entry exactly matches `other` at the same index.
    pub fn has_only_index_attributes_matching(&self, other: &Self, index: AttrIndex) -> bool {
        self.entries
            .iter()
            .all(|(entry_index, attrs)| *entry_index == index || attrs.is_empty())
            && self.index_has_same_attributes(other, index)
    }

    /// `true` when every non-empty entry in this storage is at `index`, and
    /// that entry contains only attributes also present in `other` at the same
    /// index.
    pub fn has_only_index_attributes_subset_of(&self, other: &Self, index: AttrIndex) -> bool {
        self.entries
            .iter()
            .all(|(entry_index, attrs)| *entry_index == index || attrs.is_empty())
            && self.index_is_subset_of(other, index)
    }

    fn index_is_subset_of(&self, other: &Self, index: AttrIndex) -> bool {
        let Some(attrs) = self.get(index) else {
            return true;
        };
        match other.get(index) {
            Some(other_attrs) => attrs.iter().all(|attr| other_attrs.contains(attr)),
            None => attrs.is_empty(),
        }
    }

    /// Borrow the slice of stored attributes at `index`, or `None`
    /// when no entry exists. Used by the AsmWriter for parameter /
    /// return / function attribute printing.
    pub(super) fn get(&self, index: AttrIndex) -> Option<&[AttributeStored]> {
        self.entries
            .iter()
            .find_map(|(i, set)| (*i == index).then_some(set.as_slice()))
    }
}

/// Upstream provenance: mirrors `class Attribute` / `AttributeSet` /
/// `AttributeList` from `lib/IR/Attributes.cpp`, exercised at runtime by
/// `unittests/IR/AttributesTest.cpp`. Display assertions track the
/// `getAsString` shape used in `test/Assembler/unnamed_addr.ll` and other
/// `test/Assembler/*.ll` fixtures.
#[cfg(test)]
mod tests {
    use super::*;
    type TestAttribute<'ctx> = Attribute<'ctx, Brand<'ctx>>;
    type TestAttributeSet<'ctx> = AttributeSet<'ctx, Brand<'ctx>>;
    type TestAttributeList<'ctx> = AttributeList<'ctx, Brand<'ctx>>;

    /// Mirrors `Attribute::get(LLVMContext &, AttrKind)` /
    /// `Attribute::get(LLVMContext &, AttrKind, uint64_t)` validation in
    /// `lib/IR/Attributes.cpp`. Closest unit-test:
    /// `unittests/IR/AttributesTest.cpp::TEST(Attributes, AttributeRoundTrip)`.
    #[test]
    fn enum_kind_constructors_validate() {
        assert!(TestAttribute::<'_>::enum_attr(AttrKind::AlwaysInline).is_some());
        // Integer kind rejected by enum_attr:
        assert!(TestAttribute::<'_>::enum_attr(AttrKind::Alignment).is_none());
        // Enum kind rejected by int constructor:
        assert!(TestAttribute::<'_>::int(AttrKind::AlwaysInline, 8).is_none());
        // Integer kind accepted:
        assert!(matches!(
            TestAttribute::<'_>::int(AttrKind::Alignment, 8),
            Some(Attribute::Int(AttrKind::Alignment, 8))
        ));
    }

    /// llvmkit-specific: Rust enum partition. Closest upstream:
    /// `Attribute::isEnumAttribute` / `isIntAttribute` / `isTypeAttribute`
    /// in `lib/IR/Attributes.cpp`, extended for `RangeAttr` and `Memory`.
    #[test]
    fn kind_partition_is_total() {
        // Every variant is exactly one of enum/int/type/range/memory.
        let kinds = [
            AttrKind::AlwaysInline,
            AttrKind::Alignment,
            AttrKind::ByVal,
            AttrKind::Range,
            AttrKind::Memory,
        ];
        for k in kinds {
            let categories = [
                k.is_enum_kind(),
                k.is_int_kind(),
                k.is_type_kind(),
                k.is_range_kind(),
                k.is_memory_kind(),
            ];
            assert_eq!(
                categories.into_iter().filter(|category| *category).count(),
                1,
                "{k:?} fails partition"
            );
        }
    }

    /// Mirrors `Attribute::getAsString` in `lib/IR/Attributes.cpp`;
    /// rendered shape matches assembler emission in
    /// `test/Assembler/unnamed_addr.ll` and `lib/IR/AsmWriter.cpp`.
    #[test]
    fn display_renders_attribute_text() {
        assert_eq!(
            format!("{}", TestAttribute::<'_>::Enum(AttrKind::NoReturn)),
            "noreturn"
        );
        assert_eq!(
            format!("{}", TestAttribute::<'_>::Int(AttrKind::Alignment, 8)),
            "align(8)"
        );
        let s = TestAttribute::<'_>::string("target-features", "+sse2");
        assert_eq!(format!("{s}"), "\"target-features\"=\"+sse2\"");
        let bare = TestAttribute::<'_>::string("nobuiltin", "");
        assert_eq!(format!("{bare}"), "\"nobuiltin\"");
        assert_eq!(
            format!(
                "{}",
                TestAttribute::<'_>::memory(MemoryEffects::read_only())
            ),
            "memory(read)"
        );
        assert_eq!(
            format!(
                "{}",
                TestAttribute::<'_>::memory(
                    MemoryEffects::none()
                        .with_mod_ref(MemoryLocation::ArgMem, ModRefInfo::Ref)
                        .with_mod_ref(MemoryLocation::TargetMem0, ModRefInfo::Mod)
                )
            ),
            "memory(argmem: read, target_mem0: write)"
        );
    }

    /// Mirrors `AttributeSetNode::get` dedup semantics in
    /// `lib/IR/Attributes.cpp` (and `unittests/IR/AttributesTest.cpp`).
    #[test]
    fn attribute_set_dedupes_and_iterates() {
        let mut s = TestAttributeSet::<'_>::new();
        s.add(TestAttribute::Enum(AttrKind::NoReturn));
        s.add(TestAttribute::Enum(AttrKind::NoReturn)); // duplicate ignored
        s.add(TestAttribute::Int(AttrKind::Alignment, 8));
        assert_eq!(s.len(), 2);
        assert!(s.has_kind(AttrKind::NoReturn));
        assert_eq!(s.int_value(AttrKind::Alignment), Some(8));
    }

    /// Mirrors `AttributeList::addAttributeAtIndex` /
    /// `getAttributes(AttrIndex)` in `lib/IR/Attributes.cpp`.
    #[test]
    fn attribute_list_indexed_storage() {
        let mut l = TestAttributeList::<'_>::new();
        l.add(AttrIndex::Function, TestAttribute::Enum(AttrKind::NoReturn));
        l.add(
            AttrIndex::Param(0),
            TestAttribute::Enum(AttrKind::NoCapture),
        );
        assert!(
            l.get(AttrIndex::Function)
                .unwrap()
                .has_kind(AttrKind::NoReturn)
        );
        assert!(
            l.get(AttrIndex::Param(0))
                .unwrap()
                .has_kind(AttrKind::NoCapture)
        );
        assert!(l.get(AttrIndex::Param(1)).is_none());
    }
}
