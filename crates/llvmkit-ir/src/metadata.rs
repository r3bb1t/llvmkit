//! Metadata types. Mirrors `llvm/include/llvm/IR/Metadata.h`.
//!
//! Models numbered metadata tuples/strings, named metadata operands,
//! attachment sets, and the core specialized DI node surface the assembler
//! parser needs to round-trip debug metadata without storing opaque IR text.
//!
//! # Two currencies, one arena
//!
//! The metadata vocabulary is split exactly the way 0.0.4 cycle A split
//! the *value* currency:
//!
//! - `MetadataSlot` is the bare arena index — crate-internal, carrying neither
//!   a [`ModuleId`] tag nor a brand, the metadata twin of [`ValueSlot`].
//! - [`MetadataId<B>`] is the public currency: `Copy + Send + 'static`, a
//!   `(tag, slot)` pair that only ever reaches the arena through a module-tag
//!   check, the metadata twin of [`ValueId`].
//!
//! Every vocabulary type that *carries* a metadata reference is therefore
//! generic in the brand: [`MetadataKind`], [`SpecializedMetadataNode`],
//! [`MetadataField`], [`MetadataFieldValue`], [`DebugRecord`],
//! [`DebugVariableRecord`], [`DebugMetadataOperand`], and
//! [`MetadataAttachmentSet`].
//!
//! Storage cannot be generic — a module's arena is brand-free (`ModuleCore` has
//! no `B`) — so it holds those very same types at a crate-private storage
//! brand. The two forms meet at exactly two crate-internal conversions:
//! `into_stored`, which performs the tag check on the way in, and
//! `from_stored`, a pure retag of ids the arena already owns. One definition
//! per concept, with the check at a single choke point, rather than a public
//! type and a private twin that could drift apart.

use core::iter::FusedIterator;
use core::marker::PhantomData;

use crate::Branded;
use crate::error::{IrError, IrResult};
use crate::module::{Invariant, ModuleBrand, ModuleId};
use crate::value::ValueSlot;
use crate::value_id::ValueId;

// --------------------------------------------------------------------------
// The arena index, the storage brand, and the public id
// --------------------------------------------------------------------------

/// Stable index into the module-level metadata arena.
///
/// Crate-internal on purpose: a slot is a bare `usize` carrying neither a
/// [`ModuleId`] tag nor a brand, so it means something only inside the module
/// that minted it. The public currency is [`MetadataId<B>`]; a slot is reached
/// from one only through the arena choke point, which checks the tag first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct MetadataSlot(pub(crate) usize);

impl MetadataSlot {
    /// Numeric index of this slot. Used by the AsmWriter for `!N` slot
    /// numbering, which walks the arena in index order.
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

/// The brand under which a module holds its **own** metadata in the arena.
///
/// A brand exists to keep two modules' handles from being interchangeable. Data
/// already *inside* an arena has no such duty: every id the arena holds was
/// minted by the module that holds it, so it is native by construction. What
/// the storage brand buys is that the stored form and the public form are the
/// *same types* — [`MetadataKind<StoredBrand>`] and [`MetadataKind<B>`], not a
/// public enum plus a private twin that could drift apart.
///
/// Crate-private, and it never appears in a public signature: the only way
/// across the boundary is `into_stored` (tag-checked) and `from_stored` (a pure
/// retag). It is deliberately **not** [`DynBrand`](crate::DynBrand), which is a
/// real user-facing brand with its own meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct StoredBrand;
impl ModuleBrand for StoredBrand {}

/// Storable, module-tagged id for a metadata node — the metadata currency.
///
/// The metadata twin of [`ValueId`]: `{ tag: ModuleId, slot: MetadataSlot }`,
/// `Copy`, lifetime-free, and `'static` for every brand, so it can be stored
/// anywhere including across a thread boundary. It carries **no** cached node
/// content; the node is recovered from the arena when the id is resolved.
///
/// The `tag` is the process-unique [`ModuleId`] of the owning module, checked
/// before the arena is touched, so an id from a foreign module can never
/// mis-resolve against an in-range slot. The `_brand` phantom is always
/// `Invariant<B>` (`PhantomData<fn(B) -> B>`): `Send`-neutral and invariant in
/// `B`, exactly like [`ValueId`] and the borrowing handles, so two distinct
/// named brands are two distinct id types.
///
/// Mint one with [`Module::metadata_string`](crate::Module::metadata_string),
/// [`metadata_tuple`](crate::Module::metadata_tuple),
/// [`metadata_node`](crate::Module::metadata_node), and their siblings; read
/// the node back with
/// [`Module::metadata_get`](crate::Module::metadata_get).
pub struct MetadataId<B: ModuleBrand> {
    tag: ModuleId,
    slot: MetadataSlot,
    _brand: Invariant<B>,
}

impl<B: ModuleBrand> MetadataId<B> {
    /// Crate-internal: mint an id from an already-resolved tag + slot. The only
    /// callers are the module-level metadata constructors, which pass their own
    /// [`ModuleId`] and the slot the arena just handed back.
    #[inline]
    pub(crate) fn from_raw(tag: ModuleId, slot: MetadataSlot) -> Self {
        Self {
            tag,
            slot,
            _brand: PhantomData,
        }
    }

    /// **The metadata currency's tag check.** Convert a caller-supplied id into
    /// the storage form, rejecting one minted by a different module.
    ///
    /// This is the only route from a caller's [`MetadataId<B>`] to a
    /// [`MetadataSlot`]: `slot()` exists solely on `MetadataId<StoredBrand>`,
    /// which can only be produced here or by `from_stored` on an id the arena
    /// already owns. So the check cannot be forgotten one level up — a call
    /// site that wants the slot must first name this function and handle its
    /// `Err`.
    #[inline]
    pub(crate) fn into_stored(self, owner: ModuleId) -> IrResult<MetadataId<StoredBrand>> {
        if self.tag != owner {
            return Err(IrError::ForeignMetadataId);
        }
        Ok(MetadataId::from_raw(self.tag, self.slot))
    }

    /// Crate-internal: retag an id the arena already owns back into the
    /// caller's brand. Infallible by construction — a stored id was minted by
    /// the module that stores it, so its tag already matches.
    #[inline]
    pub(crate) fn from_stored(stored: MetadataId<StoredBrand>) -> Self {
        Self::from_raw(stored.tag, stored.slot)
    }
}

impl MetadataId<StoredBrand> {
    /// Crate-internal: the arena slot this **stored** id names.
    ///
    /// Defined only for the storage brand, which is the whole point: a stored
    /// id is native to the module that holds it, so no tag check is owed. A
    /// caller-supplied `MetadataId<B>` has no such accessor and must go through
    /// [`into_stored`](MetadataId::into_stored) instead.
    #[inline]
    pub(crate) fn slot(self) -> MetadataSlot {
        self.slot
    }
}

// Hand-written rather than derived so `Debug` prints `tag`/`slot` only, never
// the brand phantom — the same reason `decl_value_id!` writes these out for the
// value ids.
impl<B: ModuleBrand> Clone for MetadataId<B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<B: ModuleBrand> Copy for MetadataId<B> {}
impl<B: ModuleBrand> PartialEq for MetadataId<B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag && self.slot == other.slot
    }
}
impl<B: ModuleBrand> Eq for MetadataId<B> {}
impl<B: ModuleBrand> core::hash::Hash for MetadataId<B> {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        self.slot.hash(state);
    }
}
impl<B: ModuleBrand> core::fmt::Debug for MetadataId<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MetadataId")
            .field("tag", &self.tag)
            .field("slot", &self.slot)
            .finish()
    }
}

// --------------------------------------------------------------------------
// Attachment kinds
// --------------------------------------------------------------------------

/// LLVM metadata attachment names with the upstream fixed set represented as
/// enum variants. Unknown `!name` attachments are valid IR and stay custom.
///
/// The fixed variants mirror the `LLVM_FIXED_MD_KIND` entries of
/// `llvm/include/llvm/IR/FixedMetadataKinds.def` (which
/// `LLVMContext::LLVMContext` includes to register the fixed kinds), listed
/// here in the `.def`'s own order. The enum is exhaustive, so a future
/// upstream addition is a breaking change every un-updated `match` reports;
/// [`Custom`](Self::Custom) carries the open remainder of the namespace, which
/// is where genuinely unbounded kinds belong.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MetadataAttachmentKind {
    Dbg,
    Tbaa,
    Prof,
    Fpmath,
    Range,
    TbaaStruct,
    InvariantLoad,
    AliasScope,
    NoAlias,
    NonTemporal,
    MemParallelLoopAccess,
    NonNull,
    Dereferenceable,
    DereferenceableOrNull,
    MakeImplicit,
    Unpredictable,
    InvariantGroup,
    Align,
    Loop,
    Type,
    SectionPrefix,
    AbsoluteSymbol,
    Associated,
    Callees,
    IrrLoop,
    AccessGroup,
    Callback,
    PreserveAccessIndex,
    VcallVisibility,
    NoUndef,
    Annotation,
    NoSanitize,
    FuncSanitize,
    Exclude,
    Memprof,
    Callsite,
    KcfiType,
    PcSections,
    DiAssignId,
    CoroOutsideFrame,
    Mmra,
    NoAliasAddrspace,
    CalleeType,
    NoFree,
    Captures,
    AllocToken,
    ImplicitRef,
    Custom(String),
}

impl MetadataAttachmentKind {
    pub fn from_name(name: &str) -> Self {
        match name {
            "dbg" => Self::Dbg,
            "tbaa" => Self::Tbaa,
            "prof" => Self::Prof,
            "fpmath" => Self::Fpmath,
            "range" => Self::Range,
            "tbaa.struct" => Self::TbaaStruct,
            "invariant.load" => Self::InvariantLoad,
            "alias.scope" => Self::AliasScope,
            "noalias" => Self::NoAlias,
            "nontemporal" => Self::NonTemporal,
            "llvm.mem.parallel_loop_access" => Self::MemParallelLoopAccess,
            "nonnull" => Self::NonNull,
            "dereferenceable" => Self::Dereferenceable,
            "dereferenceable_or_null" => Self::DereferenceableOrNull,
            "make.implicit" => Self::MakeImplicit,
            "unpredictable" => Self::Unpredictable,
            "invariant.group" => Self::InvariantGroup,
            "align" => Self::Align,
            "llvm.loop" => Self::Loop,
            "type" => Self::Type,
            "section_prefix" => Self::SectionPrefix,
            "absolute_symbol" => Self::AbsoluteSymbol,
            "associated" => Self::Associated,
            "callees" => Self::Callees,
            "irr_loop" => Self::IrrLoop,
            "llvm.access.group" => Self::AccessGroup,
            "callback" => Self::Callback,
            "llvm.preserve.access.index" => Self::PreserveAccessIndex,
            "vcall_visibility" => Self::VcallVisibility,
            "noundef" => Self::NoUndef,
            "annotation" => Self::Annotation,
            "nosanitize" => Self::NoSanitize,
            "func_sanitize" => Self::FuncSanitize,
            "exclude" => Self::Exclude,
            "memprof" => Self::Memprof,
            "callsite" => Self::Callsite,
            "kcfi_type" => Self::KcfiType,
            "pcsections" => Self::PcSections,
            "DIAssignID" => Self::DiAssignId,
            "coro.outside.frame" => Self::CoroOutsideFrame,
            "mmra" => Self::Mmra,
            "noalias.addrspace" => Self::NoAliasAddrspace,
            "callee_type" => Self::CalleeType,
            "nofree" => Self::NoFree,
            "captures" => Self::Captures,
            "alloc_token" => Self::AllocToken,
            "implicit.ref" => Self::ImplicitRef,
            other => Self::Custom(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Dbg => "dbg",
            Self::Tbaa => "tbaa",
            Self::Prof => "prof",
            Self::Fpmath => "fpmath",
            Self::Range => "range",
            Self::TbaaStruct => "tbaa.struct",
            Self::InvariantLoad => "invariant.load",
            Self::AliasScope => "alias.scope",
            Self::NoAlias => "noalias",
            Self::NonTemporal => "nontemporal",
            Self::MemParallelLoopAccess => "llvm.mem.parallel_loop_access",
            Self::NonNull => "nonnull",
            Self::Dereferenceable => "dereferenceable",
            Self::DereferenceableOrNull => "dereferenceable_or_null",
            Self::MakeImplicit => "make.implicit",
            Self::Unpredictable => "unpredictable",
            Self::InvariantGroup => "invariant.group",
            Self::Align => "align",
            Self::Loop => "llvm.loop",
            Self::Type => "type",
            Self::SectionPrefix => "section_prefix",
            Self::AbsoluteSymbol => "absolute_symbol",
            Self::Associated => "associated",
            Self::Callees => "callees",
            Self::IrrLoop => "irr_loop",
            Self::AccessGroup => "llvm.access.group",
            Self::Callback => "callback",
            Self::PreserveAccessIndex => "llvm.preserve.access.index",
            Self::VcallVisibility => "vcall_visibility",
            Self::NoUndef => "noundef",
            Self::Annotation => "annotation",
            Self::NoSanitize => "nosanitize",
            Self::FuncSanitize => "func_sanitize",
            Self::Exclude => "exclude",
            Self::Memprof => "memprof",
            Self::Callsite => "callsite",
            Self::KcfiType => "kcfi_type",
            Self::PcSections => "pcsections",
            Self::DiAssignId => "DIAssignID",
            Self::CoroOutsideFrame => "coro.outside.frame",
            Self::Mmra => "mmra",
            Self::NoAliasAddrspace => "noalias.addrspace",
            Self::CalleeType => "callee_type",
            Self::NoFree => "nofree",
            Self::Captures => "captures",
            Self::AllocToken => "alloc_token",
            Self::ImplicitRef => "implicit.ref",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// The fixed metadata-kind ID upstream assigns this attachment name, or
    /// `None` for a [`Custom`](Self::Custom) attachment (those receive
    /// context-dependent IDs past the fixed range at runtime).
    ///
    /// Values mirror the `LLVM_FIXED_MD_KIND(EnumID, Name, Value)` entries in
    /// `llvm/include/llvm/IR/FixedMetadataKinds.def`.
    pub const fn fixed_id(&self) -> Option<u32> {
        match self {
            Self::Dbg => Some(0),
            Self::Tbaa => Some(1),
            Self::Prof => Some(2),
            Self::Fpmath => Some(3),
            Self::Range => Some(4),
            Self::TbaaStruct => Some(5),
            Self::InvariantLoad => Some(6),
            Self::AliasScope => Some(7),
            Self::NoAlias => Some(8),
            Self::NonTemporal => Some(9),
            Self::MemParallelLoopAccess => Some(10),
            Self::NonNull => Some(11),
            Self::Dereferenceable => Some(12),
            Self::DereferenceableOrNull => Some(13),
            Self::MakeImplicit => Some(14),
            Self::Unpredictable => Some(15),
            Self::InvariantGroup => Some(16),
            Self::Align => Some(17),
            Self::Loop => Some(18),
            Self::Type => Some(19),
            Self::SectionPrefix => Some(20),
            Self::AbsoluteSymbol => Some(21),
            Self::Associated => Some(22),
            Self::Callees => Some(23),
            Self::IrrLoop => Some(24),
            Self::AccessGroup => Some(25),
            Self::Callback => Some(26),
            Self::PreserveAccessIndex => Some(27),
            Self::VcallVisibility => Some(28),
            Self::NoUndef => Some(29),
            Self::Annotation => Some(30),
            Self::NoSanitize => Some(31),
            Self::FuncSanitize => Some(32),
            Self::Exclude => Some(33),
            Self::Memprof => Some(34),
            Self::Callsite => Some(35),
            Self::KcfiType => Some(36),
            Self::PcSections => Some(37),
            Self::DiAssignId => Some(38),
            Self::CoroOutsideFrame => Some(39),
            Self::Mmra => Some(40),
            Self::NoAliasAddrspace => Some(41),
            Self::CalleeType => Some(42),
            Self::NoFree => Some(43),
            Self::Captures => Some(44),
            Self::AllocToken => Some(45),
            Self::ImplicitRef => Some(46),
            Self::Custom(_) => None,
        }
    }
}

/// What a specialized `DI*` field accepts.
///
/// One variant per `LLParser::parseMDField` overload (`LLParser.cpp`); the
/// overload *is* the grammar, and each carries its own rejection. The ranges
/// come from the field struct a class names in its `VISIT_MD_FIELDS` entry —
/// `LineField` is `MDUnsignedField(0, UINT32_MAX)`, `ColumnField` is
/// `(0, UINT16_MAX)`, and a bare `MDUnsignedField` may narrow further.
///
/// Exhaustive, and this type is why the rule is worth having. The parser
/// matches on this to pick a validation, and a catch-all arm would mean a field
/// kind added by a future LLVM bump silently parsed *unchecked* — which is the
/// exact divergence class this type exists to close. Exhaustiveness makes that
/// a compile error in `ll_parser.rs` instead. Adding a variant is breaking, and
/// should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataFieldKind {
    /// `MDField` — a metadata reference. `allow_null` is false where upstream
    /// writes `(/* AllowNull */ false)`, which rejects `null` with
    /// `'<name>' cannot be null`.
    Metadata { allow_null: bool },
    /// `MDStringField`. `empty_is_error` is true for `EmptyIs::Error`, which
    /// rejects `""` with `'<name>' cannot be empty`.
    MetadataString { empty_is_error: bool },
    /// `MDUnsignedField` and its `LineField` / `ColumnField` narrowings. Over
    /// `max` upstream reports `value for '<name>' too large, limit is <max>`.
    Unsigned { max: u64 },
    /// `MDSignedField`, with `too small` / `too large` at the bounds.
    Signed { min: i64, max: i64 },
    /// `MDBoolField` — `true` or `false` only.
    Bool,
    /// `MDAPSIntField` — any integer literal.
    ApsInt,
    /// `MDFieldList` — a `!{...}` tuple.
    MetadataList,
    /// `MDSignedOrMDField` — a signed literal or a metadata reference.
    SignedOrMetadata,
    /// `MDUnsignedOrMDField` — an unsigned literal or a metadata reference.
    UnsignedOrMetadata { max: u64 },
    /// `DwarfTagField` — a `DW_TAG_*` keyword or an unsigned encoding.
    DwarfTag,
    /// `DwarfAttEncodingField` — a `DW_ATE_*` keyword or an encoding.
    DwarfAttEncoding,
    /// `DwarfVirtualityField` — a `DW_VIRTUALITY_*` keyword or an encoding.
    DwarfVirtuality,
    /// `DwarfLangField` — a `DW_LANG_*` keyword or an encoding.
    DwarfLang,
    /// `DwarfSourceLangNameField` — a `DW_LNAME_*` keyword or an encoding.
    DwarfSourceLangName,
    /// `DwarfCCField` — a `DW_CC_*` keyword or an encoding.
    DwarfCc,
    /// `DwarfMacinfoTypeField` — a `DW_MACINFO_*` keyword or an encoding.
    DwarfMacinfoType,
    /// `DwarfEnumKindField` — a `DW_APPLE_ENUM_KIND_*` keyword or an encoding.
    DwarfEnumKind,
    /// `DIFlagField` — one or more `DIFlag*` names joined with `|`.
    DiFlags,
    /// `DISPFlagField` — one or more `DISPFlag*` names joined with `|`.
    DispFlags,
    /// `EmissionKindField` — `NoDebug` / `FullDebug` / `LineTablesOnly` /
    /// `DebugDirectivesOnly`, or an encoding.
    EmissionKind,
    /// `NameTableKindField` — `Default` / `GNU` / `Apple` / `None`, or an
    /// encoding.
    NameTableKind,
    /// `ChecksumKindField` — `CSK_MD5` / `CSK_SHA1` / `CSK_SHA256`.
    ChecksumKind,
    /// `FixedPointKindField` — `Binary` / `Decimal` / `Rational`, the three
    /// spellings `DIFixedPointType::getFixedPointKind` accepts.
    FixedPointKind,
}

/// One field a specialized `DI*` class declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpecializedMetadataField {
    name: &'static str,
    kind: MetadataFieldKind,
    required: bool,
}

impl SpecializedMetadataField {
    /// The spelling `PARSE_MD_FIELD` matches on.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// The value grammar, i.e. which `parseMDField` overload applies.
    pub const fn kind(self) -> MetadataFieldKind {
        self.kind
    }

    /// Whether upstream declares this `REQUIRED` rather than `OPTIONAL`.
    pub const fn is_required(self) -> bool {
        self.required
    }
}

/// Table constructor, kept short so the 239 generated rows stay readable.
const fn field(
    name: &'static str,
    kind: MetadataFieldKind,
    required: bool,
) -> SpecializedMetadataField {
    SpecializedMetadataField {
        name,
        kind,
        required,
    }
}

/// Specialized debug metadata node families accepted by LLVM's assembler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecializedMetadataKind {
    DiFile,
    DiCompileUnit,
    DiSubprogram,
    DiLocation,
    DiLocalVariable,
    DiBasicType,
    DiDerivedType,
    DiCompositeType,
    DiSubrange,
    DiNamespace,
    DiExpression,
    DiGlobalVariable,
    DiGlobalVariableExpression,
    DiSubroutineType,
    DiEnumerator,
    DiModule,
    DiTemplateTypeParameter,
    DiTemplateValueParameter,
    GenericDiNode,
    DiSubrangeType,
    DiGenericSubrange,
    DiFixedPointType,
    DiStringType,
    DiLexicalBlock,
    DiLexicalBlockFile,
    DiCommonBlock,
    DiMacro,
    DiMacroFile,
    DiLabel,
    DiObjcProperty,
    DiImportedEntity,
    DiAssignId,
}

impl SpecializedMetadataKind {
    /// Every modelled kind, in declaration order.
    ///
    /// Every `HANDLE_SPECIALIZED_MDNODE_LEAF` entry in
    /// `llvm/IR/Metadata.def` — the set is complete, not a subset.
    pub const ALL: [Self; 32] = [
        Self::DiFile,
        Self::DiCompileUnit,
        Self::DiSubprogram,
        Self::DiLocation,
        Self::DiLocalVariable,
        Self::DiBasicType,
        Self::DiDerivedType,
        Self::DiCompositeType,
        Self::DiSubrange,
        Self::DiNamespace,
        Self::DiExpression,
        Self::DiGlobalVariable,
        Self::DiGlobalVariableExpression,
        Self::DiSubroutineType,
        Self::DiEnumerator,
        Self::DiModule,
        Self::DiTemplateTypeParameter,
        Self::DiTemplateValueParameter,
        Self::GenericDiNode,
        Self::DiSubrangeType,
        Self::DiGenericSubrange,
        Self::DiFixedPointType,
        Self::DiStringType,
        Self::DiLexicalBlock,
        Self::DiLexicalBlockFile,
        Self::DiCommonBlock,
        Self::DiMacro,
        Self::DiMacroFile,
        Self::DiLabel,
        Self::DiObjcProperty,
        Self::DiImportedEntity,
        Self::DiAssignId,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "DIFile" => Self::DiFile,
            "DICompileUnit" => Self::DiCompileUnit,
            "DISubprogram" => Self::DiSubprogram,
            "DILocation" => Self::DiLocation,
            "DILocalVariable" => Self::DiLocalVariable,
            "DIBasicType" => Self::DiBasicType,
            "DIDerivedType" => Self::DiDerivedType,
            "DICompositeType" => Self::DiCompositeType,
            "DISubrange" => Self::DiSubrange,
            "DINamespace" => Self::DiNamespace,
            "DIExpression" => Self::DiExpression,
            "DIGlobalVariable" => Self::DiGlobalVariable,
            "DIGlobalVariableExpression" => Self::DiGlobalVariableExpression,
            "DISubroutineType" => Self::DiSubroutineType,
            "DIEnumerator" => Self::DiEnumerator,
            "DIModule" => Self::DiModule,
            "DITemplateTypeParameter" => Self::DiTemplateTypeParameter,
            "DITemplateValueParameter" => Self::DiTemplateValueParameter,
            "GenericDINode" => Self::GenericDiNode,
            "DISubrangeType" => Self::DiSubrangeType,
            "DIGenericSubrange" => Self::DiGenericSubrange,
            "DIFixedPointType" => Self::DiFixedPointType,
            "DIStringType" => Self::DiStringType,
            "DILexicalBlock" => Self::DiLexicalBlock,
            "DILexicalBlockFile" => Self::DiLexicalBlockFile,
            "DICommonBlock" => Self::DiCommonBlock,
            "DIMacro" => Self::DiMacro,
            "DIMacroFile" => Self::DiMacroFile,
            "DILabel" => Self::DiLabel,
            "DIObjCProperty" => Self::DiObjcProperty,
            "DIImportedEntity" => Self::DiImportedEntity,
            "DIAssignID" => Self::DiAssignId,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::DiFile => "DIFile",
            Self::DiCompileUnit => "DICompileUnit",
            Self::DiSubprogram => "DISubprogram",
            Self::DiLocation => "DILocation",
            Self::DiLocalVariable => "DILocalVariable",
            Self::DiBasicType => "DIBasicType",
            Self::DiDerivedType => "DIDerivedType",
            Self::DiCompositeType => "DICompositeType",
            Self::DiSubrange => "DISubrange",
            Self::DiNamespace => "DINamespace",
            Self::DiExpression => "DIExpression",
            Self::DiGlobalVariable => "DIGlobalVariable",
            Self::DiGlobalVariableExpression => "DIGlobalVariableExpression",
            Self::DiSubroutineType => "DISubroutineType",
            Self::DiEnumerator => "DIEnumerator",
            Self::DiModule => "DIModule",
            Self::DiTemplateTypeParameter => "DITemplateTypeParameter",
            Self::DiTemplateValueParameter => "DITemplateValueParameter",
            Self::GenericDiNode => "GenericDINode",
            Self::DiSubrangeType => "DISubrangeType",
            Self::DiGenericSubrange => "DIGenericSubrange",
            Self::DiFixedPointType => "DIFixedPointType",
            Self::DiStringType => "DIStringType",
            Self::DiLexicalBlock => "DILexicalBlock",
            Self::DiLexicalBlockFile => "DILexicalBlockFile",
            Self::DiCommonBlock => "DICommonBlock",
            Self::DiMacro => "DIMacro",
            Self::DiMacroFile => "DIMacroFile",
            Self::DiLabel => "DILabel",
            Self::DiObjcProperty => "DIObjCProperty",
            Self::DiImportedEntity => "DIImportedEntity",
            Self::DiAssignId => "DIAssignID",
        }
    }

    /// Every field this node family declares, in upstream's order.
    ///
    /// One row per entry of the matching `LLParser::parse*`'s
    /// `VISIT_MD_FIELDS` block: the spelling `PARSE_MD_FIELD` matches, the
    /// field *type* whose `LLParser::parseMDField` overload validates the
    /// value, and whether the entry is `REQUIRED` rather than `OPTIONAL`.
    ///
    /// One table rather than three: the accepted set, the required subset, and
    /// the per-field value grammar all come from the same upstream line, so
    /// keeping them apart would let them drift.
    ///
    /// [`Self::DiExpression`] and [`Self::DiAssignId`] are empty, and that is
    /// upstream's shape too — `parseDIExpression` routes to
    /// `parseDIExpressionBody` (a positional `DW_OP_*` list) and
    /// `parseDIAssignID` takes no fields at all.
    pub const fn declared_fields(self) -> &'static [SpecializedMetadataField] {
        match self {
            Self::DiLocation => {
                const {
                    &[
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "column",
                            MetadataFieldKind::Unsigned {
                                max: u16::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: false },
                            true,
                        ),
                        field(
                            "inlinedAt",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("isImplicitCode", MetadataFieldKind::Bool, false),
                        field(
                            "atomGroup",
                            MetadataFieldKind::Unsigned { max: u64::MAX },
                            false,
                        ),
                        field(
                            "atomRank",
                            MetadataFieldKind::Unsigned { max: u64::MAX },
                            false,
                        ),
                    ]
                }
            }
            Self::GenericDiNode => {
                const {
                    &[
                        field("tag", MetadataFieldKind::DwarfTag, true),
                        field(
                            "header",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field("operands", MetadataFieldKind::MetadataList, false),
                    ]
                }
            }
            Self::DiSubrangeType => {
                const {
                    &[
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "baseType",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "size",
                            MetadataFieldKind::UnsignedOrMetadata { max: u64::MAX },
                            false,
                        ),
                        field(
                            "align",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field("flags", MetadataFieldKind::DiFlags, false),
                        field("lowerBound", MetadataFieldKind::SignedOrMetadata, false),
                        field("upperBound", MetadataFieldKind::SignedOrMetadata, false),
                        field("stride", MetadataFieldKind::SignedOrMetadata, false),
                        field("bias", MetadataFieldKind::SignedOrMetadata, false),
                    ]
                }
            }
            Self::DiSubrange => {
                const {
                    &[
                        field("count", MetadataFieldKind::SignedOrMetadata, false),
                        field("lowerBound", MetadataFieldKind::SignedOrMetadata, false),
                        field("upperBound", MetadataFieldKind::SignedOrMetadata, false),
                        field("stride", MetadataFieldKind::SignedOrMetadata, false),
                    ]
                }
            }
            Self::DiGenericSubrange => {
                const {
                    &[
                        field("count", MetadataFieldKind::SignedOrMetadata, false),
                        field("lowerBound", MetadataFieldKind::SignedOrMetadata, false),
                        field("upperBound", MetadataFieldKind::SignedOrMetadata, false),
                        field("stride", MetadataFieldKind::SignedOrMetadata, false),
                    ]
                }
            }
            Self::DiEnumerator => {
                const {
                    &[
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            true,
                        ),
                        field("value", MetadataFieldKind::ApsInt, true),
                        field("isUnsigned", MetadataFieldKind::Bool, false),
                    ]
                }
            }
            Self::DiBasicType => {
                const {
                    &[
                        field("tag", MetadataFieldKind::DwarfTag, false),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "size",
                            MetadataFieldKind::UnsignedOrMetadata { max: u64::MAX },
                            false,
                        ),
                        field(
                            "align",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "dataSize",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field("encoding", MetadataFieldKind::DwarfAttEncoding, false),
                        field(
                            "num_extra_inhabitants",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field("flags", MetadataFieldKind::DiFlags, false),
                    ]
                }
            }
            Self::DiFixedPointType => {
                const {
                    &[
                        field("tag", MetadataFieldKind::DwarfTag, false),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "size",
                            MetadataFieldKind::UnsignedOrMetadata { max: u64::MAX },
                            false,
                        ),
                        field(
                            "align",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field("encoding", MetadataFieldKind::DwarfAttEncoding, false),
                        field("flags", MetadataFieldKind::DiFlags, false),
                        field("kind", MetadataFieldKind::FixedPointKind, false),
                        field(
                            "factor",
                            MetadataFieldKind::Signed {
                                min: i64::MIN,
                                max: i64::MAX,
                            },
                            false,
                        ),
                        field("numerator", MetadataFieldKind::ApsInt, false),
                        field("denominator", MetadataFieldKind::ApsInt, false),
                    ]
                }
            }
            Self::DiStringType => {
                const {
                    &[
                        field("tag", MetadataFieldKind::DwarfTag, false),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "stringLength",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "stringLengthExpression",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "stringLocationExpression",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "size",
                            MetadataFieldKind::UnsignedOrMetadata { max: u64::MAX },
                            false,
                        ),
                        field(
                            "align",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field("encoding", MetadataFieldKind::DwarfAttEncoding, false),
                    ]
                }
            }
            Self::DiDerivedType => {
                const {
                    &[
                        field("tag", MetadataFieldKind::DwarfTag, true),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "baseType",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field(
                            "size",
                            MetadataFieldKind::UnsignedOrMetadata { max: u64::MAX },
                            false,
                        ),
                        field(
                            "align",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "offset",
                            MetadataFieldKind::UnsignedOrMetadata { max: u64::MAX },
                            false,
                        ),
                        field("flags", MetadataFieldKind::DiFlags, false),
                        field(
                            "extraData",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "dwarfAddressSpace",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "annotations",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("ptrAuthKey", MetadataFieldKind::Unsigned { max: 7 }, false),
                        field(
                            "ptrAuthIsAddressDiscriminated",
                            MetadataFieldKind::Bool,
                            false,
                        ),
                        field(
                            "ptrAuthExtraDiscriminator",
                            MetadataFieldKind::Unsigned { max: 0xffff },
                            false,
                        ),
                        field("ptrAuthIsaPointer", MetadataFieldKind::Bool, false),
                        field(
                            "ptrAuthAuthenticatesNullValues",
                            MetadataFieldKind::Bool,
                            false,
                        ),
                    ]
                }
            }
            Self::DiCompositeType => {
                const {
                    &[
                        field("tag", MetadataFieldKind::DwarfTag, true),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "baseType",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "size",
                            MetadataFieldKind::UnsignedOrMetadata { max: u64::MAX },
                            false,
                        ),
                        field(
                            "align",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "offset",
                            MetadataFieldKind::UnsignedOrMetadata { max: u64::MAX },
                            false,
                        ),
                        field("flags", MetadataFieldKind::DiFlags, false),
                        field(
                            "elements",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("runtimeLang", MetadataFieldKind::DwarfLang, false),
                        field("enumKind", MetadataFieldKind::DwarfEnumKind, false),
                        field(
                            "vtableHolder",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "templateParams",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "identifier",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "discriminator",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "dataLocation",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "associated",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "allocated",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("rank", MetadataFieldKind::SignedOrMetadata, false),
                        field(
                            "annotations",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "num_extra_inhabitants",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "specification",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "bitStride",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                    ]
                }
            }
            Self::DiSubroutineType => {
                const {
                    &[
                        field("flags", MetadataFieldKind::DiFlags, false),
                        field("cc", MetadataFieldKind::DwarfCc, false),
                        field(
                            "types",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                    ]
                }
            }
            Self::DiFile => {
                const {
                    &[
                        field(
                            "filename",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            true,
                        ),
                        field(
                            "directory",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            true,
                        ),
                        field("checksumkind", MetadataFieldKind::ChecksumKind, false),
                        field(
                            "checksum",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "source",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                    ]
                }
            }
            Self::DiCompileUnit => {
                const {
                    &[
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: false },
                            true,
                        ),
                        field("language", MetadataFieldKind::DwarfLang, false),
                        field(
                            "sourceLanguageName",
                            MetadataFieldKind::DwarfSourceLangName,
                            false,
                        ),
                        field(
                            "sourceLanguageVersion",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "producer",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field("isOptimized", MetadataFieldKind::Bool, false),
                        field(
                            "flags",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "runtimeVersion",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "splitDebugFilename",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field("emissionKind", MetadataFieldKind::EmissionKind, false),
                        field(
                            "enums",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "retainedTypes",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "globals",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "imports",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "macros",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "dwoId",
                            MetadataFieldKind::Unsigned { max: u64::MAX },
                            false,
                        ),
                        field("splitDebugInlining", MetadataFieldKind::Bool, false),
                        field("debugInfoForProfiling", MetadataFieldKind::Bool, false),
                        field("nameTableKind", MetadataFieldKind::NameTableKind, false),
                        field("rangesBaseAddress", MetadataFieldKind::Bool, false),
                        field(
                            "sysroot",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "sdk",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                    ]
                }
            }
            Self::DiSubprogram => {
                const {
                    &[
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "linkageName",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "type",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("isLocal", MetadataFieldKind::Bool, false),
                        field("isDefinition", MetadataFieldKind::Bool, false),
                        field(
                            "scopeLine",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "containingType",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("virtuality", MetadataFieldKind::DwarfVirtuality, false),
                        field(
                            "virtualIndex",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "thisAdjustment",
                            MetadataFieldKind::Signed {
                                min: i32::MIN as i64,
                                max: i32::MAX as i64,
                            },
                            false,
                        ),
                        field("flags", MetadataFieldKind::DiFlags, false),
                        field("spFlags", MetadataFieldKind::DispFlags, false),
                        field("isOptimized", MetadataFieldKind::Bool, false),
                        field(
                            "unit",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "templateParams",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "declaration",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "retainedNodes",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "thrownTypes",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "annotations",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "targetFuncName",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field("keyInstructions", MetadataFieldKind::Bool, false),
                    ]
                }
            }
            Self::DiLexicalBlock => {
                const {
                    &[
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: false },
                            true,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "column",
                            MetadataFieldKind::Unsigned {
                                max: u16::MAX as u64,
                            },
                            false,
                        ),
                    ]
                }
            }
            Self::DiLexicalBlockFile => {
                const {
                    &[
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: false },
                            true,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "discriminator",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            true,
                        ),
                    ]
                }
            }
            Self::DiCommonBlock => {
                const {
                    &[
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field(
                            "declaration",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                    ]
                }
            }
            Self::DiNamespace => {
                const {
                    &[
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field("exportSymbols", MetadataFieldKind::Bool, false),
                    ]
                }
            }
            Self::DiMacro => {
                const {
                    &[
                        field("type", MetadataFieldKind::DwarfMacinfoType, true),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            true,
                        ),
                        field(
                            "value",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                    ]
                }
            }
            Self::DiMacroFile => {
                const {
                    &[
                        field("type", MetadataFieldKind::DwarfMacinfoType, false),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field(
                            "nodes",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                    ]
                }
            }
            Self::DiModule => {
                const {
                    &[
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            true,
                        ),
                        field(
                            "configMacros",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "includePath",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "apinotes",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field("isDecl", MetadataFieldKind::Bool, false),
                    ]
                }
            }
            Self::DiTemplateTypeParameter => {
                const {
                    &[
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "type",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field("defaulted", MetadataFieldKind::Bool, false),
                    ]
                }
            }
            Self::DiTemplateValueParameter => {
                const {
                    &[
                        field("tag", MetadataFieldKind::DwarfTag, false),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "type",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("defaulted", MetadataFieldKind::Bool, false),
                        field(
                            "value",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                    ]
                }
            }
            Self::DiGlobalVariable => {
                const {
                    &[
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: true,
                            },
                            false,
                        ),
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "linkageName",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "type",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("isLocal", MetadataFieldKind::Bool, false),
                        field("isDefinition", MetadataFieldKind::Bool, false),
                        field(
                            "templateParams",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "declaration",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "align",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "annotations",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                    ]
                }
            }
            Self::DiLocalVariable => {
                const {
                    &[
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: false },
                            true,
                        ),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "arg",
                            MetadataFieldKind::Unsigned {
                                max: u16::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "type",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field("flags", MetadataFieldKind::DiFlags, false),
                        field(
                            "align",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "annotations",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                    ]
                }
            }
            Self::DiLabel => {
                const {
                    &[
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: false },
                            true,
                        ),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            true,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            true,
                        ),
                        field(
                            "column",
                            MetadataFieldKind::Unsigned {
                                max: u16::MAX as u64,
                            },
                            false,
                        ),
                        field("isArtificial", MetadataFieldKind::Bool, false),
                        field(
                            "coroSuspendIdx",
                            MetadataFieldKind::Unsigned { max: u64::MAX },
                            false,
                        ),
                    ]
                }
            }
            Self::DiGlobalVariableExpression => {
                const {
                    &[
                        field(
                            "var",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field(
                            "expr",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                    ]
                }
            }
            Self::DiObjcProperty => {
                const {
                    &[
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "setter",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "getter",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "attributes",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "type",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                    ]
                }
            }
            Self::DiImportedEntity => {
                const {
                    &[
                        field("tag", MetadataFieldKind::DwarfTag, true),
                        field(
                            "scope",
                            MetadataFieldKind::Metadata { allow_null: true },
                            true,
                        ),
                        field(
                            "entity",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "file",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                        field(
                            "line",
                            MetadataFieldKind::Unsigned {
                                max: u32::MAX as u64,
                            },
                            false,
                        ),
                        field(
                            "name",
                            MetadataFieldKind::MetadataString {
                                empty_is_error: false,
                            },
                            false,
                        ),
                        field(
                            "elements",
                            MetadataFieldKind::Metadata { allow_null: true },
                            false,
                        ),
                    ]
                }
            }
            Self::DiExpression | Self::DiAssignId => &[],
        }
    }

    /// The declaration for `name`, if this family declares one.
    pub fn field(self, name: &str) -> Option<SpecializedMetadataField> {
        self.declared_fields()
            .iter()
            .copied()
            .find(|field| field.name() == name)
    }

    /// Whether this family declares a field called `name`.
    ///
    /// The `PARSE_MD_FIELD` match; a `false` here is what upstream reports as
    /// `invalid field '<name>'`.
    pub fn accepts_field(self, name: &str) -> bool {
        self.field(name).is_some()
    }

    /// The fields upstream marks `REQUIRED`, checked at the closing `)` by
    /// `REQUIRE_FIELD`.
    pub fn required_fields(self) -> impl Iterator<Item = SpecializedMetadataField> {
        self.declared_fields()
            .iter()
            .copied()
            .filter(|field| field.is_required())
    }
}

// --------------------------------------------------------------------------
// Debug-info flag bitfields
// --------------------------------------------------------------------------

/// The bit a `DIFlag*` / `DISPFlag*` spelling names, or `FlagZero` when the
/// table does not carry it — `StringSwitch<…>(Flag).Case(…).Default(FlagZero)`
/// under both `DINode::getFlag` and `DISubprogram::getFlag`.
fn flag_bit(lookup: fn(&str) -> Option<u32>, spelling: &str) -> u32 {
    lookup(spelling).unwrap_or(0)
}

/// `DINode::DIFlags` (`include/llvm/IR/DebugInfoMetadata.h`) — the `flags:`
/// field of a specialized `DI*` node, as one bitfield rather than the source
/// text that produced it.
///
/// The three routines below are `DINode::getFlag`, `DINode::getFlagString` and
/// `DINode::splitFlags` (`lib/IR/DebugInfoMetadata.cpp`); between them they are
/// why `flags: 4 | DIFlagPublic` and `flags: DIFlagProtected | DIFlagPrivate`
/// are read as bit sets and printed back canonically instead of echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DiFlags(u32);

impl DiFlags {
    /// `DINode::FlagZero`.
    pub const ZERO: Self = Self(0);

    /// The raw bitfield.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Wrap a raw bitfield. The unsigned integer term `parseMDField`'s
    /// `parseFlag` accepts arrives this way.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Whether every bit of `other` is set. Upstream spells this
    /// `(Flags & Other) == Other`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// `Flags |= Val`, the accumulator of `parseMDField`'s `do`/`while` loop.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Mirrors `DINode::getFlag`: the flag a `DIFlag*` spelling names, or
    /// [`Self::ZERO`] for one the table does not carry. Callers reject a zero
    /// result, which is how `DIFlagZero` itself is refused upstream.
    pub fn get_flag(spelling: &str) -> Self {
        Self(flag_bit(crate::dwarf::di_flag, spelling))
    }

    /// Mirrors `DINode::getFlagString`, whose empty `StringRef` is [`None`]
    /// here.
    pub fn flag_string(self) -> Option<&'static str> {
        crate::dwarf::di_flag_string(self.0)
    }

    /// Mirrors `DINode::splitFlags`: push each component onto `split` and
    /// return the unrecognised remainder, which the printer emits as a
    /// trailing number.
    ///
    /// The two composite fields come first and in upstream's order, with
    /// upstream's own comment for the first: emit `DIFlagPublic` and not
    /// `DIFlagPrivate | DIFlagProtected`.
    pub fn split_flags(mut self, split: &mut Vec<Self>) -> Self {
        let bit = |name: &str| Self(flag_bit(crate::dwarf::di_flag, name));
        let accessibility = bit("DIFlagPrivate")
            .union(bit("DIFlagProtected"))
            .union(bit("DIFlagPublic"));
        let ptr_to_member_rep = bit("DIFlagSingleInheritance")
            .union(bit("DIFlagMultipleInheritance"))
            .union(bit("DIFlagVirtualInheritance"));
        let indirect_virtual_base = bit("DIFlagIndirectVirtualBase");

        let a = self.0 & accessibility.0;
        if a != 0 {
            if a == bit("DIFlagPrivate").0 {
                split.push(bit("DIFlagPrivate"));
            } else if a == bit("DIFlagProtected").0 {
                split.push(bit("DIFlagProtected"));
            } else {
                split.push(bit("DIFlagPublic"));
            }
            self.0 &= !a;
        }
        let r = self.0 & ptr_to_member_rep.0;
        if r != 0 {
            if r == bit("DIFlagSingleInheritance").0 {
                split.push(bit("DIFlagSingleInheritance"));
            } else if r == bit("DIFlagMultipleInheritance").0 {
                split.push(bit("DIFlagMultipleInheritance"));
            } else {
                split.push(bit("DIFlagVirtualInheritance"));
            }
            self.0 &= !r;
        }
        if self.contains(indirect_virtual_base) {
            self.0 &= !indirect_virtual_base.0;
            split.push(indirect_virtual_base);
        }
        // `#define HANDLE_DI_FLAG(ID, NAME) if (DIFlags Bit = Flags & Flag##NAME)
        //  { SplitFlags.push_back(Bit); Flags &= ~Bit; }` over the whole `.def`,
        // in its order — which is this table's order.
        for &(_, value) in crate::dwarf::DI_FLAGS {
            let bit = self.0 & value;
            if bit != 0 {
                split.push(Self(bit));
                self.0 &= !bit;
            }
        }
        self
    }
}

/// `DISubprogram::DISPFlags` (`include/llvm/IR/DebugInfoMetadata.h`) — the
/// `spFlags:` field of `!DISubprogram`, as one bitfield.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DispFlags(u32);

impl DispFlags {
    /// `DISubprogram::SPFlagZero`.
    pub const ZERO: Self = Self(0);

    /// The raw bitfield.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Wrap a raw bitfield.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// `Flags |= Val`.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// `DISubprogram::SPFlagDefinition`, the one bit a caller outside this
    /// module asks about: `parseDISubprogram`'s `IsDefinition` guard reads the
    /// computed `SPFlags`, not the `isDefinition:` field.
    pub fn definition() -> Self {
        Self(flag_bit(crate::dwarf::disp_flag, "DISPFlagDefinition"))
    }

    /// Mirrors `DISubprogram::getFlag`.
    pub fn get_flag(spelling: &str) -> Self {
        Self(flag_bit(crate::dwarf::disp_flag, spelling))
    }

    /// Mirrors `DISubprogram::getFlagString`. Its `case SPFlagVirtuality:
    /// return "";` arm — added to appease a warning, for a value no
    /// `HANDLE_DISP_FLAG` row carries — falls out of the table lookup as
    /// [`None`].
    pub fn flag_string(self) -> Option<&'static str> {
        crate::dwarf::disp_flag_string(self.0)
    }

    /// Mirrors `DISubprogram::splitFlags`, which is the bare `HANDLE_DISP_FLAG`
    /// loop: upstream's comment notes that the only multi-bit field is
    /// virtuality and all its values are single-bit, so the right behaviour
    /// falls out with no special case.
    pub fn split_flags(mut self, split: &mut Vec<Self>) -> Self {
        for &(_, value) in crate::dwarf::DISP_FLAGS {
            let bit = self.0 & value;
            if bit != 0 {
                split.push(Self(bit));
                self.0 &= !bit;
            }
        }
        self
    }
}

// --------------------------------------------------------------------------
// Specialized `DI*` node fields
// --------------------------------------------------------------------------

/// A typed field value inside a specialized `DI*` node.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub enum MetadataFieldValue<B: ModuleBrand> {
    Null,
    Bool(bool),
    Integer(i128),
    String(String),
    Enum(String),
    /// A `flags:` bitfield. Its terms are OR-ed at parse time, exactly as
    /// `parseMDField(DIFlagField&)`'s `do`/`while` loop does, so the written
    /// order, duplicates and alias spellings do not survive into storage.
    DiFlags(DiFlags),
    /// A `spFlags:` bitfield, the `DISPFlagField` twin.
    DispFlags(DispFlags),
    Metadata(MetadataId<B>),
    MetadataList(Vec<MetadataId<B>>),
}

impl<B: ModuleBrand> MetadataFieldValue<B> {
    /// Crate-internal: tag-check every metadata reference against `owner` and
    /// hand back the storage form.
    fn into_stored(self, owner: ModuleId) -> IrResult<MetadataFieldValue<StoredBrand>> {
        Ok(match self {
            Self::Null => MetadataFieldValue::Null,
            Self::Bool(b) => MetadataFieldValue::Bool(b),
            Self::Integer(i) => MetadataFieldValue::Integer(i),
            Self::String(s) => MetadataFieldValue::String(s),
            Self::Enum(s) => MetadataFieldValue::Enum(s),
            Self::DiFlags(f) => MetadataFieldValue::DiFlags(f),
            Self::DispFlags(f) => MetadataFieldValue::DispFlags(f),
            Self::Metadata(id) => MetadataFieldValue::Metadata(id.into_stored(owner)?),
            Self::MetadataList(ids) => MetadataFieldValue::MetadataList(
                ids.into_iter()
                    .map(|id| id.into_stored(owner))
                    .collect::<IrResult<Vec<_>>>()?,
            ),
        })
    }

    /// Crate-internal: retag stored content back into the caller's brand.
    fn from_stored(stored: &MetadataFieldValue<StoredBrand>) -> Self {
        match stored {
            MetadataFieldValue::Null => Self::Null,
            MetadataFieldValue::Bool(b) => Self::Bool(*b),
            MetadataFieldValue::Integer(i) => Self::Integer(*i),
            MetadataFieldValue::String(s) => Self::String(s.clone()),
            MetadataFieldValue::Enum(s) => Self::Enum(s.clone()),
            MetadataFieldValue::DiFlags(f) => Self::DiFlags(*f),
            MetadataFieldValue::DispFlags(f) => Self::DispFlags(*f),
            MetadataFieldValue::Metadata(id) => Self::Metadata(MetadataId::from_stored(*id)),
            MetadataFieldValue::MetadataList(ids) => Self::MetadataList(
                ids.iter()
                    .map(|id| MetadataId::from_stored(*id))
                    .collect::<Vec<_>>(),
            ),
        }
    }
}

/// One `name: value` pair in a specialized `DI*` node.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct MetadataField<B: ModuleBrand> {
    name: String,
    value: MetadataFieldValue<B>,
}

impl<B: ModuleBrand> MetadataField<B> {
    pub fn new<Name>(name: Name, value: MetadataFieldValue<B>) -> Self
    where
        Name: Into<String>,
    {
        Self {
            name: name.into(),
            value,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &MetadataFieldValue<B> {
        &self.value
    }

    fn into_stored(self, owner: ModuleId) -> IrResult<MetadataField<StoredBrand>> {
        Ok(MetadataField {
            name: self.name,
            value: self.value.into_stored(owner)?,
        })
    }

    fn from_stored(stored: &MetadataField<StoredBrand>) -> Self {
        Self {
            name: stored.name.clone(),
            value: MetadataFieldValue::from_stored(&stored.value),
        }
    }
}

/// One element of a `DIExpression` body.
///
/// Upstream stores these as `uint64_t` encodings — `DIExpression`'s `Elements`,
/// filled by `LLParser::parseDIExpressionBody` (`LLParser.cpp`) through
/// `dwarf::getOperationEncoding` / `getAttributeEncoding`. llvmkit keeps the
/// **source spelling** instead, and recovers the encoding on demand through
/// [`Self::element`]: [`crate::dwarf`] is a drift-locked transcription of
/// `Dwarf.def`, so that mapping is total for every spelling the parser
/// accepts — it rejects one the tables do not carry, exactly as upstream does.
///
/// What the spelling model still costs is *normalisation*: a numerically
/// written element such as `!DIExpression(15)` stays a [`Self::Literal`] and
/// prints back as `15`, where `llvm-dis` prints the operation name that value
/// encodes. That direction is a separate recorded difference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DwarfExpressionOperand {
    /// A `DW_OP_*` or `DW_ATE_*` keyword, kept as written.
    Operation(String),
    /// A literal unsigned element. Upstream rejects a signed or `> u64::MAX`
    /// element in `parseDIExpressionBody`; so does the parser here.
    Literal(u64),
}

impl DwarfExpressionOperand {
    /// The `uint64_t` this operand contributes to upstream's
    /// `DIExpression::Elements`.
    ///
    /// `None` only for an [`Self::Operation`] spelling neither
    /// [`crate::dwarf::operation_encoding`] nor
    /// [`crate::dwarf::attribute_encoding`] carries — unreachable from the
    /// parser, which rejects such a spelling, and reachable only by building
    /// the node through the IR API.
    pub fn element(&self) -> Option<u64> {
        match self {
            Self::Literal(value) => Some(*value),
            Self::Operation(name) => crate::dwarf::operation_encoding(name)
                .or_else(|| crate::dwarf::attribute_encoding(name))
                .map(u64::from),
        }
    }
}

/// The `uint64_t` element list an operand list stands for — upstream's
/// `DIExpression::getElements()`.
///
/// `None` when any operand fails [`DwarfExpressionOperand::element`].
pub fn expression_elements(operands: &[DwarfExpressionOperand]) -> Option<Vec<u64>> {
    operands
        .iter()
        .map(DwarfExpressionOperand::element)
        .collect()
}

/// The canonical `DW_OP_*` spelling for an element, if it is one.
///
/// llvmkit's `Dwarf.def` transcription is a name/encoding *table* rather than
/// a set of named C++ constants, so the ports below switch on the spelling
/// where upstream switches on `dwarf::DW_OP_*`. The two are the same set: the
/// table is the `.def` file, and `dwarf_def_drift.rs` keeps it so.
fn expression_operation_name(element: u64) -> Option<&'static str> {
    u32::try_from(element)
        .ok()
        .and_then(crate::dwarf::operation_encoding_string)
}

/// `name` is `DW_OP_reg0` … `DW_OP_reg31` — upstream's
/// `Op >= dwarf::DW_OP_reg0 && Op <= dwarf::DW_OP_reg31`. `DW_OP_regx` sits
/// outside that range upstream, and its `x` suffix is what excludes it here.
fn is_numbered_register_operation(name: &str) -> bool {
    numbered_operation_suffix(name, "DW_OP_reg").is_some_and(|number| number <= 31)
}

/// `name` is `DW_OP_breg0` … `DW_OP_breg31`, upstream's second range.
/// `DW_OP_bregx` is likewise excluded.
fn is_numbered_base_register_operation(name: &str) -> bool {
    numbered_operation_suffix(name, "DW_OP_breg").is_some_and(|number| number <= 31)
}

fn numbered_operation_suffix(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.parse::<u32>().ok()
}

/// The number of elements one expression operand occupies — the operation
/// itself plus its arguments.
///
/// Ports `DIExpression::ExprOperand::getSize` (`DebugInfoMetadata.cpp`).
pub fn expression_operand_size(element: u64) -> usize {
    let Some(name) = expression_operation_name(element) else {
        return 1;
    };
    if is_numbered_base_register_operation(name) {
        return 2;
    }
    match name {
        "DW_OP_LLVM_convert"
        | "DW_OP_LLVM_fragment"
        | "DW_OP_LLVM_extract_bits_sext"
        | "DW_OP_LLVM_extract_bits_zext"
        | "DW_OP_bregx" => 3,
        "DW_OP_constu"
        | "DW_OP_consts"
        | "DW_OP_deref_size"
        | "DW_OP_plus_uconst"
        | "DW_OP_LLVM_tag_offset"
        | "DW_OP_LLVM_entry_value"
        | "DW_OP_LLVM_arg"
        | "DW_OP_regx" => 2,
        _ => 1,
    }
}

/// Whether an element list is a well-formed `DIExpression` body.
///
/// Ports `DIExpression::isValid` (`DebugInfoMetadata.cpp`) element for
/// element: upstream walks `expr_op_begin()` … `expr_op_end()` over a
/// `uint64_t` array, and this walks the same array by index, so upstream's
/// `I->get() + I->getSize()` reads here as `index + size`.
pub fn expression_is_valid(elements: &[u64]) -> bool {
    let mut index = 0;
    while index < elements.len() {
        let operation = elements[index];
        let size = expression_operand_size(operation);
        // Check that there is space for the operand.
        if index + size > elements.len() {
            return false;
        }

        let name = expression_operation_name(operation);
        if name.is_some_and(|name| {
            is_numbered_register_operation(name) || is_numbered_base_register_operation(name)
        }) {
            return true;
        }

        // Check that the operand is valid.
        match name {
            // A fragment operator must appear at the end.
            Some("DW_OP_LLVM_fragment") => return index + size == elements.len(),
            // Must be the last one or followed by a DW_OP_LLVM_fragment.
            Some("DW_OP_stack_value") => {
                if index + size != elements.len()
                    && expression_operation_name(elements[index + size])
                        != Some("DW_OP_LLVM_fragment")
                {
                    return false;
                }
            }
            // Must be more than one implicit element on the stack.
            Some("DW_OP_swap") => {
                if elements.len() == 1 {
                    return false;
                }
            }
            // An entry value operator must appear at the beginning or
            // immediately following `DW_OP_LLVM_arg 0`, and the number of
            // operations it covers can currently only be 1, because only
            // entry values of a simple register location are supported.
            Some("DW_OP_LLVM_entry_value") => {
                let mut first = 0;
                if expression_operation_name(elements[0]) == Some("DW_OP_LLVM_arg")
                    && elements.get(1) == Some(&0)
                {
                    first = expression_operand_size(elements[0]);
                }
                return index == first && elements[index + 1] == 1;
            }
            Some(
                "DW_OP_LLVM_implicit_pointer"
                | "DW_OP_LLVM_convert"
                | "DW_OP_LLVM_arg"
                | "DW_OP_LLVM_tag_offset"
                | "DW_OP_LLVM_extract_bits_sext"
                | "DW_OP_LLVM_extract_bits_zext"
                | "DW_OP_constu"
                | "DW_OP_plus_uconst"
                | "DW_OP_plus"
                | "DW_OP_minus"
                | "DW_OP_mul"
                | "DW_OP_div"
                | "DW_OP_mod"
                | "DW_OP_or"
                | "DW_OP_and"
                | "DW_OP_xor"
                | "DW_OP_shl"
                | "DW_OP_shr"
                | "DW_OP_shra"
                | "DW_OP_deref"
                | "DW_OP_deref_size"
                | "DW_OP_xderef"
                | "DW_OP_lit0"
                | "DW_OP_not"
                | "DW_OP_dup"
                | "DW_OP_regx"
                | "DW_OP_bregx"
                | "DW_OP_push_object_address"
                | "DW_OP_over"
                | "DW_OP_rot"
                | "DW_OP_consts"
                | "DW_OP_eq"
                | "DW_OP_ne"
                | "DW_OP_gt"
                | "DW_OP_ge"
                | "DW_OP_lt"
                | "DW_OP_le"
                | "DW_OP_neg"
                | "DW_OP_abs",
            ) => {}
            _ => return false,
        }
        index += size;
    }
    true
}

/// The body of a specialized `DI*` node.
///
/// Two shapes, because upstream has two: every class with a `VISIT_MD_FIELDS`
/// block carries `name: value` pairs, while `DIExpression` — which
/// `LLParser::parseDIExpression` routes to `parseDIExpressionBody`, not to
/// `PARSE_MD_FIELDS` — carries a positional operation list. Modelling that as
/// one enum rather than two vectors is what keeps "a `DIExpression` with named
/// fields" unrepresentable (D1).
///
/// Derives [`Branded`] rather than the std traits: a std `derive` would bound
/// `B`, which a bare brand does not satisfy.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub enum SpecializedMetadataBody<B: ModuleBrand> {
    /// `name: value` pairs, validated against
    /// [`SpecializedMetadataKind::declared_fields`].
    Fields(Vec<MetadataField<B>>),
    /// A positional DWARF operation list. [`SpecializedMetadataKind::DiExpression`]
    /// only.
    Expression(Vec<DwarfExpressionOperand>),
}

/// Stored specialized node. Field order is significant and mirrors source.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct SpecializedMetadataNode<B: ModuleBrand> {
    distinct: bool,
    kind: SpecializedMetadataKind,
    body: SpecializedMetadataBody<B>,
}

impl<B: ModuleBrand> SpecializedMetadataNode<B> {
    /// A node of `kind` with an empty body.
    ///
    /// The body shape follows the kind:
    /// [`SpecializedMetadataKind::DiExpression`] starts as an empty
    /// [`SpecializedMetadataBody::Expression`], every other kind as an empty
    /// [`SpecializedMetadataBody::Fields`].
    pub fn new(kind: SpecializedMetadataKind) -> Self {
        let body = match kind {
            SpecializedMetadataKind::DiExpression => {
                SpecializedMetadataBody::Expression(Vec::new())
            }
            _ => SpecializedMetadataBody::Fields(Vec::new()),
        };
        Self {
            distinct: false,
            kind,
            body,
        }
    }

    /// Mark the node `distinct`. Default off.
    #[must_use]
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    /// Append one `name: value` field.
    ///
    /// Ignored for [`SpecializedMetadataKind::DiExpression`], whose body is a
    /// positional operation list — that kind declares no fields at all
    /// ([`SpecializedMetadataKind::declared_fields`] is empty for it), so there is no
    /// field it could legitimately carry.
    #[must_use]
    pub fn field(mut self, field: MetadataField<B>) -> Self {
        if let SpecializedMetadataBody::Fields(fields) = &mut self.body {
            fields.push(field);
        }
        self
    }

    /// Append several `name: value` fields. Same `DIExpression` caveat as
    /// [`Self::field`].
    #[must_use]
    pub fn with_fields<Fields>(mut self, fields: Fields) -> Self
    where
        Fields: IntoIterator<Item = MetadataField<B>>,
    {
        if let SpecializedMetadataBody::Fields(existing) = &mut self.body {
            existing.extend(fields);
        }
        self
    }

    /// Append several positional `DIExpression` operands. Ignored for every
    /// other kind, which carries fields instead.
    #[must_use]
    pub fn with_expression_operands<Operands>(mut self, operands: Operands) -> Self
    where
        Operands: IntoIterator<Item = DwarfExpressionOperand>,
    {
        if let SpecializedMetadataBody::Expression(existing) = &mut self.body {
            existing.extend(operands);
        }
        self
    }

    pub const fn is_distinct(&self) -> bool {
        self.distinct
    }

    pub const fn kind(&self) -> SpecializedMetadataKind {
        self.kind
    }

    /// This node's body.
    pub const fn body(&self) -> &SpecializedMetadataBody<B> {
        &self.body
    }

    /// The `name: value` fields, or an empty slice for a `DIExpression`.
    pub fn fields(&self) -> &[MetadataField<B>] {
        match &self.body {
            SpecializedMetadataBody::Fields(fields) => fields,
            SpecializedMetadataBody::Expression(_) => &[],
        }
    }

    /// The positional `DIExpression` operands, or an empty slice for every
    /// other kind.
    pub fn expression_operands(&self) -> &[DwarfExpressionOperand] {
        match &self.body {
            SpecializedMetadataBody::Expression(operands) => operands,
            SpecializedMetadataBody::Fields(_) => &[],
        }
    }

    pub(crate) fn into_stored(
        self,
        owner: ModuleId,
    ) -> IrResult<SpecializedMetadataNode<StoredBrand>> {
        Ok(SpecializedMetadataNode {
            distinct: self.distinct,
            kind: self.kind,
            body: match self.body {
                SpecializedMetadataBody::Fields(fields) => SpecializedMetadataBody::Fields(
                    fields
                        .into_iter()
                        .map(|field| field.into_stored(owner))
                        .collect::<IrResult<Vec<_>>>()?,
                ),
                // Operands carry no metadata reference, so there is no tag to
                // check — the conversion is a move.
                SpecializedMetadataBody::Expression(operands) => {
                    SpecializedMetadataBody::Expression(operands)
                }
            },
        })
    }

    fn from_stored(stored: &SpecializedMetadataNode<StoredBrand>) -> Self {
        Self {
            distinct: stored.distinct,
            kind: stored.kind,
            body: match &stored.body {
                SpecializedMetadataBody::Fields(fields) => SpecializedMetadataBody::Fields(
                    fields.iter().map(MetadataField::from_stored).collect(),
                ),
                SpecializedMetadataBody::Expression(operands) => {
                    SpecializedMetadataBody::Expression(operands.clone())
                }
            },
        }
    }
}

// --------------------------------------------------------------------------
// Debug records
// --------------------------------------------------------------------------

/// Metadata operand used by new-format `#dbg_*` records. Operands are stored by
/// id so the record remains lifetime-free inside instruction storage.
#[derive(Branded)]
pub enum DebugMetadataOperand<B: ModuleBrand> {
    Metadata(MetadataId<B>),
    Value(ValueId<B>),
}

impl<B: ModuleBrand> DebugMetadataOperand<B> {
    fn into_stored(self, owner: ModuleId) -> IrResult<DebugMetadataOperand<StoredBrand>> {
        Ok(match self {
            Self::Metadata(id) => DebugMetadataOperand::Metadata(id.into_stored(owner)?),
            Self::Value(id) => DebugMetadataOperand::Value(value_id_into_stored(id, owner)?),
        })
    }

    fn from_stored(stored: DebugMetadataOperand<StoredBrand>) -> Self {
        match stored {
            DebugMetadataOperand::Metadata(id) => Self::Metadata(MetadataId::from_stored(id)),
            DebugMetadataOperand::Value(id) => Self::Value(value_id_from_stored(id)),
        }
    }
}

impl DebugMetadataOperand<StoredBrand> {
    pub(crate) fn value_slot(self) -> Option<ValueSlot> {
        match self {
            Self::Value(id) => Some(id.slot()),
            Self::Metadata(_) => None,
        }
    }

    fn replace_value_slot(&mut self, from: ValueSlot, to: ValueSlot) {
        if let Self::Value(id) = *self
            && id.slot() == from
        {
            *self = Self::Value(ValueId::from_raw(id.tag(), to));
        }
    }
}

/// Tag-check a caller-supplied value id on the debug-record path. The metadata
/// twin of [`MetadataId::into_stored`] — a `#dbg_value` operand names a value,
/// and a value from another module is the same defect as a node from another
/// module.
fn value_id_into_stored<B: ModuleBrand>(
    id: ValueId<B>,
    owner: ModuleId,
) -> IrResult<ValueId<StoredBrand>> {
    if id.tag() != owner {
        return Err(IrError::ForeignValueId);
    }
    Ok(ValueId::from_raw(id.tag(), id.slot()))
}

fn value_id_from_stored<B: ModuleBrand>(stored: ValueId<StoredBrand>) -> ValueId<B> {
    ValueId::from_raw(stored.tag(), stored.slot())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugVariableRecordKind {
    Declare,
    Value,
    Assign,
    DeclareValue,
}

impl DebugVariableRecordKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Declare => "declare",
            Self::Value => "value",
            Self::Assign => "assign",
            Self::DeclareValue => "declare_value",
        }
    }
}

#[derive(Branded)]
#[branded(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugVariableRecord<B: ModuleBrand> {
    kind: DebugVariableRecordKind,
    location: DebugMetadataOperand<B>,
    variable: MetadataId<B>,
    expression: MetadataId<B>,
    assign_id: Option<MetadataId<B>>,
    address_location: Option<DebugMetadataOperand<B>>,
    address_expression: Option<MetadataId<B>>,
    debug_loc: MetadataId<B>,
}

impl<B: ModuleBrand> DebugVariableRecord<B> {
    pub fn new(
        kind: DebugVariableRecordKind,
        location: DebugMetadataOperand<B>,
        variable: MetadataId<B>,
        expression: MetadataId<B>,
        debug_loc: MetadataId<B>,
    ) -> Self {
        Self {
            kind,
            location,
            variable,
            expression,
            assign_id: None,
            address_location: None,
            address_expression: None,
            debug_loc,
        }
    }

    #[must_use]
    pub fn with_assign_id(mut self, assign_id: MetadataId<B>) -> Self {
        self.assign_id = Some(assign_id);
        self
    }

    #[must_use]
    pub fn with_address_location(mut self, address_location: DebugMetadataOperand<B>) -> Self {
        self.address_location = Some(address_location);
        self
    }

    #[must_use]
    pub fn with_address_expression(mut self, address_expression: MetadataId<B>) -> Self {
        self.address_expression = Some(address_expression);
        self
    }

    pub const fn kind(&self) -> DebugVariableRecordKind {
        self.kind
    }

    pub const fn location(&self) -> DebugMetadataOperand<B> {
        self.location
    }

    pub const fn variable(&self) -> MetadataId<B> {
        self.variable
    }

    pub const fn expression(&self) -> MetadataId<B> {
        self.expression
    }

    pub const fn assign_id(&self) -> Option<MetadataId<B>> {
        self.assign_id
    }

    pub const fn address_location(&self) -> Option<DebugMetadataOperand<B>> {
        self.address_location
    }

    pub const fn address_expression(&self) -> Option<MetadataId<B>> {
        self.address_expression
    }

    pub const fn debug_loc(&self) -> MetadataId<B> {
        self.debug_loc
    }

    fn into_stored(self, owner: ModuleId) -> IrResult<DebugVariableRecord<StoredBrand>> {
        Ok(DebugVariableRecord {
            kind: self.kind,
            location: self.location.into_stored(owner)?,
            variable: self.variable.into_stored(owner)?,
            expression: self.expression.into_stored(owner)?,
            assign_id: self.assign_id.map(|id| id.into_stored(owner)).transpose()?,
            address_location: self
                .address_location
                .map(|operand| operand.into_stored(owner))
                .transpose()?,
            address_expression: self
                .address_expression
                .map(|id| id.into_stored(owner))
                .transpose()?,
            debug_loc: self.debug_loc.into_stored(owner)?,
        })
    }

    fn from_stored(stored: &DebugVariableRecord<StoredBrand>) -> Self {
        Self {
            kind: stored.kind,
            location: DebugMetadataOperand::from_stored(stored.location),
            variable: MetadataId::from_stored(stored.variable),
            expression: MetadataId::from_stored(stored.expression),
            assign_id: stored.assign_id.map(MetadataId::from_stored),
            address_location: stored
                .address_location
                .map(DebugMetadataOperand::from_stored),
            address_expression: stored.address_expression.map(MetadataId::from_stored),
            debug_loc: MetadataId::from_stored(stored.debug_loc),
        }
    }
}

#[derive(Branded)]
#[branded(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DebugRecord<B: ModuleBrand> {
    Variable(DebugVariableRecord<B>),
    Label {
        label: MetadataId<B>,
        debug_loc: MetadataId<B>,
    },
}

impl<B: ModuleBrand> DebugRecord<B> {
    /// Crate-internal: tag-check every operand against `owner` and hand back
    /// the storage form. The choke point for
    /// [`InstructionView::push_debug_record`](crate::InstructionView::push_debug_record).
    pub(crate) fn into_stored(self, owner: ModuleId) -> IrResult<DebugRecord<StoredBrand>> {
        Ok(match self {
            Self::Variable(record) => DebugRecord::Variable(record.into_stored(owner)?),
            Self::Label { label, debug_loc } => DebugRecord::Label {
                label: label.into_stored(owner)?,
                debug_loc: debug_loc.into_stored(owner)?,
            },
        })
    }

    /// Crate-internal: retag a stored record back into the caller's brand.
    pub(crate) fn from_stored(stored: &DebugRecord<StoredBrand>) -> Self {
        match stored {
            DebugRecord::Variable(record) => {
                Self::Variable(DebugVariableRecord::from_stored(record))
            }
            DebugRecord::Label { label, debug_loc } => Self::Label {
                label: MetadataId::from_stored(*label),
                debug_loc: MetadataId::from_stored(*debug_loc),
            },
        }
    }
}

impl DebugRecord<StoredBrand> {
    pub(crate) fn for_each_value<F>(&self, mut f: F)
    where
        F: FnMut(ValueSlot),
    {
        match self {
            Self::Variable(record) => {
                if let Some(slot) = record.location.value_slot() {
                    f(slot);
                }
                if let Some(address) = record.address_location
                    && let Some(slot) = address.value_slot()
                {
                    f(slot);
                }
            }
            Self::Label { .. } => {}
        }
    }

    pub(crate) fn replace_value_slot(&mut self, from: ValueSlot, to: ValueSlot) {
        match self {
            Self::Variable(record) => {
                record.location.replace_value_slot(from, to);
                if let Some(address) = &mut record.address_location {
                    address.replace_value_slot(from, to);
                }
            }
            Self::Label { .. } => {}
        }
    }
}

// --------------------------------------------------------------------------
// Node content
// --------------------------------------------------------------------------

/// Base metadata discriminant. Mirrors `Metadata::MetadataKind` in `Metadata.h`.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub enum MetadataKind<B: ModuleBrand> {
    /// `null` metadata operand placeholder.
    Null,
    /// `!"..."` — a string node. Mirrors `MDString`.
    String(String),
    /// `i64 1`, `ptr null`, ... — a typed constant metadata operand.
    Constant(ValueId<B>),
    /// `!{ op, op, ... }` — a tuple. Mirrors `MDTuple`.
    Tuple {
        distinct: bool,
        operands: Vec<MetadataId<B>>,
    },
    /// `!N` — reference to an already-interned metadata node.
    Ref(MetadataId<B>),
    /// `!DIFile(...)`, `!DILocation(...)`, and sibling specialized nodes.
    Specialized(SpecializedMetadataNode<B>),
    /// `!DIArgList(i32 %a, i64 7)` — a positional list of value-as-metadata
    /// operands.
    ///
    /// Mirrors `DIArgList`, which `Metadata.def` declares as a top-level
    /// `HANDLE_METADATA_LEAF` rather than a specialized `DI*` node — which is
    /// why [`SpecializedMetadataKind`] is correctly complete without it, and
    /// why it lives here beside [`Constant`](Self::Constant) instead.
    ///
    /// Three properties follow from `LLParser::parseDIArgList` and are enforced
    /// by the parser: it is always uniqued (there is no `distinct !DIArgList`
    /// spelling to parse), an empty list is legal, and it may only ever appear
    /// as an *inline* operand — never as an `!N = ` definition — because its
    /// operands may be function-local values.
    ArgList { arguments: Vec<ValueId<B>> },
}

impl<B: ModuleBrand> MetadataKind<B> {
    /// Crate-internal: tag-check every reference this node carries against
    /// `owner` and hand back the storage form. The choke point for
    /// [`Module::metadata_node`](crate::Module::metadata_node) and
    /// [`Module::metadata_set`](crate::Module::metadata_set).
    pub(crate) fn into_stored(self, owner: ModuleId) -> IrResult<MetadataKind<StoredBrand>> {
        Ok(match self {
            Self::Null => MetadataKind::Null,
            Self::String(s) => MetadataKind::String(s),
            Self::Constant(id) => MetadataKind::Constant(value_id_into_stored(id, owner)?),
            Self::Tuple { distinct, operands } => MetadataKind::Tuple {
                distinct,
                operands: operands
                    .into_iter()
                    .map(|id| id.into_stored(owner))
                    .collect::<IrResult<Vec<_>>>()?,
            },
            Self::Ref(id) => MetadataKind::Ref(id.into_stored(owner)?),
            Self::Specialized(node) => MetadataKind::Specialized(node.into_stored(owner)?),
            Self::ArgList { arguments } => MetadataKind::ArgList {
                arguments: arguments
                    .into_iter()
                    .map(|id| value_id_into_stored(id, owner))
                    .collect::<IrResult<Vec<_>>>()?,
            },
        })
    }

    /// Crate-internal: retag stored node content back into the caller's brand.
    pub(crate) fn from_stored(stored: &MetadataKind<StoredBrand>) -> Self {
        match stored {
            MetadataKind::Null => Self::Null,
            MetadataKind::String(s) => Self::String(s.clone()),
            MetadataKind::Constant(id) => Self::Constant(value_id_from_stored(*id)),
            MetadataKind::Tuple { distinct, operands } => Self::Tuple {
                distinct: *distinct,
                operands: operands
                    .iter()
                    .map(|id| MetadataId::from_stored(*id))
                    .collect(),
            },
            MetadataKind::Ref(id) => Self::Ref(MetadataId::from_stored(*id)),
            MetadataKind::Specialized(node) => {
                Self::Specialized(SpecializedMetadataNode::from_stored(node))
            }
            MetadataKind::ArgList { arguments } => Self::ArgList {
                arguments: arguments
                    .iter()
                    .copied()
                    .map(value_id_from_stored)
                    .collect(),
            },
        }
    }
}

// --------------------------------------------------------------------------
// Attachment sets
// --------------------------------------------------------------------------

/// Ordered metadata attachment set. Duplicate kinds replace the old node while
/// preserving insertion position, matching LLVM attachment semantics.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct MetadataAttachmentSet<B: ModuleBrand> {
    entries: Vec<(MetadataAttachmentKind, MetadataId<B>)>,
}

impl<B: ModuleBrand> MetadataAttachmentSet<B> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn insert(&mut self, kind: MetadataAttachmentKind, id: MetadataId<B>) {
        if let Some((_, existing)) = self.entries.iter_mut().find(|(k, _)| *k == kind) {
            *existing = id;
            return;
        }
        self.entries.push((kind, id));
    }

    pub fn get(&self, kind: &MetadataAttachmentKind) -> Option<MetadataId<B>> {
        self.entries
            .iter()
            .find_map(|(k, id)| if k == kind { Some(*id) } else { None })
    }

    pub fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = &(MetadataAttachmentKind, MetadataId<B>)>
    + DoubleEndedIterator
    + FusedIterator {
        self.entries.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Crate-internal: retag a stored attachment set into the caller's brand.
    /// The read half of the boundary the setters cross through
    /// [`MetadataId::into_stored`].
    pub(crate) fn from_stored(stored: &MetadataAttachmentSet<StoredBrand>) -> Self {
        Self {
            entries: stored
                .entries
                .iter()
                .map(|(kind, id)| (kind.clone(), MetadataId::from_stored(*id)))
                .collect(),
        }
    }
}

impl<B: ModuleBrand> Default for MetadataAttachmentSet<B> {
    fn default() -> Self {
        Self::new()
    }
}

/// `for (kind, id) in &attachments` — yields exactly what
/// [`MetadataAttachmentSet::iter`] does.
impl<'a, B: ModuleBrand> IntoIterator for &'a MetadataAttachmentSet<B> {
    type Item = &'a (MetadataAttachmentKind, MetadataId<B>);
    type IntoIter = core::slice::Iter<'a, (MetadataAttachmentKind, MetadataId<B>)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

// --------------------------------------------------------------------------
// The arena
// --------------------------------------------------------------------------

/// Storage arena for all metadata nodes. Owned by `Module`.
/// Mirrors the `LLVMContextImpl::MetadataStore` pattern.
///
/// Crate-internal, like the [`MetadataSlot`]s it hands out: everything it holds
/// is native to the owning module, so it speaks the [`StoredBrand`] form.
#[derive(Debug, Default)]
pub(crate) struct MetadataStore {
    nodes: Vec<MetadataKind<StoredBrand>>,
}

impl MetadataStore {
    /// Intern a string node. Returns an existing slot if an identical string
    /// was already inserted (mirrors `MDString::get`).
    pub(crate) fn get_string<S>(&mut self, s: S) -> MetadataSlot
    where
        S: Into<String>,
    {
        let s = s.into();
        for (i, node) in self.nodes.iter().enumerate() {
            if let MetadataKind::String(existing) = node
                && *existing == s
            {
                return MetadataSlot(i);
            }
        }
        let id = MetadataSlot(self.nodes.len());
        self.nodes.push(MetadataKind::String(s));
        id
    }

    /// Create a tuple node with explicit distinctness.
    pub(crate) fn get_tuple_with_distinct(
        &mut self,
        distinct: bool,
        operands: Vec<MetadataId<StoredBrand>>,
    ) -> MetadataSlot {
        let id = MetadataSlot(self.nodes.len());
        self.nodes.push(MetadataKind::Tuple { distinct, operands });
        id
    }

    /// Store a typed constant metadata operand.
    pub(crate) fn get_constant(&mut self, value: ValueId<StoredBrand>) -> MetadataSlot {
        let id = MetadataSlot(self.nodes.len());
        self.nodes.push(MetadataKind::Constant(value));
        id
    }

    /// Create a specialized `DI*` metadata node.
    pub(crate) fn get_specialized(
        &mut self,
        node: SpecializedMetadataNode<StoredBrand>,
    ) -> MetadataSlot {
        let id = MetadataSlot(self.nodes.len());
        self.nodes.push(MetadataKind::Specialized(node));
        id
    }

    /// Intern a `!DIArgList`.
    ///
    /// `DIArgList::get` uniques, but its operands may be function-local values,
    /// so two lists that look alike across functions are not the same node.
    /// This keeps them distinct by construction — the same choice
    /// [`get_specialized`](Self::get_specialized) makes, and for the same
    /// reason.
    pub(crate) fn get_arg_list(&mut self, arguments: Vec<ValueId<StoredBrand>>) -> MetadataSlot {
        let id = MetadataSlot(self.nodes.len());
        self.nodes.push(MetadataKind::ArgList { arguments });
        id
    }

    /// Reserve a fresh node slot with placeholder content.
    pub(crate) fn reserve(&mut self) -> MetadataSlot {
        let id = MetadataSlot(self.nodes.len());
        self.nodes.push(MetadataKind::Tuple {
            distinct: false,
            operands: Vec::new(),
        });
        id
    }

    /// Overwrite the node at `id` with `kind`. No-op if `id` is out of range —
    /// callers range-check first and report `IrError::UnknownMetadataSlot`.
    pub(crate) fn set(&mut self, id: MetadataSlot, kind: MetadataKind<StoredBrand>) {
        if let Some(slot) = self.nodes.get_mut(id.0) {
            *slot = kind;
        }
    }

    /// Total number of interned metadata nodes.
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Look up a metadata node by slot.
    pub(crate) fn get(&self, id: MetadataSlot) -> Option<&MetadataKind<StoredBrand>> {
        self.nodes.get(id.0)
    }

    /// Mutably look up a metadata node by slot.
    pub(crate) fn get_mut(&mut self, id: MetadataSlot) -> Option<&mut MetadataKind<StoredBrand>> {
        self.nodes.get_mut(id.0)
    }

    /// Slice over all nodes, indexed by their `MetadataSlot::index`.
    pub(crate) fn nodes(&self) -> &[MetadataKind<StoredBrand>] {
        &self.nodes
    }
}
