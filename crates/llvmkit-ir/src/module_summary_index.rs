//! Module summary index — the `^N` half of a `.ll` file.
//!
//! Ports `llvm/IR/ModuleSummaryIndex.h`. A summary index is a whole-program
//! description that ThinLTO produces and consumes: one entry per module path,
//! one per global value, plus type-identifier resolutions, all keyed by the
//! MD5-derived [`Guid`] of the entity's global identifier.
//!
//! # Where the index differs from the assembly that spells it
//!
//! `^N` numbers are **not** part of the model. `AsmWriter.cpp`'s
//! `SlotTracker::processIndex` re-derives them on output: module paths sorted
//! by path string come first, then global values in ascending GUID order, then
//! the compatible-vtable entries by type name, then the type identifiers by
//! GUID. `test/Assembler/index-value-order.ll` exists to prove input order is
//! not preserved — it feeds `^9`, `^4`, `^3`, `^2` in that order and matches
//! the output only through `CHECK-DAG` bindings.
//!
//! This is a deliberate deviation from the design annex, which sketched a
//! `^N`-ordered `entries: Vec<SummaryEntry>`: that shape cannot reproduce
//! upstream's bytes, and byte-for-byte printing is the contract.
//!
//! Upstream's `ValueInfo` is a pointer into the global-value map carrying three
//! spare bits. llvmkit spells the same thing as [`ValueReference`] — the
//! referent's [`Guid`] plus an [`AccessSpecifier`] — because the pointer is an
//! allocation detail and the GUID is the identity.

use std::collections::BTreeMap;
use std::fmt;

use crate::constant_range::ConstantRange;
use crate::global_value::{Linkage, Visibility};
use crate::md5::md5_hash;

/// Separator between the source file name and the value name in the global
/// identifier of a value with local linkage.
///
/// Mirrors `llvm::GlobalIdentifierDelimiter`.
pub const GLOBAL_IDENTIFIER_DELIMITER: char = ';';

/// The name used for a module path that is empty — the regular-LTO module
/// created during the thin link.
///
/// Mirrors `ModuleSummaryIndex::getRegularLTOModuleName`.
pub const REGULAR_LTO_MODULE_NAME: &str = "[Regular LTO]";

/// A global value's identity in a summary index: the low 64 bits of the MD5
/// digest of its global identifier.
///
/// Mirrors `GlobalValue::GUID`, a bare `uint64_t` upstream. It is a newtype
/// here because the value has identity semantics, not quantity semantics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Guid(u64);

impl Guid {
    /// The GUID with the given raw encoding.
    #[inline]
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// This GUID's raw encoding.
    #[inline]
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// The GUID of a global identifier.
    ///
    /// Mirrors `GlobalValue::getGUIDAssumingExternalLinkage`, which is
    /// `MD5Hash(GlobalIdentifier)`. The name says "assuming external linkage"
    /// because a local value's identifier must already have been prefixed with
    /// its source file name by [`global_identifier`].
    #[must_use]
    pub fn of_global_identifier(identifier: &str) -> Self {
        Self(md5_hash(identifier.as_bytes()))
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The global identifier of a value: its name, prefixed with the source file
/// name when its linkage is local so that two translation units' statics do
/// not collide.
///
/// Mirrors the static `GlobalValue::getGlobalIdentifier`.
#[must_use]
pub fn global_identifier(name: &str, linkage: Linkage, source_file_name: &str) -> String {
    // A leading '\1' asks the backend to leave the symbol alone; it is not part
    // of the identifier.
    let name = name.strip_prefix('\u{1}').unwrap_or(name);

    let mut identifier = String::new();
    if linkage.is_local() {
        // Do not include the full path: there is no guarantee it stays the same
        // across checkouts.
        if source_file_name.is_empty() {
            identifier.push_str("<unknown>");
        } else {
            identifier.push_str(source_file_name);
        }
        identifier.push(GLOBAL_IDENTIFIER_DELIMITER);
    }
    identifier.push_str(name);
    identifier
}

/// A module's content hash: five 32-bit words of SHA-1.
///
/// Mirrors `llvm::ModuleHash`.
pub type ModuleHash = [u32; 5];

/// Whether an imported summary describes a definition or only a declaration.
///
/// Mirrors `GlobalValueSummary::ImportKind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImportKind {
    /// Import the global value definition.
    #[default]
    Definition,
    /// Import only a declaration; the definition stays in its own module.
    Declaration,
}

impl ImportKind {
    /// `.ll` keyword for this import kind.
    ///
    /// Mirrors `AsmWriter.cpp::getImportTypeName`.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::Declaration => "declaration",
        }
    }
}

impl fmt::Display for ImportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// Flags shared by every kind of global value summary.
///
/// Mirrors `GlobalValueSummary::GVFlags`, a bitfield upstream; law 5 makes the
/// packing a serialization detail llvmkit does not reproduce.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlobalValueFlags {
    /// The value's linkage.
    pub linkage: Linkage,
    /// The value's visibility.
    pub visibility: Visibility,
    /// Set when the value cannot be imported into another module.
    pub not_eligible_to_import: bool,
    /// Set when summary-based liveness analysis found the value reachable.
    pub live: bool,
    /// Set when the value is known not to be preempted.
    pub dso_local: bool,
    /// Set when a linkonce_odr definition may be hidden after LTO.
    pub can_auto_hide: bool,
    /// Whether importing brings the definition or only a declaration.
    pub import_type: ImportKind,
}

/// The optional per-function facts a summary records.
///
/// Mirrors `FunctionSummary::FFlags`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FunctionFlags {
    /// The function reads no memory.
    pub read_none: bool,
    /// The function only reads memory.
    pub read_only: bool,
    /// The function is not part of any recursive cycle.
    pub no_recurse: bool,
    /// The returned pointer does not alias anything else.
    pub return_does_not_alias: bool,
    /// The function carries `noinline`.
    pub no_inline: bool,
    /// The function carries `alwaysinline`.
    pub always_inline: bool,
    /// The function cannot unwind.
    pub no_unwind: bool,
    /// The function may throw.
    pub may_throw: bool,
    /// The function contains a call whose callee could not be determined.
    pub has_unknown_call: bool,
    /// Every path through the function ends in `unreachable`.
    pub must_be_unreachable: bool,
}

impl FunctionFlags {
    /// Whether any flag is set. The printer emits the whole clause or none of
    /// it, so this is what decides between them.
    ///
    /// Mirrors `FunctionSummary::FFlags::anyFlagSet`.
    #[must_use]
    pub const fn any_flag_set(self) -> bool {
        self.read_none
            || self.read_only
            || self.no_recurse
            || self.return_does_not_alias
            || self.no_inline
            || self.always_inline
            || self.no_unwind
            || self.may_throw
            || self.has_unknown_call
            || self.must_be_unreachable
    }
}

impl fmt::Display for FunctionFlags {
    /// Mirrors `FunctionSummary::FFlags::operator std::string()`, which is what
    /// `AsmWriter::printFunctionSummary` streams.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "funcFlags: (readNone: {}, readOnly: {}, noRecurse: {}, returnDoesNotAlias: {}, noInline: {}, alwaysInline: {}, noUnwind: {}, mayThrow: {}, hasUnknownCall: {}, mustBeUnreachable: {})",
            u8::from(self.read_none),
            u8::from(self.read_only),
            u8::from(self.no_recurse),
            u8::from(self.return_does_not_alias),
            u8::from(self.no_inline),
            u8::from(self.always_inline),
            u8::from(self.no_unwind),
            u8::from(self.may_throw),
            u8::from(self.has_unknown_call),
            u8::from(self.must_be_unreachable),
        )
    }
}

/// How widely a vtable's virtual calls may be seen.
///
/// Mirrors `GlobalObject::VCallVisibility`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VCallVisibility {
    /// Potentially visible to external code.
    #[default]
    Public,
    /// Visible only to code that will be in this module after LTO
    /// internalization.
    LinkageUnit,
    /// Visible only to code already in this module.
    TranslationUnit,
}

impl VCallVisibility {
    /// The numeric encoding the `.ll` surface prints.
    #[must_use]
    pub const fn numeric(self) -> u32 {
        match self {
            Self::Public => 0,
            Self::LinkageUnit => 1,
            Self::TranslationUnit => 2,
        }
    }

    /// The visibility with the given numeric encoding, if it names one.
    #[must_use]
    pub const fn from_numeric(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Public),
            1 => Some(Self::LinkageUnit),
            2 => Some(Self::TranslationUnit),
            _ => None,
        }
    }
}

/// The optional per-variable facts a summary records.
///
/// Mirrors `GlobalVarSummary::GVarFlags`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlobalVariableFlags {
    /// Set when no write to the variable has been found; only meaningful once
    /// attribute propagation has run.
    pub maybe_read_only: bool,
    /// Set when no read of the variable has been found; only meaningful once
    /// attribute propagation has run.
    pub maybe_write_only: bool,
    /// The variable is `constant`.
    pub constant: bool,
    /// How widely virtual calls through this vtable may be seen.
    pub vcall_visibility: VCallVisibility,
}

/// How hot a call edge is.
///
/// Mirrors `CalleeInfo::HotnessType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Hotness {
    /// No profile information.
    #[default]
    Unknown,
    /// Executed rarely.
    Cold,
    /// Executed, but neither hot nor cold.
    None,
    /// Executed often.
    Hot,
    /// On the hottest path.
    Critical,
}

impl Hotness {
    /// `.ll` keyword for this hotness.
    ///
    /// Mirrors `AsmWriter.cpp::getHotnessName`.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Cold => "cold",
            Self::None => "none",
            Self::Hot => "hot",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for Hotness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// Whether a reference only reads or only writes the referent.
///
/// Upstream keeps two independent bits on `ValueInfo` and asserts they are
/// never both set; an enum makes the illegal state unrepresentable. The
/// declaration order is load-bearing: `FunctionSummary::specialRefCounts`
/// requires refs to be sorted by this, and `AsmWriter` prints them in that
/// order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccessSpecifier {
    /// The reference may read and write.
    #[default]
    None,
    /// The reference only reads.
    ReadOnly,
    /// The reference only writes.
    WriteOnly,
}

impl AccessSpecifier {
    /// The `.ll` prefix this specifier prints, including its trailing space.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::ReadOnly => "readonly ",
            Self::WriteOnly => "writeonly ",
        }
    }
}

/// A reference from one summary to another global value.
///
/// Mirrors `ValueInfo`, whose payload upstream is a pointer into the index's
/// global-value map plus three spare bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueReference {
    /// The referent.
    pub guid: Guid,
    /// Whether the reference only reads or only writes.
    pub access: AccessSpecifier,
}

impl ValueReference {
    /// A reference with no access restriction.
    #[inline]
    #[must_use]
    pub const fn new(guid: Guid) -> Self {
        Self {
            guid,
            access: AccessSpecifier::None,
        }
    }
}

/// A call edge from a function summary.
///
/// Mirrors `FunctionSummary::EdgeTy`, a `(ValueInfo, CalleeInfo)` pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CallEdge {
    /// The callee.
    pub callee: ValueReference,
    /// Profile hotness of the edge.
    pub hotness: Hotness,
    /// Relative block frequency, recorded instead of hotness in per-module
    /// summaries. Upstream's `0` sentinel means "not recorded".
    pub relative_block_frequency: Option<u32>,
    /// Whether the call is a tail call.
    pub has_tail_call: bool,
}

/// A virtual function found in a vtable, with its offset.
///
/// Mirrors `VirtFuncOffset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VirtualFunctionOffset {
    /// The virtual function.
    pub function: ValueReference,
    /// Its byte offset within the vtable.
    pub vtable_offset: u64,
}

/// A virtual call target: the type identifier it dispatches through, and the
/// byte offset of the slot.
///
/// Mirrors `FunctionSummary::VFuncId`. Upstream's `-1` / `-2` GUID tombstones
/// are `DenseMapInfo` artifacts and have no counterpart here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VirtualFunctionId {
    /// GUID of the type identifier.
    pub guid: Guid,
    /// Byte offset of the virtual function slot.
    pub offset: u64,
}

/// A virtual call whose arguments are compile-time constants.
///
/// Mirrors `FunctionSummary::ConstVCall`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConstantVirtualCall {
    /// The call target.
    pub virtual_function: VirtualFunctionId,
    /// The constant arguments.
    pub arguments: Vec<u64>,
}

/// The five kinds of type-test information a function summary can carry.
///
/// Mirrors `FunctionSummary::TypeIdInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeIdInfo {
    /// Type identifiers tested by `llvm.type.test`.
    pub type_tests: Vec<Guid>,
    /// Virtual calls guarded by a `llvm.type.test` plus `llvm.assume`.
    pub type_test_assume_vcalls: Vec<VirtualFunctionId>,
    /// Virtual calls loaded through `llvm.type.checked.load`.
    pub type_checked_load_vcalls: Vec<VirtualFunctionId>,
    /// Constant-argument form of [`type_test_assume_vcalls`](Self::type_test_assume_vcalls).
    pub type_test_assume_const_vcalls: Vec<ConstantVirtualCall>,
    /// Constant-argument form of [`type_checked_load_vcalls`](Self::type_checked_load_vcalls).
    pub type_checked_load_const_vcalls: Vec<ConstantVirtualCall>,
}

impl TypeIdInfo {
    /// Whether every list is empty. Upstream expresses the same test by leaving
    /// `FunctionSummary::TIdInfo` null, which is what suppresses the clause.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.type_tests.is_empty()
            && self.type_test_assume_vcalls.is_empty()
            && self.type_checked_load_vcalls.is_empty()
            && self.type_test_assume_const_vcalls.is_empty()
            && self.type_checked_load_const_vcalls.is_empty()
    }
}

/// The width, in bits, of every range in a [`ParameterAccess`].
///
/// Mirrors `FunctionSummary::ParamAccess::RangeWidth`.
pub const PARAMETER_ACCESS_RANGE_WIDTH: u32 = 64;

/// One callee a pointer parameter is forwarded to, and the offsets applied
/// before the call.
///
/// Mirrors `FunctionSummary::ParamAccess::Call`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterAccessCall {
    /// Index of the callee's parameter this value is passed as.
    pub parameter_number: u64,
    /// The callee.
    pub callee: ValueReference,
    /// Offsets applied to the pointer before the call.
    pub offsets: ConstantRange,
}

/// How a function uses one of its pointer parameters.
///
/// Mirrors `FunctionSummary::ParamAccess`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterAccess {
    /// Index of the parameter this describes.
    pub parameter_number: u64,
    /// Byte offsets from the pointer that the function itself accesses.
    pub use_range: ConstantRange,
    /// Calls the pointer is forwarded to.
    pub calls: Vec<ParameterAccessCall>,
}

/// The behavior profiled for an allocation reached by a given context.
///
/// Mirrors `AllocationType`. The values are powers of two so that a context
/// reaching an allocation more than one way can OR them together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AllocationType {
    /// No profiled behavior.
    #[default]
    None,
    /// Profiled as not cold.
    NotCold,
    /// Profiled as cold.
    Cold,
    /// Profiled as hot.
    Hot,
}

impl AllocationType {
    /// The raw encoding, as stored in `AllocInfo::Versions`.
    #[must_use]
    pub const fn raw(self) -> u8 {
        match self {
            Self::None => 0,
            Self::NotCold => 1,
            Self::Cold => 2,
            Self::Hot => 4,
        }
    }

    /// `.ll` keyword for this allocation type.
    ///
    /// Mirrors the `AllocTypeName` lambda in
    /// `AsmWriter::printFunctionSummary`.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::NotCold => "notcold",
            Self::Cold => "cold",
            Self::Hot => "hot",
        }
    }
}

impl fmt::Display for AllocationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// One memory-info block of a memprof allocation summary.
///
/// Mirrors `MIBInfo`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MemoryInfoBlock {
    /// Allocation behavior profiled for this context.
    pub allocation_type: AllocationType,
    /// The context, as indices into [`ModuleSummaryIndex::stack_ids`].
    pub stack_id_indices: Vec<u32>,
}

/// Summary of the memprof metadata on one allocation site.
///
/// Mirrors `AllocInfo`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AllocationInfo {
    /// Allocation type per clone of the containing function.
    pub versions: Vec<AllocationType>,
    /// The profiled contexts reaching this allocation.
    pub memory_info_blocks: Vec<MemoryInfoBlock>,
}

/// Summary of the memprof metadata on one call site.
///
/// Mirrors `CallsiteInfo`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CallsiteInfo {
    /// The callee, absent when the record is synthesized.
    pub callee: Option<ValueReference>,
    /// Callee clone selected per clone of the containing function.
    pub clones: Vec<u32>,
    /// The context, as indices into [`ModuleSummaryIndex::stack_ids`].
    pub stack_id_indices: Vec<u32>,
}

/// A summary of one function definition.
///
/// Mirrors `FunctionSummary`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionSummary {
    /// Number of instructions in the function.
    pub instruction_count: u32,
    /// The optional per-function facts.
    pub function_flags: FunctionFlags,
    /// Outgoing call edges.
    pub calls: Vec<CallEdge>,
    /// Type-test information, absent when every list would be empty — the
    /// state upstream spells as a null `TIdInfo`.
    pub type_id_info: Option<TypeIdInfo>,
    /// Pointer-parameter usage.
    pub parameter_accesses: Vec<ParameterAccess>,
    /// Memprof call-site summaries.
    pub callsites: Vec<CallsiteInfo>,
    /// Memprof allocation summaries.
    pub allocations: Vec<AllocationInfo>,
}

/// A summary of one global variable definition.
///
/// Mirrors `GlobalVarSummary`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalVariableSummary {
    /// The optional per-variable facts.
    pub variable_flags: GlobalVariableFlags,
    /// Virtual functions found in this vtable.
    pub vtable_functions: Vec<VirtualFunctionOffset>,
}

/// A summary of one alias definition.
///
/// Mirrors `AliasSummary`. It deliberately has no `refs` field: upstream
/// asserts an alias summary carries none, and an absent field cannot hold one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AliasSummary {
    /// The aliasee, absent when the index does not carry its summary.
    pub aliasee: Option<Guid>,
}

/// What a [`GlobalValueSummary`] describes.
///
/// Mirrors the `GlobalValueSummary` class hierarchy, whose `SummaryKind`
/// discriminator selects between three subclasses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SummaryKind {
    /// An alias.
    Alias(AliasSummary),
    /// A function.
    Function(FunctionSummary),
    /// A global variable.
    Variable(GlobalVariableSummary),
}

impl SummaryKind {
    /// `.ll` keyword introducing this summary kind.
    ///
    /// Mirrors `AsmWriter.cpp::getSummaryKindName`.
    #[must_use]
    pub const fn keyword(&self) -> &'static str {
        match self {
            Self::Alias(_) => "alias",
            Self::Function(_) => "function",
            Self::Variable(_) => "variable",
        }
    }
}

/// One summary of one global value, in one module.
///
/// Mirrors `GlobalValueSummary`: the shared base fields live beside the kind
/// discriminator rather than being duplicated into each variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalValueSummary {
    /// Path of the module this summary came from.
    pub module_path: String,
    /// The flags shared by every summary kind.
    pub flags: GlobalValueFlags,
    /// Values this one references.
    pub references: Vec<ValueReference>,
    /// The kind-specific payload.
    pub kind: SummaryKind,
}

/// Every summary recorded for one GUID, plus the value's name when known.
///
/// Mirrors `GlobalValueSummaryInfo`, whose `NameOrGV` union holds a name when
/// the index was read from assembly and a `GlobalValue*` when it was built
/// from a module.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalValueSummaryInfo {
    /// The value's name, when the index carries one.
    pub name: Option<String>,
    /// The summaries, in the order they were added.
    pub summary_list: Vec<GlobalValueSummary>,
}

impl GlobalValueSummaryInfo {
    /// The name to print, which upstream requires be both present and
    /// non-empty before it is preferred over the GUID.
    #[must_use]
    pub fn printable_name(&self) -> Option<&str> {
        self.name.as_deref().filter(|name| !name.is_empty())
    }
}

/// How a type test on a type identifier was resolved.
///
/// Mirrors `TypeTestResolution::Kind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TypeTestResolutionKind {
    /// Not resolved.
    #[default]
    Unknown,
    /// The test is never satisfied.
    Unsat,
    /// Test against a byte array.
    ByteArray,
    /// Test against an inline bit vector.
    Inline,
    /// Only one type-identifier member exists.
    Single,
    /// Every bit in the bit vector is set.
    AllOnes,
}

impl TypeTestResolutionKind {
    /// `.ll` keyword for this kind.
    ///
    /// Mirrors `AsmWriter.cpp::getTTResKindName`.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsat => "unsat",
            Self::ByteArray => "byteArray",
            Self::Inline => "inline",
            Self::Single => "single",
            Self::AllOnes => "allOnes",
        }
    }
}

impl fmt::Display for TypeTestResolutionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// The resolution of type tests against one type identifier.
///
/// This stays a product type rather than becoming a sum type keyed on
/// [`TypeTestResolutionKind`]: upstream parses and stores every field
/// regardless of the kind, so folding the fields into variants would lose
/// round-trip information.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TypeTestResolution {
    /// How the test was resolved.
    pub kind: TypeTestResolutionKind,
    /// Bit width of the `size - 1` value.
    pub size_minus_one_bit_width: u32,
    /// Log2 of the alignment of the type-identifier members.
    pub align_log2: u64,
    /// One less than the number of members.
    pub size_minus_one: u64,
    /// Byte-array mask.
    pub bit_mask: u8,
    /// Inline bit vector.
    pub inline_bits: u64,
}

/// How a whole-program devirtualization resolved a virtual call, for one set of
/// constant arguments.
///
/// Mirrors `WholeProgramDevirtResolution::ByArg::Kind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WholeProgramDevirtByArgKind {
    /// Not devirtualized.
    #[default]
    Indir,
    /// Every implementation returns the same value.
    UniformRetVal,
    /// Exactly one implementation returns a distinguished value.
    UniqueRetVal,
    /// The return value is available as a constant beside the vtable.
    VirtualConstProp,
}

impl WholeProgramDevirtByArgKind {
    /// `.ll` keyword for this kind.
    ///
    /// Mirrors `AsmWriter.cpp::getWholeProgDevirtResByArgKindName`.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Indir => "indir",
            Self::UniformRetVal => "uniformRetVal",
            Self::UniqueRetVal => "uniqueRetVal",
            Self::VirtualConstProp => "virtualConstProp",
        }
    }
}

impl fmt::Display for WholeProgramDevirtByArgKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// A whole-program devirtualization resolution for one argument list.
///
/// Mirrors `WholeProgramDevirtResolution::ByArg`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WholeProgramDevirtByArg {
    /// How the call was resolved.
    pub kind: WholeProgramDevirtByArgKind,
    /// The uniform or unique return value.
    pub info: u64,
    /// Byte offset of the constant beside the vtable.
    pub byte: u32,
    /// Bit offset of the constant beside the vtable.
    pub bit: u32,
}

/// How a whole-program devirtualization resolved a virtual call.
///
/// Mirrors `WholeProgramDevirtResolution::Kind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WholeProgramDevirtKind {
    /// Not devirtualized.
    #[default]
    Indir,
    /// Exactly one implementation exists.
    SingleImpl,
    /// Dispatch through a branch funnel.
    BranchFunnel,
}

impl WholeProgramDevirtKind {
    /// `.ll` keyword for this kind.
    ///
    /// Mirrors `AsmWriter.cpp::getWholeProgDevirtResKindName`.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Indir => "indir",
            Self::SingleImpl => "singleImpl",
            Self::BranchFunnel => "branchFunnel",
        }
    }
}

impl fmt::Display for WholeProgramDevirtKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// The whole-program devirtualization resolution for one vtable slot.
///
/// Mirrors `WholeProgramDevirtResolution`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WholeProgramDevirtResolution {
    /// How the slot was resolved.
    pub kind: WholeProgramDevirtKind,
    /// Name of the single implementation, when there is one.
    pub single_impl_name: String,
    /// Per-argument-list resolutions, ordered as upstream's `std::map` prints
    /// them.
    pub resolutions_by_argument: BTreeMap<Vec<u64>, WholeProgramDevirtByArg>,
}

/// Everything the thin link resolved about one type identifier.
///
/// Mirrors `TypeIdSummary`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeIdSummary {
    /// How type tests against this identifier resolve.
    pub type_test_resolution: TypeTestResolution,
    /// Per-slot devirtualization resolutions, keyed by byte offset.
    pub whole_program_devirt_resolutions: BTreeMap<u64, WholeProgramDevirtResolution>,
}

/// A vtable compatible with a type identifier, and its address-point offset.
///
/// Mirrors `TypeIdOffsetVtableInfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeIdOffsetVtableInfo {
    /// Offset of the address point within the vtable.
    pub address_point_offset: u64,
    /// The vtable.
    pub vtable: ValueReference,
}

/// Index-wide flags.
///
/// Mirrors the `uint64_t` that `ModuleSummaryIndex::getFlags` packs and
/// `setFlags` unpacks. The raw value is stored as it was read: upstream's
/// `assert(Flags <= 0x7ff)` is debug-only, so a release build neither rejects
/// nor crashes on unknown bits, and neither does llvmkit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IndexFlags {
    raw: u64,
}

impl IndexFlags {
    /// The flags with the given raw encoding.
    #[inline]
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self { raw }
    }

    /// The raw encoding.
    #[inline]
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.raw
    }

    /// Summary-based dead stripping has run, so `live: 0` really means dead.
    #[must_use]
    pub const fn with_global_value_dead_stripping(self) -> bool {
        self.raw & 0x1 != 0
    }

    /// The distributed backend should skip compiling this module.
    #[must_use]
    pub const fn skip_module_by_distributed_backend(self) -> bool {
        self.raw & 0x2 != 0
    }

    // Bit 0x4 is reserved upstream and must not be reused; it has no accessor.

    /// The module was compiled with `-fsplit-lto-unit`.
    #[must_use]
    pub const fn enable_split_lto_unit(self) -> bool {
        self.raw & 0x8 != 0
    }

    /// Some linked modules were split into LTO units and some were not.
    #[must_use]
    pub const fn partially_split_lto_units(self) -> bool {
        self.raw & 0x10 != 0
    }

    /// Summary-based attribute propagation has run.
    #[must_use]
    pub const fn with_attribute_propagation(self) -> bool {
        self.raw & 0x20 != 0
    }

    /// Summary-based `dso_local` propagation has run.
    #[must_use]
    pub const fn with_dso_local_propagation(self) -> bool {
        self.raw & 0x40 != 0
    }

    /// The link has whole-program visibility.
    #[must_use]
    pub const fn with_whole_program_visibility(self) -> bool {
        self.raw & 0x80 != 0
    }

    /// The link used an allocator supporting hot/cold `operator new`.
    #[must_use]
    pub const fn with_supports_hot_cold_new(self) -> bool {
        self.raw & 0x100 != 0
    }

    /// The module was compiled with `-funified-lto`.
    #[must_use]
    pub const fn has_unified_lto(self) -> bool {
        self.raw & 0x200 != 0
    }

    /// Summary-based internalization and promotion has run.
    #[must_use]
    pub const fn with_internalize_and_promote(self) -> bool {
        self.raw & 0x400 != 0
    }
}

/// A whole-program summary index.
///
/// Mirrors `ModuleSummaryIndex`. It is a free-standing owned type with no
/// brand: nothing in it lives in a [`Module`](crate::module::Module) arena.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleSummaryIndex {
    module_paths: BTreeMap<String, ModuleHash>,
    global_values: BTreeMap<Guid, GlobalValueSummaryInfo>,
    type_ids: BTreeMap<Guid, Vec<(String, TypeIdSummary)>>,
    type_id_compatible_vtables: BTreeMap<String, Vec<TypeIdOffsetVtableInfo>>,
    stack_ids: Vec<u64>,
    flags: IndexFlags,
    block_count: u64,
}

impl ModuleSummaryIndex {
    /// An index with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the index holds nothing at all — no module, value, type
    /// identifier, flag or block count.
    ///
    /// No upstream counterpart: `llvm-dis` decides whether to print an index
    /// from whether one was requested, where llvmkit prints one only when the
    /// parsed file carried summary entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.module_paths.is_empty()
            && self.global_values.is_empty()
            && self.type_ids.is_empty()
            && self.type_id_compatible_vtables.is_empty()
            && self.flags.raw() == 0
            && self.block_count == 0
    }

    /// The module paths, ordered by path — the order `SlotTracker` assigns
    /// their `^N` slots in.
    ///
    /// Mirrors `ModuleSummaryIndex::modulePaths`.
    #[must_use]
    pub fn module_paths(&self) -> &BTreeMap<String, ModuleHash> {
        &self.module_paths
    }

    /// Records a module path, keeping any hash already recorded for it.
    ///
    /// Mirrors `ModuleSummaryIndex::addModule`, which inserts and therefore
    /// does not overwrite.
    pub fn add_module(&mut self, path: impl Into<String>, hash: ModuleHash) {
        self.module_paths.entry(path.into()).or_insert(hash);
    }

    /// The global values, in ascending GUID order.
    ///
    /// Mirrors iterating `ModuleSummaryIndex` itself, which walks its
    /// `std::map<GUID, GlobalValueSummaryInfo>`.
    #[must_use]
    pub fn global_values(&self) -> &BTreeMap<Guid, GlobalValueSummaryInfo> {
        &self.global_values
    }

    /// The entry for `guid`, creating an empty one if absent.
    ///
    /// Mirrors `ModuleSummaryIndex::getOrInsertValueInfo(GUID)`.
    pub fn global_value_entry(&mut self, guid: Guid) -> &mut GlobalValueSummaryInfo {
        self.global_values.entry(guid).or_default()
    }

    /// The entry for `guid`, creating one if absent, and recording `name` as
    /// the value's name.
    ///
    /// Mirrors `ModuleSummaryIndex::getOrInsertValueInfo(GUID, Name)`, which
    /// assigns the name unconditionally.
    pub fn global_value_entry_named(
        &mut self,
        guid: Guid,
        name: impl Into<String>,
    ) -> &mut GlobalValueSummaryInfo {
        let entry = self.global_values.entry(guid).or_default();
        entry.name = Some(name.into());
        entry
    }

    /// Adds a summary to the value's summary list.
    ///
    /// Mirrors `ModuleSummaryIndex::addGlobalValueSummary`, whose parameter is
    /// a `std::unique_ptr` — a by-value move here (design law 8a).
    pub fn add_global_value_summary(&mut self, guid: Guid, summary: GlobalValueSummary) {
        self.global_values
            .entry(guid)
            .or_default()
            .summary_list
            .push(summary);
    }

    /// The `index`-th summary recorded for `guid`, for in-place edits.
    ///
    /// No upstream counterpart as a method: `LLParser` reaches the same storage
    /// through raw `ValueInfo *` pointers saved while a summary's vectors were
    /// still local, and back-patches them once the forward-referenced `^N` is
    /// defined. llvmkit records the coordinates instead and comes back through
    /// this accessor, which is why it exists.
    pub fn summary_mut(&mut self, guid: Guid, index: usize) -> Option<&mut GlobalValueSummary> {
        self.global_values.get_mut(&guid)?.summary_list.get_mut(index)
    }

    /// Whether `guid` has a summary that came from `module_path`.
    ///
    /// Mirrors `ModuleSummaryIndex::findSummaryInModule`.
    #[must_use]
    pub fn summary_in_module(&self, guid: Guid, module_path: &str) -> Option<&GlobalValueSummary> {
        self.global_values
            .get(&guid)?
            .summary_list
            .iter()
            .find(|summary| summary.module_path == module_path)
    }

    /// The type identifiers, in ascending GUID order and, within one GUID, in
    /// insertion order.
    ///
    /// Mirrors `ModuleSummaryIndex::typeIds`, a `std::multimap`.
    #[must_use]
    pub fn type_ids(&self) -> &BTreeMap<Guid, Vec<(String, TypeIdSummary)>> {
        &self.type_ids
    }

    /// The summary for `type_id`, creating an empty one if absent.
    ///
    /// Mirrors `ModuleSummaryIndex::getOrInsertTypeIdSummary`, which matches on
    /// the *name* within the GUID's equal range because a GUID collision would
    /// otherwise merge two identifiers.
    pub fn type_id_summary(&mut self, type_id: &str) -> &mut TypeIdSummary {
        let guid = Guid::of_global_identifier(type_id);
        let entries = self.type_ids.entry(guid).or_default();
        let position = entries.iter().position(|(name, _)| name == type_id);
        let index = match position {
            Some(index) => index,
            None => {
                entries.push((type_id.to_owned(), TypeIdSummary::default()));
                entries.len() - 1
            }
        };
        &mut entries[index].1
    }

    /// The compatible-vtable lists, ordered by type-identifier name.
    ///
    /// Mirrors `ModuleSummaryIndex::typeIdCompatibleVtableMap`.
    #[must_use]
    pub fn type_id_compatible_vtables(&self) -> &BTreeMap<String, Vec<TypeIdOffsetVtableInfo>> {
        &self.type_id_compatible_vtables
    }

    /// The compatible-vtable list for `type_id`, creating an empty one if
    /// absent.
    ///
    /// Mirrors `ModuleSummaryIndex::getOrInsertTypeIdCompatibleVtableSummary`.
    pub fn type_id_compatible_vtable(&mut self, type_id: &str) -> &mut Vec<TypeIdOffsetVtableInfo> {
        self.type_id_compatible_vtables
            .entry(type_id.to_owned())
            .or_default()
    }

    /// The unique stack ids referenced by memprof summaries.
    ///
    /// Mirrors `ModuleSummaryIndex::stackIds`.
    #[must_use]
    pub fn stack_ids(&self) -> &[u64] {
        &self.stack_ids
    }

    /// The index of `stack_id`, appending it if it is new.
    ///
    /// Mirrors `ModuleSummaryIndex::addOrGetStackIdIndex`.
    pub fn stack_id_index(&mut self, stack_id: u64) -> u32 {
        if let Some(index) = self.stack_ids.iter().position(|id| *id == stack_id) {
            return u32::try_from(index).unwrap_or(u32::MAX);
        }
        let index = u32::try_from(self.stack_ids.len()).unwrap_or(u32::MAX);
        self.stack_ids.push(stack_id);
        index
    }

    /// The stack id at `index`, if the index is in range.
    ///
    /// Mirrors `ModuleSummaryIndex::getStackIdAtIndex`, whose out-of-range case
    /// is an assert; law 3 turns it into an [`Option`].
    #[must_use]
    pub fn stack_id_at_index(&self, index: u32) -> Option<u64> {
        self.stack_ids.get(usize::try_from(index).ok()?).copied()
    }

    /// The index-wide flags.
    ///
    /// Mirrors `ModuleSummaryIndex::getFlags`.
    #[inline]
    #[must_use]
    pub fn flags(&self) -> IndexFlags {
        self.flags
    }

    /// Sets the index-wide flags.
    ///
    /// Mirrors `ModuleSummaryIndex::setFlags`.
    #[inline]
    pub fn set_flags(&mut self, flags: IndexFlags) {
        self.flags = flags;
    }

    /// The total number of basic blocks the index covers.
    ///
    /// Mirrors `ModuleSummaryIndex::getBlockCount`.
    #[inline]
    #[must_use]
    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    /// Sets the total number of basic blocks the index covers.
    ///
    /// Mirrors `ModuleSummaryIndex::setBlockCount`.
    #[inline]
    pub fn set_block_count(&mut self, block_count: u64) {
        self.block_count = block_count;
    }

    /// The `^N` slot assignment the printer uses.
    #[must_use]
    pub fn slots(&self) -> SummaryIndexSlots<'_> {
        SummaryIndexSlots::new(self)
    }
}

/// The `^N` slot assignment for one index.
///
/// Mirrors `SlotTracker::processIndex`, which numbers module paths first (in
/// sorted path order), then global values by ascending GUID, then
/// compatible-vtable entries by type name, then type identifiers by GUID.
#[derive(Clone, Debug)]
pub struct SummaryIndexSlots<'index> {
    index: &'index ModuleSummaryIndex,
    module_path_base: u32,
    guid_base: u32,
    compatible_vtable_base: u32,
    type_id_base: u32,
    total: u32,
}

impl<'index> SummaryIndexSlots<'index> {
    fn new(index: &'index ModuleSummaryIndex) -> Self {
        let module_path_base = 0;
        let guid_base = module_path_base + count(index.module_paths.len());
        let compatible_vtable_base = guid_base + count(index.global_values.len());
        let type_id_base = compatible_vtable_base + count(index.type_id_compatible_vtables.len());
        let type_id_count: usize = index.type_ids.values().map(Vec::len).sum();
        let total = type_id_base + count(type_id_count);
        Self {
            index,
            module_path_base,
            guid_base,
            compatible_vtable_base,
            type_id_base,
            total,
        }
    }

    /// The number of slots assigned, which is where the trailing `flags:` and
    /// `blockcount:` entries are numbered from.
    #[inline]
    #[must_use]
    pub fn total(&self) -> u32 {
        self.total
    }

    /// The slot of a module path.
    ///
    /// Mirrors `SlotTracker::getModulePathSlot`.
    #[must_use]
    pub fn module_path(&self, path: &str) -> Option<u32> {
        self.index
            .module_paths
            .keys()
            .position(|key| key == path)
            .map(|offset| self.module_path_base + count(offset))
    }

    /// The slot of a global value.
    ///
    /// Mirrors `SlotTracker::getGUIDSlot`.
    #[must_use]
    pub fn guid(&self, guid: Guid) -> Option<u32> {
        self.index
            .global_values
            .keys()
            .position(|key| *key == guid)
            .map(|offset| self.guid_base + count(offset))
    }

    /// The slot of a compatible-vtable entry.
    ///
    /// Mirrors `SlotTracker::getTypeIdCompatibleVtableSlot`.
    #[must_use]
    pub fn type_id_compatible_vtable(&self, type_id: &str) -> Option<u32> {
        self.index
            .type_id_compatible_vtables
            .keys()
            .position(|key| key == type_id)
            .map(|offset| self.compatible_vtable_base + count(offset))
    }

    /// The slot of a type identifier, named rather than keyed by GUID because a
    /// GUID may carry more than one name.
    ///
    /// Mirrors `SlotTracker::getTypeIdSlot`.
    #[must_use]
    pub fn type_id(&self, type_id: &str) -> Option<u32> {
        let mut offset = 0usize;
        for entries in self.index.type_ids.values() {
            for (name, _) in entries {
                if name == type_id {
                    return Some(self.type_id_base + count(offset));
                }
                offset += 1;
            }
        }
        None
    }
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        FunctionSummary, GlobalValueFlags, GlobalValueSummary, Guid, ImportKind,
        ModuleSummaryIndex, SummaryKind, global_identifier,
    };
    use crate::global_value::{Linkage, Visibility};

    /// The GUIDs `llvm-dis` prints beside the type-identifier entries of
    /// `llvm/test/Assembler/thinlto-summary.ll`, which are that file's own
    /// `; guid = N` CHECK lines.
    #[test]
    fn type_identifier_guids_match_upstream() {
        assert_eq!(
            Guid::of_global_identifier("_ZTS1C").raw(),
            1_884_921_850_105_019_584
        );
        assert_eq!(
            Guid::of_global_identifier("_ZTS1B").raw(),
            6_203_814_149_063_363_976
        );
        assert_eq!(
            Guid::of_global_identifier("_ZTS1A").raw(),
            7_004_155_349_499_253_778
        );
        assert_eq!(
            Guid::of_global_identifier("_ZTS1D").raw(),
            9_614_786_172_484_273_522
        );
        assert_eq!(
            Guid::of_global_identifier("_ZTS1E").raw(),
            17_437_243_864_166_745_132
        );
    }

    /// The GUID `llvm/test/Assembler/summary-flags.ll` records for `main`,
    /// which its own summary entry spells out.
    #[test]
    fn external_value_guid_matches_upstream() {
        assert_eq!(
            Guid::of_global_identifier(&global_identifier("main", Linkage::External, "tmp.bc"))
                .raw(),
            15_822_663_052_811_949_562
        );
    }

    /// Mirrors `GlobalValue::getGlobalIdentifier`: only a local linkage takes
    /// the source file name prefix, an absent file name becomes `<unknown>`,
    /// and a leading `\1` is stripped.
    #[test]
    fn global_identifier_prefixes_only_local_linkage() {
        assert_eq!(global_identifier("f", Linkage::External, "a.c"), "f");
        assert_eq!(global_identifier("f", Linkage::Internal, "a.c"), "a.c;f");
        assert_eq!(global_identifier("f", Linkage::Private, "a.c"), "a.c;f");
        assert_eq!(global_identifier("f", Linkage::Internal, ""), "<unknown>;f");
        assert_eq!(global_identifier("\u{1}f", Linkage::External, "a.c"), "f");
    }

    /// Mirrors `SlotTracker::processIndex`: module paths are numbered in sorted
    /// path order, not insertion order, and global values follow in ascending
    /// GUID order.
    #[test]
    fn slots_follow_upstream_numbering() {
        let mut index = ModuleSummaryIndex::new();
        index.add_module("b.o", [0; 5]);
        index.add_module("a.o", [0; 5]);
        index.global_value_entry(Guid::from_raw(9));
        index.global_value_entry(Guid::from_raw(4));

        let slots = index.slots();
        assert_eq!(slots.module_path("a.o"), Some(0));
        assert_eq!(slots.module_path("b.o"), Some(1));
        assert_eq!(slots.guid(Guid::from_raw(4)), Some(2));
        assert_eq!(slots.guid(Guid::from_raw(9)), Some(3));
        assert_eq!(slots.total(), 4);
    }

    /// Mirrors `ModuleSummaryIndex::getOrInsertTypeIdSummary`, which keys on
    /// the name inside the GUID's equal range, and
    /// `ModuleSummaryIndex::addOrGetStackIdIndex`.
    #[test]
    fn type_id_and_stack_id_lookups_are_idempotent() {
        let mut index = ModuleSummaryIndex::new();
        index
            .type_id_summary("_ZTS1A")
            .type_test_resolution
            .size_minus_one = 7;
        assert_eq!(
            index
                .type_id_summary("_ZTS1A")
                .type_test_resolution
                .size_minus_one,
            7
        );
        assert_eq!(index.type_ids().values().map(Vec::len).sum::<usize>(), 1);

        assert_eq!(index.stack_id_index(11), 0);
        assert_eq!(index.stack_id_index(22), 1);
        assert_eq!(index.stack_id_index(11), 0);
        assert_eq!(index.stack_id_at_index(1), Some(22));
        assert_eq!(index.stack_id_at_index(2), None);
    }

    /// The expected text is the `CHECK` block of
    /// `llvm/test/Assembler/thinlto-multiple-summaries-for-guid.ll`, which is
    /// `llvm-dis` output for the index that file's `^N` entries describe: two
    /// module paths and one GUID carrying two function summaries. The trailing
    /// `blockcount` line is what `printModuleSummaryIndex` always emits; the
    /// fixture's `CHECK-NEXT` chain simply stops before it.
    #[test]
    fn index_prints_upstream_bytes() {
        let mut index = ModuleSummaryIndex::new();
        index.add_module("[Regular LTO]", [0, 0, 0, 0, 0]);
        index.add_module(
            "main.bc",
            [
                3_499_594_384,
                1_671_013_073,
                3_271_036_935,
                1_830_411_232,
                59_290_952,
            ],
        );

        let guid = Guid::from_raw(13_351_721_993_301_222_997);
        let summary = |linkage: Linkage, not_eligible_to_import: bool| GlobalValueSummary {
            module_path: "main.bc".to_owned(),
            flags: GlobalValueFlags {
                linkage,
                visibility: Visibility::Default,
                not_eligible_to_import,
                live: true,
                dso_local: true,
                can_auto_hide: false,
                import_type: ImportKind::Definition,
            },
            references: Vec::new(),
            kind: SummaryKind::Function(FunctionSummary {
                instruction_count: 1,
                ..FunctionSummary::default()
            }),
        };
        index.add_global_value_summary(guid, summary(Linkage::LinkOnceOdr, false));
        index.add_global_value_summary(guid, summary(Linkage::AvailableExternally, true));

        assert_eq!(
            index.to_string(),
            concat!(
                "\n",
                "^0 = module: (path: \"[Regular LTO]\", hash: (0, 0, 0, 0, 0))\n",
                "^1 = module: (path: \"main.bc\", hash: (3499594384, 1671013073, 3271036935, 1830411232, 59290952))\n",
                "^2 = gv: (guid: 13351721993301222997, summaries: (function: (module: ^1, flags: (linkage: linkonce_odr, visibility: default, notEligibleToImport: 0, live: 1, dsoLocal: 1, canAutoHide: 0, importType: definition), insts: 1), function: (module: ^1, flags: (linkage: available_externally, visibility: default, notEligibleToImport: 1, live: 1, dsoLocal: 1, canAutoHide: 0, importType: definition), insts: 1)))\n",
                "^3 = blockcount: 0\n",
            )
        );
    }
}
