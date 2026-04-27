//! Attributes. Mirrors `llvm/include/llvm/IR/Attributes.{h,td}` and
//! `llvm/lib/IR/Attributes.cpp`.
//!
//! Phase B subset per the IR foundation plan: enum kind, integer attrs
//! (`align`, `dereferenceable`), string attrs, type attrs.
//! `ConstantRange`-bearing attributes (`Range`) and the deeper
//! attribute-set machinery (FoldingSet-keyed `AttributeSetNode`) are
//! deferred to a later session.
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

use crate::r#type::Type;

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

    /// `true` for plain enum / flag kinds (no payload).
    #[inline]
    pub const fn is_enum_kind(self) -> bool {
        !self.is_int_kind() && !self.is_type_kind()
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
pub enum Attribute<'ctx> {
    /// Flag-only attribute (`AlwaysInline`, `NoReturn`, ...).
    Enum(AttrKind),
    /// Integer-valued attribute (`align(8)`, `dereferenceable(N)`, ...).
    Int(AttrKind, u64),
    /// Type-valued attribute (`byval(T)`, `sret(T)`, ...).
    Type(AttrKind, Type<'ctx>),
    /// Free-form key=value string attribute. Used for target-dependent
    /// attributes (`"target-features"`, `"frame-pointer"`, ...).
    String { key: String, value: String },
}

impl<'ctx> Attribute<'ctx> {
    /// Construct an enum-flavored attribute. Returns `None` if `kind`
    /// expects a payload.
    pub fn enum_attr(kind: AttrKind) -> Option<Self> {
        if kind.is_enum_kind() {
            Some(Self::Enum(kind))
        } else {
            None
        }
    }

    /// Construct an integer-valued attribute. Returns `None` if `kind`
    /// is not an integer-flavored kind.
    pub fn int(kind: AttrKind, value: u64) -> Option<Self> {
        if kind.is_int_kind() {
            Some(Self::Int(kind, value))
        } else {
            None
        }
    }

    /// Construct a type-valued attribute. Returns `None` if `kind` is
    /// not a type-flavored kind.
    pub fn type_attr(kind: AttrKind, ty: Type<'ctx>) -> Option<Self> {
        if kind.is_type_kind() {
            Some(Self::Type(kind, ty))
        } else {
            None
        }
    }

    /// Construct a string key=value attribute. Always valid.
    pub fn string(key: impl Into<String>, value: impl Into<String>) -> Self {
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
            Self::String { .. } => None,
        }
    }
}

impl<'ctx> fmt::Display for Attribute<'ctx> {
    /// Render in `.ll` syntax (`alwaysinline`, `align(8)`, `byval(i32)`,
    /// `"key"="value"`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enum(k) => f.write_str(k.name()),
            Self::Int(k, v) => write!(f, "{}({v})", k.name()),
            Self::Type(k, t) => write!(f, "{}({t})", k.name()),
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
pub struct AttributeSet<'ctx> {
    attrs: Vec<Attribute<'ctx>>,
}

impl<'ctx> AttributeSet<'ctx> {
    pub fn new() -> Self {
        Self { attrs: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.attrs.len()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Attribute<'ctx>> {
        self.attrs.iter()
    }

    /// Add `attr` if not already present (by `==`). No-op otherwise.
    pub fn add(&mut self, attr: Attribute<'ctx>) {
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

impl<'ctx> FromIterator<Attribute<'ctx>> for AttributeSet<'ctx> {
    fn from_iter<I: IntoIterator<Item = Attribute<'ctx>>>(iter: I) -> Self {
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
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AttributeList<'ctx> {
    entries: Vec<(AttrIndex, AttributeSet<'ctx>)>,
}

impl<'ctx> AttributeList<'ctx> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only iterator over `(index, set)` pairs.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (AttrIndex, &AttributeSet<'ctx>)> {
        self.entries.iter().map(|(i, s)| (*i, s))
    }

    /// Borrow the attribute set at `index`, or `None` if no entry is
    /// present.
    pub fn get(&self, index: AttrIndex) -> Option<&AttributeSet<'ctx>> {
        self.entries
            .iter()
            .find_map(|(i, s)| (*i == index).then_some(s))
    }

    /// Mutably borrow the set at `index`, creating an empty entry if
    /// none exists.
    pub fn get_mut_or_default(&mut self, index: AttrIndex) -> &mut AttributeSet<'ctx> {
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
    pub fn add(&mut self, index: AttrIndex, attr: Attribute<'ctx>) {
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
pub(crate) enum AttributeStored {
    Enum(AttrKind),
    Int(AttrKind, u64),
    Type(AttrKind, crate::r#type::TypeId),
    String { key: String, value: String },
}

impl AttributeStored {
    /// Build storage form from a public [`Attribute`].
    pub(crate) fn from_attribute(attr: Attribute<'_>) -> Self {
        match attr {
            Attribute::Enum(k) => Self::Enum(k),
            Attribute::Int(k, v) => Self::Int(k, v),
            Attribute::Type(k, t) => Self::Type(k, t.id()),
            Attribute::String { key, value } => Self::String { key, value },
        }
    }
}

/// Lifetime-free counterpart of [`AttributeList`] used inside the
/// value arena. Stored on each [`crate::function::FunctionData`].
#[derive(Debug, Default, Clone)]
pub(crate) struct AttributeStorage {
    entries: Vec<(AttrIndex, Vec<AttributeStored>)>,
}

impl AttributeStorage {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Insert `attr` at `index`. De-duplicates by structural
    /// equality.
    pub(crate) fn add(&mut self, index: AttrIndex, attr: Attribute<'_>) {
        let stored = AttributeStored::from_attribute(attr);
        if let Some(pos) = self.entries.iter().position(|(i, _)| *i == index) {
            let set = &mut self.entries[pos].1;
            if !set.contains(&stored) {
                set.push(stored);
            }
            return;
        }
        self.entries.push((index, vec![stored]));
    }

    /// Borrow the slice of stored attributes at `index`, or `None`
    /// when no entry exists. Used by the AsmWriter for parameter /
    /// return / function attribute printing.
    pub(crate) fn get(&self, index: AttrIndex) -> Option<&[AttributeStored]> {
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

    /// Mirrors `Attribute::get(LLVMContext &, AttrKind)` /
    /// `Attribute::get(LLVMContext &, AttrKind, uint64_t)` validation in
    /// `lib/IR/Attributes.cpp`. Closest unit-test:
    /// `unittests/IR/AttributesTest.cpp::TEST(Attributes, AttributeRoundTrip)`.
    #[test]
    fn enum_kind_constructors_validate() {
        assert!(Attribute::<'_>::enum_attr(AttrKind::AlwaysInline).is_some());
        // Integer kind rejected by enum_attr:
        assert!(Attribute::<'_>::enum_attr(AttrKind::Alignment).is_none());
        // Enum kind rejected by int constructor:
        assert!(Attribute::<'_>::int(AttrKind::AlwaysInline, 8).is_none());
        // Integer kind accepted:
        assert!(matches!(
            Attribute::<'_>::int(AttrKind::Alignment, 8),
            Some(Attribute::Int(AttrKind::Alignment, 8))
        ));
    }

    /// llvmkit-specific: Rust enum partition. Closest upstream:
    /// `Attribute::isEnumAttribute` / `isIntAttribute` / `isTypeAttribute`
    /// in `lib/IR/Attributes.cpp`.
    #[test]
    fn kind_partition_is_total() {
        // Every variant is exactly one of enum/int/type.
        let kinds = [AttrKind::AlwaysInline, AttrKind::Alignment, AttrKind::ByVal];
        for k in kinds {
            let e = k.is_enum_kind();
            let i = k.is_int_kind();
            let t = k.is_type_kind();
            assert!(e as u8 + i as u8 + t as u8 == 1, "{k:?} fails partition");
        }
    }

    /// Mirrors `Attribute::getAsString` in `lib/IR/Attributes.cpp`;
    /// rendered shape matches assembler emission in
    /// `test/Assembler/unnamed_addr.ll` and `lib/IR/AsmWriter.cpp`.
    #[test]
    fn display_renders_attribute_text() {
        assert_eq!(
            format!("{}", Attribute::<'_>::Enum(AttrKind::NoReturn)),
            "noreturn"
        );
        assert_eq!(
            format!("{}", Attribute::<'_>::Int(AttrKind::Alignment, 8)),
            "align(8)"
        );
        let s = Attribute::<'_>::string("target-features", "+sse2");
        assert_eq!(format!("{s}"), "\"target-features\"=\"+sse2\"");
        let bare = Attribute::<'_>::string("nobuiltin", "");
        assert_eq!(format!("{bare}"), "\"nobuiltin\"");
    }

    /// Mirrors `AttributeSetNode::get` dedup semantics in
    /// `lib/IR/Attributes.cpp` (and `unittests/IR/AttributesTest.cpp`).
    #[test]
    fn attribute_set_dedupes_and_iterates() {
        let mut s = AttributeSet::<'_>::new();
        s.add(Attribute::Enum(AttrKind::NoReturn));
        s.add(Attribute::Enum(AttrKind::NoReturn)); // duplicate ignored
        s.add(Attribute::Int(AttrKind::Alignment, 8));
        assert_eq!(s.len(), 2);
        assert!(s.has_kind(AttrKind::NoReturn));
        assert_eq!(s.int_value(AttrKind::Alignment), Some(8));
    }

    /// Mirrors `AttributeList::addAttributeAtIndex` /
    /// `getAttributes(AttrIndex)` in `lib/IR/Attributes.cpp`.
    #[test]
    fn attribute_list_indexed_storage() {
        let mut l = AttributeList::<'_>::new();
        l.add(AttrIndex::Function, Attribute::Enum(AttrKind::NoReturn));
        l.add(AttrIndex::Param(0), Attribute::Enum(AttrKind::NoCapture));
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
