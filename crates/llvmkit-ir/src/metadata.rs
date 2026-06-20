//! Metadata types. Mirrors `llvm/include/llvm/IR/Metadata.h`.
//!
//! Models numbered metadata tuples/strings, named metadata operands,
//! attachment sets, and the core specialized DI node surface the assembler
//! parser needs to round-trip debug metadata without storing opaque IR text.

/// Stable index into the module-level metadata arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetadataId(pub(crate) usize);

impl MetadataId {
    /// Construct from a raw index. Used by the parser to map `!N` slots.
    pub fn from_index(index: usize) -> Self {
        Self(index)
    }

    /// Numeric index of this id. Used by the AsmWriter for slot numbering.
    pub fn index(self) -> usize {
        self.0
    }
}

/// Public metadata reference. `None` is the "null" metadata operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetadataRef(pub MetadataId);

/// LLVM metadata attachment names with the upstream fixed set represented as
/// enum variants. Unknown `!name` attachments are valid IR and stay custom.
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
    Callback,
    KcfiType,
    PcSections,
    DIAssignID,
    CoroOutsideFrame,
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
            "callback" => Self::Callback,
            "kcfi_type" => Self::KcfiType,
            "pcsections" => Self::PcSections,
            "DIAssignID" => Self::DIAssignID,
            "coro.outside.frame" => Self::CoroOutsideFrame,
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
            Self::Callback => "callback",
            Self::KcfiType => "kcfi_type",
            Self::PcSections => "pcsections",
            Self::DIAssignID => "DIAssignID",
            Self::CoroOutsideFrame => "coro.outside.frame",
            Self::Custom(s) => s.as_str(),
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

/// A typed field value inside a specialized `DI*` node.
#[derive(Debug, Clone)]
pub enum MetadataFieldValue {
    Null,
    Bool(bool),
    Integer(i128),
    String(String),
    Enum(String),
    Metadata(MetadataRef),
    MetadataList(Vec<MetadataRef>),
}

/// One `name: value` pair in a specialized `DI*` node.
#[derive(Debug, Clone)]
pub struct MetadataField {
    name: String,
    value: MetadataFieldValue,
}

impl MetadataField {
    pub fn new<Name>(name: Name, value: MetadataFieldValue) -> Self
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

    pub fn value(&self) -> &MetadataFieldValue {
        &self.value
    }
}

/// Metadata operand used by new-format `#dbg_*` records. Values are stored by
/// id so the record remains lifetime-free inside instruction storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugMetadataOperand {
    Metadata(MetadataRef),
    Value(crate::value::ValueId),
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugVariableRecord {
    kind: DebugVariableRecordKind,
    location: DebugMetadataOperand,
    variable: MetadataId,
    expression: MetadataId,
    assign_id: Option<MetadataId>,
    address_location: Option<DebugMetadataOperand>,
    address_expression: Option<MetadataId>,
    debug_loc: MetadataId,
}

impl DebugVariableRecord {
    pub fn new(
        kind: DebugVariableRecordKind,
        location: DebugMetadataOperand,
        variable: MetadataId,
        expression: MetadataId,
        debug_loc: MetadataId,
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

    pub fn with_assign_id(mut self, assign_id: MetadataId) -> Self {
        self.assign_id = Some(assign_id);
        self
    }

    pub fn with_address_location(mut self, address_location: DebugMetadataOperand) -> Self {
        self.address_location = Some(address_location);
        self
    }

    pub fn with_address_expression(mut self, address_expression: MetadataId) -> Self {
        self.address_expression = Some(address_expression);
        self
    }

    pub const fn kind(&self) -> DebugVariableRecordKind {
        self.kind
    }

    pub const fn location(&self) -> DebugMetadataOperand {
        self.location
    }

    pub const fn variable(&self) -> MetadataId {
        self.variable
    }

    pub const fn expression(&self) -> MetadataId {
        self.expression
    }

    pub const fn assign_id(&self) -> Option<MetadataId> {
        self.assign_id
    }

    pub const fn address_location(&self) -> Option<DebugMetadataOperand> {
        self.address_location
    }

    pub const fn address_expression(&self) -> Option<MetadataId> {
        self.address_expression
    }

    pub const fn debug_loc(&self) -> MetadataId {
        self.debug_loc
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DebugRecord {
    Variable(DebugVariableRecord),
    Label {
        label: MetadataId,
        debug_loc: MetadataId,
    },
}

/// Stored specialized node. Field order is significant and mirrors source.
#[derive(Debug, Clone)]
pub struct SpecializedMetadataNode {
    distinct: bool,
    kind: SpecializedMetadataKind,
    fields: Vec<MetadataField>,
}

impl SpecializedMetadataNode {
    pub fn new(kind: SpecializedMetadataKind) -> Self {
        Self {
            distinct: false,
            kind,
            fields: Vec::new(),
        }
    }

    pub fn distinct(mut self, distinct: bool) -> Self {
        self.distinct = distinct;
        self
    }

    pub fn field(mut self, field: MetadataField) -> Self {
        self.fields.push(field);
        self
    }

    pub fn with_fields<Fields>(mut self, fields: Fields) -> Self
    where
        Fields: IntoIterator<Item = MetadataField>,
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

    pub fn fields(&self) -> &[MetadataField] {
        &self.fields
    }
}

/// Base metadata discriminant. Mirrors `Metadata::MetadataKind` in `Metadata.h`.
#[derive(Debug, Clone)]
pub enum MetadataKind {
    /// `null` metadata operand placeholder.
    Null,
    /// `!"..."` — a string node. Mirrors `MDString`.
    String(String),
    /// `!{ op, op, ... }` — a tuple. Mirrors `MDTuple`.
    Tuple {
        distinct: bool,
        operands: Vec<MetadataRef>,
    },
    /// `!N` — reference to an already-interned metadata node.
    Ref(MetadataId),
    /// `!DIFile(...)`, `!DILocation(...)`, and sibling specialized nodes.
    Specialized(SpecializedMetadataNode),
}

/// Ordered metadata attachment set. Duplicate kinds replace the old node while
/// preserving insertion position, matching LLVM attachment semantics.
#[derive(Debug, Clone, Default)]
pub struct MetadataAttachmentSet {
    entries: Vec<(MetadataAttachmentKind, MetadataId)>,
}

impl MetadataAttachmentSet {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn insert(&mut self, kind: MetadataAttachmentKind, id: MetadataId) {
        if let Some((_, existing)) = self.entries.iter_mut().find(|(k, _)| *k == kind) {
            *existing = id;
            return;
        }
        self.entries.push((kind, id));
    }

    pub fn get(&self, kind: &MetadataAttachmentKind) -> Option<MetadataId> {
        self.entries
            .iter()
            .find_map(|(k, id)| if k == kind { Some(*id) } else { None })
    }

    pub fn iter(&self) -> impl Iterator<Item = &(MetadataAttachmentKind, MetadataId)> {
        self.entries.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Storage arena for all metadata nodes. Owned by `Module`.
/// Mirrors the `LLVMContextImpl::MetadataStore` pattern.
#[derive(Debug, Default)]
pub struct MetadataStore {
    nodes: Vec<MetadataKind>,
}

impl MetadataStore {
    /// Intern a string node. Returns an existing id if an identical string
    /// was already inserted (mirrors `MDString::get`).
    pub fn get_string<S>(&mut self, s: S) -> MetadataId
    where
        S: Into<String>,
    {
        let s = s.into();
        for (i, node) in self.nodes.iter().enumerate() {
            if let MetadataKind::String(existing) = node
                && *existing == s
            {
                return MetadataId(i);
            }
        }
        let id = MetadataId(self.nodes.len());
        self.nodes.push(MetadataKind::String(s));
        id
    }

    /// Create a non-distinct tuple node.
    pub fn get_tuple(&mut self, operands: Vec<MetadataRef>) -> MetadataId {
        self.get_tuple_with_distinct(false, operands)
    }

    /// Create a tuple node with explicit distinctness.
    pub fn get_tuple_with_distinct(
        &mut self,
        distinct: bool,
        operands: Vec<MetadataRef>,
    ) -> MetadataId {
        let id = MetadataId(self.nodes.len());
        self.nodes.push(MetadataKind::Tuple { distinct, operands });
        id
    }

    /// Create a specialized `DI*` metadata node.
    pub fn get_specialized(&mut self, node: SpecializedMetadataNode) -> MetadataId {
        let id = MetadataId(self.nodes.len());
        self.nodes.push(MetadataKind::Specialized(node));
        id
    }

    /// Reserve a fresh node id with placeholder content.
    pub fn reserve(&mut self) -> MetadataId {
        let id = MetadataId(self.nodes.len());
        self.nodes.push(MetadataKind::Tuple {
            distinct: false,
            operands: Vec::new(),
        });
        id
    }

    /// Overwrite the node at `id` with `kind`. No-op if `id` is out of range.
    pub fn set(&mut self, id: MetadataId, kind: MetadataKind) {
        if let Some(slot) = self.nodes.get_mut(id.0) {
            *slot = kind;
        }
    }

    /// Total number of interned metadata nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when the store has no interned nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Look up a metadata node by id.
    pub fn get(&self, id: MetadataId) -> Option<&MetadataKind> {
        self.nodes.get(id.0)
    }

    /// Slice over all nodes, indexed by their `MetadataId.index()`.
    pub(crate) fn nodes(&self) -> &[MetadataKind] {
        &self.nodes
    }
}
