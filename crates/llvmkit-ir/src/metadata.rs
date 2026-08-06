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
/// here in the `.def`'s own order. Marked `#[non_exhaustive]` so future
/// upstream additions are non-breaking; [`Custom`](Self::Custom) carries the
/// open remainder of the namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
    DIAssignID,
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
            "DIAssignID" => Self::DIAssignID,
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
            Self::DIAssignID => "DIAssignID",
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
            Self::DIAssignID => Some(38),
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

/// Specialized debug metadata node families accepted by LLVM's assembler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecializedMetadataKind {
    DIFile,
    DICompileUnit,
    DISubprogram,
    DILocation,
    DILocalVariable,
    DIBasicType,
    DIDerivedType,
    DICompositeType,
    DISubrange,
    DINamespace,
    DIExpression,
    DIGlobalVariable,
    DIGlobalVariableExpression,
    DISubroutineType,
    DIEnumerator,
    DIModule,
    DITemplateTypeParameter,
    DITemplateValueParameter,
}

impl SpecializedMetadataKind {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "DIFile" => Self::DIFile,
            "DICompileUnit" => Self::DICompileUnit,
            "DISubprogram" => Self::DISubprogram,
            "DILocation" => Self::DILocation,
            "DILocalVariable" => Self::DILocalVariable,
            "DIBasicType" => Self::DIBasicType,
            "DIDerivedType" => Self::DIDerivedType,
            "DICompositeType" => Self::DICompositeType,
            "DISubrange" => Self::DISubrange,
            "DINamespace" => Self::DINamespace,
            "DIExpression" => Self::DIExpression,
            "DIGlobalVariable" => Self::DIGlobalVariable,
            "DIGlobalVariableExpression" => Self::DIGlobalVariableExpression,
            "DISubroutineType" => Self::DISubroutineType,
            "DIEnumerator" => Self::DIEnumerator,
            "DIModule" => Self::DIModule,
            "DITemplateTypeParameter" => Self::DITemplateTypeParameter,
            "DITemplateValueParameter" => Self::DITemplateValueParameter,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::DIFile => "DIFile",
            Self::DICompileUnit => "DICompileUnit",
            Self::DISubprogram => "DISubprogram",
            Self::DILocation => "DILocation",
            Self::DILocalVariable => "DILocalVariable",
            Self::DIBasicType => "DIBasicType",
            Self::DIDerivedType => "DIDerivedType",
            Self::DICompositeType => "DICompositeType",
            Self::DISubrange => "DISubrange",
            Self::DINamespace => "DINamespace",
            Self::DIExpression => "DIExpression",
            Self::DIGlobalVariable => "DIGlobalVariable",
            Self::DIGlobalVariableExpression => "DIGlobalVariableExpression",
            Self::DISubroutineType => "DISubroutineType",
            Self::DIEnumerator => "DIEnumerator",
            Self::DIModule => "DIModule",
            Self::DITemplateTypeParameter => "DITemplateTypeParameter",
            Self::DITemplateValueParameter => "DITemplateValueParameter",
        }
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

/// Stored specialized node. Field order is significant and mirrors source.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct SpecializedMetadataNode<B: ModuleBrand> {
    distinct: bool,
    kind: SpecializedMetadataKind,
    fields: Vec<MetadataField<B>>,
}

impl<B: ModuleBrand> SpecializedMetadataNode<B> {
    pub fn new(kind: SpecializedMetadataKind) -> Self {
        Self {
            distinct: false,
            kind,
            fields: Vec::new(),
        }
    }

    /// Mark the node `distinct`. Default off.
    #[must_use]
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    #[must_use]
    pub fn field(mut self, field: MetadataField<B>) -> Self {
        self.fields.push(field);
        self
    }

    pub fn with_fields<Fields>(mut self, fields: Fields) -> Self
    where
        Fields: IntoIterator<Item = MetadataField<B>>,
    {
        self.fields.extend(fields);
        self
    }

    pub const fn is_distinct(&self) -> bool {
        self.distinct
    }

    pub const fn kind(&self) -> SpecializedMetadataKind {
        self.kind
    }

    pub fn fields(&self) -> &[MetadataField<B>] {
        &self.fields
    }

    pub(crate) fn into_stored(
        self,
        owner: ModuleId,
    ) -> IrResult<SpecializedMetadataNode<StoredBrand>> {
        Ok(SpecializedMetadataNode {
            distinct: self.distinct,
            kind: self.kind,
            fields: self
                .fields
                .into_iter()
                .map(|field| field.into_stored(owner))
                .collect::<IrResult<Vec<_>>>()?,
        })
    }

    fn from_stored(stored: &SpecializedMetadataNode<StoredBrand>) -> Self {
        Self {
            distinct: stored.distinct,
            kind: stored.kind,
            fields: stored
                .fields
                .iter()
                .map(MetadataField::from_stored)
                .collect(),
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
