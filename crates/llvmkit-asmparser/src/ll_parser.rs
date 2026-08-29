//! Textual `.ll` parser — module-level slice.
//!
//! Mirrors the parser entry points in `llvm/lib/AsmParser/LLParser.cpp`. The
//! shipped surface is the smallest constructive subset that
//! lets a real `.ll` module be ingested into the existing typed
//! [`llvmkit_ir::Module`]:
//!
//! - [`Parser::parse_module`] — `LLParser::Run`, top-level dispatch.
//! - [`Parser::parse_type`] — `LLParser::parseType`, the full type grammar
//!   over the type categories llvmkit-ir already models (primitive ints /
//!   floats / pointer / void / label / metadata / token / x86_amx, array,
//!   vector, anonymous + packed structs, named / numbered struct
//!   references, function types, `addrspace(N)` pointers).
//! - Top-level entities: `target datalayout / triple`, `source_filename`,
//!   `module asm`, `%foo = type ...` and `%0 = type ...` definitions, and
//!   the simple `@name = global TY CONST` / `@name = constant TY CONST`
//!   global form.
//!
//! Function bodies, attribute groups, comdats, metadata, summaries, and
//! aliases land incrementally per the parser-first roadmap.
//!
//! Parser style notes:
//! - Recursive-descent, one-token lookahead. The `current` slot caches the
//!   most recently produced lexer token; helper methods peek at it and only
//!   advance on a structural match (mirrors the
//!   `Lex.getKind() == lltok::X` pattern in `LLParser.cpp`).
//! - All errors funnel through [`crate::parse_error::ParseError`].
//! - Cross-module mixing is rejected by the borrow checker through the
//!   `'ctx` brand on [`llvmkit_ir::Module`].

use core::cell::RefCell;
use core::marker::PhantomData;
use std::borrow::Cow;

use llvmkit_ir::DataLayout;
use llvmkit_ir::attributes::{
    AttrIndex, AttrKind, Attribute, AttributeStorage, MemoryEffects, MemoryLocation, ModRefInfo,
};
use llvmkit_ir::constant_range::ConstantRange;
use llvmkit_ir::metadata::{
    DebugMetadataOperand, DebugRecord, MetadataAttachmentKind, MetadataFieldValue, MetadataId,
    MetadataKind,
};
use std::collections::BTreeMap;
use std::collections::HashMap;

use llvmkit_ir::{
    Align, AnyTypeEnum, ApFloat, ApFloatSemantics, ApFloatSign, ApInt, AtomicOrdering,
    AtomicRmwBinOp, CallingConv, Constant, ConstantExprFlags, ConstantExprInRange,
    ConstantExprOpcode, ConstantExprOptions, DllStorageClass, Dyn, FastMathFlags, FloatPredicate,
    FpClassTest, GepNoWrapFlags, IntCastFlags, IntDyn, IntType, IntValue, IntrinsicNameResolution,
    IrBuilder, IrError, IrResult, Linkage, MaybeAlign, Module, ModuleBrand, NoFolder, PointerValue,
    Positioned, RoundingMode, SelectionKind, Signedness, StructType, SyncScope, ThreadLocalMode,
    Type, TypeKind, UiToFpFlags, UnnamedAddr, Unverified, ValueCategory, Visibility,
    derived_types::PointerType, resolve_intrinsic_name, shufflevector_mask_from_constant,
};
use llvmkit_ir::{FunctionValue, IsValue};
use llvmkit_macros::Branded;
use llvmkit_support::{Span, Spanned};

use super::asm_parser_context::AsmParserContext;
use super::file_loc::{FileLoc, FileLocRange};
use super::ll_lexer::{LexError, Lexer};
use super::ll_token::Opcode;
use super::ll_token::{IntLit, Keyword, NumBase, PrimitiveTy, Sign, Token};
use super::parser::ParserConfig;
use llvmkit_ir::module_summary_index::{
    AccessSpecifier, AliasSummary, AllocationInfo, AllocationType, CallEdge, CallsiteInfo,
    ConstantVirtualCall, FunctionFlags, FunctionSummary, GlobalValueFlags, GlobalValueSummary,
    GlobalVariableFlags, GlobalVariableSummary, Guid, Hotness, ImportKind, IndexFlags,
    MemoryInfoBlock, ModuleSummaryIndex, PARAMETER_ACCESS_RANGE_WIDTH, ParameterAccess,
    ParameterAccessCall, SummaryKind, TypeIdInfo, TypeIdOffsetVtableInfo, TypeIdSummary,
    TypeTestResolution, TypeTestResolutionKind, VCallVisibility, ValueReference, VirtualFunctionId,
    VirtualFunctionOffset, WholeProgramDevirtByArg, WholeProgramDevirtByArgKind,
    WholeProgramDevirtKind, WholeProgramDevirtResolution, global_identifier,
};

use super::numbered_values::AddError;
use super::numbered_values::NumberedValues;
use super::parse_error::{DiagLoc, ParseError, ParseResult};
use super::parse_error::{SymbolId, SymbolKind};
use super::slot_mapping::{GlobalRef, SlotMapping};

/// Unwrap an `IrResult` from a metadata API the parser drives against **its own**
/// module.
///
/// Every metadata id the parser hands back to `self.module` was minted by that
/// same module a moment earlier (`metadata_reserve`, `metadata_string`,
/// `metadata_node`, ...), so the module-tag check on the way in cannot fail and
/// the slot cannot be out of range. Naming that once here keeps the parser free
/// of a raw-slot escape hatch: it speaks the tagged currency like any other
/// caller and simply has no foreign ids to hand over.
fn own_metadata<T>(result: IrResult<T>) -> T {
    result.expect("metadata id minted by the module the parser is populating")
}

/// Byte offset of the first byte of each line of `src`, `[0]` first.
///
/// Stands in for the line table `SourceMgr` builds lazily in
/// `SourceMgr::getLineAndColumn`. A line ends at `\n`; a `\r` before it is an
/// ordinary column, which is what `SourceMgr` counts too.
fn line_start_offsets(src: &[u8]) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (index, byte) in src.iter().enumerate() {
        if *byte == b'\n'
            && let Ok(next) = u32::try_from(index + 1)
        {
            starts.push(next);
        }
    }
    starts
}

// ── Type pre-resolution table (mirrors LLParser::NamedTypes / NumberedTypes) ─

/// A type that has been parsed but may carry an unresolved forward
/// reference to an opaque-named struct. Mirrors the
/// `std::pair<Type *, LocTy>` entries in `LLParser`'s `NamedTypes` /
/// `NumberedTypes` maps: we keep the type handle plus the location of the
/// most recent forward reference so `validateEndOfModule` can
/// blame the right span if the definition never lands.
#[derive(Branded)]
#[branded(Debug, Clone, Copy)]
struct TypeEntry<'ctx, B: ModuleBrand> {
    ty: Type<'ctx, B>,
    /// Where this type was first *referenced*, when no definition has been
    /// seen yet. `None` means defined.
    ///
    /// Upstream stores the same fact as the `LocTy` half of
    /// `NamedTypes` / `NumberedTypes`'s `std::pair<Type *, LocTy>`, where an
    /// *invalid* location means "defined". That one bit drives both
    /// `redefinition of type` and `use of undefined type`.
    forward_ref_loc: Option<Span>,
}

struct MetadataSlotEntry<B: ModuleBrand> {
    id: MetadataId<B>,
    defined: bool,
    first_ref: Span,
}

struct FunctionSuffix<'ctx, B: ModuleBrand> {
    attr_groups: Vec<u32>,
    /// `BuiltinLoc` — where a `builtin` attribute was written in the header,
    /// which `parseFunctionHeader` rejects once the whole chain has parsed.
    builtin_loc: Option<Span>,
    section: Option<String>,
    partition: Option<String>,
    comdat: Option<Option<String>>,
    align: MaybeAlign,
    gc: Option<String>,
    prefix_data: Option<llvmkit_ir::Constant<'ctx, B>>,
    prologue_data: Option<llvmkit_ir::Constant<'ctx, B>>,
    personality_fn: Option<ParsedPersonalityFn<'ctx, B>>,
    _marker: core::marker::PhantomData<&'ctx ()>,
}

impl<'ctx, B: ModuleBrand + 'ctx> Default for FunctionSuffix<'ctx, B> {
    fn default() -> Self {
        Self {
            attr_groups: Vec::new(),
            builtin_loc: None,
            section: None,
            partition: None,
            comdat: None,
            align: MaybeAlign::NONE,
            gc: None,
            prefix_data: None,
            prologue_data: None,
            personality_fn: None,
            _marker: PhantomData,
        }
    }
}

enum ParsedPersonalityFn<'ctx, B: ModuleBrand> {
    Resolved(llvmkit_ir::Constant<'ctx, B>),
    /// `ty` is the pointer type the `personality` clause spelled. Upstream
    /// never defers, so `getGlobalVal` sees that type directly; llvmkit has
    /// to carry it to the end-of-module fixup or the deferred reference would
    /// be checked against a fabricated `ptr`.
    ForwardName {
        name: String,
        ty: Type<'ctx, B>,
        loc: Span,
    },
}

/// One entry of a parsed argument list — mirrors `LLParser::ArgInfo`.
///
/// Upstream shares `parseArgumentList` between a function *type* and a
/// function *header*, which is why an argument can carry a name and
/// attributes that the type path then rejects.
struct ArgInfo<'ctx, B: ModuleBrand> {
    /// The argument type's first token. Upstream anchors *every* argument
    /// diagnostic here (its `TypeLoc`), including the numbering check, which
    /// reports at the type rather than at the `%N` that failed.
    loc: Span,
    ty: Type<'ctx, B>,
    /// `None` when the argument carries no `%name`; its number is then the
    /// matching entry of `parse_argument_list`'s `unnamed_arg_nums` output.
    name: Option<String>,
    /// Whether an attribute list followed the type. Upstream keeps the
    /// `AttrBuilder` in `ArgInfo` and lets `parseFunctionType` ask
    /// `hasAttributes()`; here the attributes are already installed at
    /// `AttrIndex::Param(slot)`, so the answer travels instead of the set.
    has_attributes: bool,
}

/// Whether `attrs` carries anything at `index`.
///
/// `AttributeStorage::get` is private to `llvmkit-ir`, so ask the public
/// set-equality predicate against an empty storage instead.
fn has_attributes_at(attrs: &AttributeStorage, index: AttrIndex) -> bool {
    !attrs.index_has_same_attributes(&AttributeStorage::new(), index)
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// Core parser state. Holds the lexer, a one-token cache, the IR module
/// being populated, and the slot tables that mirror upstream's
/// `LLParser::NumberedTypes` / `NamedTypes` / `NumberedVals` fields.
pub struct Parser<'src, 'ctx, B: ModuleBrand> {
    lex: Lexer<'src>,
    src: &'src [u8],
    /// Most recently produced token. The constructor primes this with the
    /// first token (mirrors `LLParser::Run`'s leading `Lex.Lex();`).
    current: Spanned<Token<'src>>,
    /// Byte offset one past the end of the token *before* [`Self::current`].
    /// Mirrors `LLLexer::PrevTokEnd`, which `LLLexer::LexToken` sets to
    /// `CurPtr` on entry — before whitespace is skipped, so it is the end of
    /// the last token consumed and not the start of the next one. This is the
    /// exclusive end every `AsmParserContext` range is closed at
    /// (`LLLexer::getPrevTokEndLineColumnPos`).
    prev_token_end: u32,
    /// Byte offset of the first byte of each source line, `line_starts[0] == 0`.
    /// Stands in for `SourceMgr::getLineAndColumn`, which llvmkit has no
    /// equivalent of because the lexer works in byte offsets throughout.
    /// Built only when [`Self::parser_context`] is `Some`; an ordinary parse
    /// never pays for it.
    line_starts: Vec<u32>,
    /// The file-location registry being filled, when the caller asked for one.
    /// Mirrors `LLParser::ParserContext`: a `None` here is upstream's null
    /// pointer, which makes every `addFunctionLocation` / `addBlockLocation` /
    /// `addInstructionLocation` site a no-op.
    parser_context: Option<AsmParserContext<'ctx, B>>,

    /// The module token being populated.
    module: &'ctx Module<B, Unverified>,

    /// Named struct-type table (`%foo = type {...}`).
    named_types: HashMap<String, TypeEntry<'ctx, B>>,
    /// Numbered struct-type table (`%0 = type {...}`).
    numbered_types: HashMap<u32, TypeEntry<'ctx, B>>,
    /// Slot id of the next anonymous numbered type, mirroring upstream's
    /// `LLParser::NumberedTypes`'s `getNext()` discipline.
    next_unnamed_type_id: u32,

    /// Numbered global / function table. Exposed via [`Parser::take_slot_mapping`].
    numbered_globals: NumberedValues<GlobalRef<'ctx, B>>,
    numbered_attr_groups: NumberedValues<llvmkit_ir::attributes::AttributeStorage>,

    /// Maps a textual metadata slot (`!N`) to the [`MetadataId`] it names and
    /// whether a matching `!N = ...` definition was seen.
    metadata_slots: HashMap<u32, MetadataSlotEntry<B>>,
    deferred_block_addresses: Vec<DeferredBlockAddress<'ctx, B>>,
    deferred_personality_fns: Vec<DeferredPersonalityFn<'ctx, B>>,
    deferred_alias_targets: Vec<DeferredAliasTarget<'ctx, B>>,
    deferred_intrinsic_attribute_checks: Vec<DeferredIntrinsicAttributeCheck>,
    /// Every instruction that took a `!tbaa` attachment, in attachment order.
    /// Mirrors `LLParser::InstsWithTBAATag`, which `parseInstructionMetadata`
    /// fills and `validateEndOfModule` drains through `UpgradeTBAANode`.
    /// Global-object attachments are deliberately not recorded — upstream
    /// pushes only from the instruction routine.
    insts_with_tbaa_tag: Vec<llvmkit_ir::InstructionView<'ctx, B>>,
    /// `@name` referenced before it was defined, holding the placeholder
    /// minted at the first use. Mirrors `LLParser::ForwardRefVals`; ordered
    /// because `validateEndOfModule` reports `begin()`.
    /// `$name` used before `$name = comdat ...` was seen, with the first
    /// use's span. Mirrors `LLParser::ForwardRefComdats`; ordered because
    /// `validateEndOfModule` reports `begin()`.
    forward_ref_comdats: BTreeMap<String, Span>,
    forward_ref_globals: BTreeMap<String, ForwardRef<'ctx, B>>,
    /// `@N` referenced before it was defined. Mirrors
    /// `LLParser::ForwardRefValIDs`.
    forward_ref_global_ids: BTreeMap<u32, ForwardRef<'ctx, B>>,
    /// `dso_local_equivalent @N` whose `@N` was not defined yet, holding the
    /// placeholder minted at the first such reference. Mirrors
    /// `LLParser::ForwardRefDSOLocalEquivalentIDs`.
    ///
    /// Deliberately **not** `forward_ref_global_ids`: that map resolves a
    /// reference to the global itself, so a placeholder parked there would
    /// become the bare `@N` rather than a `dso_local_equivalent` of it.
    /// Upstream keeps the two apart for the same reason.
    forward_ref_dso_local_equivalent_ids: BTreeMap<u32, ForwardRef<'ctx, B>>,
    /// The `@name` half. Mirrors
    /// `LLParser::ForwardRefDSOLocalEquivalentNames`; ordered because
    /// `validateEndOfModule` drains it in key order, ids before names.
    forward_ref_dso_local_equivalent_names: BTreeMap<String, ForwardRef<'ctx, B>>,
    /// `no_cfi @name` / `@N` whose referent was not defined yet.
    ///
    /// Upstream needs no map: its `kw_no_cfi` arm only sets `ValID::NoCFI`,
    /// and `convertValIDToValue` wraps whatever `getGlobalVal` returns —
    /// including the forward-reference placeholder — in a `NoCFIValue`, which
    /// re-interns itself through `handleOperandChange` when the placeholder is
    /// RAUW'd. llvmkit's `ConstantData::NoCfi` deliberately does not register
    /// an operand use (`docs/divergences.md` D2/D3), so nothing would rewrite
    /// it; the wrapper is built at end of module instead. The *reference*
    /// still goes into `forward_ref_globals` / `forward_ref_global_ids`
    /// exactly as `getGlobalVal` does, so an undefined referent is reported by
    /// upstream's own `ForwardRefVals` sweep, in its position and with its
    /// wording.
    pending_no_cfi: Vec<PendingNoCfi<'ctx, B>>,

    /// The summary index being filled, when the caller asked for one. Mirrors
    /// `LLParser::Index`: a `None` here is upstream's null `Index`, which makes
    /// `parseSummaryEntry` skip the entry rather than parse it.
    summary_index: Option<ModuleSummaryIndex>,
    /// Whether module-level entities are parsed at all. Mirrors upstream's
    /// non-null `LLParser::M`; a `false` here is its index-only mode, where
    /// `parseTopLevelEntities` reads `^N` and `source_filename` and lexes past
    /// everything else.
    parses_module_entities: bool,
    /// `^N` of a module entry to its path. Mirrors `LLParser::ModuleIdMap`.
    summary_module_paths: BTreeMap<u32, String>,
    /// `^N` of a global-value entry to the GUID it resolved to. Mirrors
    /// `LLParser::NumberedValueInfos`, whose holes are empty `ValueInfo`s.
    numbered_value_infos: Vec<Option<Guid>>,
    /// Sites that referenced a `^N` before it was defined, keyed by that `^N`.
    /// Mirrors `LLParser::ForwardRefValueInfos`; ordered because
    /// `validateEndOfIndex` reports `begin()`.
    forward_ref_summary_values: BTreeMap<u32, Vec<(SummaryValueRefSite, Span)>>,
    /// Alias summaries whose aliasee `^N` was not yet defined. Mirrors
    /// `LLParser::ForwardRefAliasees`.
    forward_ref_summary_aliasees: BTreeMap<u32, Vec<(SummaryAliaseeSite, Span)>>,
    /// Sites that referenced a type identifier's `^N` before it was defined.
    /// Mirrors `LLParser::ForwardRefTypeIds`.
    forward_ref_summary_type_ids: BTreeMap<u32, Vec<(SummaryTypeIdRefSite, Span)>>,
    /// References the summary currently being parsed made to `^N`s that are not
    /// yet defined, before the summary has a place in the index to name them
    /// by. Drained by [`Parser::add_global_value_to_index`].
    ///
    /// Upstream needs no equivalent: it saves raw `ValueInfo *` pointers into
    /// vectors that are still local and stay valid across the move into the
    /// summary. llvmkit patches by coordinate, and coordinates only exist once
    /// the summary is in the index.
    pending_summary_value_refs: Vec<(u32, SummaryValueRefField, Span)>,
    /// Type-identifier references, same shape and for the same reason.
    pending_summary_type_ids: Vec<(u32, SummaryTypeIdRefField, Span)>,
    /// The aliasee `^N` of the alias summary being parsed, when it was not yet
    /// defined.
    pending_summary_aliasee: Option<(u32, Span)>,

    /// Whether a `#dbg_*` record has been seen. Mirrors
    /// `LLParser::SeenNewDbgInfoFormat`.
    seen_new_dbg_info_format: bool,
    /// Whether a call to `llvm.dbg.declare` / `.value` / `.assign` has been
    /// seen. Mirrors `LLParser::SeenOldDbgInfoFormat`.
    ///
    /// A module may use one debug-info format or the other, never both, and
    /// the two flags are what catch the mixture. Upstream asserts they are
    /// never both set by the time `validateEndOfModule` runs, which is exactly
    /// the invariant these two diagnostics maintain.
    seen_old_dbg_info_format: bool,
    _brand: PhantomData<B>,
}

/// Where inside the index a `ValueInfo` that named a forward `^N` lives.
///
/// Upstream records the same thing as a `ValueInfo *`. That works because the
/// vectors it points into are heap-allocated and survive the move into the
/// summary; llvmkit records where to look instead.
#[derive(Clone, Debug)]
struct SummaryValueRefSite {
    /// The global value whose summary carries the reference.
    owner: Guid,
    /// Which of that value's summaries.
    summary: usize,
    /// Which field of it.
    field: SummaryValueRefField,
}

/// The field of one summary that carries a `ValueInfo`.
#[derive(Clone, Debug)]
enum SummaryValueRefField {
    /// `calls: ((callee: ^N, ...))`.
    Call(usize),
    /// `refs: (^N)`.
    Reference(usize),
    /// `vTableFuncs: ((virtFunc: ^N, ...))`.
    VtableFunction(usize),
    /// `callsites: ((callee: ^N, ...))`.
    Callsite(usize),
    /// `params: ((param: N, ..., calls: ((callee: ^N, ...))))`.
    ParameterAccessCall { parameter: usize, call: usize },
    /// `typeidCompatibleVTable: (name: "...", summary: ((offset: N, ^M)))`,
    /// which hangs off a type identifier rather than a global value.
    CompatibleVtable { type_id: String, index: usize },
}

/// The alias summary whose `aliasee:` named a forward `^N`.
#[derive(Clone, Copy, Debug)]
struct SummaryAliaseeSite {
    owner: Guid,
    summary: usize,
}

/// Where inside the index a type-identifier GUID that named a forward `^N`
/// lives. Upstream records a `GlobalValue::GUID *`.
#[derive(Clone, Debug)]
struct SummaryTypeIdRefSite {
    owner: Guid,
    summary: usize,
    field: SummaryTypeIdRefField,
}

/// One `funcFlags:` field, named so the keyword can be matched before the
/// value is read without holding a borrow of the flag struct across the read.
#[derive(Clone, Copy, Debug)]
enum FunctionFlagField {
    ReadNone,
    ReadOnly,
    NoRecurse,
    ReturnDoesNotAlias,
    NoInline,
    AlwaysInline,
    NoUnwind,
    MayThrow,
    HasUnknownCall,
    MustBeUnreachable,
}

/// A `^N` reference as `parseGVReference` leaves it: the value it resolved to
/// (a placeholder when it did not), the id it named, and whether the id is
/// still owed a definition.
#[derive(Clone, Copy, Debug)]
struct ParsedGvReference {
    value: ValueReference,
    summary_id: u32,
    is_forward: bool,
}

/// The field of one function summary that carries a type-identifier GUID.
#[derive(Clone, Copy, Debug)]
enum SummaryTypeIdRefField {
    /// `typeIdInfo: (typeTests: (^N))`.
    Test(usize),
    /// `typeIdInfo: (typeTestAssumeVCalls: (vFuncId: (^N, ...)))`.
    AssumeVcall(usize),
    /// `typeIdInfo: (typeCheckedLoadVCalls: (vFuncId: (^N, ...)))`.
    CheckedLoadVcall(usize),
    /// `typeIdInfo: (typeTestAssumeConstVCalls: ((vFuncId: (^N, ...))))`.
    AssumeConstVcall(usize),
    /// `typeIdInfo: (typeCheckedLoadConstVCalls: ((vFuncId: (^N, ...))))`.
    CheckedLoadConstVcall(usize),
}

/// One parsed `ParamAccess` and, for each of its calls that named a forward
/// `^N`, the call's index in the access plus that id and its span.
type ParsedParameterAccess = (ParameterAccess, Vec<(usize, u32, Span)>);

/// A cursor summary, not a dump. The parser owns every module-level slot
/// table it has filled so far, so a derived `Debug` would print each parsed
/// type, numbered global, and metadata slot at every `dbg!`. What a caller
/// debugging a parse actually wants is where the cursor is, what it is
/// looking at, and how much has resolved — table *sizes*, not contents.
impl<B: ModuleBrand> core::fmt::Debug for Parser<'_, '_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Parser")
            .field("module", &self.module.name())
            .field("module_id", &self.module.id())
            .field("source_len", &self.src.len())
            .field("position", &self.lex.position())
            .field("token", &format_args!("{}", self.current.value))
            .field("token_span", &self.current.span)
            .field("named_types", &self.named_types.len())
            .field("numbered_types", &self.numbered_types.len())
            .field("metadata_slots", &self.metadata_slots.len())
            .finish_non_exhaustive()
    }
}

/// What the parser produces at end-of-module. Successful runs return the
/// module-level slot mapping so callers can re-use it for follow-on
/// `parse_constant_value` / `parse_type` calls (mirrors upstream's
/// `parseAssemblyString(..., SlotMapping *)` pattern).
///
/// The three fields are upstream's three out-parameters. `SlotMapping *` and
/// `AsmParserContext *` are filled in place there and are null when the caller
/// did not ask for them; `ModuleSummaryIndex` comes back beside the module in
/// `ParsedModuleAndIndex`. Here they travel together, because the by-product
/// borrows the module it was parsed against.
#[derive(Branded)]
#[branded(Debug, Default)]
pub struct ParsedModule<'ctx, B: ModuleBrand> {
    pub slot_mapping: SlotMapping<'ctx, B>,
    /// The `ModuleSummaryIndex` half of upstream's `ParsedModuleAndIndex`.
    /// `None` when the parser was built without one, which is upstream's null
    /// `LLParser::Index` — every `^N` entry is then skipped rather than parsed.
    pub summary_index: Option<ModuleSummaryIndex>,
    /// The file-location registry, when the parser was built with
    /// [`Parser::with_context`]. `None` is upstream's null
    /// `AsmParserContext *`.
    pub parser_context: Option<AsmParserContext<'ctx, B>>,
}

/// A `no_cfi @name` / `no_cfi @N` whose referent had not been defined when it
/// was read, holding the placeholder its uses were built against.
struct PendingNoCfi<'ctx, B: ModuleBrand> {
    placeholder: llvmkit_ir::ForwardRefValue<'ctx, B>,
    reference: NameOrId,
    loc: Span,
}

struct DeferredBlockAddress<'ctx, B: ModuleBrand> {
    placeholder: llvmkit_ir::ForwardRefValue<'ctx, B>,
    function: DeferredBlockAddressFunction<'ctx, B>,
    label: BlockLabel,
    /// `ID.Loc` — the `blockaddress` keyword itself, which is where
    /// `convertValIDToValue`'s `t_Constant` arm anchors a type mismatch.
    value_loc: Span,
    /// `Fn.Loc` — the `@f` operand. `validateEndOfModule`'s
    /// `ForwardRefBlockAddresses` guard reports `expected function name in
    /// blockaddress` here.
    function_loc: Span,
    /// `Label.Loc` — the `%bb` operand. Both of
    /// `PerFunctionState::resolveForwardRefBlockAddresses`'s diagnostics are
    /// anchored here.
    label_loc: Span,
}

/// Which function a deferred `blockaddress` is waiting on.
///
/// A function that has already been installed is matched by identity, not by
/// how it was spelled: `define void @0()` has no name to match on, so keying
/// an unnamed function by `NameOrId::Name("")` would strand its own
/// `blockaddress(@0, %1)` until end of module.
#[derive(Branded)]
#[branded(Clone)]
enum DeferredBlockAddressFunction<'ctx, B: ModuleBrand> {
    Installed(llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>),
    Forward(NameOrId),
}

struct DeferredPersonalityFn<'ctx, B: ModuleBrand> {
    function: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>,
    name: String,
    /// The pointer type the clause spelled — see
    /// [`ParsedPersonalityFn::ForwardName`].
    ty: Type<'ctx, B>,
    loc: Span,
}

/// An alias or ifunc whose target names a global that had not been declared
/// yet. The printer emits aliases and ifuncs before function declarations, so
/// a printed module routinely forward-references its own resolver; globals
/// have carried deferred initializers for the same reason since the parser
/// was written.
struct DeferredAliasTarget<'ctx, B: ModuleBrand> {
    object: DeferredAliasObject<'ctx, B>,
    name: String,
    /// The pointer type the `alias` / `ifunc` clause spelled, carried for the
    /// same reason [`ParsedPersonalityFn::ForwardName`] carries one.
    ty: Type<'ctx, B>,
    loc: Span,
}

enum DeferredAliasObject<'ctx, B: ModuleBrand> {
    Alias(llvmkit_ir::GlobalAlias<'ctx, B>),
    Ifunc(llvmkit_ir::GlobalIfunc<'ctx, B>),
}

struct DeferredIntrinsicAttributeCheck {
    attrs: AttributeStorage,
    attr_groups: Vec<u32>,
    expected_attrs: AttributeStorage,
    loc: Span,
}

/// How a `blockaddress` names its basic block. Upstream keeps the two
/// spellings apart as `ValID::t_LocalName` / `t_LocalID` because they resolve
/// through different tables — and because a *numeric* label is only reachable
/// while the function's own numbering is still live. Collapsing them to a
/// string, as llvmkit did, makes `blockaddress(@f, %5)` look for a block
/// literally named `"5"`, which no unnamed block ever is.
#[derive(Clone, Debug)]
enum BlockLabel {
    Named(String),
    Numbered(u32),
}

/// The `CastOpcode` a constant-expression cast opcode denotes, for
/// `cast_is_valid`. The two enums are separate because `ConstantExprOpcode`
/// also covers non-cast forms; upstream shares one `Instruction::CastOps`
/// numbering between the constant and instruction paths.
fn cast_opcode_for(opcode: ConstantExprOpcode) -> llvmkit_ir::CastOpcode {
    use llvmkit_ir::CastOpcode as C;
    match opcode {
        ConstantExprOpcode::Trunc => C::Trunc,
        ConstantExprOpcode::PtrToAddr => C::PtrToAddr,
        ConstantExprOpcode::PtrToInt => C::PtrToInt,
        ConstantExprOpcode::IntToPtr => C::IntToPtr,
        ConstantExprOpcode::BitCast => C::BitCast,
        ConstantExprOpcode::AddrSpaceCast => C::AddrSpaceCast,
        // The arm that reaches `cast_opcode_for` is gated on the cast
        // opcodes above; the remaining constant-expression opcodes are not
        // casts and never get here.
        _ => C::BitCast,
    }
}

/// Which legacy typed-pointer suffix is being applied. The two arms of
/// `LLParser::parseType`'s suffix loop word the `void` rejection differently,
/// so the caller has to say which one it is.
#[derive(Clone, Copy)]
enum PointerSuffix {
    Star,
    AddrSpace,
}

enum ParsedBlockAddressFunction<'ctx, B: ModuleBrand> {
    Resolved(llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>),
    Forward { function: NameOrId, loc: Span },
}

/// The three locations a `blockaddress` can be blamed at, kept together
/// because the routines that retire a deferred one each pick a different
/// member — see [`DeferredBlockAddress`].
#[derive(Clone, Copy)]
struct BlockAddressLocs {
    value: Span,
    function: Span,
    label: Span,
}

enum ParsedDirectCallee<'ctx, B: ModuleBrand> {
    Name {
        name: String,
        loc: Span,
    },
    Id {
        id: u32,
        loc: Span,
    },
    InlineAsm(ParsedInlineAsm),
    Value {
        v: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    },
}

/// `ValID::t_InlineAsm`'s payload: upstream packs the four keyword bits into
/// `ID.UIntVal` and the two strings into `StrVal` / `StrVal2`.
struct ParsedInlineAsm {
    asm: String,
    constraints: String,
    has_side_effects: bool,
    is_align_stack: bool,
    dialect: llvmkit_ir::AsmDialect,
    can_unwind: bool,
    /// Where the `asm` keyword began — `ID.Loc`, which every diagnostic
    /// `convertValIDToValue` raises for this arm reports against.
    loc: Span,
}

enum ParsedCallee<'ctx, B: ModuleBrand> {
    Function(llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>),
    InlineAsm(llvmkit_ir::InlineAsm<'ctx, B>),
    Indirect(llvmkit_ir::PointerValue<'ctx, B>),
}

impl<'ctx, B: ModuleBrand> ParsedCallee<'ctx, B> {
    /// The one `Value *Callee` that `LLParser::convertValIDToValue` writes
    /// through its out-parameter. Upstream's switch over `ValID::Kind` ends in
    /// a single erased value and every call/invoke/callbr construction site
    /// downstream sees only that; llvmkit keeps the variants because
    /// `parse_invoke` and `parse_callbr` each still reach a *different*
    /// builder entry point per callee shape (`docs/future-work.md`), so the
    /// collapse is spelled here.
    fn as_erased(&self) -> llvmkit_ir::Value<'ctx, B> {
        match self {
            ParsedCallee::Function(f) => IsValue::as_erased(*f),
            ParsedCallee::InlineAsm(asm) => asm.as_erased(),
            ParsedCallee::Indirect(p) => IsValue::as_erased(*p),
        }
    }
}

/// What an attribute list yields besides the attributes themselves — the two
/// out-parameters of `LLParser::parseFnAttributeValuePairs`, returned rather
/// than written through (ported-type design law 6).
#[derive(Default)]
struct ParsedAttrList {
    /// `#N` group references, resolved at end of module (`FwdRefAttrGrps`).
    groups: Vec<u32>,
    /// Where `builtin` was written, if it was (`BuiltinLoc`).
    /// `parseFunctionHeader` is the only caller that reads it, to report
    /// `'builtin' attribute not valid on function`; a *call site* may carry
    /// the attribute, which is why the check cannot live in the loop.
    builtin_loc: Option<Span>,
}

/// Which of upstream's attribute-list productions a call to
/// `Parser::parse_fn_attribute_value_pairs` is playing.
///
/// llvmkit merges three upstream shapes into one loop, and they disagree on
/// more than one axis, so a bool cannot express it: `InAttrGrp` selects the
/// `align = N` equals grammar and turns an unrecognised token into a hard
/// error, while *group references* are legal only in the one context where
/// `InAttrGrp` is false and the list belongs to a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttrListContext {
    /// A function header or call site: `parseFnAttributeValuePairs` with
    /// `InAttrGrp == false`. `#N` references are collected, and a token that
    /// is not an attribute simply ends the list.
    FunctionHeader,
    /// The body of `attributes #N = { … }`: `parseFnAttributeValuePairs` with
    /// `InAttrGrp == true`. `align` and `alignstack` take their equals form,
    /// a `#N` reference is `cannot have an attribute group reference in an
    /// attribute group`, and a token that is not an attribute is
    /// `unterminated attribute group`.
    AttributeGroup,
    /// A parameter or return list: `parseOptionalParamOrReturnAttrs`, which
    /// upstream writes as a separate function. It has no `#N` arm at all and
    /// always passes `InAttrGroup == false`.
    ParamOrReturn,
}

impl AttrListContext {
    /// `InAttrGroup`, as `parseEnumAttribute` takes it.
    fn in_attr_group(self) -> bool {
        matches!(self, Self::AttributeGroup)
    }
}

/// An integer literal token, holding exactly what `LLLexer` puts in
/// `APSIntVal`: the value at the width the *token itself* needs, plus the
/// signedness that decides how a consumer widens it. Mirrors `APSInt`.
///
/// The width is never the destination's. That is what makes
/// `LLParser::parseRangeAttr`'s `integer is too large for the bit width of
/// specified type` askable at all — the check is on
/// `Lex.getAPSIntVal().getBitWidth()` — and it is what makes `s0x0F` the
/// **−1** upstream reads rather than the `+15` a destination-width parse
/// gives, because `LLLexer` truncates a `[us]0x` literal to its active bits
/// before stamping the signedness on it.
#[derive(Debug, Clone)]
struct ParsedApsInt {
    value: ApInt,
    signedness: Signedness,
}

/// The signedness `LLLexer` stamps on an integer token's `APSInt`, and what
/// `Lex.getAPSIntVal().isSigned()` reads back off it.
///
/// Two upstream sites decide it between them. `LLLexer::lexIdentifier`'s
/// `[us]0x[0-9A-Fa-f]+` block passes `TokStart[0] == 'u'` as `APSInt`'s
/// `isUnsigned` flag, so `u0x…` is unsigned and `s0x…` signed; every other
/// integer spelling reaches `APSInt::APSInt(StringRef)`, which is signed
/// exactly when `Str[0] == '-'`.
fn int_lit_signedness(lit: IntLit<'_>) -> Signedness {
    match lit.base {
        NumBase::HexSigned => Signedness::Signed,
        NumBase::HexUnsigned => Signedness::Unsigned,
        NumBase::Dec => match lit.sign {
            Sign::Neg => Signedness::Signed,
            Sign::Pos => Signedness::Unsigned,
        },
    }
}

impl ParsedApsInt {
    /// The token's own bit width, as `APSInt::getBitWidth` reports it.
    fn bit_width(&self) -> u32 {
        self.value.bit_width()
    }

    /// Widen (or narrow) to `dest_width` by the token's signedness. Mirrors
    /// `APSInt::extend` where the caller has already checked the width fits,
    /// and `APInt::extOrTrunc` where it has not.
    fn extend_or_truncate(&self, dest_width: u32) -> ApInt {
        match self.signedness {
            Signedness::Signed => self.value.sext_or_trunc(dest_width),
            Signedness::Unsigned => self.value.zext_or_trunc(dest_width),
        }
    }
}

#[derive(Branded)]
#[branded(Debug)]
enum ValIdKind<'ctx, B: ModuleBrand> {
    LocalId(u32),
    GlobalId(u32),
    LocalName(String),
    GlobalName(String),
    ApsInt(ParsedApsInt),
    ApFloat(ApFloat),
    Null,
    Undef,
    Poison,
    Zero,
    Constant(llvmkit_ir::Constant<'ctx, B>),
    Value(llvmkit_ir::Value<'ctx, B>),
    ConstantSplat(llvmkit_ir::Constant<'ctx, B>),
    /// `[]`. Upstream's `t_EmptyArray` deliberately carries no type: with no
    /// elements there is nothing to derive one from.
    EmptyArray,
    /// `{ ... }` — the elements only. The struct type they are checked
    /// against belongs to `convertValIDToValue`.
    ConstantStruct(Vec<llvmkit_ir::Constant<'ctx, B>>),
    /// `<{ ... }>`, kept distinct so the packedness check has something to
    /// compare against.
    PackedConstantStruct(Vec<llvmkit_ir::Constant<'ctx, B>>),
}

/// `LLParser::ValID` — the value form together with the location
/// `parseValID` records as its **first** statement, `ID.Loc = Lex.getLoc();`.
///
/// Upstream keeps both in one struct, and every diagnostic
/// `convertValIDToValue` raises reports at `ID.Loc` — the ValID's own first
/// token, not wherever the lexer has since advanced to. llvmkit spells the
/// `Kind`-plus-payload half as a Rust enum ([`ValIdKind`]), so the `Loc`
/// member travels beside it here rather than inside it. It is a field and not
/// a parameter on purpose: a parameter is what was being passed wrongly.
struct ValId<'ctx, B: ModuleBrand> {
    kind: ValIdKind<'ctx, B>,
    /// `ValID::Loc`.
    loc: Span,
}

/// `Function::getValueSymbolTable()->lookup(Name)` — one lookup across every
/// named local a function owns: arguments, basic blocks and value-producing
/// instructions alike. Upstream keeps them in a single symbol table, which is
/// why `parseUseListOrderBB` can find an argument where it wanted a block and
/// has to reject it by class afterwards; llvmkit has no such table, so the
/// walk stands in for it.
fn function_local_by_name<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, Dyn, B>,
    name: &str,
) -> Option<llvmkit_ir::Value<'ctx, B>> {
    for argument in function.params() {
        if argument.name().as_deref() == Some(name) {
            return Some(argument.as_erased());
        }
    }
    for block in function.basic_blocks() {
        if block.name().as_deref() == Some(name) {
            return Some(block.to_erased());
        }
        for instruction in block.instructions() {
            if instruction.name().as_deref() == Some(name) {
                return Some(IsValue::as_erased(instruction));
            }
        }
    }
    None
}

/// The over-estimate `APSInt::APSInt(StringRef)` opens with, before
/// truncating to the value's own width: `((Str.size() * 64) / 19) + 2`.
fn decimal_scratch_bits(digits: &str) -> u32 {
    let digit_count = u32::try_from(digits.len()).unwrap_or(u32::MAX / 64);
    digit_count
        .saturating_mul(64)
        .saturating_div(19)
        .saturating_add(2)
}

fn parsed_apsint_to_i128(parsed: &ParsedApsInt) -> Option<i128> {
    match parsed.signedness {
        Signedness::Unsigned => i128::try_from(parsed.value.try_zext_u128()?).ok(),
        Signedness::Signed => parsed.value.try_sext_i128(),
    }
}

fn is_supported_constant_expr_opcode(op: Opcode) -> bool {
    matches!(
        op,
        Opcode::GetElementPtr
            | Opcode::BitCast
            | Opcode::AddrSpaceCast
            | Opcode::IntToPtr
            | Opcode::PtrToInt
            | Opcode::PtrToAddr
            | Opcode::Trunc
            | Opcode::Add
            | Opcode::Sub
            | Opcode::Xor
            | Opcode::ExtractElement
            | Opcode::InsertElement
            | Opcode::ShuffleVector
    )
}

fn linkage_keyword(keyword: Keyword) -> Option<Linkage> {
    Some(match keyword {
        Keyword::External => Linkage::External,
        Keyword::AvailableExternally => Linkage::AvailableExternally,
        Keyword::Linkonce => Linkage::LinkOnceAny,
        Keyword::LinkonceOdr => Linkage::LinkOnceOdr,
        Keyword::Weak => Linkage::WeakAny,
        Keyword::WeakOdr => Linkage::WeakOdr,
        Keyword::Appending => Linkage::Appending,
        Keyword::Internal => Linkage::Internal,
        Keyword::Private => Linkage::Private,
        Keyword::ExternWeak => Linkage::ExternalWeak,
        Keyword::Common => Linkage::Common,
        _ => return None,
    })
}

/// Whether `name` is one of the three debug intrinsics that the record-based
/// debug-info format replaces.
///
/// Mirrors `llvm::isOldDbgFormatIntrinsic`, including its early exit: the
/// `llvm.dbg.` prefix is checked first because almost every call is not one of
/// these, and only `dbg_declare` / `dbg_value` / `dbg_assign` count —
/// `llvm.dbg.label`, for instance, does not.
fn is_old_dbg_format_intrinsic(name: &str) -> bool {
    if !name.starts_with("llvm.dbg.") {
        return false;
    }
    matches!(
        name,
        "llvm.dbg.declare" | "llvm.dbg.value" | "llvm.dbg.assign"
    )
}

fn is_declaration_linkage(linkage: Linkage) -> bool {
    matches!(linkage, Linkage::External | Linkage::ExternalWeak)
}

/// `setInstName`'s first arm — an instruction of void type may carry neither
/// a name nor an id — split out so the void-typed instructions that never mint
/// a `Value` can reach it: every terminator but `invoke`, `callbr` and
/// `catchswitch`, plus `store` and `fence`. Upstream needs no split, because
/// `parseBasicBlock` calls `setInstName` on *every* instruction and the
/// routine reads `Inst->getType()` itself. `bind_local`, the rest of
/// `setInstName`, delegates its own void arm here.
fn reject_named_void(lhs: &LocalLhs, loc: Span) -> ParseResult<()> {
    match lhs {
        LocalLhs::None => Ok(()),
        LocalLhs::Named(_) | LocalLhs::Numbered(_) => Err(ParseError::Message {
            message: "instructions returning void cannot have a name".into(),
            loc: DiagLoc::span(loc),
        }),
    }
}

/// Reject a numbered slot that goes *backwards*. Mirrors
/// `LLParser::checkValueID`.
///
/// Skipping ahead is legal — `test/Assembler/skip-value-numbers.ll` accepts
/// `%10 = add i32 1, 2` as a function's first instruction and renumbers it to
/// `%1` on output. Only a slot below the frontier is an error, because that
/// slot is already taken or already passed. llvmkit required exact equality
/// until 0.0.5, so every skip-ahead spelling in that fixture was rejected.
///
/// `kind` and `prefix` are upstream's own arguments: `("global", "@")`,
/// `("function", "@")`, `("argument", "%")`, `("instruction", "%")` and
/// `("label", "")` — the label form deliberately has no sigil, so it reads
/// `label expected to be numbered '11' or greater`.
fn check_value_id(
    kind: &'static str,
    prefix: &'static str,
    next_id: u32,
    id: u32,
    loc: Span,
) -> ParseResult<()> {
    if id < next_id {
        return Err(ParseError::Message {
            message: format!("{kind} expected to be numbered '{prefix}{next_id}' or greater")
                .into(),
            loc: DiagLoc::span(loc),
        });
    }
    Ok(())
}

fn is_int_or_int_vector_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> bool {
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Int(_) => true,
        AnyTypeEnum::Vector(v) => v.element().is_integer(),
        _ => false,
    }
}

/// Whether `ty` is what `isa<FPMathOperator>` accepts: a floating-point scalar
/// or a vector of them.
fn is_fp_or_fp_vector_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> bool {
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Float(_) => true,
        AnyTypeEnum::Vector(v) => v.element().is_floating_point(),
        _ => false,
    }
}

/// Mirrors `AtomicCmpXchgInst::isValidSuccessOrdering`.
fn cmpxchg_success_ordering_is_valid(ordering: AtomicOrdering) -> bool {
    !matches!(
        ordering,
        AtomicOrdering::NotAtomic | AtomicOrdering::Unordered
    )
}

/// Mirrors `AtomicCmpXchgInst::isValidFailureOrdering`, which additionally
/// denies the two orderings that imply a release.
fn cmpxchg_failure_ordering_is_valid(ordering: AtomicOrdering) -> bool {
    !matches!(
        ordering,
        AtomicOrdering::NotAtomic
            | AtomicOrdering::Unordered
            | AtomicOrdering::AcquireRelease
            | AtomicOrdering::Release
    )
}

/// The `IsFP` flag `LLParser::parseAtomicRMW` sets in its operation switch —
/// the operations whose operand must be floating-point.
fn atomicrmw_op_is_floating_point(op: AtomicRmwBinOp) -> bool {
    matches!(
        op,
        AtomicRmwBinOp::Fadd
            | AtomicRmwBinOp::Fsub
            | AtomicRmwBinOp::Fmax
            | AtomicRmwBinOp::Fmin
            | AtomicRmwBinOp::Fmaximum
            | AtomicRmwBinOp::Fminimum
    )
}

fn is_ptr_or_ptr_vector_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> bool {
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Pointer(_) => true,
        AnyTypeEnum::Vector(v) => v.element().is_pointer(),
        _ => false,
    }
}

fn vector_shape_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Option<(u32, bool)> {
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Vector(v) => Some((v.min_len(), v.is_scalable())),
        _ => None,
    }
}
#[derive(Debug, Clone)]
struct ParsedGepConstantExprFlags {
    no_wrap: GepNoWrapFlags,
    in_range: Option<(ParsedApsInt, ParsedApsInt)>,
}

/// `GV->getValueType()->isFunctionTy()` for whichever global kind `r` names.
/// A `GlobalVariable`'s value type can never be a function type — that is
/// `invalid type for global variable` — so it answers `false` by construction
/// rather than by omission.
fn global_ref_value_type_is_function<'ctx, B: ModuleBrand + 'ctx>(r: GlobalRef<'ctx, B>) -> bool {
    match r {
        GlobalRef::Function(_) => true,
        GlobalRef::Variable(g) => g.value_type().is_function(),
        GlobalRef::Alias(a) => a.value_type().is_function(),
        GlobalRef::Ifunc(i) => i.value_type().is_function(),
    }
}

fn pointer_address_space_or_vector_element<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
) -> Option<u32> {
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Pointer(ptr_ty) => Some(ptr_ty.address_space()),
        AnyTypeEnum::Vector(vector_ty) => match vector_ty.element().into_type_enum() {
            AnyTypeEnum::Pointer(ptr_ty) => Some(ptr_ty.address_space()),
            _ => None,
        },
        _ => None,
    }
}

/// `ExtractElementInst::isValidOperands(Val, Index)` — the operand guard only;
/// the result type is the vector's element type and is derived, not demanded.
fn is_valid_extractelement<'ctx, B: ModuleBrand + 'ctx>(
    vector_ty: Type<'ctx, B>,
    index_ty: Type<'ctx, B>,
) -> bool {
    vector_ty.is_vector() && index_ty.is_integer()
}

/// `InsertElementInst::isValidOperands(Vec, Elt, Index)`.
fn is_valid_insertelement<'ctx, B: ModuleBrand + 'ctx>(
    vector_ty: Type<'ctx, B>,
    value_ty: Type<'ctx, B>,
    index_ty: Type<'ctx, B>,
) -> bool {
    let AnyTypeEnum::Vector(vector_ty) = AnyTypeEnum::from(vector_ty) else {
        return false;
    };
    vector_ty.element() == value_ty && index_ty.is_integer()
}

/// `ShuffleVectorInst::isValidOperands(V1, V2, Mask)`, the `Value *Mask`
/// overload: `V1` and `V2` are vectors of the same type, and the mask is a
/// vector of `i32` of the same kind — fixed against fixed, scalable against
/// scalable.
///
/// The mask's *element range* half (`CI->uge(V1Size * 2)`) is not repeated
/// here: `validate_constant_expr_data`'s `ShuffleVector` arm runs it through
/// `valid_shufflevector_mask_constant`, and `build_constant_expr` renders that
/// rejection as this same `invalid operands to shufflevector`.
fn is_valid_shufflevector<'ctx, B: ModuleBrand + 'ctx>(
    lhs_ty: Type<'ctx, B>,
    rhs_ty: Type<'ctx, B>,
    mask_ty: Type<'ctx, B>,
) -> bool {
    let AnyTypeEnum::Vector(lhs_ty) = AnyTypeEnum::from(lhs_ty) else {
        return false;
    };
    let AnyTypeEnum::Vector(rhs_ty) = AnyTypeEnum::from(rhs_ty) else {
        return false;
    };
    let AnyTypeEnum::Vector(mask_ty) = AnyTypeEnum::from(mask_ty) else {
        return false;
    };
    lhs_ty.element() == rhs_ty.element()
        && lhs_ty.min_len() == rhs_ty.min_len()
        && lhs_ty.is_scalable() == rhs_ty.is_scalable()
        && matches!(mask_ty.element().kind(), TypeKind::Integer { bits: 32 })
        && mask_ty.is_scalable() == lhs_ty.is_scalable()
}

#[derive(Clone, Copy)]
struct ParsedAliasHeader {
    linkage: Linkage,
    dso_locality: llvmkit_ir::DsoLocality,
    visibility: Visibility,
    dll_storage_class: DllStorageClass,
    thread_local_mode: ThreadLocalMode,
    unnamed_addr: UnnamedAddr,
}

fn map_lex_error(e: LexError) -> ParseError {
    match e {
        LexError::IntegerWidthOutOfRange { width, max, span } => {
            ParseError::IntegerWidthOutOfRange {
                width,
                max,
                loc: DiagLoc::span(span),
            }
        }
        other => ParseError::Lex(other),
    }
}

impl<'src, 'ctx, B: ModuleBrand + 'ctx> Parser<'src, 'ctx, B> {
    /// Construct a parser over `src`, populating `module`. Primes the lexer
    /// once (mirrors `LLParser::Run`'s leading `Lex.Lex()`).
    pub fn new(src: &'src [u8], module: &'ctx Module<B, Unverified>) -> ParseResult<Self> {
        let mut lex = Lexer::new(src);
        let current = lex.next_token().map_err(map_lex_error)?;
        Ok(Self {
            lex,
            src,
            current,
            prev_token_end: 0,
            line_starts: Vec::new(),
            parser_context: None,
            module,
            named_types: HashMap::new(),
            numbered_types: HashMap::new(),
            next_unnamed_type_id: 0,
            numbered_globals: NumberedValues::new(),
            numbered_attr_groups: NumberedValues::new(),
            deferred_block_addresses: Vec::new(),
            metadata_slots: HashMap::new(),
            deferred_personality_fns: Vec::new(),
            deferred_alias_targets: Vec::new(),
            deferred_intrinsic_attribute_checks: Vec::new(),
            insts_with_tbaa_tag: Vec::new(),
            forward_ref_comdats: BTreeMap::new(),
            forward_ref_globals: BTreeMap::new(),
            forward_ref_global_ids: BTreeMap::new(),
            forward_ref_dso_local_equivalent_ids: BTreeMap::new(),
            forward_ref_dso_local_equivalent_names: BTreeMap::new(),
            pending_no_cfi: Vec::new(),
            summary_index: None,
            parses_module_entities: true,
            summary_module_paths: BTreeMap::new(),
            numbered_value_infos: Vec::new(),
            forward_ref_summary_values: BTreeMap::new(),
            forward_ref_summary_aliasees: BTreeMap::new(),
            forward_ref_summary_type_ids: BTreeMap::new(),
            pending_summary_value_refs: Vec::new(),
            pending_summary_type_ids: Vec::new(),
            pending_summary_aliasee: None,
            seen_new_dbg_info_format: false,
            seen_old_dbg_info_format: false,
            _brand: PhantomData,
        })
    }

    /// A parser that also builds a [`ModuleSummaryIndex`] from the file's `^N`
    /// entries. Mirrors constructing `LLParser` with a non-null `Index`, which
    /// is what `parseAssemblyWithIndex` does.
    pub fn with_summary_index(
        src: &'src [u8],
        module: &'ctx Module<B, Unverified>,
    ) -> ParseResult<Self> {
        let mut parser = Self::new(src, module)?;
        parser.summary_index = Some(ModuleSummaryIndex::new());
        Ok(parser)
    }

    /// A parser that builds only a [`ModuleSummaryIndex`], lexing past every
    /// module-level entity. Mirrors constructing `LLParser` with a null `M`,
    /// which is what `parseSummaryIndexAssembly` does.
    pub fn summary_index_only(
        src: &'src [u8],
        module: &'ctx Module<B, Unverified>,
    ) -> ParseResult<Self> {
        let mut parser = Self::with_summary_index(src, module)?;
        parser.parses_module_entities = false;
        Ok(parser)
    }

    pub fn with_slot_mapping(
        src: &'src [u8],
        module: &'ctx Module<B, Unverified>,
        slots: &SlotMapping<'ctx, B>,
    ) -> ParseResult<Self> {
        let mut parser = Self::new(src, module)?;
        parser.numbered_globals = slots.global_values.clone();
        parser.numbered_attr_groups = slots.attribute_groups.clone();
        parser.named_types = slots
            .named_types
            .iter()
            .map(|(name, ty)| {
                (
                    name.clone(),
                    TypeEntry {
                        ty: *ty,
                        forward_ref_loc: None,
                    },
                )
            })
            .collect();
        parser.numbered_types = slots
            .numbered_types
            .iter()
            .map(|(id, ty)| {
                (
                    *id,
                    TypeEntry {
                        ty: *ty,
                        forward_ref_loc: None,
                    },
                )
            })
            .collect();
        parser.next_unnamed_type_id = slots
            .numbered_types
            .keys()
            .next_back()
            .map_or(0, |id| id.saturating_add(1));
        parser.metadata_slots = slots
            .metadata_nodes
            .iter()
            .map(|(slot, id)| {
                (
                    slot,
                    MetadataSlotEntry {
                        id: *id,
                        defined: true,
                        first_ref: Span::new(0, 0),
                    },
                )
            })
            .collect();
        Ok(parser)
    }

    /// A parser that also records the source range of every function, basic
    /// block and instruction it builds. Mirrors constructing `LLParser` with a
    /// non-null `AsmParserContext *`, which is what `parseAssemblyString` /
    /// `parseAssembly` do when handed one; the registry comes back out through
    /// [`ParsedModule::parser_context`].
    pub fn with_context(src: &'src [u8], module: &'ctx Module<B, Unverified>) -> ParseResult<Self> {
        let mut parser = Self::new(src, module)?;
        parser.parser_context = Some(AsmParserContext::new());
        parser.line_starts = line_start_offsets(src);
        Ok(parser)
    }

    fn resolve_md_slot(&mut self, slot: u32, loc: Span) -> MetadataId<B> {
        if let Some(entry) = self.metadata_slots.get(&slot) {
            return entry.id;
        }
        let id = self.module.metadata_reserve();
        self.metadata_slots.insert(
            slot,
            MetadataSlotEntry {
                id,
                defined: false,
                first_ref: loc,
            },
        );
        id
    }

    fn define_md_slot(
        &mut self,
        slot: u32,
        content: MetadataKind<B>,
        loc: Span,
    ) -> ParseResult<MetadataId<B>> {
        if let Some(entry) = self.metadata_slots.get_mut(&slot) {
            if entry.defined {
                // `parseStandaloneMetadata`'s `try_emplace` guard. Upstream
                // names neither the id nor the kind, and capitalises the
                // first word — the only place it does so outside `parseScope`
                // and `parseOrdering`.
                return Err(ParseError::Message {
                    message: "Metadata id is already used".into(),
                    loc: DiagLoc::span(loc),
                });
            }
            // The id was reserved by *this* module (`resolve_md_slot` ->
            // `metadata_reserve`), so its tag matches and its slot is in range.
            self.module
                .metadata_set(entry.id, content)
                .expect("metadata id was reserved by this module");
            entry.defined = true;
            return Ok(entry.id);
        }

        let id = self.module.metadata_reserve();
        self.module
            .metadata_set(id, content)
            .expect("id returned by metadata_reserve on this module");
        self.metadata_slots.insert(
            slot,
            MetadataSlotEntry {
                id,
                defined: true,
                first_ref: loc,
            },
        );
        Ok(id)
    }

    /// Drive the parser to EOF under [`ParserConfig::DEFAULT`]. Mirrors
    /// `LLParser::Run(/*UpgradeDebugInfo=*/true, <the nullopt callback>)`,
    /// which is what `parseAssembly` and `parseAssemblyWithIndex` pass.
    pub fn parse_module(self) -> ParseResult<ParsedModule<'ctx, B>> {
        self.parse_module_with_config(&ParserConfig::DEFAULT)
    }

    /// Drive the parser to EOF. Mirrors `LLParser::Run`, whose two parameters
    /// plus the `-allow-incomplete-ir` `cl::opt` are what [`ParserConfig`]
    /// carries.
    pub fn parse_module_with_config(
        mut self,
        config: &ParserConfig<'_>,
    ) -> ParseResult<ParsedModule<'ctx, B>> {
        // Upstream's `!M` mode: with no module to build, only summary entries
        // and the source file name are read and everything else is lexed past.
        // `Run` skips `parseTargetDefinitions` whole in that mode — it is
        // guarded by `if (M)`.
        if !self.parses_module_entities {
            loop {
                match self.current.value {
                    Token::Eof => break,
                    Token::SummaryId(_) => self.parse_summary_entry()?,
                    Token::Kw(Keyword::SourceFilename) => self.parse_source_filename()?,
                    _ => {
                        self.bump()?;
                    }
                }
            }
            // `validateEndOfModule` opens with `if (!M) return false;`, so the
            // module-level resolution below is skipped whole.
            self.validate_end_of_index()?;
            let summary_index = self.summary_index.take();
            let parser_context = self.parser_context.take();
            return Ok(ParsedModule {
                slot_mapping: self.into_slot_mapping(),
                summary_index,
                parser_context,
            });
        }

        // `if (M) { if (parseTargetDefinitions(DataLayoutCallback)) return true; }`.
        self.parse_target_definitions(config)?;

        loop {
            match self.current.value {
                Token::Eof => break,
                Token::Kw(Keyword::Target) => self.parse_late_target_definition()?,
                Token::Kw(Keyword::SourceFilename) => self.parse_source_filename()?,
                Token::Kw(Keyword::Module) => self.parse_module_asm()?,
                Token::Kw(Keyword::Uselistorder) => self.parse_use_list_order(None)?,
                Token::Kw(Keyword::UselistorderBb) => self.parse_use_list_order_bb()?,
                Token::ComdatVar(_) => self.parse_comdat_definition()?,
                Token::LocalVar(_) => self.parse_named_type_definition()?,
                Token::LocalVarId(_) => self.parse_unnamed_type_definition()?,
                Token::GlobalVar(_) | Token::GlobalId(_) => self.parse_global_or_function()?,
                Token::Kw(Keyword::Declare) => self.parse_declare()?,
                Token::Kw(Keyword::Define) => self.parse_define()?,
                Token::Kw(Keyword::Attributes) => self.parse_unnamed_attr_group()?,
                Token::Exclaim => self.parse_standalone_metadata()?,
                Token::MetadataVar(_) => self.parse_named_metadata()?,
                Token::SummaryId(_) => self.parse_summary_entry()?,
                _ => return Err(self.token_error("top-level entity")),
            }
        }

        // --- end of module ---
        //
        // The sequence below is `LLParser::validateEndOfModule`'s, in its
        // order. **The order is part of parity**: every step here can be the
        // one that fails, so which error a module with two unrelated defects
        // reports is observable. llvmkit used to run these in an order of its
        // own — metadata second, blockaddresses fourth, comdats eighth — so a
        // module with an undefined comdat *and* an undefined metadata node
        // reported the metadata where upstream reports the comdat, and a
        // dangling `blockaddress` lost to an undefined type.
        //
        // One llvmkit-only step leads, standing in for something upstream does
        // earlier than this routine: the deferred intrinsic-attribute checks
        // need a function's attribute groups merged, which is
        // `validateEndOfModule`'s own first step (not yet ported — see
        // `docs/future-work.md`), so they sit at that step's position.
        self.validate_deferred_intrinsic_attribute_checks()?;

        // `if (!ForwardRefBlockAddresses.empty())` — before the type, comdat
        // and value leftovers, not after.
        self.resolve_deferred_block_addresses()?;
        // The `ForwardRefDSOLocalEquivalentIDs` then
        // `ForwardRefDSOLocalEquivalentNames` loops.
        self.resolve_forward_ref_dso_local_equivalents()?;
        // The `NumberedTypes` then `NamedTypes` loops.
        self.validate_forward_ref_types()?;
        // `if (!ForwardRefComdats.empty())`.
        self.validate_forward_ref_comdats()?;
        // The `ForwardRefVals` loop: intrinsic auto-declaration first, then
        // `use of undefined value '@x'`. llvmkit spreads the same work over
        // the personality, aliasee and global-reference maps.
        self.resolve_deferred_personality_fns()?;
        self.resolve_deferred_alias_targets()?;
        self.resolve_forward_ref_globals(config.allow_incomplete_ir)?;
        // Every `no_cfi` referent is a `ForwardRefVals` entry, so by here it is
        // either defined or already reported. Upstream needs no step of its
        // own: its `NoCFIValue` wraps the placeholder directly and re-interns
        // itself when the sweep above RAUWs it.
        self.resolve_pending_no_cfi()?;
        // `if (!ForwardRefMDNodes.empty())` — metadata is the *last* of the
        // leftovers, after every value one.
        for (slot, entry) in &self.metadata_slots {
            if !entry.defined {
                return Err(ParseError::UndefinedSymbol {
                    kind: SymbolKind::Metadata,
                    id: SymbolId::Numbered(*slot),
                    loc: DiagLoc::span(entry.first_ref),
                });
            }
        }

        // --- AutoUpgrade ---
        //
        // `validateEndOfModule` closes with nine `AutoUpgrade.h` entry points,
        // in this order: `UpgradeTBAANode` (over `InstsWithTBAATag`),
        // `UpgradeCallsToIntrinsic` (which is where `UpgradeIntrinsicFunction`
        // / `UpgradeIntrinsicCall` are reached for a *declared* intrinsic; the
        // `ForwardRefVals` sweep above reaches the same pair for an
        // undeclared one), `llvm::UpgradeDebugInfo`, `UpgradeModuleFlags`,
        // `UpgradeNVVMAnnotations`, `UpgradeSectionAttributes` and
        // `copyModuleAttrToFunctions`. Four of them are ported; the five that
        // are not are recorded in `docs/future-work.md` with what each is
        // blocked on. The ported ones sit at their own positions, so adding
        // the rest is insertion, not re-ordering.
        //
        // Upstream resolves metadata cycles (`N.second->resolveCycles()`)
        // immediately before the TBAA loop. llvmkit's metadata arena has no
        // temporary-node forwarding to resolve — a forward `!N` is a reserved
        // slot that `metadata_set` fills in place — so there is no step here.
        self.upgrade_tbaa_tags();
        llvmkit_ir::auto_upgrade::upgrade_module_flags(self.module);
        llvmkit_ir::auto_upgrade::upgrade_nvvm_annotations(self.module);
        llvmkit_ir::auto_upgrade::upgrade_section_attributes(self.module);

        // `Run` is `parseTopLevelEntities() || validateEndOfModule(...) ||
        // validateEndOfIndex()`, in that order, so an unresolved `^N` is
        // reported only once the module itself is whole.
        self.validate_end_of_index()?;

        let summary_index = self.summary_index.take();
        let parser_context = self.parser_context.take();
        Ok(ParsedModule {
            slot_mapping: self.into_slot_mapping(),
            summary_index,
            parser_context,
        })
    }

    /// Rewrite every scalar-format `!tbaa` tag into the struct-path-aware
    /// format. Mirrors `validateEndOfModule`'s `InstsWithTBAATag` loop, which
    /// re-attaches only when `UpgradeTBAANode` hands back a *different* node.
    ///
    /// Upstream asserts the attachment is still present (it dropped only
    /// under `-allow-incomplete-ir`); here a missing attachment is simply
    /// nothing to upgrade, since an attachment set cannot lose an entry.
    fn upgrade_tbaa_tags(&mut self) {
        for inst in core::mem::take(&mut self.insts_with_tbaa_tag) {
            let Some(tag) = inst.metadata().get(&MetadataAttachmentKind::Tbaa) else {
                continue;
            };
            let upgraded = llvmkit_ir::auto_upgrade::upgrade_tbaa_node(self.module, tag);
            if upgraded != tag {
                own_metadata(inst.set_metadata(
                    self.module,
                    MetadataAttachmentKind::Tbaa,
                    upgraded,
                ));
            }
        }
    }

    /// Drain `ForwardRefDSOLocalEquivalentIDs`, then
    /// `ForwardRefDSOLocalEquivalentNames`, against the definitions that
    /// arrived. Mirrors `validateEndOfModule`'s
    /// `ResolveForwardRefDSOLocalEquivalents` lambda and the two loops that
    /// call it — ids before names, both in key order.
    fn resolve_forward_ref_dso_local_equivalents(&mut self) -> ParseResult<()> {
        for (id, entry) in core::mem::take(&mut self.forward_ref_dso_local_equivalent_ids) {
            let global = self.numbered_globals.get(id).copied();
            // `GVRef.StrVal` is empty for the numbered spelling, and upstream
            // interpolates it regardless: the message really does read
            // `unknown function '' referenced by dso_local_equivalent`.
            self.resolve_one_forward_ref_dso_local_equivalent(global, "", entry)?;
        }
        for (name, entry) in core::mem::take(&mut self.forward_ref_dso_local_equivalent_names) {
            let global = self.resolve_global_name_as_ref(name.clone()).ok();
            self.resolve_one_forward_ref_dso_local_equivalent(global, &name, entry)?;
        }
        Ok(())
    }

    /// The body of `ResolveForwardRefDSOLocalEquivalents`: the referent must
    /// exist, and its value type must be a function type.
    fn resolve_one_forward_ref_dso_local_equivalent(
        &self,
        global: Option<GlobalRef<'ctx, B>>,
        name: &str,
        entry: ForwardRef<'ctx, B>,
    ) -> ParseResult<()> {
        let Some(global) = global else {
            return Err(ParseError::Message {
                message: format!("unknown function '{name}' referenced by dso_local_equivalent")
                    .into(),
                loc: DiagLoc::span(entry.loc),
            });
        };
        if !global_ref_value_type_is_function(global) {
            return Err(ParseError::Message {
                message: "expected a function, alias to function, or ifunc in dso_local_equivalent"
                    .into(),
                loc: DiagLoc::span(entry.loc),
            });
        }
        let equivalent = self
            .module
            .dso_local_equivalent_global(self.global_ref_to_constant(global))
            .map_err(|e| self.builder_err("dso_local_equivalent", e))?;
        entry
            .placeholder
            .replace_all_uses_with(equivalent.as_erased())
            .map_err(|e| self.builder_err("forward dso_local_equivalent", e))
    }

    /// Build the `no_cfi` wrappers whose referent was still forward-referenced
    /// when they were read.
    ///
    /// Upstream has no counterpart step — see the `pending_no_cfi` field for
    /// why llvmkit needs one. It runs immediately after the `ForwardRefVals`
    /// sweep, so every referent named here is already installed.
    fn resolve_pending_no_cfi(&mut self) -> ParseResult<()> {
        for item in core::mem::take(&mut self.pending_no_cfi) {
            let global = match &item.reference {
                NameOrId::Name(name) => self.resolve_global_name_as_ref(name.clone()).ok(),
                NameOrId::Id(id) => self.numbered_globals.get(*id).copied(),
            };
            let Some(global) = global else {
                return Err(ParseError::UndefinedSymbol {
                    kind: SymbolKind::GlobalValue,
                    id: match item.reference {
                        NameOrId::Name(name) => SymbolId::Named(name),
                        NameOrId::Id(id) => SymbolId::Numbered(id),
                    },
                    loc: DiagLoc::span(item.loc),
                });
            };
            let no_cfi = self
                .module
                .no_cfi_global(self.global_ref_to_constant(global))
                .map_err(|e| self.builder_err("no_cfi", e))?;
            item.placeholder
                .replace_all_uses_with(no_cfi.as_erased())
                .map_err(|e| self.builder_err("forward no_cfi", e))?;
        }
        Ok(())
    }

    /// Retire every deferred `blockaddress` that names `function`, now that
    /// its body has been parsed and its labels are all defined. Mirrors
    /// `LLParser::PerFunctionState::resolveForwardRefBlockAddresses`.
    fn resolve_block_addresses_for_function(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        function_ref: &NameOrId,
    ) -> ParseResult<()> {
        let mut pending = Vec::new();
        let mut resolved = Vec::new();
        for item in core::mem::take(&mut self.deferred_block_addresses) {
            let is_ours = match &item.function {
                DeferredBlockAddressFunction::Installed(f) => {
                    f.as_erased() == state.func.as_erased()
                }
                DeferredBlockAddressFunction::Forward(reference) => reference == function_ref,
            };
            if is_ours {
                resolved.push(item);
            } else {
                pending.push(item);
            }
        }
        self.deferred_block_addresses = pending;
        for item in resolved {
            let Some(block) = state.defined_block(&item.label) else {
                return Err(ParseError::Message {
                    message: "referenced value is not a basic block".into(),
                    loc: DiagLoc::span(item.label_loc),
                });
            };
            let block = state.value_as_block_view(block, item.label_loc)?;
            let address = self
                .module
                .block_address(state.func, &block)
                .map_err(|e| self.builder_err("blockaddress", e))?;
            let expected = item.placeholder.ty();
            let got = address.ty();
            if got != expected {
                // The two entry paths disagree about the diagnostic, because
                // upstream reaches them through different routines.
                //
                // A `blockaddress` naming the function it sits in is not
                // deferred upstream at all: `BlockAddressPFS->getBB`
                // forward-declares the block, `BlockAddress::get` is built at
                // once, and `convertValIDToValue`'s `t_Constant` arm compares
                // its type against the context's. That is
                // `constant expression type mismatch`, anchored at the
                // `blockaddress` keyword.
                //
                // A `blockaddress` naming a function that has not been seen
                // yet *is* deferred, into `ForwardRefBlockAddresses`, and
                // `PerFunctionState::resolveForwardRefBlockAddresses` retires
                // it through `checkValidVariableType` instead — a different
                // sentence, anchored at the *label*, and quoting the label's
                // own spelling. Note upstream passes `BBID.StrVal` bare: no
                // `%` sigil, and empty for a numbered label.
                return Err(match &item.function {
                    DeferredBlockAddressFunction::Installed(_) => ParseError::Message {
                        message: format!(
                            "constant expression type mismatch: got type '{got}' but expected '{expected}'"
                        )
                        .into(),
                        loc: DiagLoc::span(item.value_loc),
                    },
                    DeferredBlockAddressFunction::Forward(_) => ParseError::DefinedWithWrongType {
                        name: match &item.label {
                            BlockLabel::Named(name) => name.clone(),
                            BlockLabel::Numbered(_) => String::new(),
                        },
                        defined: got.to_string(),
                        expected: expected.to_string(),
                        loc: DiagLoc::span(item.label_loc),
                    },
                });
            }
            item.placeholder
                .replace_all_uses_with(address.as_erased())
                .map_err(|e| self.builder_err("forward blockaddress", e))?;
        }
        Ok(())
    }

    /// Anything still deferred at end of module names a function that was
    /// never defined. Mirrors the `ForwardRefBlockAddresses` guard at the top
    /// of `LLParser::validateEndOfModule`, whose wording this reproduces —
    /// the reference is reported, not the label.
    fn resolve_deferred_block_addresses(&mut self) -> ParseResult<()> {
        if let Some(item) = self.deferred_block_addresses.first() {
            return Err(ParseError::Expected {
                expected: "function name in blockaddress".into(),
                loc: DiagLoc::span(item.function_loc),
            });
        }
        Ok(())
    }

    fn resolve_deferred_alias_targets(&mut self) -> ParseResult<()> {
        let deferred = std::mem::take(&mut self.deferred_alias_targets);
        for item in deferred {
            let target = self
                .resolve_global_name_as_constant(item.loc, item.name.clone(), item.ty)
                .map_err(|err| match err {
                    ParseError::UndefinedSymbol { kind, id, .. } => ParseError::UndefinedSymbol {
                        kind,
                        id,
                        loc: DiagLoc::span(item.loc),
                    },
                    other => other,
                })?;
            match item.object {
                DeferredAliasObject::Alias(a) => a
                    .set_aliasee(self.module, target)
                    .map_err(|e| self.builder_err("deferred alias target", e))?,
                DeferredAliasObject::Ifunc(i) => i
                    .set_resolver(self.module, target)
                    .map_err(|e| self.builder_err("deferred ifunc resolver", e))?,
            }
        }
        Ok(())
    }

    fn resolve_deferred_personality_fns(&mut self) -> ParseResult<()> {
        let deferred = std::mem::take(&mut self.deferred_personality_fns);
        for item in deferred {
            let personality = self
                .resolve_global_name_as_constant(item.loc, item.name.clone(), item.ty)
                .map_err(|err| match err {
                    ParseError::UndefinedSymbol { kind, id, .. } => ParseError::UndefinedSymbol {
                        kind,
                        id,
                        loc: DiagLoc::span(item.loc),
                    },
                    other => other,
                })?;
            item.function
                .set_personality_fn(self.module, personality)
                .map_err(|e| self.builder_err("function personality", e))?;
        }
        Ok(())
    }
    fn validate_deferred_intrinsic_attribute_checks(&self) -> ParseResult<()> {
        for item in &self.deferred_intrinsic_attribute_checks {
            if !self.intrinsic_declaration_attrs_match(
                &item.attrs,
                &item.attr_groups,
                &item.expected_attrs,
            )? {
                return Err(self.intrinsic_attribute_error(item.loc));
            }
        }
        Ok(())
    }

    fn intrinsic_declaration_attrs_match(
        &self,
        attrs: &AttributeStorage,
        attr_groups: &[u32],
        expected_attrs: &AttributeStorage,
    ) -> ParseResult<bool> {
        if !attrs.is_subset_of(expected_attrs) {
            return Ok(false);
        }
        self.intrinsic_declaration_attr_groups_match(attr_groups, expected_attrs)
    }

    fn intrinsic_declaration_attr_groups_match(
        &self,
        attr_groups: &[u32],
        expected_attrs: &AttributeStorage,
    ) -> ParseResult<bool> {
        if attr_groups.is_empty() {
            return Ok(true);
        }
        if Self::has_duplicate_attr_groups(attr_groups) {
            return Ok(false);
        }
        let mut group_attrs = AttributeStorage::new();
        for group in attr_groups {
            let Some(attrs) = self.numbered_attr_groups.get(*group) else {
                return Ok(false);
            };
            group_attrs.merge_from(attrs);
        }
        Ok(group_attrs.has_only_index_attributes_subset_of(expected_attrs, AttrIndex::Function))
    }

    fn intrinsic_declaration_attrs_are_pending(&self, attr_groups: &[u32]) -> bool {
        attr_groups
            .iter()
            .any(|group| self.numbered_attr_groups.get(*group).is_none())
    }

    fn has_duplicate_attr_groups(attr_groups: &[u32]) -> bool {
        let mut seen = Vec::new();
        for group in attr_groups {
            if seen.contains(group) {
                return true;
            }
            seen.push(*group);
        }
        false
    }

    /// Retire every `@`-forward reference against the definition that
    /// arrived, and report the first that never did.
    ///
    /// Mirrors the `ForwardRefVals` / `ForwardRefValIDs` sweep in
    /// `LLParser::validateEndOfModule`, including its noun: an unresolved
    /// `@`-reference is a `use of undefined **value**`, where a *redefinition*
    /// of the same namespace says `global`.
    fn resolve_forward_ref_globals(&mut self, allow_incomplete_ir: bool) -> ParseResult<()> {
        let named = core::mem::take(&mut self.forward_ref_globals);
        for (name, entry) in named {
            let target = match self.resolve_global_name_as_ref(name.clone()) {
                Ok(target) => target,
                // `if (!AllowIncompleteIR) continue;` — with the option on, a
                // leftover that is *not* an intrinsic gets a declaration
                // synthesised for it instead of ending the parse. Names under
                // `llvm.` never reach the option upstream: the intrinsic
                // auto-declaration branch above it has already `continue`d.
                Err(_) if allow_incomplete_ir && !name.starts_with("llvm.") => {
                    let placeholder = entry.placeholder.as_value();
                    self.declare_incomplete_forward_ref(&name, placeholder, entry.loc)?
                }
                Err(_) => {
                    return Err(ParseError::UndefinedSymbol {
                        kind: SymbolKind::GlobalValue,
                        id: SymbolId::Named(name),
                        loc: DiagLoc::span(entry.loc),
                    });
                }
            };
            let target = self.global_ref_to_constant(target);
            Self::resolve_global_forward_ref(entry, target)?;
        }
        let numbered = core::mem::take(&mut self.forward_ref_global_ids);
        for (id, entry) in numbered {
            let Some(target) = self.numbered_globals.get(id).copied() else {
                return Err(ParseError::UndefinedSymbol {
                    kind: SymbolKind::GlobalValue,
                    id: SymbolId::Numbered(id),
                    loc: DiagLoc::span(entry.loc),
                });
            };
            let target = self.global_ref_to_constant(target);
            Self::resolve_global_forward_ref(entry, target)?;
        }
        Ok(())
    }

    fn resolve_global_forward_ref(
        entry: ForwardRef<'ctx, B>,
        target: llvmkit_ir::Constant<'ctx, B>,
    ) -> ParseResult<()> {
        if entry.placeholder.ty() != target.ty() {
            return Err(ParseError::Message {
                message: "forward reference and definition of global have different types".into(),
                loc: DiagLoc::span(entry.loc),
            });
        }
        entry
            .placeholder
            .replace_all_uses_with(target.as_erased())
            .map_err(|e| ParseError::Message {
                message: format!("cannot resolve forward reference: {e}").into(),
                loc: DiagLoc::span(entry.loc),
            })
    }

    /// Synthesise a declaration for a `@name` that was never defined, under
    /// `-allow-incomplete-ir`. Mirrors the tail of `validateEndOfModule`'s
    /// `ForwardRefVals` loop: a function when every use is a call at one common
    /// signature, an `i8` global otherwise.
    fn declare_incomplete_forward_ref(
        &mut self,
        name: &str,
        placeholder: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<GlobalRef<'ctx, B>> {
        match Self::common_call_site_function_type(placeholder) {
            // `if (auto *FTy = dyn_cast<FunctionType>(Ty))
            //    GV = Function::Create(FTy, ExternalLinkage, Name, M);`
            Some(signature) => {
                let id = self
                    .module
                    .add_function_dyn(name, signature, Linkage::External)
                    .map_err(|e| ParseError::Message {
                        message: format!("cannot declare incomplete forward reference: {e}").into(),
                        loc: DiagLoc::span(loc),
                    })?;
                Ok(GlobalRef::Function(self.module.view(id)))
            }
            // `else GV = new GlobalVariable(*M, Ty, false, ExternalLinkage,
            //    nullptr, Name);` with `Ty = Type::getInt8Ty(Context)`.
            None => {
                let id = self
                    .module
                    .add_external_global(name, self.module.i8_type().as_type())
                    .map_err(|e| ParseError::Message {
                        message: format!("cannot declare incomplete forward reference: {e}").into(),
                        loc: DiagLoc::span(loc),
                    })?;
                Ok(GlobalRef::Variable(self.module.view(id)))
            }
        }
    }

    /// The one function type every use of `value` calls it at, or `None` if the
    /// uses disagree, if any use is not a call, or if there are none at all.
    /// Mirrors `validateEndOfModule`'s `GetCommonFunctionType` lambda, whose
    /// `return nullptr` covers all three.
    ///
    /// **Divergence:** upstream walks `V->uses()`, which is every `Use` edge.
    /// llvmkit's [`Value::users`](llvmkit_ir::Value::users) yields only the
    /// instruction edges, so the constant-expression and global-field edges
    /// upstream would see as non-`CallBase` users are counted here through
    /// [`Value::num_uses`](llvmkit_ir::Value::num_uses) instead: any edge that
    /// is not an instruction use forces the `i8` fallback, exactly as a
    /// non-call user does upstream. That count also includes metadata and
    /// debug-record edges, which upstream does not track as `Use`s at all
    /// (`docs/divergences.md` D5), so a forward reference named by metadata
    /// *and* called takes the fallback here and the function type upstream.
    fn common_call_site_function_type(
        value: llvmkit_ir::Value<'ctx, B>,
    ) -> Option<llvmkit_ir::FunctionType<'ctx, B>> {
        let users: Vec<_> = value.users().collect();
        if users.len() != value.num_uses() {
            return None;
        }
        let mut signature = None;
        for user in users {
            let called_at = Self::callee_function_type(&user, value)?;
            if signature.is_some_and(|already| already != called_at) {
                return None;
            }
            signature = Some(called_at);
        }
        signature
    }

    /// The call site's function type when `user` calls `value` — upstream's
    /// `dyn_cast<CallBase>(U.getUser())` plus `CB->isCallee(&U)`. `None` when
    /// `user` is not a call form, or when it names `value` somewhere other than
    /// the callee position.
    fn callee_function_type(
        user: &llvmkit_ir::InstructionView<'ctx, B>,
        value: llvmkit_ir::Value<'ctx, B>,
    ) -> Option<llvmkit_ir::FunctionType<'ctx, B>> {
        match user.classify() {
            llvmkit_ir::Classified::Inst(llvmkit_ir::InstructionKind::Call(call)) => {
                (call.callee() == value).then(|| call.function_type())
            }
            llvmkit_ir::Classified::Term(llvmkit_ir::TerminatorKind::Invoke(invoke)) => {
                (invoke.callee() == value).then(|| invoke.function_type())
            }
            llvmkit_ir::Classified::Term(llvmkit_ir::TerminatorKind::CallBr(call_br)) => {
                (call_br.callee() == value).then(|| call_br.function_type())
            }
            _ => None,
        }
    }

    /// Mint (or reuse) the placeholder standing for a not-yet-defined `@`
    /// symbol. Mirrors the tail of `LLParser::getGlobalVal`, whose
    /// `createGlobalFwdRef` likewise builds a stand-in at the demanded
    /// pointer type.
    fn global_forward_ref(
        &mut self,
        name: Option<&str>,
        id: Option<u32>,
        ty: Type<'ctx, B>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        if !ty.is_pointer() {
            return Err(ParseError::Message {
                message: "global variable reference must have pointer type".into(),
                loc: DiagLoc::span(loc),
            });
        }
        // `getGlobalVal`'s `ForwardRefVals` / `ForwardRefValIDs` hit is the
        // *same* `if (Val)` as the symbol-table hit, so it runs
        // `checkValidVariableType` too — against the placeholder's own type,
        // which `createGlobalFwdRef` minted at the first reference's address
        // space. A second reference at a different one is the error.
        if let Some(name) = name
            && let Some(entry) = self.forward_ref_globals.get(name)
        {
            let (placeholder_ty, constant) =
                (entry.placeholder.ty(), entry.placeholder.as_constant());
            check_valid_variable_type(loc, &format!("@{name}"), ty, placeholder_ty)?;
            return Ok(constant);
        }
        if let Some(id) = id
            && let Some(entry) = self.forward_ref_global_ids.get(&id)
        {
            let (placeholder_ty, constant) =
                (entry.placeholder.ty(), entry.placeholder.as_constant());
            check_valid_variable_type(loc, &format!("@{id}"), ty, placeholder_ty)?;
            return Ok(constant);
        }
        let placeholder =
            self.module
                .forward_ref_value_placeholder(ty)
                .map_err(|e| ParseError::Message {
                    message: format!("cannot create forward reference: {e}").into(),
                    loc: DiagLoc::span(loc),
                })?;
        let constant = placeholder.as_constant();
        let entry = ForwardRef { placeholder, loc };
        match (name, id) {
            (Some(name), _) => {
                self.forward_ref_globals.insert(name.to_owned(), entry);
            }
            (None, Some(id)) => {
                self.forward_ref_global_ids.insert(id, entry);
            }
            (None, None) => {
                unreachable!("a global forward reference is either named or numbered")
            }
        }
        Ok(constant)
    }

    fn intrinsic_parse_error(&self, loc: Span, err: IrError) -> ParseError {
        let expected = match err {
            IrError::UnknownIntrinsic { .. } => "unknown intrinsic",
            IrError::IntrinsicSignatureMismatch { .. } => "intrinsic signature mismatch",
            IrError::ReservedIntrinsicName { .. } => "intrinsic declaration modifier",
            _ => "intrinsic signature mismatch",
        };
        ParseError::Expected {
            expected: expected.into(),
            loc: DiagLoc::span(loc),
        }
    }

    fn intrinsic_modifier_error(&self, loc: Span) -> ParseError {
        ParseError::Expected {
            expected: "intrinsic declaration modifier".into(),
            loc: DiagLoc::span(loc),
        }
    }

    fn intrinsic_attribute_error(&self, loc: Span) -> ParseError {
        ParseError::Expected {
            expected: "intrinsic declaration attribute mismatch".into(),
            loc: DiagLoc::span(loc),
        }
    }

    fn into_slot_mapping(self) -> SlotMapping<'ctx, B> {
        let mut named_types = HashMap::with_capacity(self.named_types.len());
        for (name, entry) in self.named_types {
            named_types.insert(name, entry.ty);
        }
        let mut numbered_types = std::collections::BTreeMap::new();
        for (id, entry) in self.numbered_types {
            numbered_types.insert(id, entry.ty);
        }
        let mut metadata_nodes = NumberedValues::new();
        let mut metadata_entries: Vec<_> = self
            .metadata_slots
            .into_iter()
            .filter(|(_, entry)| entry.defined)
            .collect();
        metadata_entries.sort_by_key(|(slot, _)| *slot);
        for (slot, entry) in metadata_entries {
            let _ = metadata_nodes.add(slot, entry.id);
        }
        let mut attribute_groups = NumberedValues::new();
        let mut attr_entries: Vec<_> = self.module.attribute_groups().collect();
        attr_entries.sort_by_key(|(slot, _)| *slot);
        for (slot, storage) in attr_entries {
            let _ = attribute_groups.add(slot, storage);
        }
        SlotMapping {
            global_values: self.numbered_globals,
            named_types,
            numbered_types,
            attribute_groups,
            metadata_nodes,
        }
    }

    // ── Token plumbing ────────────────────────────────────────────────────

    /// Read the cached lookahead without advancing. Mirrors
    /// `LLParser::Lex.getKind()`.
    #[inline]
    fn peek(&self) -> &Token<'src> {
        &self.current.value
    }

    /// Span of the cached lookahead.
    #[inline]
    fn loc(&self) -> Span {
        self.current.span
    }

    /// Advance to the next lexer token, returning the *previous* span. Used
    /// by helpers that consume a punctuation / keyword and want to anchor a
    /// later diagnostic on the just-eaten token.
    fn bump(&mut self) -> ParseResult<Span> {
        let prev = self.current.span;
        // `LLLexer::LexToken` opens with `PrevTokEnd = CurPtr;` — the end of
        // the token being left behind, recorded before any whitespace is
        // skipped. Setting it here is the same instant.
        self.prev_token_end = prev.end;
        self.current = self.lex.next_token().map_err(map_lex_error)?;
        Ok(prev)
    }

    // ── Source positions (`AsmParserContext`) ────────────────────────────
    //
    // llvmkit's lexer works in byte offsets and has no `SourceMgr`, so the
    // `(line, column)` projection upstream gets from
    // `SourceMgr::getLineAndColumn` is computed here against a line-start
    // index built once, when the caller asks for a registry.

    /// Zero-based `(line, column)` of `offset`, mirroring
    /// `LLLexer::getTokLineColumnPos` / `getPrevTokEndLineColumnPos` — both of
    /// which subtract one from `SourceMgr`'s one-based answer.
    fn file_loc(&self, offset: u32) -> FileLoc {
        let line_index = match self.line_starts.binary_search(&offset) {
            Ok(index) => index,
            // `line_starts[0]` is 0 and `offset` is unsigned, so the
            // insertion point is never 0 and the subtraction cannot wrap.
            Err(index) => index.saturating_sub(1),
        };
        let line_start = self.line_starts.get(line_index).copied().unwrap_or(0);
        let line = u32::try_from(line_index).unwrap_or(u32::MAX);
        FileLoc::new(line, offset.saturating_sub(line_start))
    }

    /// The half-open range `[start, PrevTokEnd)` every `AsmParserContext`
    /// entry is closed at.
    fn file_loc_range_to_prev_token_end(&self, start: u32) -> FileLocRange {
        FileLocRange::new(self.file_loc(start), self.file_loc(self.prev_token_end))
    }

    fn require_eof(&self) -> ParseResult<()> {
        if matches!(self.peek(), Token::Eof) {
            Ok(())
        } else {
            Err(ParseError::Expected {
                expected: "end of string".into(),
                loc: DiagLoc::span(self.loc()),
            })
        }
    }

    pub(super) fn parse_type_at_beginning(mut self) -> ParseResult<(Type<'ctx, B>, usize)> {
        let start = self.loc().start;
        let ty = self.parse_type(true)?;
        let consumed = self.loc().start.saturating_sub(start);
        let consumed = usize::try_from(consumed).map_err(|_| ParseError::Expected {
            expected: "type byte count fits in usize".into(),
            loc: DiagLoc::span(self.loc()),
        })?;
        Ok((ty, consumed))
    }

    pub(super) fn parse_standalone_type(mut self) -> ParseResult<Type<'ctx, B>> {
        let ty = self.parse_type(true)?;
        self.require_eof()?;
        Ok(ty)
    }

    pub(super) fn parse_standalone_constant_value(
        mut self,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        // No re-wording. `LLParser::parseStandaloneConstantValue` propagates
        // `parseValID`, and its own `expected end of string` is a *parser*
        // diagnostic — `ErrorPriority::Parser` — so where the lexer already
        // recorded one (`end of file in string constant`, say) upstream's
        // priority rule keeps the lexer's and drops the parser's. A trailing
        // token that merely fails to lex is `Token::Error`, which
        // `require_eof` below reports as `expected end of string` on its own.
        // The arm that used to rewrite every `ParseError::Lex` into that
        // message therefore both duplicated the working case and mis-worded
        // the one upstream reports differently.
        let loc = self.loc();
        let id = self.parse_val_id(None, Some(ty))?;
        // `LLParser::parseConstantValue` switches on the *kind* and accepts a
        // fixed set. Everything outside it — a local or global name, inline
        // asm, `[]` — is `expected a constant value`, even where
        // `convertValIDToValue` would have had something to say.
        let value = match id.kind {
            ValIdKind::ApsInt(_)
            | ValIdKind::ApFloat(_)
            | ValIdKind::Undef
            | ValIdKind::Poison
            | ValIdKind::Zero
            | ValIdKind::Constant(_)
            | ValIdKind::ConstantSplat(_)
            | ValIdKind::ConstantStruct(_)
            | ValIdKind::PackedConstantStruct(_) => self.convert_val_id_to_constant(
                ty,
                ValId {
                    kind: id.kind,
                    loc: id.loc,
                },
            )?,
            // Upstream takes `Constant::getNullValue(Ty)` directly here rather
            // than going through the conversion, so `null` at a non-pointer
            // type is the type's zero rather than a diagnostic.
            ValIdKind::Null => self.zero_initializer_constant(id.loc, ty)?,
            ValIdKind::LocalId(_)
            | ValIdKind::LocalName(_)
            | ValIdKind::GlobalId(_)
            | ValIdKind::GlobalName(_)
            | ValIdKind::EmptyArray
            | ValIdKind::Value(_) => {
                return Err(ParseError::Message {
                    message: "expected a constant value".into(),
                    loc: DiagLoc::span(loc),
                });
            }
        };
        self.require_eof()?;
        Ok(value)
    }

    /// If the lookahead is `Punct`, consume it and return `true`. Otherwise
    /// leave the cursor untouched.
    fn eat_punct(&mut self, t: PunctKind) -> ParseResult<bool> {
        if t.matches(self.peek()) {
            self.bump()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Consume `t`, or return [`ParseError::Expected`] with the supplied
    /// description. Mirrors `LLParser::parseToken`.
    fn expect_punct(&mut self, t: PunctKind, expected: &'static str) -> ParseResult<Span> {
        if t.matches(self.peek()) {
            self.bump()
        } else {
            Err(self.expected(expected))
        }
    }

    /// [`expect_punct`](Self::expect_punct) for the handful of `parseToken`
    /// labels upstream writes with a capital `E`, which therefore cannot go
    /// through [`ParseError::Expected`]'s prefix-adding rendering. The message
    /// is carried verbatim.
    fn expect_message_punct(&mut self, t: PunctKind, message: &'static str) -> ParseResult<Span> {
        if t.matches(self.peek()) {
            self.bump()
        } else {
            Err(self.message(message))
        }
    }

    /// Consume `Kw(k)` if present.
    fn eat_keyword(&mut self, k: Keyword) -> ParseResult<bool> {
        if matches!(self.peek(), Token::Kw(got) if *got == k) {
            self.bump()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect_keyword(&mut self, k: Keyword, expected: &'static str) -> ParseResult<Span> {
        if matches!(self.peek(), Token::Kw(got) if *got == k) {
            self.bump()
        } else {
            Err(self.expected(expected))
        }
    }

    fn token_error(&self, expected: impl Into<Cow<'static, str>>) -> ParseError {
        self.expected(expected)
    }

    fn expected(&self, expected: impl Into<Cow<'static, str>>) -> ParseError {
        ParseError::Expected {
            expected: expected.into(),
            loc: DiagLoc::span(self.loc()),
        }
    }

    /// A diagnostic rendered verbatim, anchored at the current token.
    /// Mirrors `LLParser::tokError`.
    ///
    /// Use this whenever upstream's message does *not* begin with
    /// `expected `; [`Self::expected`] is for the ones that do, and stores
    /// only the production that follows the word.
    fn message(&self, message: impl Into<Cow<'static, str>>) -> ParseError {
        self.message_at(self.loc(), message)
    }

    /// A diagnostic rendered verbatim, anchored at an explicit span.
    /// Mirrors `LLParser::error(LocTy, const Twine &)`.
    fn message_at(&self, loc: Span, message: impl Into<Cow<'static, str>>) -> ParseError {
        ParseError::Message {
            message: message.into(),
            loc: DiagLoc::span(loc),
        }
    }

    /// [`Self::expected`] anchored at an explicit span — the `error(LocTy, …)`
    /// counterpart for the messages that do begin with `expected `.
    fn expected_at(&self, loc: Span, expected: impl Into<Cow<'static, str>>) -> ParseError {
        ParseError::Expected {
            expected: expected.into(),
            loc: DiagLoc::span(loc),
        }
    }

    /// Consume a `STRINGCONSTANT` token and decode it as UTF-8. Mirrors
    /// `LLParser::parseStringConstant`.
    fn parse_string_constant(&mut self, expected: &'static str) -> ParseResult<String> {
        let s = match self.peek() {
            Token::StringConstant(bytes) => {
                let s = std::str::from_utf8(bytes.as_ref())
                    .map_err(|_| self.expected("UTF-8 string constant"))?
                    .to_owned();
                Some(s)
            }
            _ => None,
        };
        match s {
            Some(s) => {
                self.bump()?;
                Ok(s)
            }
            None => Err(self.expected(expected)),
        }
    }

    /// Consume a `(` u32 `)` block. Mirrors `LLParser::parseOptionalAddrSpace`
    /// / its mandatory cousin.
    /// `addrspace ( <uint32> | "A" | "G" | "P" | "<datalayout name>" )`.
    /// Mirrors the inner `ParseAddrspaceValue` lambda of
    /// `LLParser::parseOptionalAddrSpace`.
    ///
    /// Every symbolic spelling resolves through the module's data layout,
    /// which is why `target datalayout` has to have been seen already —
    /// upstream guarantees that by parsing target definitions in their own
    /// pass before any entity.
    fn parse_addr_space_paren(&mut self) -> ParseResult<u32> {
        self.expect_punct(PunctKind::LParen, "'(' in address space")?;
        let addr_space = match self.peek() {
            Token::StringConstant(bytes) => {
                let name = std::str::from_utf8(bytes.as_ref())
                    .map_err(|_| self.expected("valid UTF-8 symbolic address space"))?
                    .to_owned();
                let layout = self.module.data_layout();
                let resolved = match name.as_str() {
                    "A" => layout.alloca_addr_space(),
                    "G" => layout.default_globals_addr_space(),
                    "P" => layout.program_addr_space(),
                    // `ParseAddrspaceValue`'s fourth arm:
                    // `M->getDataLayout().getNamedAddressSpace(AddrSpaceStr)`,
                    // a name the datalayout itself gave to an address space —
                    // `p2(global):32:8` makes `addrspace("global")` mean 2.
                    // Order is load-bearing: `A` / `G` / `P` are tested first,
                    // so they win over a datalayout name spelled the same way.
                    _ => match layout.named_address_space(&name) {
                        Some(addr_space) => addr_space,
                        None => {
                            return Err(ParseError::Message {
                                message: format!("invalid symbolic addrspace '{name}'").into(),
                                loc: DiagLoc::span(self.loc()),
                            });
                        }
                    },
                };
                self.bump()?;
                resolved
            }
            Token::IntegerLit(_) => {
                let loc = self.loc();
                let n = self.parse_uint32()?;
                // `isUInt<24>` — upstream checks the parsed value, not the
                // token, so the diagnostic points at the number.
                if n >= (1 << 24) {
                    return Err(
                        self.message_at(loc, "invalid address space, must be a 24-bit integer")
                    );
                }
                n
            }
            _ => return Err(self.expected("integer or string constant")),
        };
        self.expect_punct(PunctKind::RParen, "')' in address space")?;
        Ok(addr_space)
    }

    /// `addrspace(...)` where omitting it means the *program* address space.
    /// Mirrors `LLParser::parseOptionalProgramAddrSpace`.
    fn parse_optional_program_addr_space(&mut self) -> ParseResult<u32> {
        if self.eat_keyword(Keyword::Addrspace)? {
            self.parse_addr_space_paren()
        } else {
            Ok(self.module.data_layout().program_addr_space())
        }
    }

    /// `Lex.getKind() != lltok::APSInt || Lex.getAPSIntVal().isSigned()` — the
    /// guard `parseUInt32` and `parseUInt64` share, answered **without**
    /// consuming the token.
    ///
    /// The token has to survive the answer: upstream's `Lex.Lex()` comes after
    /// `parseUInt32`'s range check, so `expected 32-bit integer (too large)`
    /// is a `tokError` on the integer itself. The base and the digit count are
    /// never inspected — only the token kind and the signedness are.
    fn peek_unsigned_apsint(&self) -> Option<IntLit<'src>> {
        match self.peek() {
            Token::IntegerLit(lit) if int_lit_signedness(*lit) == Signedness::Unsigned => {
                Some(*lit)
            }
            _ => None,
        }
    }

    /// Mirrors `LLParser::parseUInt32`, including its second message: a value
    /// that does not round-trip through `unsigned` is
    /// `expected 32-bit integer (too large)`, which is why
    /// `attributes #0 = { align = 4294967296 }` fails where the inline
    /// `align 4294967296` succeeds.
    fn parse_uint32(&mut self) -> ParseResult<u32> {
        let Some(lit) = self.peek_unsigned_apsint() else {
            return Err(self.expected("integer"));
        };
        // `getLimitedValue(0xFFFFFFFFULL + 1)` saturates rather than failing,
        // so a literal too wide even for 64 bits still reaches the range check
        // below and answers `expected 32-bit integer (too large)` rather than
        // `expected integer`.
        let value64 = self
            .apsint_from_int_lit(lit)?
            .value
            .limited_value(0xFFFF_FFFF_u64 + 1);
        let Ok(value) = u32::try_from(value64) else {
            return Err(self.expected("32-bit integer (too large)"));
        };
        self.bump()?;
        Ok(value)
    }

    /// Mirrors `LLParser::parseUInt64`. Its one message is `expected integer`
    /// at every one of upstream's call sites, so this takes no label; the
    /// bespoke per-site wordings llvmkit used to pass were all divergences.
    ///
    /// `getLimitedValue()`'s default limit is `UINT64_MAX`, so a literal wider
    /// than 64 bits **saturates** instead of failing — `align 99999…9` reaches
    /// `parseOptionalAlignment`'s `alignment is not a power of two` rather
    /// than being refused as a non-integer.
    fn parse_uint64(&mut self) -> ParseResult<u64> {
        let Some(lit) = self.peek_unsigned_apsint() else {
            return Err(self.expected("integer"));
        };
        let value = self.apsint_from_int_lit(lit)?.value.limited_value(u64::MAX);
        self.bump()?;
        Ok(value)
    }

    /// Read one integer-literal token into the `APSInt` the lexer would have
    /// produced. The value's width is the token's own; widening to whatever
    /// the destination wants is [`ParsedApsInt::extend_or_truncate`]'s job,
    /// exactly as it is `LLParser`'s.
    ///
    /// Two upstream rules meet here, both in `LLLexer`:
    ///
    /// - a decimal literal goes through `APSInt::APSInt(StringRef)`, which
    ///   parses at an over-estimated width and then truncates to the value's
    ///   significant bits (negative, signed) or active bits (non-negative,
    ///   unsigned);
    /// - a `[us]0x…` literal is built at `4 * digits` bits by
    ///   `LLLexer::lexIdentifier` and truncated to its **active** bits when
    ///   those are fewer, *before* the `s`/`u` prefix decides the signedness.
    ///   That truncation is why `s0x0F` is −1 and not 15.
    fn parse_int_literal(&mut self) -> ParseResult<ParsedApsInt> {
        let lit = match self.peek() {
            Token::IntegerLit(lit) => *lit,
            _ => return Err(self.expected("integer literal")),
        };
        let parsed = self.apsint_from_int_lit(lit)?;
        self.bump()?;
        Ok(parsed)
    }

    /// The value half of [`Self::parse_int_literal`], **without** the
    /// `Lex.Lex()`. It stands where `LLLexer` builds `APSIntVal`, so the
    /// routines that read a token twice — `parseUInt32` inspecting the value
    /// and then reporting on the still-current token — have somewhere to ask.
    fn apsint_from_int_lit(&self, lit: IntLit<'_>) -> ParseResult<ParsedApsInt> {
        let signedness = int_lit_signedness(lit);
        Ok(match lit.base {
            NumBase::Dec => {
                let scratch_width = decimal_scratch_bits(lit.digits);
                let magnitude = ApInt::from_string(scratch_width, lit.digits, 10)
                    .map_err(|_| self.expected("valid integer literal"))?;
                let value = if matches!(lit.sign, Sign::Neg) {
                    let value = magnitude.negate();
                    let minimum = value.significant_bits().max(1);
                    value.trunc(minimum).unwrap_or(value)
                } else {
                    let active = magnitude.active_bits().max(1);
                    magnitude.trunc(active).unwrap_or(magnitude)
                };
                ParsedApsInt { value, signedness }
            }
            NumBase::HexSigned | NumBase::HexUnsigned => {
                let digit_width = u32::try_from(lit.digits.len())
                    .unwrap_or(u32::MAX / 4)
                    .saturating_mul(4);
                let value = ApInt::from_string(digit_width, lit.digits, 16)
                    .map_err(|_| self.expected("valid hexadecimal integer literal"))?;
                let active = value.active_bits();
                let value = if active > 0 && active < digit_width {
                    value.trunc(active).unwrap_or(value)
                } else {
                    value
                };
                ParsedApsInt { value, signedness }
            }
        })
    }

    // ── Instruction modifier parsing ──────────────────────────────────────

    /// Parse optional fast-math flags: `nnan ninf nsz arcp contract reassoc afn fast`.
    /// Mirrors `LLParser::parseOptionalFastMathFlags` (LLParser.cpp ~6490).
    fn parse_optional_fmf(&mut self) -> ParseResult<FastMathFlags> {
        let mut flags = FastMathFlags::empty();
        loop {
            match self.peek() {
                Token::Kw(Keyword::Nnan) => {
                    flags |= FastMathFlags::NO_NANS;
                    self.bump()?;
                }
                Token::Kw(Keyword::Ninf) => {
                    flags |= FastMathFlags::NO_INFS;
                    self.bump()?;
                }
                Token::Kw(Keyword::Nsz) => {
                    flags |= FastMathFlags::NO_SIGNED_ZEROS;
                    self.bump()?;
                }
                Token::Kw(Keyword::Arcp) => {
                    flags |= FastMathFlags::ALLOW_RECIPROCAL;
                    self.bump()?;
                }
                Token::Kw(Keyword::Contract) => {
                    flags |= FastMathFlags::ALLOW_CONTRACT;
                    self.bump()?;
                }
                Token::Kw(Keyword::Afn) => {
                    flags |= FastMathFlags::APPROX_FUNC;
                    self.bump()?;
                }
                Token::Kw(Keyword::Reassoc) => {
                    flags |= FastMathFlags::ALLOW_REASSOC;
                    self.bump()?;
                }
                Token::Kw(Keyword::Fast) => {
                    flags = FastMathFlags::fast();
                    self.bump()?;
                }
                _ => break,
            }
        }
        Ok(flags)
    }

    /// `align N` / `align(N)`. Ports `LLParser::parseOptionalAlignment` from
    /// the point where the `align` keyword is known to be present.
    ///
    /// Both diagnostics are anchored at `AlignLoc`, which upstream captures
    /// immediately after eating `align` and **before** the optional paren —
    /// so `AlignLoc == ParenLoc`, and `align(3)` reports under the `(`, not
    /// under the `3`. The `expected ')'` is anchored there too.
    fn parse_optional_alignment_value(&mut self, allow_parens: bool) -> ParseResult<u64> {
        self.expect_keyword(Keyword::Align, "'align'")?;
        let align_loc = self.loc();
        let have_parens = allow_parens && self.eat_punct(PunctKind::LParen)?;
        let value = self.parse_uint64()?;
        if have_parens && !self.eat_punct(PunctKind::RParen)? {
            return Err(self.message_at(align_loc, "expected ')'"));
        }
        self.check_alignment_value(value, align_loc)?;
        Ok(value)
    }

    /// `parseOptionalAlignment`'s two value checks, in its order. Split out
    /// because the attribute-group `align = N` form reaches them from a
    /// different grammar.
    ///
    /// `MaximumAlignment` is `1 << 32` (`llvm/IR/Value.h`); note that a
    /// non-power-of-two is reported first, so `align 3` is never "huge".
    fn check_alignment_value(&self, value: u64, loc: Span) -> ParseResult<()> {
        if !value.is_power_of_two() {
            return Err(self.message_at(loc, "alignment is not a power of two"));
        }
        if value > (1u64 << 32) {
            return Err(self.message_at(loc, "huge alignments are not supported yet"));
        }
        Ok(())
    }

    /// `alignstack(N)`. Ports `LLParser::parseOptionalStackAlignment` from the
    /// point where the keyword is known to be present.
    ///
    /// Unlike `align`, this one's `AlignLoc` is the *number* token — it is
    /// captured after the `(` is eaten — and the power-of-two check runs only
    /// after the closing paren. `0` fails it, since `isPowerOf2_32(0)` is
    /// false.
    fn parse_stack_alignment_value(&mut self) -> ParseResult<u64> {
        self.expect_keyword(Keyword::Alignstack, "'alignstack'")?;
        self.expect_punct(PunctKind::LParen, "'('")?;
        let align_loc = self.loc();
        let value = u64::from(self.parse_uint32()?);
        self.expect_punct(PunctKind::RParen, "')'")?;
        if !value.is_power_of_two() {
            return Err(self.message_at(align_loc, "stack alignment is not a power of two"));
        }
        Ok(value)
    }

    /// Parse `align N` for an instruction or global. Returns the alignment.
    ///
    /// Mirrors `LLParser::parseOptionalAlignment` with `AllowParens = false`,
    /// which is how every instruction and global site calls it.
    fn parse_align_val(&mut self) -> ParseResult<Align> {
        let value = self.parse_optional_alignment_value(false)?;
        Align::new(value).map_err(|_| {
            // `check_alignment_value` has already refused zero, a
            // non-power-of-two, and anything above `1 << 32`, so `Align::new`
            // cannot fail here.
            self.message_at(self.loc(), "alignment is not a power of two")
        })
    }

    /// Parse `, align N` if lookahead is `,` followed by `align`.
    /// Returns `None` without consuming when the comma starts a different suffix.
    fn parse_optional_comma_align(&mut self) -> ParseResult<Option<Align>> {
        if !matches!(self.peek(), Token::Comma) {
            return Ok(None);
        }
        let saved_lex = self.lex.clone();
        let saved_current = self.current.clone();
        let saved_prev_token_end = self.prev_token_end;
        self.bump()?;
        if matches!(self.peek(), Token::Kw(Keyword::Align)) {
            Ok(Some(self.parse_align_val()?))
        } else {
            self.lex = saved_lex;
            self.current = saved_current;
            self.prev_token_end = saved_prev_token_end;
            Ok(None)
        }
    }

    /// Parse an atomic ordering keyword.
    /// Mirrors `LLParser::parseOrdering` (LLParser.cpp ~2810).
    /// Mirrors `LLParser::parseOrdering`, whose one message —
    /// capital `E` included — is the same at every call site, so this takes
    /// no label. The per-instruction complaints (`invalid cmpxchg success
    /// ordering` and friends) are *validity* checks that run after this,
    /// not alternative spellings of it.
    fn parse_atomic_ordering(&mut self) -> ParseResult<AtomicOrdering> {
        let ord = match self.peek() {
            Token::Kw(Keyword::Unordered) => AtomicOrdering::Unordered,
            Token::Kw(Keyword::Monotonic) => AtomicOrdering::Monotonic,
            Token::Kw(Keyword::Acquire) => AtomicOrdering::Acquire,
            Token::Kw(Keyword::Release) => AtomicOrdering::Release,
            Token::Kw(Keyword::AcqRel) => AtomicOrdering::AcquireRelease,
            Token::Kw(Keyword::SeqCst) => AtomicOrdering::SequentiallyConsistent,
            _ => return Err(self.message("Expected ordering on atomic instruction")),
        };
        self.bump()?;
        Ok(ord)
    }

    /// `isValidVisibilityForLinkage` and `isValidDLLStorageClassForLinkage`
    /// (`LLParser.cpp`), together, since upstream asks them as a pair at every
    /// one of its three call sites: `parseAliasOrIFunc`, `parseGlobal` and
    /// `parseFunctionHeader`.
    ///
    /// Both read the same way: a *local* linkage — internal or private —
    /// forces default visibility and default DLL storage class, and any other
    /// linkage constrains neither. Both are reported at `NameLoc`, the
    /// entity's own name, not at the offending keyword.
    fn check_linkage_agreement(
        linkage: Linkage,
        visibility: Visibility,
        dll_storage_class: DllStorageClass,
        name_loc: Span,
    ) -> ParseResult<()> {
        if !matches!(linkage, Linkage::Internal | Linkage::Private) {
            return Ok(());
        }
        if visibility != Visibility::Default {
            return Err(ParseError::Message {
                message: "symbol with local linkage must have default visibility".into(),
                loc: DiagLoc::span(name_loc),
            });
        }
        if dll_storage_class != DllStorageClass::Default {
            return Err(ParseError::Message {
                message: "symbol with local linkage cannot have a DLL storage class".into(),
                loc: DiagLoc::span(name_loc),
            });
        }
        Ok(())
    }

    /// `code_model "small"`. Ports `LLParser::parseOptionalCodeModel`.
    ///
    /// Both failures — an unrecognised spelling and a token that is not a
    /// string at all — carry the same text, because upstream binds it to a
    /// local and uses it twice. Note the order: it inspects `getStrVal()`
    /// *before* checking the token is a `StringConstant`, so a non-string
    /// token reads as the empty string and falls through the chain to the
    /// same message.
    fn parse_optional_code_model(&mut self) -> ParseResult<llvmkit_ir::CodeModel> {
        self.expect_keyword(Keyword::CodeModel, "'code_model'")?;
        const ERR: &str = "global code model string";
        let Token::StringConstant(bytes) = self.peek() else {
            return Err(self.expected(ERR));
        };
        let Ok(spelled) = std::str::from_utf8(bytes.as_ref()) else {
            return Err(self.expected(ERR));
        };
        let Some(model) = llvmkit_ir::CodeModel::from_name(spelled) else {
            return Err(self.expected(ERR));
        };
        self.bump()?;
        Ok(model)
    }

    /// `isSanitizer` plus the `parseSanitizer` switch, as one lookup: which
    /// field of `SanitizerMetadata` a token sets, or `None` when the token is
    /// not a sanitizer keyword at all.
    ///
    /// Upstream's `default:` arm — `non-sanitizer token passed to
    /// LLParser::parseSanitizer()` — is unreachable by construction, since
    /// `parseGlobal` guards the call with `isSanitizer(Lex.getKind())`. The
    /// `Option` here is that guard, so the message has no counterpart.
    fn sanitizer_for_token(
        &self,
        token: &Token<'_>,
    ) -> Option<fn(llvmkit_ir::SanitizerMetadata) -> llvmkit_ir::SanitizerMetadata> {
        match token {
            Token::Kw(Keyword::NoSanitizeAddress) => Some(|mut m| {
                m.no_address = true;
                m
            }),
            Token::Kw(Keyword::NoSanitizeHwaddress) => Some(|mut m| {
                m.no_hwaddress = true;
                m
            }),
            Token::Kw(Keyword::SanitizeMemtag) => Some(|mut m| {
                m.memtag = true;
                m
            }),
            Token::Kw(Keyword::SanitizeAddressDyninit) => Some(|mut m| {
                m.is_dyn_init = true;
                m
            }),
            _ => None,
        }
    }

    /// Parse optional `syncscope("...")`. Returns `SyncScope::System` if absent.
    /// Mirrors `LLParser::parseOptionalScope` (LLParser.cpp).
    ///
    /// Only `"singlethread"` and the absent default map to the two well-known
    /// scopes: `LLVMContext::LLVMContext` seeds `getOrInsertSyncScopeID` with
    /// `"singlethread"` and the *empty* string (the canonical `System` name),
    /// so a source-level `syncscope("system")` is an ordinary named scope
    /// distinct from the default and must round-trip as text.
    fn parse_optional_syncscope(&mut self) -> ParseResult<SyncScope> {
        if !matches!(self.peek(), Token::Kw(Keyword::Syncscope)) {
            return Ok(SyncScope::System);
        }
        self.bump()?; // eat `syncscope`
        // Upstream's three messages here open with a capital `E`, alone among
        // its diagnostics. Contractual, not a typo to tidy.
        let paren_loc = self.loc();
        if !self.eat_punct(PunctKind::LParen)? {
            return Err(self.message_at(paren_loc, "Expected '(' in syncscope"));
        }
        let name_loc = self.loc();
        let Ok(name) = self.parse_string_constant("sync scope name") else {
            return Err(self.message_at(name_loc, "Expected synchronization scope name"));
        };
        let end_loc = self.loc();
        if !self.eat_punct(PunctKind::RParen)? {
            return Err(self.message_at(end_loc, "Expected ')' in syncscope"));
        }
        Ok(match name.as_str() {
            "singlethread" => SyncScope::SingleThread,
            _ => SyncScope::Named(name),
        })
    }

    fn current_str_payload(&self) -> Option<String> {
        match self.peek() {
            Token::GlobalVar(s) | Token::LocalVar(s) => {
                std::str::from_utf8(s.as_ref()).ok().map(str::to_owned)
            }
            _ => None,
        }
    }

    // ── Top-level entities ───────────────────────────────────────────────

    /// The leading run of `target ...` / `source_filename = ...` entities, and
    /// the data-layout override that follows it. Mirrors
    /// `LLParser::parseTargetDefinitions`.
    ///
    /// The split from `parseTopLevelEntities` is what makes
    /// `DataLayoutCallbackTy` possible at all: the file's own layout string is
    /// held *tentative* until the whole run has been read, so the callback sees
    /// the target triple beside it and can answer with a replacement — which is
    /// how a module carrying a layout string this build cannot parse is still
    /// importable.
    ///
    /// **Divergence, unchanged by this routine:** upstream's
    /// `parseTopLevelEntities` has no `kw_target` and no `kw_source_filename`
    /// arm, so a `target triple` written after any other entity is
    /// `expected top-level entity` there. llvmkit still accepts one, through
    /// [`Self::parse_late_target_definition`]; a late `target datalayout` is
    /// therefore validated and installed immediately and never reaches the
    /// callback (`docs/divergences.md` D15).
    fn parse_target_definitions(&mut self, config: &ParserConfig<'_>) -> ParseResult<()> {
        // `std::string TentativeDLStr = M->getDataLayoutStr();`
        let mut tentative_layout = self.module.data_layout().to_string();
        let mut layout_loc = None;
        loop {
            match self.peek() {
                Token::Kw(Keyword::Target) => {
                    self.parse_target_definition(&mut tentative_layout, &mut layout_loc)?;
                }
                Token::Kw(Keyword::SourceFilename) => self.parse_source_filename()?,
                // `default: Done = true;`
                _ => break,
            }
        }
        if let Some(callback) = config.data_layout_callback {
            // `DataLayoutCallback(M->getTargetTriple().str(), TentativeDLStr)`.
            // An unset triple is the empty string there, not a null.
            let triple = self.module.target_triple().unwrap_or_default();
            if let Some(overridden) = callback(&triple, &tentative_layout) {
                tentative_layout = overridden;
                // `DLStrLoc = {};` — an overridden string is no longer
                // anchored at anything the file wrote.
                layout_loc = None;
            }
        }
        // `Expected<DataLayout> MaybeDL = DataLayout::parse(TentativeDLStr);`
        // runs unconditionally, so a file with no `target datalayout` re-parses
        // the empty string and installs the same default layout it already had.
        self.set_data_layout(&tentative_layout, layout_loc)
    }

    /// `target datalayout = STRING` / `target triple = STRING`. Mirrors
    /// `LLParser::parseTargetDefinition`, whose two out-parameters this takes:
    /// the layout string is *not* installed here, only remembered together with
    /// the location its diagnostic would be anchored at.
    fn parse_target_definition(
        &mut self,
        tentative_layout: &mut String,
        layout_loc: &mut Option<Span>,
    ) -> ParseResult<()> {
        self.expect_keyword(Keyword::Target, "'target'")?;
        match self.peek() {
            Token::Kw(Keyword::Triple) => {
                self.bump()?;
                self.expect_punct(PunctKind::Equal, "'=' after target triple")?;
                let s = self.parse_string_constant("target-triple string constant")?;
                self.module.set_target_triple(s);
                Ok(())
            }
            Token::Kw(Keyword::Datalayout) => {
                self.bump()?;
                self.expect_punct(PunctKind::Equal, "'=' after target datalayout")?;
                *layout_loc = Some(self.loc());
                *tentative_layout =
                    self.parse_string_constant("target-datalayout string constant")?;
                Ok(())
            }
            // `parseTargetDefinition`'s `default:` arm.
            _ => Err(self.message("unknown target property")),
        }
    }

    /// A `target ...` entity reached *after* the leading run — something
    /// upstream rejects outright (see [`Self::parse_target_definitions`]).
    /// llvmkit keeps accepting it, and installs the layout on the spot because
    /// there is no tentative phase left to defer it to.
    fn parse_late_target_definition(&mut self) -> ParseResult<()> {
        let mut tentative_layout = self.module.data_layout().to_string();
        let mut layout_loc = None;
        self.parse_target_definition(&mut tentative_layout, &mut layout_loc)?;
        match layout_loc {
            Some(_) => self.set_data_layout(&tentative_layout, layout_loc),
            // `target triple` — nothing to install.
            None => Ok(()),
        }
    }

    /// `M->setDataLayout(MaybeDL.get())`, with upstream's
    /// `error(DLStrLoc, toString(MaybeDL.takeError()))` on the failing half.
    /// A `None` anchor is upstream's default-constructed `LocTy`, which
    /// `SourceMgr` renders without a caret; llvmkit has no such rendering, so
    /// the diagnostic falls back to the current token.
    fn set_data_layout(&mut self, layout: &str, layout_loc: Option<Span>) -> ParseResult<()> {
        let loc = layout_loc.unwrap_or_else(|| self.loc());
        let parsed = DataLayout::parse(layout).map_err(|e| match e {
            IrError::InvalidDataLayout { reason } => ParseError::Expected {
                expected: format!("valid datalayout: {reason}").into(),
                loc: DiagLoc::span(loc),
            },
            other => ParseError::Expected {
                expected: format!("valid datalayout: {other}").into(),
                loc: DiagLoc::span(loc),
            },
        })?;
        self.module.set_data_layout(parsed);
        Ok(())
    }

    /// `source_filename = STRING`. Mirrors `LLParser::parseSourceFileName`,
    /// which stores the string on the parser and then, `if (M)`, on the module
    /// — llvmkit reads it back off the module wherever upstream reads its own
    /// copy, which is the local-symbol GUID computation.
    fn parse_source_filename(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::SourceFilename, "'source_filename'")?;
        self.expect_punct(PunctKind::Equal, "'=' after source_filename")?;
        let source_filename = self.parse_string_constant("source-filename string constant")?;
        self.module.set_source_filename(source_filename);
        Ok(())
    }

    // ── Module summary index ──────────────────────────────────────────────
    //
    // Ports the `^N` half of `LLParser.cpp`. Upstream keeps these routines in
    // the same class as the rest of the parser and shares its lexer, its
    // `parseToken` helper and its diagnostics; llvmkit used to keep a second,
    // more permissive parser with a lexer of its own, which is why it accepted
    // a module entry with no `hash:` and discarded a `typeid:` payload whole.

    /// The index under construction. `None` is upstream's null `Index`.
    fn summary_index_mut(&mut self) -> Option<&mut ModuleSummaryIndex> {
        self.summary_index.as_mut()
    }

    /// SummaryEntry
    ///   ::= SummaryID '=' GVEntry | ModuleEntry | TypeIdEntry
    ///
    /// Mirrors `LLParser::parseSummaryEntry`, whose early returns deliberately
    /// leave the lexer in colon-splitting mode: a failed `'='` and the
    /// no-index skip both bypass the `setIgnoreColonInIdentifiers(false)` at
    /// the bottom.
    fn parse_summary_entry(&mut self) -> ParseResult<()> {
        let Token::SummaryId(summary_id) = self.current.value else {
            return Err(self.token_error("summary id"));
        };

        // Inside a summary entry a colon is a token of its own rather than the
        // tail of a label.
        self.lex.ignore_colon_in_idents = true;

        self.bump()?;
        self.expect_punct(PunctKind::Equal, "'=' here")?;

        if self.summary_index.is_none() {
            return self.skip_module_summary_entry();
        }

        let loc = self.loc();
        let result = match self.peek() {
            Token::Kw(Keyword::Gv) => self.parse_gv_entry(summary_id),
            Token::Kw(Keyword::Module) => self.parse_module_entry(summary_id),
            Token::Kw(Keyword::Typeid) => self.parse_type_id_entry(summary_id),
            Token::Kw(Keyword::TypeidCompatibleVtable) => {
                self.parse_type_id_compatible_vtable_entry(summary_id)
            }
            Token::Kw(Keyword::Flags) => self.parse_summary_index_flags(),
            Token::Kw(Keyword::Blockcount) => self.parse_block_count(),
            _ => Err(self.message_at(loc, "unexpected summary kind")),
        };
        self.lex.ignore_colon_in_idents = false;
        result
    }

    /// Mirrors `LLParser::skipModuleSummaryEntry`. Note the keyword guard does
    /// not list `typeidCompatibleVTable`, so that spelling is rejected here
    /// even though `parseSummaryEntry` dispatches it when an index is present.
    fn skip_module_summary_entry(&mut self) -> ParseResult<()> {
        if !matches!(
            self.peek(),
            Token::Kw(
                Keyword::Gv
                    | Keyword::Module
                    | Keyword::Typeid
                    | Keyword::Flags
                    | Keyword::Blockcount
            )
        ) {
            return Err(self.message(
                "Expected 'gv', 'module', 'typeid', 'flags' or 'blockcount' at the start of summary entry",
            ));
        }
        if matches!(self.peek(), Token::Kw(Keyword::Flags)) {
            return self.parse_summary_index_flags();
        }
        if matches!(self.peek(), Token::Kw(Keyword::Blockcount)) {
            return self.parse_block_count();
        }
        self.bump()?;
        self.expect_punct(PunctKind::Colon, "':' at start of summary entry")?;
        self.expect_punct(PunctKind::LParen, "'(' at start of summary entry")?;

        // Walk the parenthesized entry until the count of open parentheses
        // returns to zero; the first `(` was consumed above.
        let mut open_parens = 1u32;
        loop {
            match self.peek() {
                Token::LParen => open_parens += 1,
                Token::RParen => open_parens -= 1,
                Token::Eof => {
                    return Err(self.message("found end of file while parsing summary entry"));
                }
                // Skip everything in between parentheses.
                _ => {}
            }
            self.bump()?;
            if open_parens == 0 {
                return Ok(());
            }
        }
    }

    /// Mirrors `LLParser::parseModuleEntry`. The hash clause is *not* optional.
    fn parse_module_entry(&mut self, id: u32) -> ParseResult<()> {
        self.expect_keyword(Keyword::Module, "'module' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        self.expect_keyword(Keyword::Path, "'path' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let path = self.parse_string_constant("string constant")?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        self.expect_keyword(Keyword::Hash_, "'hash' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut hash = [0u32; 5];
        for (position, word) in hash.iter_mut().enumerate() {
            if position != 0 {
                self.expect_punct(PunctKind::Comma, "',' here")?;
            }
            *word = self.parse_uint32()?;
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        self.expect_punct(PunctKind::RParen, "')' here")?;

        if let Some(index) = self.summary_index_mut() {
            index.add_module(path.clone(), hash);
        }
        self.summary_module_paths.insert(id, path);
        Ok(())
    }

    /// Mirrors `LLParser::parseSummaryIndexFlags`.
    fn parse_summary_index_flags(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Flags, "'flags' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let flags = self.parse_uint64()?;
        if let Some(index) = self.summary_index_mut() {
            index.set_flags(IndexFlags::from_raw(flags));
        }
        Ok(())
    }

    /// Mirrors `LLParser::parseBlockCount`.
    fn parse_block_count(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Blockcount, "'blockcount' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let block_count = self.parse_uint64()?;
        if let Some(index) = self.summary_index_mut() {
            index.set_block_count(block_count);
        }
        Ok(())
    }

    /// GVEntry
    ///   ::= 'gv' ':' '(' ('name' ':' STRINGCONSTANT | 'guid' ':' UInt64)
    ///         [',' 'summaries' ':' Summary[',' Summary]* ]? ')'
    ///
    /// Mirrors `LLParser::parseGVEntry`.
    fn parse_gv_entry(&mut self, id: u32) -> ParseResult<()> {
        self.expect_keyword(Keyword::Gv, "'gv' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let loc = self.loc();
        let mut name = String::new();
        let mut guid = None;
        match self.peek() {
            Token::Kw(Keyword::Name) => {
                self.bump()?;
                self.expect_punct(PunctKind::Colon, "':' here")?;
                // The GUID cannot be computed until the linkage is known.
                name = self.parse_string_constant("string constant")?;
            }
            Token::Kw(Keyword::Guid) => {
                self.bump()?;
                self.expect_punct(PunctKind::Colon, "':' here")?;
                guid = Some(Guid::from_raw(self.parse_uint64()?));
            }
            _ => return Err(self.message_at(loc, "expected name or guid tag")),
        }

        if !self.eat_punct(PunctKind::Comma)? {
            // No summaries. This entry was created for a call to an external or
            // indirect target: a GUID with no summary came from a `VALUE_GUID`
            // record, a name with no GUID from an external definition. External
            // linkage is passed because it is only consulted when the GUID must
            // be computed from the name, and then the symbol must be external.
            self.expect_punct(PunctKind::RParen, "')' here")?;
            return self.add_global_value_to_index(&name, guid, Linkage::External, id, None, loc);
        }

        self.expect_keyword(Keyword::Summaries, "'summaries' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        loop {
            let kind_loc = self.loc();
            match self.peek() {
                Token::Kw(Keyword::Function) => self.parse_function_summary(&name, guid, id)?,
                Token::Kw(Keyword::Variable) => self.parse_variable_summary(&name, guid, id)?,
                Token::Kw(Keyword::Alias) => self.parse_alias_summary(&name, guid, id)?,
                _ => return Err(self.message_at(kind_loc, "expected summary type")),
            }
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(())
    }

    /// FunctionSummary
    ///   ::= 'function' ':' '(' 'module' ':' ModuleReference ',' GVFlags
    ///         ',' 'insts' ':' UInt32 [',' OptionalFFlags]? [',' OptionalCalls]?
    ///         [',' OptionalTypeIdInfo]? [',' OptionalParamAccesses]?
    ///         [',' OptionalRefs]? ')'
    ///
    /// Mirrors `LLParser::parseFunctionSummary`.
    fn parse_function_summary(
        &mut self,
        name: &str,
        guid: Option<Guid>,
        id: u32,
    ) -> ParseResult<()> {
        let loc = self.loc();
        self.expect_keyword(Keyword::Function, "'function' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        let module_path = self.parse_module_reference()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        let flags = self.parse_gv_flags()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        self.expect_keyword(Keyword::Insts, "'insts' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let instruction_count = self.parse_uint32()?;

        let mut summary = FunctionSummary {
            instruction_count,
            ..FunctionSummary::default()
        };
        let mut references = Vec::new();
        let mut type_id_info = TypeIdInfo::default();

        while self.eat_punct(PunctKind::Comma)? {
            let field_loc = self.loc();
            match self.peek() {
                Token::Kw(Keyword::FuncFlags) => {
                    summary.function_flags = self.parse_optional_function_flags()?;
                }
                Token::Kw(Keyword::Calls) => summary.calls = self.parse_optional_calls()?,
                Token::Kw(Keyword::TypeIdInfo) => {
                    type_id_info = self.parse_optional_type_id_info()?;
                }
                Token::Kw(Keyword::Refs) => references = self.parse_optional_refs()?,
                Token::Kw(Keyword::Params) => {
                    summary.parameter_accesses = self.parse_optional_param_accesses()?;
                }
                Token::Kw(Keyword::Allocs) => {
                    summary.allocations = self.parse_optional_allocs()?;
                }
                Token::Kw(Keyword::Callsites) => {
                    summary.callsites = self.parse_optional_callsites()?;
                }
                _ => {
                    return Err(
                        self.message_at(field_loc, "expected optional function summary field")
                    );
                }
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;

        // `FunctionSummary`'s constructor leaves `TIdInfo` null unless one of
        // its five lists is non-empty, and a null `TIdInfo` is what suppresses
        // the whole `typeIdInfo:` clause on output.
        if !type_id_info.is_empty() {
            summary.type_id_info = Some(type_id_info);
        }

        let linkage = flags.linkage;
        self.add_global_value_to_index(
            name,
            guid,
            linkage,
            id,
            Some(GlobalValueSummary {
                module_path,
                flags,
                references,
                kind: SummaryKind::Function(summary),
            }),
            loc,
        )
    }

    /// VariableSummary
    ///   ::= 'variable' ':' '(' 'module' ':' ModuleReference ',' GVFlags
    ///         ',' GVarFlags [',' OptionalVTableFuncs]? [',' OptionalRefs]? ')'
    ///
    /// Mirrors `LLParser::parseVariableSummary`.
    fn parse_variable_summary(
        &mut self,
        name: &str,
        guid: Option<Guid>,
        id: u32,
    ) -> ParseResult<()> {
        let loc = self.loc();
        self.expect_keyword(Keyword::Variable, "'variable' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        let module_path = self.parse_module_reference()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        let flags = self.parse_gv_flags()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        let variable_flags = self.parse_gvar_flags()?;

        let mut summary = GlobalVariableSummary {
            variable_flags,
            vtable_functions: Vec::new(),
        };
        let mut references = Vec::new();

        while self.eat_punct(PunctKind::Comma)? {
            let field_loc = self.loc();
            match self.peek() {
                Token::Kw(Keyword::VtableFuncs) => {
                    summary.vtable_functions = self.parse_optional_vtable_funcs()?;
                }
                Token::Kw(Keyword::Refs) => references = self.parse_optional_refs()?,
                _ => {
                    return Err(
                        self.message_at(field_loc, "expected optional variable summary field")
                    );
                }
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;

        let linkage = flags.linkage;
        self.add_global_value_to_index(
            name,
            guid,
            linkage,
            id,
            Some(GlobalValueSummary {
                module_path,
                flags,
                references,
                kind: SummaryKind::Variable(summary),
            }),
            loc,
        )
    }

    /// AliasSummary
    ///   ::= 'alias' ':' '(' 'module' ':' ModuleReference ',' GVFlags ','
    ///         'aliasee' ':' GVReference ')'
    ///
    /// Mirrors `LLParser::parseAliasSummary`.
    fn parse_alias_summary(&mut self, name: &str, guid: Option<Guid>, id: u32) -> ParseResult<()> {
        let loc = self.loc();
        self.expect_keyword(Keyword::Alias, "'alias' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        let module_path = self.parse_module_reference()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        let flags = self.parse_gv_flags()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        self.expect_keyword(Keyword::Aliasee, "'aliasee' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;

        let mut aliasee = None;
        if !self.eat_keyword(Keyword::Null)? {
            let reference = self.parse_gv_reference()?;
            if reference.is_forward {
                // Recorded rather than resolved: the aliasee is not parsed yet.
                self.pending_summary_aliasee = Some((reference.summary_id, loc));
            } else {
                // Upstream looks the aliasee's summary up in the same module
                // and asserts it is a definition. The lookup answers the same
                // GUID the reference already carries, and the assert is not a
                // diagnostic, so only the GUID is kept.
                aliasee = Some(reference.value.guid);
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;

        let linkage = flags.linkage;
        self.add_global_value_to_index(
            name,
            guid,
            linkage,
            id,
            Some(GlobalValueSummary {
                module_path,
                flags,
                references: Vec::new(),
                kind: SummaryKind::Alias(AliasSummary { aliasee }),
            }),
            loc,
        )
    }

    /// Stores the given name/GUID and summary into the index, and resolves any
    /// forward reference to this entry's `^N`.
    ///
    /// Mirrors `LLParser::addGlobalValueToIndex`, with one reordering. Upstream
    /// resolves forward references *before* moving the summary into the index,
    /// because it patches through `ValueInfo *` pointers that stay valid across
    /// the move. llvmkit patches by coordinate, and a coordinate only exists
    /// once the summary is in the index, so the summary is added first and the
    /// pending references are registered against it. The verdicts are the same,
    /// including a summary that references its own `^N`.
    fn add_global_value_to_index(
        &mut self,
        name: &str,
        guid: Option<Guid>,
        linkage: Linkage,
        id: u32,
        summary: Option<GlobalValueSummary>,
        loc: Span,
    ) -> ParseResult<()> {
        // First determine the ValueInfo, from the GUID or from the name.
        let guid = match guid {
            Some(guid) => {
                if let Some(index) = self.summary_index_mut() {
                    index.global_value_entry(guid);
                }
                guid
            }
            None if self.parses_module_entities => {
                // With a module in hand the name must name something in it, and
                // the GUID comes from that value's own linkage.
                let global = self
                    .resolve_global_name_as_ref(name.to_owned())
                    .map_err(|_| {
                        self.message_at(loc, format!("Reference to undefined global \"{name}\""))
                    })?;
                let guid = self.guid_of_global(global);
                if let Some(index) = self.summary_index_mut() {
                    index.global_value_entry_named(guid, name);
                }
                guid
            }
            None => {
                // Index-only mode. Upstream asserts a local linkage has a
                // source file name to distinguish it by; without one,
                // `getGlobalIdentifier` substitutes `<unknown>`, which is what
                // llvmkit does rather than crash.
                let source_filename = self
                    .module
                    .source_filename()
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                let guid =
                    Guid::of_global_identifier(&global_identifier(name, linkage, &source_filename));
                if let Some(index) = self.summary_index_mut() {
                    index.global_value_entry_named(guid, name);
                }
                guid
            }
        };

        // Add the summary, which is what gives this entry's own forward
        // references somewhere to be recorded against.
        let summary_position = match summary {
            Some(summary) => {
                let position = self
                    .summary_index_mut()
                    .map(|index| {
                        let entry = index.global_value_entry(guid);
                        entry.summary_list.push(summary);
                        entry.summary_list.len() - 1
                    })
                    .unwrap_or_default();
                Some(position)
            }
            None => None,
        };

        if let Some(position) = summary_position {
            for (target, field, span) in std::mem::take(&mut self.pending_summary_value_refs) {
                self.forward_ref_summary_values
                    .entry(target)
                    .or_default()
                    .push((
                        SummaryValueRefSite {
                            owner: guid,
                            summary: position,
                            field,
                        },
                        span,
                    ));
            }
            for (target, field, span) in std::mem::take(&mut self.pending_summary_type_ids) {
                self.forward_ref_summary_type_ids
                    .entry(target)
                    .or_default()
                    .push((
                        SummaryTypeIdRefSite {
                            owner: guid,
                            summary: position,
                            field,
                        },
                        span,
                    ));
            }
            if let Some((target, span)) = self.pending_summary_aliasee.take() {
                self.forward_ref_summary_aliasees
                    .entry(target)
                    .or_default()
                    .push((
                        SummaryAliaseeSite {
                            owner: guid,
                            summary: position,
                        },
                        span,
                    ));
            }
        }

        // Resolve forward references from calls, refs and the rest.
        if let Some(sites) = self.forward_ref_summary_values.remove(&id) {
            for (site, _) in sites {
                self.patch_summary_value_ref(&site, guid);
            }
        }

        // Resolve forward references from aliases.
        if let Some(sites) = self.forward_ref_summary_aliasees.remove(&id) {
            for (site, _) in sites {
                if let Some(index) = self.summary_index_mut()
                    && let Some(summary) = index.summary_mut(site.owner, site.summary)
                    && let SummaryKind::Alias(alias) = &mut summary.kind
                {
                    alias.aliasee = Some(guid);
                }
            }
        }

        // Save the ValueInfo for later references by id. Upstream resizes to
        // accept non-continuous numbering, which is what makes test
        // simplification easier.
        let Ok(slot) = usize::try_from(id) else {
            return Err(self.message_at(loc, "expected summary id to fit in a machine word"));
        };
        if slot >= self.numbered_value_infos.len() {
            self.numbered_value_infos.resize(slot + 1, None);
        }
        self.numbered_value_infos[slot] = Some(guid);
        Ok(())
    }

    /// The GUID of a global value already in the module, which upstream reaches
    /// through `GlobalValue::getGUID`: the value's own linkage and the module's
    /// source file name decide the identifier.
    fn guid_of_global(&self, global: GlobalRef<'ctx, B>) -> Guid {
        let (name, linkage) = match global {
            GlobalRef::Function(f) => (f.name().to_owned(), f.linkage()),
            GlobalRef::Variable(g) => (g.name().to_owned(), g.linkage()),
            GlobalRef::Alias(a) => (a.name().to_owned(), a.linkage()),
            GlobalRef::Ifunc(i) => (i.name().to_owned(), i.linkage()),
        };
        let source_filename = self
            .module
            .source_filename()
            .map(|name| name.to_string())
            .unwrap_or_default();
        Guid::of_global_identifier(&global_identifier(&name, linkage, &source_filename))
    }

    /// Point one recorded reference site at `guid`.
    fn patch_summary_value_ref(&mut self, site: &SummaryValueRefSite, guid: Guid) {
        let Some(index) = self.summary_index.as_mut() else {
            return;
        };
        if let SummaryValueRefField::CompatibleVtable {
            type_id,
            index: position,
        } = &site.field
        {
            if let Some(entry) = index.type_id_compatible_vtable(type_id).get_mut(*position) {
                entry.vtable.guid = guid;
            }
            return;
        }
        let Some(summary) = index.summary_mut(site.owner, site.summary) else {
            return;
        };
        match &site.field {
            SummaryValueRefField::Reference(position) => {
                if let Some(reference) = summary.references.get_mut(*position) {
                    reference.guid = guid;
                }
            }
            // Every remaining field belongs to exactly one summary kind, and
            // the site was minted by the routine that parses that kind, so a
            // mismatch here would mean the coordinate was built for a different
            // summary than the one it names.
            SummaryValueRefField::Call(position) => {
                let SummaryKind::Function(function) = &mut summary.kind else {
                    unreachable!("a call edge is only recorded while parsing a function summary")
                };
                if let Some(call) = function.calls.get_mut(*position) {
                    call.callee.guid = guid;
                }
            }
            SummaryValueRefField::Callsite(position) => {
                let SummaryKind::Function(function) = &mut summary.kind else {
                    unreachable!("a callsite is only recorded while parsing a function summary")
                };
                if let Some(callsite) = function.callsites.get_mut(*position)
                    && let Some(callee) = &mut callsite.callee
                {
                    callee.guid = guid;
                }
            }
            SummaryValueRefField::ParameterAccessCall { parameter, call } => {
                let SummaryKind::Function(function) = &mut summary.kind else {
                    unreachable!(
                        "a parameter access is only recorded while parsing a function summary"
                    )
                };
                if let Some(access) = function.parameter_accesses.get_mut(*parameter)
                    && let Some(call) = access.calls.get_mut(*call)
                {
                    call.callee.guid = guid;
                }
            }
            SummaryValueRefField::VtableFunction(position) => {
                let SummaryKind::Variable(variable) = &mut summary.kind else {
                    unreachable!(
                        "a vtable function is only recorded while parsing a variable summary"
                    )
                };
                if let Some(entry) = variable.vtable_functions.get_mut(*position) {
                    entry.function.guid = guid;
                }
            }
            SummaryValueRefField::CompatibleVtable { .. } => {
                unreachable!("handled before the summary lookup")
            }
        }
    }

    /// Flag
    ///   ::= [0|1]
    ///
    /// Mirrors `LLParser::parseFlag`, which takes the token's *boolean* value:
    /// any non-zero unsigned integer reads as set, and only a signed one is
    /// rejected. Signedness is the token's own — a negative decimal and an
    /// `s0x…` literal are both signed, an `u0x…` one is not — so the check is
    /// `APSInt::isSigned`, not "does it look negative".
    fn parse_summary_flag(&mut self) -> ParseResult<bool> {
        let value = match self.peek() {
            Token::IntegerLit(IntLit {
                sign: Sign::Pos,
                base: NumBase::Dec | NumBase::HexUnsigned,
                digits,
            }) => digits.chars().any(|digit| digit != '0'),
            _ => return Err(self.expected("integer")),
        };
        self.bump()?;
        Ok(value)
    }

    /// ModuleReference
    ///   ::= 'module' ':' SummaryID
    ///
    /// Mirrors `LLParser::parseModuleReference`, whose lookup is an assert:
    /// every module entry is parsed before anything that references one.
    fn parse_module_reference(&mut self) -> ParseResult<String> {
        self.expect_keyword(Keyword::Module, "'module' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let Token::SummaryId(module_id) = self.current.value else {
            return Err(self.expected("module ID"));
        };
        self.bump()?;
        Ok(self
            .summary_module_paths
            .get(&module_id)
            .cloned()
            .unwrap_or_default())
    }

    /// GVReference
    ///   ::= ['readonly'|'writeonly'] SummaryID
    ///
    /// Mirrors `LLParser::parseGVReference`, which mints an empty `ValueInfo`
    /// for a `^N` it has not seen and leaves the caller to record the site.
    fn parse_gv_reference(&mut self) -> ParseResult<ParsedGvReference> {
        let read_only = self.eat_keyword(Keyword::Readonly)?;
        let write_only = if read_only {
            false
        } else {
            self.eat_keyword(Keyword::Writeonly)?
        };
        let Token::SummaryId(summary_id) = self.current.value else {
            return Err(self.expected("GV ID"));
        };
        self.bump()?;

        let resolved = usize::try_from(summary_id)
            .ok()
            .and_then(|slot| self.numbered_value_infos.get(slot).copied().flatten());
        let access = if read_only {
            AccessSpecifier::ReadOnly
        } else if write_only {
            AccessSpecifier::WriteOnly
        } else {
            AccessSpecifier::None
        };
        Ok(ParsedGvReference {
            value: ValueReference {
                // A forward reference carries a placeholder that is either
                // patched when its `^N` is defined or reported by
                // `validate_end_of_index`.
                guid: resolved.unwrap_or_default(),
                access,
            },
            summary_id,
            is_forward: resolved.is_none(),
        })
    }

    /// GVFlags
    ///   ::= 'flags' ':' '(' Flag [',' Flag]* ')'
    ///
    /// Mirrors `LLParser::parseGVFlags`.
    fn parse_gv_flags(&mut self) -> ParseResult<GlobalValueFlags> {
        self.expect_keyword(Keyword::Flags, "'flags' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut flags = GlobalValueFlags::default();
        loop {
            let field_loc = self.loc();
            match self.peek() {
                Token::Kw(Keyword::Linkage) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    // `parseOptionalLinkageAux` answers external linkage for a
                    // keyword it does not know and consumes it anyway; the
                    // `HasLinkage` guard upstream pairs it with is an assert,
                    // so a release build accepts the token silently.
                    if let Token::Kw(keyword) = self.peek() {
                        flags.linkage = linkage_keyword(*keyword).unwrap_or(Linkage::External);
                    } else {
                        flags.linkage = Linkage::External;
                    }
                    self.bump()?;
                }
                Token::Kw(Keyword::Visibility) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.visibility = self.parse_optional_visibility()?;
                }
                Token::Kw(Keyword::NotEligibleToImport) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.not_eligible_to_import = self.parse_summary_flag()?;
                }
                Token::Kw(Keyword::Live) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.live = self.parse_summary_flag()?;
                }
                Token::Kw(Keyword::DsoLocal_) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.dso_local = self.parse_summary_flag()?;
                }
                Token::Kw(Keyword::CanAutoHide) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.can_auto_hide = self.parse_summary_flag()?;
                }
                Token::Kw(Keyword::ImportType) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.import_type = self.parse_optional_import_type()?;
                }
                _ => return Err(self.message_at(field_loc, "expected gv flag type")),
            }
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(flags)
    }

    /// Mirrors `LLParser::parseOptionalImportType`.
    fn parse_optional_import_type(&mut self) -> ParseResult<ImportKind> {
        let kind = match self.peek() {
            Token::Kw(Keyword::Definition) => ImportKind::Definition,
            Token::Kw(Keyword::Declaration) => ImportKind::Declaration,
            _ => {
                return Err(self.message("unknown import kind. Expect definition or declaration."));
            }
        };
        self.bump()?;
        Ok(kind)
    }

    /// GVarFlags
    ///   ::= 'varFlags' ':' '(' 'readonly' ':' Flag
    ///                      ',' 'writeonly' ':' Flag
    ///                      ',' 'constant' ':' Flag ')'
    ///
    /// Mirrors `LLParser::parseGVarFlags`. `vcall_visibility` goes through
    /// `parseFlag` upstream too, so only `0` and `1` are expressible.
    fn parse_gvar_flags(&mut self) -> ParseResult<GlobalVariableFlags> {
        self.expect_keyword(Keyword::VarFlags, "'varFlags' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut flags = GlobalVariableFlags::default();
        loop {
            let field_loc = self.loc();
            match self.peek() {
                Token::Kw(Keyword::Readonly) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.maybe_read_only = self.parse_summary_flag()?;
                }
                Token::Kw(Keyword::Writeonly) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.maybe_write_only = self.parse_summary_flag()?;
                }
                Token::Kw(Keyword::Constant) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.constant = self.parse_summary_flag()?;
                }
                Token::Kw(Keyword::VcallVisibility) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    flags.vcall_visibility = if self.parse_summary_flag()? {
                        VCallVisibility::LinkageUnit
                    } else {
                        VCallVisibility::Public
                    };
                }
                _ => return Err(self.message_at(field_loc, "expected gvar flag type")),
            }
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(flags)
    }

    /// OptionalFFlags
    ///   ::= 'funcFlags' ':' '(' ['readNone' ':' Flag]? ... ')'
    ///
    /// Mirrors `LLParser::parseOptionalFFlags`.
    fn parse_optional_function_flags(&mut self) -> ParseResult<FunctionFlags> {
        self.expect_keyword(Keyword::FuncFlags, "'funcFlags' here")?;
        self.expect_punct(PunctKind::Colon, "':' in funcFlags")?;
        self.expect_punct(PunctKind::LParen, "'(' in funcFlags")?;

        let mut flags = FunctionFlags::default();
        loop {
            let field_loc = self.loc();
            let field = match self.peek() {
                Token::Kw(Keyword::ReadNone) => FunctionFlagField::ReadNone,
                Token::Kw(Keyword::ReadOnly) => FunctionFlagField::ReadOnly,
                Token::Kw(Keyword::NoRecurse) => FunctionFlagField::NoRecurse,
                Token::Kw(Keyword::ReturnDoesNotAlias) => FunctionFlagField::ReturnDoesNotAlias,
                Token::Kw(Keyword::NoInline) => FunctionFlagField::NoInline,
                Token::Kw(Keyword::AlwaysInline) => FunctionFlagField::AlwaysInline,
                Token::Kw(Keyword::NoUnwind) => FunctionFlagField::NoUnwind,
                Token::Kw(Keyword::MayThrow) => FunctionFlagField::MayThrow,
                Token::Kw(Keyword::HasUnknownCall) => FunctionFlagField::HasUnknownCall,
                Token::Kw(Keyword::MustBeUnreachable) => FunctionFlagField::MustBeUnreachable,
                _ => return Err(self.message_at(field_loc, "expected function flag type")),
            };
            self.bump()?;
            self.expect_punct(PunctKind::Colon, "':'")?;
            let value = self.parse_summary_flag()?;
            match field {
                FunctionFlagField::ReadNone => flags.read_none = value,
                FunctionFlagField::ReadOnly => flags.read_only = value,
                FunctionFlagField::NoRecurse => flags.no_recurse = value,
                FunctionFlagField::ReturnDoesNotAlias => flags.return_does_not_alias = value,
                FunctionFlagField::NoInline => flags.no_inline = value,
                FunctionFlagField::AlwaysInline => flags.always_inline = value,
                FunctionFlagField::NoUnwind => flags.no_unwind = value,
                FunctionFlagField::MayThrow => flags.may_throw = value,
                FunctionFlagField::HasUnknownCall => flags.has_unknown_call = value,
                FunctionFlagField::MustBeUnreachable => flags.must_be_unreachable = value,
            }
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' in funcFlags")?;
        Ok(flags)
    }

    /// OptionalCalls
    ///   := 'calls' ':' '(' Call [',' Call]* ')'
    ///
    /// Mirrors `LLParser::parseOptionalCalls`.
    fn parse_optional_calls(&mut self) -> ParseResult<Vec<CallEdge>> {
        self.expect_keyword(Keyword::Calls, "'calls' here")?;
        self.expect_punct(PunctKind::Colon, "':' in calls")?;
        self.expect_punct(PunctKind::LParen, "'(' in calls")?;

        let mut calls: Vec<CallEdge> = Vec::new();
        loop {
            self.expect_punct(PunctKind::LParen, "'(' in call")?;
            self.expect_keyword(Keyword::Callee, "'callee' in call")?;
            self.expect_punct(PunctKind::Colon, "':'")?;

            let loc = self.loc();
            let callee = self.parse_gv_reference()?;

            let mut hotness = Hotness::Unknown;
            let mut relative_block_frequency = 0u32;
            let mut has_tail_call = false;
            while self.eat_punct(PunctKind::Comma)? {
                let field_loc = self.loc();
                match self.peek() {
                    Token::Kw(Keyword::Hotness) => {
                        self.bump()?;
                        self.expect_punct(PunctKind::Colon, "':'")?;
                        hotness = self.parse_hotness()?;
                    }
                    Token::Kw(Keyword::Relbf) => {
                        self.bump()?;
                        self.expect_punct(PunctKind::Colon, "':'")?;
                        relative_block_frequency = self.parse_uint32()?;
                    }
                    Token::Kw(Keyword::Tail) => {
                        self.bump()?;
                        self.expect_punct(PunctKind::Colon, "':'")?;
                        has_tail_call = self.parse_summary_flag()?;
                    }
                    _ => return Err(self.message_at(field_loc, "expected hotness, relbf, or tail")),
                }
            }
            if hotness != Hotness::Unknown && relative_block_frequency > 0 {
                return Err(self.message("Expected only one of hotness or relbf"));
            }
            if callee.is_forward {
                self.pending_summary_value_refs.push((
                    callee.summary_id,
                    SummaryValueRefField::Call(calls.len()),
                    loc,
                ));
            }
            calls.push(CallEdge {
                callee: callee.value,
                hotness,
                relative_block_frequency: (relative_block_frequency > 0)
                    .then_some(relative_block_frequency),
                has_tail_call,
            });

            self.expect_punct(PunctKind::RParen, "')' in call")?;
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' in calls")?;
        Ok(calls)
    }

    /// Hotness
    ///   := ('unknown'|'cold'|'none'|'hot'|'critical')
    ///
    /// Mirrors `LLParser::parseHotness`.
    fn parse_hotness(&mut self) -> ParseResult<Hotness> {
        let loc = self.loc();
        let hotness = match self.peek() {
            Token::Kw(Keyword::Unknown) => Hotness::Unknown,
            Token::Kw(Keyword::Cold) => Hotness::Cold,
            Token::Kw(Keyword::None) => Hotness::None,
            Token::Kw(Keyword::Hot) => Hotness::Hot,
            Token::Kw(Keyword::Critical) => Hotness::Critical,
            _ => return Err(self.message_at(loc, "invalid call edge hotness")),
        };
        self.bump()?;
        Ok(hotness)
    }

    /// OptionalVTableFuncs
    ///   := 'vTableFuncs' ':' '(' VTableFunc [',' VTableFunc]* ')'
    ///
    /// Mirrors `LLParser::parseOptionalVTableFuncs`, including the message that
    /// names `'callee'` where the token is `virtFunc`.
    fn parse_optional_vtable_funcs(&mut self) -> ParseResult<Vec<VirtualFunctionOffset>> {
        self.expect_keyword(Keyword::VtableFuncs, "'vTableFuncs' here")?;
        self.expect_punct(PunctKind::Colon, "':' in vTableFuncs")?;
        self.expect_punct(PunctKind::LParen, "'(' in vTableFuncs")?;

        let mut entries: Vec<VirtualFunctionOffset> = Vec::new();
        loop {
            self.expect_punct(PunctKind::LParen, "'(' in vTableFunc")?;
            self.expect_keyword(Keyword::VirtFunc, "'callee' in vTableFunc")?;
            self.expect_punct(PunctKind::Colon, "':'")?;

            let loc = self.loc();
            let function = self.parse_gv_reference()?;

            self.expect_punct(PunctKind::Comma, "comma")?;
            self.expect_keyword(Keyword::Offset, "offset")?;
            self.expect_punct(PunctKind::Colon, "':'")?;
            let vtable_offset = self.parse_uint64()?;

            if function.is_forward {
                self.pending_summary_value_refs.push((
                    function.summary_id,
                    SummaryValueRefField::VtableFunction(entries.len()),
                    loc,
                ));
            }
            entries.push(VirtualFunctionOffset {
                function: function.value,
                vtable_offset,
            });

            self.expect_punct(PunctKind::RParen, "')' in vTableFunc")?;
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' in vTableFuncs")?;
        Ok(entries)
    }

    /// OptionalRefs
    ///   := 'refs' ':' '(' GVReference [',' GVReference]* ')'
    ///
    /// Mirrors `LLParser::parseOptionalRefs`, which sorts the references so
    /// that the read-only and write-only ones sit at the end — the order
    /// `FunctionSummary::specialRefCounts` and the printer both rely on.
    ///
    /// Upstream's sort is the comparator overload of `llvm::sort`, i.e.
    /// `std::sort` preceded by `detail::presortShuffle` under
    /// `EXPENSIVE_CHECKS`, so **ties are left unspecified**: two references of
    /// one access class may come out in either order. `sort_by_key` is Rust's
    /// stable sort, so llvmkit pins source order — which is one of the orders
    /// `llvm::sort` may produce, not a different one. This is a deliberate
    /// refinement of an unspecified contract, not a divergence from a
    /// specified one — and a ported fixture *does* depend on the permutation:
    /// `test/Assembler/thinlto-vtable-summary.ll`'s `RUN` line is a `diff` of
    /// the `^`-lines before and after a `llvm-as | llvm-dis` round-trip, over
    /// a summary carrying `refs: (^3, ^1)` and `refs: (^1, ^5)`, both ties.
    /// Source order is the only permutation that survives that `diff`, so
    /// `sort_unstable_by_key` here would be a regression rather than a
    /// closer port.
    ///
    /// Both ends order the classes themselves identically: upstream's
    /// `ValueInfo::getAccessSpecifier` yields `0 < ReadOnly < WriteOnly` from
    /// `{HaveGV = 1, ReadOnly = 2, WriteOnly = 4}`, and llvmkit's derived `Ord`
    /// on `AccessSpecifier { None, ReadOnly, WriteOnly }` yields the same
    /// sequence.
    fn parse_optional_refs(&mut self) -> ParseResult<Vec<ValueReference>> {
        self.expect_keyword(Keyword::Refs, "'refs' here")?;
        self.expect_punct(PunctKind::Colon, "':' in refs")?;
        self.expect_punct(PunctKind::LParen, "'(' in refs")?;

        let mut parsed = Vec::new();
        loop {
            let loc = self.loc();
            let reference = self.parse_gv_reference()?;
            parsed.push((reference, loc));
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }
        parsed.sort_by_key(|(reference, _)| reference.value.access);

        let mut references = Vec::with_capacity(parsed.len());
        for (reference, loc) in parsed {
            if reference.is_forward {
                self.pending_summary_value_refs.push((
                    reference.summary_id,
                    SummaryValueRefField::Reference(references.len()),
                    loc,
                ));
            }
            references.push(reference.value);
        }

        self.expect_punct(PunctKind::RParen, "')' in refs")?;
        Ok(references)
    }

    /// Mirrors `LLParser::validateEndOfIndex`, reporting the lowest unresolved
    /// `^N` of each kind at its first use.
    fn validate_end_of_index(&mut self) -> ParseResult<()> {
        if let Some((id, sites)) = self.forward_ref_summary_values.iter().next()
            && let Some((_, span)) = sites.first()
        {
            return Err(self.message_at(*span, format!("use of undefined summary '^{id}'")));
        }
        if let Some((id, sites)) = self.forward_ref_summary_aliasees.iter().next()
            && let Some((_, span)) = sites.first()
        {
            return Err(self.message_at(*span, format!("use of undefined summary '^{id}'")));
        }
        if let Some((id, sites)) = self.forward_ref_summary_type_ids.iter().next()
            && let Some((_, span)) = sites.first()
        {
            return Err(self.message_at(*span, format!("use of undefined type id summary '^{id}'")));
        }
        Ok(())
    }

    /// TypeIdEntry
    ///   ::= 'typeid' ':' '(' 'name' ':' STRINGCONSTANT ',' TypeIdSummary ')'
    ///
    /// Mirrors `LLParser::parseTypeIdEntry`.
    fn parse_type_id_entry(&mut self, id: u32) -> ParseResult<()> {
        self.expect_keyword(Keyword::Typeid, "'typeid' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        self.expect_keyword(Keyword::Name, "'name' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let name = self.parse_string_constant("string constant")?;

        self.expect_punct(PunctKind::Comma, "',' here")?;
        let summary = self.parse_type_id_summary()?;
        self.expect_punct(PunctKind::RParen, "')' here")?;

        if let Some(index) = self.summary_index_mut() {
            *index.type_id_summary(&name) = summary;
        }
        self.resolve_forward_ref_type_ids(id, &name);
        Ok(())
    }

    /// TypeIdSummary
    ///   ::= 'summary' ':' '(' TypeTestResolution [',' OptionalWpdResolutions]? ')'
    ///
    /// Mirrors `LLParser::parseTypeIdSummary`.
    fn parse_type_id_summary(&mut self) -> ParseResult<TypeIdSummary> {
        self.expect_keyword(Keyword::Summary, "'summary' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        let type_test_resolution = self.parse_type_test_resolution()?;

        let mut whole_program_devirt_resolutions = BTreeMap::new();
        if self.eat_punct(PunctKind::Comma)? {
            self.parse_optional_wpd_resolutions(&mut whole_program_devirt_resolutions)?;
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(TypeIdSummary {
            type_test_resolution,
            whole_program_devirt_resolutions,
        })
    }

    /// TypeIdCompatibleVtableEntry
    ///   ::= 'typeidCompatibleVTable' ':' '(' 'name' ':' STRINGCONSTANT ','
    ///       TypeIdCompatibleVtableInfo ')'
    ///
    /// Mirrors `LLParser::parseTypeIdCompatibleVtableEntry`.
    fn parse_type_id_compatible_vtable_entry(&mut self, id: u32) -> ParseResult<()> {
        self.expect_keyword(
            Keyword::TypeidCompatibleVtable,
            "'typeidCompatibleVTable' here",
        )?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        self.expect_keyword(Keyword::Name, "'name' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let name = self.parse_string_constant("string constant")?;

        self.expect_punct(PunctKind::Comma, "',' here")?;
        self.expect_keyword(Keyword::Summary, "'summary' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        // The list may already hold entries from an earlier occurrence of this
        // type identifier, and the recorded indices are into the whole of it.
        let base = self
            .summary_index_mut()
            .map(|index| index.type_id_compatible_vtable(&name).len())
            .unwrap_or_default();

        let mut entries = Vec::new();
        let mut pending = Vec::new();
        loop {
            self.expect_punct(PunctKind::LParen, "'(' here")?;
            self.expect_keyword(Keyword::Offset, "'offset' here")?;
            self.expect_punct(PunctKind::Colon, "':' here")?;
            let address_point_offset = self.parse_uint64()?;
            self.expect_punct(PunctKind::Comma, "',' here")?;

            let loc = self.loc();
            let vtable = self.parse_gv_reference()?;
            if vtable.is_forward {
                pending.push((vtable.summary_id, base + entries.len(), loc));
            }
            entries.push(TypeIdOffsetVtableInfo {
                address_point_offset,
                vtable: vtable.value,
            });

            self.expect_punct(PunctKind::RParen, "')' in call")?;
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        if let Some(index) = self.summary_index_mut() {
            index.type_id_compatible_vtable(&name).extend(entries);
        }
        for (summary_id, position, loc) in pending {
            self.forward_ref_summary_values
                .entry(summary_id)
                .or_default()
                .push((
                    SummaryValueRefSite {
                        // The owner is unused for this field: a compatible
                        // vtable hangs off a type identifier, not a value.
                        owner: Guid::default(),
                        summary: 0,
                        field: SummaryValueRefField::CompatibleVtable {
                            type_id: name.clone(),
                            index: position,
                        },
                    },
                    loc,
                ));
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        self.expect_punct(PunctKind::RParen, "')' here")?;

        self.resolve_forward_ref_type_ids(id, &name);
        Ok(())
    }

    /// Retire every recorded reference to this type identifier's `^N`, now
    /// that its name — and so its GUID — is known. Mirrors the `ForwardRefTypeIds`
    /// block both type-identifier entries end with.
    fn resolve_forward_ref_type_ids(&mut self, id: u32, name: &str) {
        let Some(sites) = self.forward_ref_summary_type_ids.remove(&id) else {
            return;
        };
        let guid = Guid::of_global_identifier(name);
        for (site, _) in sites {
            let Some(index) = self.summary_index.as_mut() else {
                return;
            };
            let Some(summary) = index.summary_mut(site.owner, site.summary) else {
                continue;
            };
            let SummaryKind::Function(function) = &mut summary.kind else {
                unreachable!("a type identifier is only referenced from a function summary")
            };
            let Some(info) = &mut function.type_id_info else {
                continue;
            };
            match site.field {
                SummaryTypeIdRefField::Test(position) => {
                    if let Some(slot) = info.type_tests.get_mut(position) {
                        *slot = guid;
                    }
                }
                SummaryTypeIdRefField::AssumeVcall(position) => {
                    if let Some(call) = info.type_test_assume_vcalls.get_mut(position) {
                        call.guid = guid;
                    }
                }
                SummaryTypeIdRefField::CheckedLoadVcall(position) => {
                    if let Some(call) = info.type_checked_load_vcalls.get_mut(position) {
                        call.guid = guid;
                    }
                }
                SummaryTypeIdRefField::AssumeConstVcall(position) => {
                    if let Some(call) = info.type_test_assume_const_vcalls.get_mut(position) {
                        call.virtual_function.guid = guid;
                    }
                }
                SummaryTypeIdRefField::CheckedLoadConstVcall(position) => {
                    if let Some(call) = info.type_checked_load_const_vcalls.get_mut(position) {
                        call.virtual_function.guid = guid;
                    }
                }
            }
        }
    }

    /// TypeTestResolution
    ///   ::= 'typeTestRes' ':' '(' 'kind' ':' ... ')'
    ///
    /// Mirrors `LLParser::parseTypeTestResolution`.
    fn parse_type_test_resolution(&mut self) -> ParseResult<TypeTestResolution> {
        self.expect_keyword(Keyword::TypeTestRes, "'typeTestRes' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        self.expect_keyword(Keyword::Kind, "'kind' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;

        let kind_loc = self.loc();
        let kind = match self.peek() {
            Token::Kw(Keyword::Unknown) => TypeTestResolutionKind::Unknown,
            Token::Kw(Keyword::Unsat) => TypeTestResolutionKind::Unsat,
            Token::Kw(Keyword::ByteArray) => TypeTestResolutionKind::ByteArray,
            Token::Kw(Keyword::Inline) => TypeTestResolutionKind::Inline,
            Token::Kw(Keyword::Single) => TypeTestResolutionKind::Single,
            Token::Kw(Keyword::AllOnes) => TypeTestResolutionKind::AllOnes,
            _ => return Err(self.message_at(kind_loc, "unexpected TypeTestResolution kind")),
        };
        self.bump()?;

        self.expect_punct(PunctKind::Comma, "',' here")?;
        self.expect_keyword(Keyword::SizeM1BitWidth, "'sizeM1BitWidth' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let size_minus_one_bit_width = self.parse_uint32()?;

        let mut resolution = TypeTestResolution {
            kind,
            size_minus_one_bit_width,
            ..TypeTestResolution::default()
        };
        while self.eat_punct(PunctKind::Comma)? {
            let field_loc = self.loc();
            match self.peek() {
                Token::Kw(Keyword::AlignLog2) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    resolution.align_log2 = self.parse_uint64()?;
                }
                Token::Kw(Keyword::SizeM1) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    resolution.size_minus_one = self.parse_uint64()?;
                }
                Token::Kw(Keyword::BitMask) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    // Upstream reads a `uint32`, asserts it fits a byte, and
                    // then narrows with a C cast — so a release build keeps the
                    // low eight bits rather than diagnosing or saturating.
                    let value = self.parse_uint32()?;
                    resolution.bit_mask = u8::try_from(value & 0xFF)
                        .unwrap_or_else(|_| unreachable!("a value masked to 8 bits fits a u8"));
                }
                Token::Kw(Keyword::InlineBits) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':'")?;
                    resolution.inline_bits = self.parse_uint64()?;
                }
                _ => {
                    return Err(
                        self.message_at(field_loc, "expected optional TypeTestResolution field")
                    );
                }
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(resolution)
    }

    /// OptionalWpdResolutions
    ///   ::= 'wpdResolutions' ':' '(' WpdResolution [',' WpdResolution]* ')'
    ///
    /// Mirrors `LLParser::parseOptionalWpdResolutions`.
    fn parse_optional_wpd_resolutions(
        &mut self,
        resolutions: &mut BTreeMap<u64, WholeProgramDevirtResolution>,
    ) -> ParseResult<()> {
        self.expect_keyword(Keyword::WpdResolutions, "'wpdResolutions' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        loop {
            self.expect_punct(PunctKind::LParen, "'(' here")?;
            self.expect_keyword(Keyword::Offset, "'offset' here")?;
            self.expect_punct(PunctKind::Colon, "':' here")?;
            let offset = self.parse_uint64()?;
            self.expect_punct(PunctKind::Comma, "',' here")?;
            let resolution = self.parse_wpd_res()?;
            self.expect_punct(PunctKind::RParen, "')' here")?;
            resolutions.insert(offset, resolution);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(())
    }

    /// WpdRes
    ///   ::= 'wpdRes' ':' '(' 'kind' ':' ... [',' OptionalResByArg]? ')'
    ///
    /// Mirrors `LLParser::parseWpdRes`.
    fn parse_wpd_res(&mut self) -> ParseResult<WholeProgramDevirtResolution> {
        self.expect_keyword(Keyword::WpdRes, "'wpdRes' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        self.expect_keyword(Keyword::Kind, "'kind' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;

        let kind_loc = self.loc();
        let kind = match self.peek() {
            Token::Kw(Keyword::Indir) => WholeProgramDevirtKind::Indir,
            Token::Kw(Keyword::SingleImpl) => WholeProgramDevirtKind::SingleImpl,
            Token::Kw(Keyword::BranchFunnel) => WholeProgramDevirtKind::BranchFunnel,
            _ => {
                return Err(
                    self.message_at(kind_loc, "unexpected WholeProgramDevirtResolution kind")
                );
            }
        };
        self.bump()?;

        let mut resolution = WholeProgramDevirtResolution {
            kind,
            ..WholeProgramDevirtResolution::default()
        };
        while self.eat_punct(PunctKind::Comma)? {
            let field_loc = self.loc();
            match self.peek() {
                Token::Kw(Keyword::SingleImplName) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::Colon, "':' here")?;
                    resolution.single_impl_name = self.parse_string_constant("string constant")?;
                }
                Token::Kw(Keyword::ResByArg) => {
                    self.parse_optional_res_by_arg(&mut resolution.resolutions_by_argument)?;
                }
                _ => {
                    return Err(self.message_at(
                        field_loc,
                        "expected optional WholeProgramDevirtResolution field",
                    ));
                }
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(resolution)
    }

    /// OptionalResByArg
    ///   ::= 'resByArg' ':' '(' ResByArg[, ResByArg]* ')'
    ///
    /// Mirrors `LLParser::parseOptionalResByArg`, including its unbalanced
    /// quote in `expected 'byArg here`.
    fn parse_optional_res_by_arg(
        &mut self,
        resolutions: &mut BTreeMap<Vec<u64>, WholeProgramDevirtByArg>,
    ) -> ParseResult<()> {
        self.expect_keyword(Keyword::ResByArg, "'resByArg' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        loop {
            let args = self.parse_summary_args()?;
            self.expect_punct(PunctKind::Comma, "',' here")?;
            self.expect_keyword(Keyword::ByArg, "'byArg here")?;
            self.expect_punct(PunctKind::Colon, "':' here")?;
            self.expect_punct(PunctKind::LParen, "'(' here")?;
            self.expect_keyword(Keyword::Kind, "'kind' here")?;
            self.expect_punct(PunctKind::Colon, "':' here")?;

            let kind_loc = self.loc();
            let kind = match self.peek() {
                Token::Kw(Keyword::Indir) => WholeProgramDevirtByArgKind::Indir,
                Token::Kw(Keyword::UniformRetVal) => WholeProgramDevirtByArgKind::UniformRetVal,
                Token::Kw(Keyword::UniqueRetVal) => WholeProgramDevirtByArgKind::UniqueRetVal,
                Token::Kw(Keyword::VirtualConstProp) => {
                    WholeProgramDevirtByArgKind::VirtualConstProp
                }
                _ => {
                    return Err(self.message_at(
                        kind_loc,
                        "unexpected WholeProgramDevirtResolution::ByArg kind",
                    ));
                }
            };
            self.bump()?;

            let mut by_arg = WholeProgramDevirtByArg {
                kind,
                ..WholeProgramDevirtByArg::default()
            };
            while self.eat_punct(PunctKind::Comma)? {
                let field_loc = self.loc();
                match self.peek() {
                    Token::Kw(Keyword::Info) => {
                        self.bump()?;
                        self.expect_punct(PunctKind::Colon, "':' here")?;
                        by_arg.info = self.parse_uint64()?;
                    }
                    Token::Kw(Keyword::Byte) => {
                        self.bump()?;
                        self.expect_punct(PunctKind::Colon, "':' here")?;
                        by_arg.byte = self.parse_uint32()?;
                    }
                    Token::Kw(Keyword::Bit) => {
                        self.bump()?;
                        self.expect_punct(PunctKind::Colon, "':' here")?;
                        by_arg.bit = self.parse_uint32()?;
                    }
                    _ => {
                        return Err(self.message_at(
                            field_loc,
                            "expected optional whole program devirt field",
                        ));
                    }
                }
            }

            self.expect_punct(PunctKind::RParen, "')' here")?;
            resolutions.insert(args, by_arg);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(())
    }

    /// Args
    ///   ::= 'args' ':' '(' UInt64[, UInt64]* ')'
    ///
    /// Mirrors `LLParser::parseArgs`.
    fn parse_summary_args(&mut self) -> ParseResult<Vec<u64>> {
        self.expect_keyword(Keyword::Args, "'args' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut args = Vec::new();
        loop {
            args.push(self.parse_uint64()?);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(args)
    }

    /// OptionalTypeIdInfo
    ///   := 'typeIdInfo' ':' '(' [TypeTests]? [',' TypeTestAssumeVCalls]? ... ')'
    ///
    /// Mirrors `LLParser::parseOptionalTypeIdInfo`.
    fn parse_optional_type_id_info(&mut self) -> ParseResult<TypeIdInfo> {
        self.expect_keyword(Keyword::TypeIdInfo, "'typeIdInfo' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' in typeIdInfo")?;

        let mut info = TypeIdInfo::default();
        loop {
            let field_loc = self.loc();
            match self.peek() {
                Token::Kw(Keyword::TypeTests) => {
                    info.type_tests = self.parse_type_tests()?;
                }
                Token::Kw(Keyword::TypeTestAssumeVcalls) => {
                    info.type_test_assume_vcalls = self.parse_vfunc_id_list(
                        Keyword::TypeTestAssumeVcalls,
                        SummaryTypeIdRefField::AssumeVcall,
                    )?;
                }
                Token::Kw(Keyword::TypeCheckedLoadVcalls) => {
                    info.type_checked_load_vcalls = self.parse_vfunc_id_list(
                        Keyword::TypeCheckedLoadVcalls,
                        SummaryTypeIdRefField::CheckedLoadVcall,
                    )?;
                }
                Token::Kw(Keyword::TypeTestAssumeConstVcalls) => {
                    info.type_test_assume_const_vcalls = self.parse_const_vcall_list(
                        Keyword::TypeTestAssumeConstVcalls,
                        SummaryTypeIdRefField::AssumeConstVcall,
                    )?;
                }
                Token::Kw(Keyword::TypeCheckedLoadConstVcalls) => {
                    info.type_checked_load_const_vcalls = self.parse_const_vcall_list(
                        Keyword::TypeCheckedLoadConstVcalls,
                        SummaryTypeIdRefField::CheckedLoadConstVcall,
                    )?;
                }
                _ => return Err(self.message_at(field_loc, "invalid typeIdInfo list type")),
            }
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' in typeIdInfo")?;
        Ok(info)
    }

    /// TypeTests
    ///   ::= 'typeTests' ':' '(' (SummaryID | UInt64) [',' (SummaryID | UInt64)]* ')'
    ///
    /// Mirrors `LLParser::parseTypeTests`.
    fn parse_type_tests(&mut self) -> ParseResult<Vec<Guid>> {
        self.expect_keyword(Keyword::TypeTests, "'typeTests' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' in typeIdInfo")?;

        let mut tests = Vec::new();
        loop {
            if let Token::SummaryId(summary_id) = self.current.value {
                let loc = self.loc();
                self.pending_summary_type_ids.push((
                    summary_id,
                    SummaryTypeIdRefField::Test(tests.len()),
                    loc,
                ));
                self.bump()?;
                tests.push(Guid::default());
            } else {
                tests.push(Guid::from_raw(self.parse_uint64()?));
            }
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' in typeIdInfo")?;
        Ok(tests)
    }

    /// VFuncIdList
    ///   ::= Kind ':' '(' VFuncId [',' VFuncId]* ')'
    ///
    /// Mirrors `LLParser::parseVFuncIdList`.
    fn parse_vfunc_id_list(
        &mut self,
        keyword: Keyword,
        field: fn(usize) -> SummaryTypeIdRefField,
    ) -> ParseResult<Vec<VirtualFunctionId>> {
        self.expect_keyword(keyword, "list keyword here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut ids = Vec::new();
        loop {
            let (id, pending) = self.parse_vfunc_id()?;
            if let Some((summary_id, loc)) = pending {
                self.pending_summary_type_ids
                    .push((summary_id, field(ids.len()), loc));
            }
            ids.push(id);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(ids)
    }

    /// ConstVCallList
    ///   ::= Kind ':' '(' ConstVCall [',' ConstVCall]* ')'
    ///
    /// Mirrors `LLParser::parseConstVCallList`.
    fn parse_const_vcall_list(
        &mut self,
        keyword: Keyword,
        field: fn(usize) -> SummaryTypeIdRefField,
    ) -> ParseResult<Vec<ConstantVirtualCall>> {
        self.expect_keyword(keyword, "list keyword here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut calls = Vec::new();
        loop {
            let (call, pending) = self.parse_const_vcall()?;
            if let Some((summary_id, loc)) = pending {
                self.pending_summary_type_ids
                    .push((summary_id, field(calls.len()), loc));
            }
            calls.push(call);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(calls)
    }

    /// ConstVCall
    ///   ::= '(' VFuncId ',' Args ')'
    ///
    /// Mirrors `LLParser::parseConstVCall`.
    fn parse_const_vcall(&mut self) -> ParseResult<(ConstantVirtualCall, Option<(u32, Span)>)> {
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        let (virtual_function, pending) = self.parse_vfunc_id()?;

        let mut arguments = Vec::new();
        if self.eat_punct(PunctKind::Comma)? {
            arguments = self.parse_summary_args()?;
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok((
            ConstantVirtualCall {
                virtual_function,
                arguments,
            },
            pending,
        ))
    }

    /// VFuncId
    ///   ::= 'vFuncId' ':' '(' (SummaryID | 'guid' ':' UInt64) ','
    ///         'offset' ':' UInt64 ')'
    ///
    /// Mirrors `LLParser::parseVFuncId`.
    fn parse_vfunc_id(&mut self) -> ParseResult<(VirtualFunctionId, Option<(u32, Span)>)> {
        self.expect_keyword(Keyword::VfuncId, "'vFuncId' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut guid = Guid::default();
        let mut pending = None;
        if let Token::SummaryId(summary_id) = self.current.value {
            pending = Some((summary_id, self.loc()));
            self.bump()?;
        } else {
            self.expect_keyword(Keyword::Guid, "'guid' here")?;
            self.expect_punct(PunctKind::Colon, "':' here")?;
            guid = Guid::from_raw(self.parse_uint64()?);
        }

        self.expect_punct(PunctKind::Comma, "',' here")?;
        self.expect_keyword(Keyword::Offset, "'offset' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        let offset = self.parse_uint64()?;
        self.expect_punct(PunctKind::RParen, "')' here")?;

        Ok((VirtualFunctionId { guid, offset }, pending))
    }

    /// ParamNo
    ///   := 'param' ':' UInt64
    ///
    /// Mirrors `LLParser::parseParamNo`.
    fn parse_param_no(&mut self) -> ParseResult<u64> {
        self.expect_keyword(Keyword::Param, "'param' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.parse_uint64()
    }

    /// ParamAccessOffset
    ///   := 'offset' ':' '[' APSINTVAL ',' APSINTVAL ']'
    ///
    /// Mirrors `LLParser::parseParamAccessOffset`, which reads each bound at
    /// the token's own width, sign-extends it to 64 bits, and then makes the
    /// range half-open by incrementing the upper bound.
    fn parse_param_access_offset(&mut self) -> ParseResult<ConstantRange> {
        self.expect_keyword(Keyword::Offset, "'offset' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LSquare, "'[' here")?;
        let lower = self.parse_param_access_bound()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        let upper = self.parse_param_access_bound()?;
        self.expect_punct(PunctKind::RSquare, "']' here")?;

        let one = ApInt::new(
            PARAMETER_ACCESS_RANGE_WIDTH,
            1,
            Signedness::Unsigned,
            llvmkit_ir::ap_int::ApIntTruncation::Truncate,
        )
        .map_err(|err| self.builder_err("parameter access offset", err))?;
        let upper = upper.wrapping_add(&one);
        if lower == upper && !lower.is_max_signed_value() {
            return Ok(ConstantRange::empty(PARAMETER_ACCESS_RANGE_WIDTH));
        }
        ConstantRange::new(lower, upper)
            .map_err(|err| self.builder_err("parameter access offset", err))
    }

    /// One bound of a `ParamAccessOffset`, read the way upstream's inner
    /// `ParseAPSInt` lambda reads it.
    fn parse_param_access_bound(&mut self) -> ParseResult<ApInt> {
        if !matches!(self.peek(), Token::IntegerLit(_)) {
            return Err(self.expected("integer"));
        }
        let literal = self.parse_int_literal()?;
        Ok(literal.extend_or_truncate(PARAMETER_ACCESS_RANGE_WIDTH))
    }

    /// ParamAccessCall
    ///   := '(' 'callee' ':' GVReference ',' ParamNo ',' ParamAccessOffset ')'
    ///
    /// Mirrors `LLParser::parseParamAccessCall`.
    fn parse_param_access_call(
        &mut self,
    ) -> ParseResult<(ParameterAccessCall, Option<(u32, Span)>)> {
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        self.expect_keyword(Keyword::Callee, "'callee' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;

        let loc = self.loc();
        let callee = self.parse_gv_reference()?;

        self.expect_punct(PunctKind::Comma, "',' here")?;
        let parameter_number = self.parse_param_no()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        let offsets = self.parse_param_access_offset()?;
        self.expect_punct(PunctKind::RParen, "')' here")?;

        Ok((
            ParameterAccessCall {
                parameter_number,
                callee: callee.value,
                offsets,
            },
            callee.is_forward.then_some((callee.summary_id, loc)),
        ))
    }

    /// ParamAccess
    ///   := '(' ParamNo ',' ParamAccessOffset [',' OptionalParamAccessCalls]? ')'
    ///
    /// Mirrors `LLParser::parseParamAccess`.
    fn parse_param_access(&mut self) -> ParseResult<ParsedParameterAccess> {
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        let parameter_number = self.parse_param_no()?;
        self.expect_punct(PunctKind::Comma, "',' here")?;
        let use_range = self.parse_param_access_offset()?;

        let mut calls = Vec::new();
        let mut pending = Vec::new();
        if self.eat_punct(PunctKind::Comma)? {
            self.expect_keyword(Keyword::Calls, "'calls' here")?;
            self.expect_punct(PunctKind::Colon, "':' here")?;
            self.expect_punct(PunctKind::LParen, "'(' here")?;
            loop {
                let (call, forward) = self.parse_param_access_call()?;
                if let Some((summary_id, loc)) = forward {
                    pending.push((calls.len(), summary_id, loc));
                }
                calls.push(call);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
            self.expect_punct(PunctKind::RParen, "')' here")?;
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok((
            ParameterAccess {
                parameter_number,
                use_range,
                calls,
            },
            pending,
        ))
    }

    /// OptionalParamAccesses
    ///   := 'params' ':' '(' ParamAccess [',' ParamAccess]* ')'
    ///
    /// Mirrors `LLParser::parseOptionalParamAccesses`.
    fn parse_optional_param_accesses(&mut self) -> ParseResult<Vec<ParameterAccess>> {
        self.expect_keyword(Keyword::Params, "'params' here")?;
        self.expect_punct(PunctKind::Colon, "':' here")?;
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut accesses = Vec::new();
        let mut pending = Vec::new();
        loop {
            let (access, forward) = self.parse_param_access()?;
            for (call, summary_id, loc) in forward {
                pending.push((accesses.len(), call, summary_id, loc));
            }
            accesses.push(access);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;

        for (parameter, call, summary_id, loc) in pending {
            self.pending_summary_value_refs.push((
                summary_id,
                SummaryValueRefField::ParameterAccessCall { parameter, call },
                loc,
            ));
        }
        Ok(accesses)
    }

    /// OptionalAllocs
    ///   := 'allocs' ':' '(' Alloc [',' Alloc]* ')'
    ///
    /// Mirrors `LLParser::parseOptionalAllocs`.
    fn parse_optional_allocs(&mut self) -> ParseResult<Vec<AllocationInfo>> {
        self.expect_keyword(Keyword::Allocs, "'allocs' here")?;
        self.expect_punct(PunctKind::Colon, "':' in allocs")?;
        self.expect_punct(PunctKind::LParen, "'(' in allocs")?;

        let mut allocations = Vec::new();
        loop {
            self.expect_punct(PunctKind::LParen, "'(' in alloc")?;
            self.expect_keyword(Keyword::Versions, "'versions' in alloc")?;
            self.expect_punct(PunctKind::Colon, "':'")?;
            self.expect_punct(PunctKind::LParen, "'(' in versions")?;

            let mut versions = Vec::new();
            loop {
                versions.push(self.parse_alloc_type()?);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }

            self.expect_punct(PunctKind::RParen, "')' in versions")?;
            self.expect_punct(PunctKind::Comma, "',' in alloc")?;
            let memory_info_blocks = self.parse_mem_profs()?;
            allocations.push(AllocationInfo {
                versions,
                memory_info_blocks,
            });
            self.expect_punct(PunctKind::RParen, "')' in alloc")?;
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' in allocs")?;
        Ok(allocations)
    }

    /// MemProfs
    ///   := 'memProf' ':' '(' MemProf [',' MemProf]* ')'
    ///
    /// Mirrors `LLParser::parseMemProfs`.
    fn parse_mem_profs(&mut self) -> ParseResult<Vec<MemoryInfoBlock>> {
        self.expect_keyword(Keyword::MemProf, "'memProf' here")?;
        self.expect_punct(PunctKind::Colon, "':' in memprof")?;
        self.expect_punct(PunctKind::LParen, "'(' in memprof")?;

        let mut blocks = Vec::new();
        loop {
            self.expect_punct(PunctKind::LParen, "'(' in memprof")?;
            self.expect_keyword(Keyword::Type, "'type' in memprof")?;
            self.expect_punct(PunctKind::Colon, "':'")?;
            let allocation_type = self.parse_alloc_type()?;

            self.expect_punct(PunctKind::Comma, "',' in memprof")?;
            self.expect_keyword(Keyword::StackIds, "'stackIds' in memprof")?;
            self.expect_punct(PunctKind::Colon, "':'")?;
            self.expect_punct(PunctKind::LParen, "'(' in stackIds")?;
            let stack_id_indices = self.parse_summary_stack_ids()?;
            self.expect_punct(PunctKind::RParen, "')' in stackIds")?;

            blocks.push(MemoryInfoBlock {
                allocation_type,
                stack_id_indices,
            });
            self.expect_punct(PunctKind::RParen, "')' in memprof")?;
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' in memprof")?;
        Ok(blocks)
    }

    /// The `stackIds` list body, which a combined-index record may leave empty.
    /// Shared by `parseMemProfs` and `parseOptionalCallsites`, which spell it
    /// identically.
    fn parse_summary_stack_ids(&mut self) -> ParseResult<Vec<u32>> {
        let mut indices = Vec::new();
        if matches!(self.peek(), Token::RParen) {
            return Ok(indices);
        }
        loop {
            let stack_id = self.parse_uint64()?;
            if let Some(index) = self.summary_index_mut() {
                indices.push(index.stack_id_index(stack_id));
            }
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }
        Ok(indices)
    }

    /// AllocType
    ///   := ('none'|'notcold'|'cold'|'hot')
    ///
    /// Mirrors `LLParser::parseAllocType`.
    fn parse_alloc_type(&mut self) -> ParseResult<AllocationType> {
        let loc = self.loc();
        let allocation_type = match self.peek() {
            Token::Kw(Keyword::None) => AllocationType::None,
            Token::Kw(Keyword::Notcold) => AllocationType::NotCold,
            Token::Kw(Keyword::Cold) => AllocationType::Cold,
            Token::Kw(Keyword::Hot) => AllocationType::Hot,
            _ => return Err(self.message_at(loc, "invalid alloc type")),
        };
        self.bump()?;
        Ok(allocation_type)
    }

    /// OptionalCallsites
    ///   := 'callsites' ':' '(' Callsite [',' Callsite]* ')'
    ///
    /// Mirrors `LLParser::parseOptionalCallsites`.
    fn parse_optional_callsites(&mut self) -> ParseResult<Vec<CallsiteInfo>> {
        self.expect_keyword(Keyword::Callsites, "'callsites' here")?;
        self.expect_punct(PunctKind::Colon, "':' in callsites")?;
        self.expect_punct(PunctKind::LParen, "'(' in callsites")?;

        let mut callsites: Vec<CallsiteInfo> = Vec::new();
        loop {
            self.expect_punct(PunctKind::LParen, "'(' in callsite")?;
            self.expect_keyword(Keyword::Callee, "'callee' in callsite")?;
            self.expect_punct(PunctKind::Colon, "':'")?;

            let loc = self.loc();
            let mut callee = None;
            let mut forward = None;
            if !self.eat_keyword(Keyword::Null)? {
                let reference = self.parse_gv_reference()?;
                if reference.is_forward {
                    forward = Some(reference.summary_id);
                }
                callee = Some(reference.value);
            }

            self.expect_punct(PunctKind::Comma, "',' in callsite")?;
            self.expect_keyword(Keyword::Clones, "'clones' in callsite")?;
            self.expect_punct(PunctKind::Colon, "':'")?;
            self.expect_punct(PunctKind::LParen, "'(' in clones")?;
            let mut clones = Vec::new();
            loop {
                clones.push(self.parse_uint32()?);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
            self.expect_punct(PunctKind::RParen, "')' in clones")?;

            self.expect_punct(PunctKind::Comma, "',' in callsite")?;
            self.expect_keyword(Keyword::StackIds, "'stackIds' in callsite")?;
            self.expect_punct(PunctKind::Colon, "':'")?;
            self.expect_punct(PunctKind::LParen, "'(' in stackIds")?;
            let stack_id_indices = self.parse_summary_stack_ids()?;
            self.expect_punct(PunctKind::RParen, "')' in stackIds")?;

            if let Some(summary_id) = forward {
                self.pending_summary_value_refs.push((
                    summary_id,
                    SummaryValueRefField::Callsite(callsites.len()),
                    loc,
                ));
            }
            callsites.push(CallsiteInfo {
                callee,
                clones,
                stack_id_indices,
            });

            self.expect_punct(PunctKind::RParen, "')' in callsite")?;
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')' in callsites")?;
        Ok(callsites)
    }

    /// Reference a comdat by name, recording a forward reference when no
    /// `$name = comdat ...` has been seen yet. Mirrors `LLParser::getComdat`,
    /// which likewise creates the `Comdat` eagerly and remembers that its
    /// selection kind is still owed.
    fn comdat_ref(&mut self, name: &str, loc: Span) -> llvmkit_ir::comdat::ComdatRef<'ctx, B> {
        if let Some(comdat) = self.module.comdat(name) {
            return comdat;
        }
        self.forward_ref_comdats
            .entry(name.to_owned())
            .or_insert(loc);
        self.module.get_or_insert_comdat(name)
    }

    /// A `%name` / `%N` that was referenced but never defined.
    ///
    /// Mirrors the two `NumberedTypes` / `NamedTypes` loops in
    /// `LLParser::validateEndOfModule`, including their order — numbered
    /// first — and their different nouns: `use of undefined type '%N'` for a
    /// slot, `use of undefined type named 'x'` for a name.
    ///
    /// llvmkit previously claimed upstream did not diagnose this at all, and
    /// left every unresolved reference as a silently opaque struct.
    fn validate_forward_ref_types(&self) -> ParseResult<()> {
        let mut numbered: Vec<(u32, Span)> = self
            .numbered_types
            .iter()
            .filter_map(|(id, entry)| entry.forward_ref_loc.map(|loc| (*id, loc)))
            .collect();
        numbered.sort_unstable_by_key(|(id, _)| *id);
        if let Some((id, loc)) = numbered.first() {
            return Err(ParseError::UndefinedSymbol {
                kind: SymbolKind::Type,
                id: SymbolId::Numbered(*id),
                loc: DiagLoc::span(*loc),
            });
        }
        let mut named: Vec<(&String, Span)> = self
            .named_types
            .iter()
            .filter_map(|(name, entry)| entry.forward_ref_loc.map(|loc| (name, loc)))
            .collect();
        named.sort_unstable_by_key(|(name, _)| name.as_str());
        if let Some((name, loc)) = named.first() {
            return Err(ParseError::Message {
                message: format!("use of undefined type named '{name}'").into(),
                loc: DiagLoc::span(*loc),
            });
        }
        Ok(())
    }

    /// The first comdat that no definition ever gave a selection kind.
    /// Mirrors the `ForwardRefComdats` guard in
    /// `LLParser::validateEndOfModule`.
    fn validate_forward_ref_comdats(&self) -> ParseResult<()> {
        if let Some((name, loc)) = self.forward_ref_comdats.iter().next() {
            return Err(ParseError::UndefinedSymbol {
                kind: SymbolKind::Comdat,
                id: SymbolId::Named(name.clone()),
                loc: DiagLoc::span(*loc),
            });
        }
        Ok(())
    }

    fn parse_comdat_definition(&mut self) -> ParseResult<()> {
        let name_loc = self.loc();
        let name = match self.peek() {
            Token::ComdatVar(bytes) => std::str::from_utf8(bytes.as_ref())
                .map_err(|_| self.expected("valid UTF-8 comdat name"))?
                .to_owned(),
            _ => return Err(self.expected("comdat variable")),
        };
        self.bump()?;
        self.expect_punct(PunctKind::Equal, "'=' here")?;
        // `if (parseToken(lltok::kw_comdat, "expected comdat keyword"))
        //    return tokError("expected comdat type");`
        //
        // Both messages are raised on the one failure, at the same token —
        // `parseToken` leaves it unconsumed — and both go through
        // `LLLexer::Error` at `ErrorPriority::Parser`, which early-returns only
        // on `Priority < ErrorInfo.Priority`. `Parser < Parser` is false, so
        // the second overwrites the first and `expected comdat keyword` can
        // never reach a user from this site. Discarding the first error here is
        // that overwrite.
        self.expect_keyword(Keyword::Comdat, "comdat keyword")
            .map_err(|_| self.message("expected comdat type"))?;
        let kind = if self.eat_keyword(Keyword::Any)? {
            SelectionKind::Any
        } else if self.eat_keyword(Keyword::Exactmatch)? {
            SelectionKind::ExactMatch
        } else if self.eat_keyword(Keyword::Largest)? {
            SelectionKind::Largest
        } else if self.eat_keyword(Keyword::Nodeduplicate)? {
            SelectionKind::NoDeduplicate
        } else if self.eat_keyword(Keyword::Samesize)? {
            SelectionKind::SameSize
        } else {
            // `parseComdat`'s default arm.
            return Err(self.message("unknown selection kind"));
        };
        // A comdat already in the table is a redefinition *unless* it got
        // there through a forward reference, which this definition satisfies.
        // Mirrors the `!ForwardRefComdats.erase(Name)` guard in
        // `LLParser::parseComdat`.
        let was_forward_referenced = self.forward_ref_comdats.remove(&name).is_some();
        if self.module.comdat(&name).is_some() && !was_forward_referenced {
            return Err(ParseError::Redefinition {
                kind: SymbolKind::Comdat,
                id: SymbolId::Named(name),
                loc: DiagLoc::span(name_loc),
            });
        }
        let comdat = self.module.get_or_insert_comdat(&name);
        comdat.set_selection_kind(self.module, kind);
        Ok(())
    }

    /// `'{' uint32 (',' uint32)+ '}'`. Mirrors
    /// `LLParser::parseUseListOrderIndexes`.
    fn parse_use_list_order_indexes(&mut self) -> ParseResult<Box<[u32]>> {
        let loc = self.loc();
        self.expect_punct(PunctKind::LBrace, "'{' here")?;
        if matches!(self.peek(), Token::RBrace) {
            return Err(self.expected("non-empty list of uselistorder indexes"));
        }
        // Upstream's three consistency accumulators, kept under their own
        // names. `Offset` is `unsigned` arithmetic there and relies on
        // wrapping — it sums `Index - position` over the whole vector and is
        // zero exactly when the indexes sum to `0 + 1 + … + (n-1)`.
        let mut offset: u32 = 0;
        let mut max: u32 = 0;
        let mut is_ordered = true;
        let mut indexes: Vec<u32> = Vec::new();
        loop {
            let index = self.parse_uint32()?;
            let position = u32::try_from(indexes.len()).unwrap_or(u32::MAX);
            offset = offset.wrapping_add(index.wrapping_sub(position));
            max = max.max(index);
            is_ordered &= index == position;
            indexes.push(index);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }
        self.expect_punct(PunctKind::RBrace, "'}' here")?;

        let count = u32::try_from(indexes.len()).unwrap_or(u32::MAX);
        if indexes.len() < 2 {
            return Err(self.expected_at(loc, ">= 2 uselistorder indexes"));
        }
        if offset != 0 || max >= count {
            return Err(self.expected_at(loc, "distinct uselistorder indexes in range [0, size)"));
        }
        if is_ordered {
            return Err(self.expected_at(loc, "uselistorder indexes to change the order"));
        }
        Ok(indexes.into_boxed_slice())
    }

    /// `'uselistorder' Type Value ',' UseListOrderIndexes`. Mirrors
    /// `LLParser::parseUseListOrder`; `pfs` is its `PerFunctionState *`,
    /// null at module level.
    fn parse_use_list_order(&mut self, pfs: Option<&PerFunctionState<'ctx, B>>) -> ParseResult<()> {
        let loc = self.loc();
        self.expect_keyword(Keyword::Uselistorder, "uselistorder directive")?;
        let ty = self.parse_type(false)?;
        let val_id = self.parse_val_id(pfs, Some(ty))?;
        let value = self.convert_val_id_to_value(ty, val_id, pfs)?;
        self.expect_punct(PunctKind::Comma, "comma in uselistorder directive")?;
        let indexes = self.parse_use_list_order_indexes()?;
        self.sort_use_list_order(value, &indexes, loc)
    }

    /// `LLParser::sortUseListOrder`'s tail: apply the permutation and render
    /// its three `error(Loc, …)` texts, all of which anchor at the directive's
    /// first token rather than the current one.
    fn sort_use_list_order(
        &self,
        value: llvmkit_ir::Value<'ctx, B>,
        indexes: &[u32],
        loc: Span,
    ) -> ParseResult<()> {
        value
            .sort_use_list(indexes)
            .map_err(|e| self.message_at(loc, e.to_string()))
    }

    /// `'uselistorder_bb' @foo ',' %bar ',' UseListOrderIndexes`. Mirrors
    /// `LLParser::parseUseListOrderBB`, including its use of the *untyped*
    /// `parseValID` for both operands: neither is resolved through
    /// `convertValIDToValue`, so the diagnostics below are the only route.
    fn parse_use_list_order_bb(&mut self) -> ParseResult<()> {
        let loc = self.loc();
        self.expect_keyword(Keyword::UselistorderBb, "'uselistorder_bb'")?;

        let function_loc = self.loc();
        let function_id = self.parse_val_id(None, None)?;
        self.expect_punct(PunctKind::Comma, "comma in uselistorder_bb directive")?;
        let label_loc = self.loc();
        let label_id = self.parse_val_id(None, None)?;
        self.expect_punct(PunctKind::Comma, "comma in uselistorder_bb directive")?;
        let indexes = self.parse_use_list_order_indexes()?;

        // Check the function. `M->getNamedValue` / `NumberedVals.get` answer
        // with any global value, so "not a function" and "not defined yet" are
        // separate verdicts with separate texts.
        let global = match &function_id.kind {
            ValIdKind::GlobalName(name) => self.named_global_value(name),
            ValIdKind::GlobalId(id) => self.numbered_globals.get(*id).copied(),
            _ => {
                return Err(self.expected_at(function_loc, "function name in uselistorder_bb"));
            }
        };
        let Some(global) = global else {
            return Err(self.message_at(
                function_loc,
                "invalid function forward reference in uselistorder_bb",
            ));
        };
        let GlobalRef::Function(function) = global else {
            return Err(self.expected_at(function_loc, "function name in uselistorder_bb"));
        };
        // `F->isDeclaration()` — a function with no body.
        if function.basic_blocks().count() == 0 {
            return Err(self.message_at(function_loc, "invalid declaration in uselistorder_bb"));
        }

        // Check the basic block. Upstream looks the label up in the function's
        // *value* symbol table, which holds arguments and named instructions
        // too — hence the "found, but not a block" arm.
        let name = match &label_id.kind {
            ValIdKind::LocalId(_) => {
                return Err(self.message_at(label_loc, "invalid numeric label in uselistorder_bb"));
            }
            ValIdKind::LocalName(name) => name,
            _ => {
                return Err(self.expected_at(label_loc, "basic block name in uselistorder_bb"));
            }
        };
        let Some(local) = function_local_by_name(function, name) else {
            return Err(self.message_at(label_loc, "invalid basic block in uselistorder_bb"));
        };
        if local.category() != ValueCategory::BasicBlock {
            return Err(self.expected_at(label_loc, "basic block in uselistorder_bb"));
        }

        self.sort_use_list_order(local, &indexes, loc)
    }

    /// `Module::getNamedValue` — the one symbol table every global value kind
    /// shares. llvmkit keeps four maps, so the lookup is spelled as four.
    fn named_global_value(&self, name: &str) -> Option<GlobalRef<'ctx, B>> {
        if let Some(id) = self.module.function_dyn(name) {
            Some(GlobalRef::Function(self.module.view(id)))
        } else if let Some(id) = self.module.global(name) {
            Some(GlobalRef::Variable(self.module.view(id)))
        } else if let Some(id) = self.module.alias(name) {
            Some(GlobalRef::Alias(self.module.view(id)))
        } else {
            self.module
                .ifunc(name)
                .map(|id| GlobalRef::Ifunc(self.module.view(id)))
        }
    }

    /// `module asm STRING`. Mirrors `LLParser::parseModuleAsm`.
    fn parse_module_asm(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Module, "'module'")?;
        self.expect_keyword(Keyword::Asm, "'asm' after 'module'")?;
        let asm = self.parse_string_constant("module-asm string constant")?;
        self.module.append_module_asm(asm);
        Ok(())
    }

    // ── Metadata definitions ──────────────────────────────────────────────

    /// `!N = <md-node>`. Mirrors `LLParser::parseStandaloneMetadata`.
    ///
    /// Syntax:
    ///   `!0 = !{...}`
    ///   `!0 = distinct !{...}`
    fn parse_standalone_metadata(&mut self) -> ParseResult<()> {
        let loc = self.bump()?; // consume Token::Exclaim
        let slot = self.parse_uint32()?;
        self.expect_punct(PunctKind::Equal, "'=' here")?;
        // "Detect common error, from old metadata syntax" — `!0 = metadata
        // !{...}` used to be legal, so a type token here gets its own message
        // rather than the generic one.
        if matches!(self.peek(), Token::PrimitiveType(_)) {
            return Err(self.message("unexpected type in metadata definition"));
        }
        let distinct = self.eat_keyword(Keyword::Distinct)?;
        match self.peek() {
            Token::Exclaim => {
                self.bump()?;
                match self.peek() {
                    Token::LBrace => {
                        let content = self.parse_md_node_after_bang(distinct)?;
                        self.define_md_slot(slot, content, loc)?;
                        Ok(())
                    }
                    // `parseMDTuple` -> `parseMDNodeVector`'s opening token.
                    // Lowercase here, unlike `parseNamedMetadata`'s own
                    // `Expected '{' here` a few lines away in the same file —
                    // this path is the one `invalid-mdnode-vector.ll` pins.
                    _ => Err(self.message("expected '{' here")),
                }
            }
            Token::MetadataVar(_) => {
                let content = self.parse_md_node_after_bang(distinct)?;
                self.define_md_slot(slot, content, loc)?;
                Ok(())
            }
            _ => Err(self.expected("metadata string or tuple")),
        }
    }

    /// `!name = !{ !N, !N, ... }`. Mirrors `LLParser::parseNamedMetadata`.
    fn parse_named_metadata(&mut self) -> ParseResult<()> {
        let name = match self.peek() {
            Token::MetadataVar(bytes) => std::str::from_utf8(bytes.as_ref())
                .map_err(|_| self.expected("valid UTF-8 metadata name"))?
                .to_owned(),
            _ => return Err(self.expected("metadata name")),
        };
        self.bump()?;

        self.expect_punct(PunctKind::Equal, "'=' here")?;

        // `!{ !N, !N, ... }`. Upstream's own three-`parseToken` chain, and the
        // last two are verbatim messages carrying its capital `E` — a few
        // lines from `parseMDNodeVector`'s *lowercase* `expected '{' here`,
        // so the two spellings cannot be unified. llvmkit had invented all
        // three (`'=' after metadata name`, `'!' before '{' in named
        // metadata`, `'{' in named metadata`).
        if !matches!(self.peek(), Token::Exclaim) {
            return Err(self.message("Expected '!' here"));
        }
        self.bump()?;
        if !matches!(self.peek(), Token::LBrace) {
            return Err(self.message("Expected '{' here"));
        }
        self.bump()?;
        let named_metadata_id = self.module.get_or_insert_named_metadata(name);

        // Parse comma-separated operands. Almost all are `!N` slot
        // references, but `parseNamedMetadata` special-cases two spellings
        // that arrive as a single `MetadataVar` token.
        if !matches!(self.peek(), Token::RBrace) {
            loop {
                let specialized = match self.peek() {
                    Token::MetadataVar(bytes) => {
                        std::str::from_utf8(bytes.as_ref()).ok().map(str::to_owned)
                    }
                    _ => None,
                };
                let id = match specialized.as_deref() {
                    // "parse DIExpressions inline as a special case. They are
                    // still MDNodes, so they can still appear in named
                    // metadata."
                    Some("DIExpression") => {
                        let content = self.parse_md_node_after_bang(false)?;
                        own_metadata(self.module.metadata_node(content))
                    }
                    // "DIArgLists should only appear inline in a function, as
                    // they may contain LocalAsMetadata arguments which require
                    // a function context."
                    Some("DIArgList") => {
                        return Err(self.message("found DIArgList outside of function"));
                    }
                    _ => {
                        self.expect_exclaim("'!' before metadata operand")?;
                        let loc = self.loc();
                        let slot = self.parse_uint32()?;
                        self.resolve_md_slot(slot, loc)
                    }
                };
                // `named_metadata_id` came from `get_or_insert_named_metadata`
                // on this same module, so neither handle can be foreign.
                self.module
                    .named_metadata_add_operand(named_metadata_id, id)
                    .expect("named metadata id and operand minted by this module");
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }

        // `parseNamedMetadata` and `parseMDNodeVector` close with the same
        // label, a few lines from the capital-`E` pair above.
        self.expect_punct(PunctKind::RBrace, "end of metadata node")?;
        Ok(())
    }

    /// Whether the lookahead can begin a type.
    ///
    /// Exactly the case labels of `LLParser::parseType`'s leading
    /// `switch (Lex.getKind())` — `lltok::Type`, `kw_target`, `lbrace`,
    /// `lsquare`, `less`, `LocalVar`, `LocalVarID` — so the negation is that
    /// switch's `default:` arm, the one place `parseType` reports its `Msg`
    /// parameter. Callers that hold an upstream `TypeMsg` test this before
    /// calling [`Self::parse_type`], which is what keeps `Msg` from swallowing
    /// the messages `parseType`'s later arms and its nested routines raise.
    fn peek_begins_a_type(&self) -> bool {
        matches!(
            self.peek(),
            Token::PrimitiveType(_)
                | Token::Kw(Keyword::Target)
                | Token::LBrace
                | Token::LSquare
                | Token::Less
                | Token::LocalVar(_)
                | Token::LocalVarId(_)
        )
    }

    /// `i32 %local` / `i32 @global` / `i32 7` — a type-and-value pair wrapped
    /// as metadata. Upstream's own grammar comment, verbatim:
    ///
    /// ```text
    /// /// parseValueAsMetadata
    /// ///  ::= i32 %local
    /// ///  ::= i32 @global
    /// ///  ::= i32 7
    /// ```
    ///
    /// Mirrors `LLParser::parseValueAsMetadata` statement for statement:
    /// record the type's location, parse the type under `TypeMsg`, reject a
    /// `metadata` type *before* any value is read, parse the value, then
    /// `ValueAsMetadata::get`. Every caller upstream gives it arrives here, so
    /// the roundtrip guard exists once, as upstream has it once.
    ///
    /// `pfs` is upstream's nullable `PerFunctionState *`. `None` stands in for
    /// the no-function-state path and is rendered as `parse_global_value`,
    /// which is **not** equivalent: upstream's `convertValIDToValue` reaches
    /// `t_LocalName` with a null `PFS` and answers `invalid use of
    /// function-local name` at the local token, where llvmkit answers
    /// `expected constant value` at the token after it. That difference is
    /// gap **G17** in `docs/fixture-coverage.md` and is older than this
    /// routine.
    ///
    /// `type_msg` is upstream's `const Twine &TypeMsg`, and it reaches the
    /// output where upstream's does: `parseType`'s `Msg` is read in exactly one
    /// place, its leading `switch (Lex.getKind())`'s `default:` arm, so
    /// [`Self::peek_begins_a_type`] renders that arm and every other failure
    /// keeps the message of the nested routine that produced it.
    fn parse_value_as_metadata(
        &mut self,
        type_msg: &'static str,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<MetadataId<B>> {
        // `LocTy Loc;` then `if (parseType(Ty, TypeMsg, Loc)) return true;` —
        // upstream records `Loc` before the parse, so the guard below anchors
        // at the type token rather than wherever the type parse ended.
        let type_loc = self.loc();
        // `parseType`'s `default: return tokError(Msg);`. It switches on the
        // first token, so the lookahead decides it, and `TypeMsg` fires here
        // and nowhere else: `void`, `ptr*`, `label*` and a malformed struct or
        // array body all fail *after* this point and carry their own text,
        // anchored where `parseType` anchors it.
        if !self.peek_begins_a_type() {
            return Err(self.expected(type_msg));
        }
        let ty = self.parse_type(false)?;
        // `if (Ty->isMetadataTy())
        //    return error(Loc, "invalid metadata-value-metadata roundtrip");`
        if ty.is_metadata() {
            return Err(self.message_at(type_loc, "invalid metadata-value-metadata roundtrip"));
        }
        // `Value *V; if (parseValue(Ty, V, PFS)) return true;`
        let value_id = match pfs {
            Some(state) => self.parse_value(state, ty)?.id(),
            None => self.parse_global_value(ty)?.as_erased().id(),
        };
        // `MD = ValueAsMetadata::get(V);`
        Ok(own_metadata(
            self.module.metadata_node(MetadataKind::Constant(value_id)),
        ))
    }

    /// `metadata i32 %local` / `metadata !0` / `metadata !"string"` — the
    /// `metadata`-typed operand form, with the `metadata` type already
    /// consumed by the caller.
    ///
    /// Mirrors `LLParser::parseMetadataAsValue`, a two-statement wrapper:
    /// `parseMetadata(MD, &PFS)` then `MetadataAsValue::get`. Its
    /// `PerFunctionState &` is *non-optional* — every caller
    /// (`parseParameterList`, `parseExceptionArgs`,
    /// `parseOptionalOperandBundles`) is inside a function body — and it
    /// forwards it to `parseMetadata`'s nullable `PerFunctionState *`.
    fn parse_metadata_as_value(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.parse_metadata_value_operand(Some(state))
    }

    /// Parse a `metadata`-typed value operand where the function state may be
    /// absent. Mirrors `LLParser::parseMetadata` followed by
    /// `MetadataAsValue::get` — the pair `parseMetadataAsValue` performs, but
    /// with `parseMetadata`'s nullable `PerFunctionState *`, which is what
    /// `parseValID`'s metadata arms need at module scope. Callers that do hold
    /// a function state go through [`Self::parse_metadata_as_value`],
    /// upstream's own entry point, and the non-`!` fall-through delegates to
    /// [`Self::parse_value_as_metadata`], as `parseMetadata` does. Slot refs
    /// (`!N`), inline tuples (`!{...}`) and MDStrings (`!"..."`) are all legal
    /// metadata values.
    ///
    /// The `MetadataVar` branch keeps `parseMetadata`'s own `DIArgList`
    /// special case ahead of the specialized-node dispatch, which is the only
    /// reason the state is threaded here at all.
    fn parse_metadata_value_operand(
        &mut self,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let id = self.parse_metadata(pfs)?;
        // `parseMetadataAsValue`'s second statement,
        // `V = MetadataAsValue::get(Context, MD);`.
        Ok(own_metadata(self.module.metadata_as_value(id)))
    }

    /// Mirrors `LLParser::parseMetadata(Metadata *&MD, PerFunctionState *PFS)`
    /// on its own — the routine `parseMetadataAsValue` wraps and
    /// `parseDebugRecord` calls **unwrapped**, for a `Metadata *` rather than
    /// a `MetadataAsValue`.
    fn parse_metadata(
        &mut self,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<MetadataId<B>> {
        if matches!(self.peek(), Token::MetadataVar(_)) {
            // `// DIArgLists are a special case, as they are a list of
            // ValueAsMetadata and so parsing this requires a Function State.`
            // `if (Lex.getStrVal() == "DIArgList") { … parseDIArgList(AL, PFS)
            // … }` — dispatched *before* `parseSpecializedMDNode`, which is why
            // `parseMetadataAsValue` forwards a `PerFunctionState &` at all.
            if self.peek_is_di_arg_list() {
                // `parseDIArgList` opens `assert(PFS && "Expected valid
                // function state")`, so upstream aborts on a module-scope
                // `DIArgList` rather than diagnosing one. llvmkit raises no
                // runtime panics, so it reports instead — the message
                // `parseNamedMetadata` uses for the same shape.
                let Some(pfs) = pfs else {
                    return Err(self.message("found DIArgList outside of function"));
                };
                return self.parse_di_arg_list(pfs);
            }
            let kind = self.parse_md_node_after_bang(false)?;
            return Ok(own_metadata(self.module.metadata_node(kind)));
        }

        // `parseMetadata`'s fallthrough — `if (Lex.getKind() != lltok::exclaim)
        // return parseValueAsMetadata(MD, "expected metadata operand", PFS);`.
        // Anything that is not a `!` at all is a `ValueAsMetadata`: a *type and
        // value* pair, which is how every old-format debug intrinsic spells its
        // operands (`llvm.dbg.value(metadata i32 %a, …)`). llvmkit demanded the
        // sigil here and so could not parse them at all.
        if !matches!(self.peek(), Token::Exclaim) {
            return self.parse_value_as_metadata("metadata operand", pfs);
        }

        self.expect_exclaim("'!' in metadata operand")?;
        let id = match self.peek() {
            Token::StringConstant(_) => {
                let s = self.parse_string_constant("metadata string")?;
                self.module.metadata_string(s)
            }
            Token::LBrace => {
                self.bump()?;
                let mut operands = Vec::new();
                if !matches!(self.peek(), Token::RBrace) {
                    loop {
                        operands.push(self.parse_md_tuple_operand()?);
                        if !self.eat_punct(PunctKind::Comma)? {
                            break;
                        }
                    }
                }
                self.expect_punct(PunctKind::RBrace, "end of metadata node")?;
                own_metadata(self.module.metadata_tuple(operands))
            }
            Token::MetadataVar(_) => {
                let kind = self.parse_md_node_after_bang(false)?;
                own_metadata(self.module.metadata_node(kind))
            }
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32()?;
                self.resolve_md_slot(slot, loc)
            }
        };
        Ok(id)
    }

    fn parse_metadata_attachment_operand(&mut self) -> ParseResult<MetadataId<B>> {
        match self.peek() {
            Token::MetadataVar(_) => {
                let kind = self.parse_md_node_after_bang(false)?;
                Ok(own_metadata(self.module.metadata_node(kind)))
            }
            Token::Exclaim => {
                self.bump()?;
                match self.peek() {
                    Token::LBrace | Token::MetadataVar(_) => {
                        let kind = self.parse_md_node_after_bang(false)?;
                        Ok(own_metadata(self.module.metadata_node(kind)))
                    }
                    _ => {
                        let loc = self.loc();
                        let slot = self.parse_uint32()?;
                        Ok(self.resolve_md_slot(slot, loc))
                    }
                }
            }
            // `parseMDNode` falls through to
            // `parseToken(lltok::exclaim, "expected '!' here")`, so anything
            // that is neither a specialized node nor a `!` is reported as the
            // missing sigil rather than as a missing operand.
            _ => Err(self.expected("'!' here")),
        }
    }

    fn parse_named_metadata_attachment(
        &mut self,
    ) -> ParseResult<(MetadataAttachmentKind, MetadataId<B>)> {
        let name = match self.peek() {
            Token::MetadataVar(bytes) => std::str::from_utf8(bytes.as_ref())
                .map_err(|_| self.expected("valid UTF-8 metadata attachment name"))?
                .to_owned(),
            _ => return Err(self.expected("metadata attachment")),
        };
        self.bump()?;
        let id = self.parse_metadata_attachment_operand()?;
        Ok((MetadataAttachmentKind::from_name(&name), id))
    }

    /// Parse a single metadata tuple operand: an inline `!"string"`
    /// (interned and referenced) or a numbered `!N` reference. The
    /// inline-string form is what the AsmWriter emits for `MDString`
    /// tuple operands (`!{!"rsp"}`), so this keeps writer output
    /// round-trippable.
    fn parse_md_tuple_operand(&mut self) -> ParseResult<MetadataId<B>> {
        if matches!(self.peek(), Token::Kw(Keyword::Null)) {
            self.bump()?;
            return Ok(own_metadata(self.module.metadata_node(MetadataKind::Null)));
        }

        if matches!(self.peek(), Token::MetadataVar(_)) {
            let content = self.parse_md_node_after_bang(false)?;
            return Ok(own_metadata(self.module.metadata_node(content)));
        }

        if self.peek_begins_a_type() {
            // `parseMDNodeVector` reaches `parseValueAsMetadata` the same way
            // the operand form does — through `parseMetadata(MD, nullptr)`, so
            // the function state is absent and the `TypeMsg` is the same one.
            // Its roundtrip guard (`!{metadata !0}` is the old syntax that hits
            // it) lives there, once, rather than in a second copy here.
            return self.parse_value_as_metadata("metadata operand", None);
        }

        // `parseMetadata`'s fallthrough: anything that is not `!` goes to
        // `parseValueAsMetadata(MD, "expected metadata operand", PFS)`, so a
        // token that begins neither a type nor a `!` reports *that*, not a
        // complaint about the missing bang.
        self.expect_exclaim("metadata operand")?;
        match self.peek() {
            Token::StringConstant(_) => {
                let s = self.parse_string_constant("metadata string operand")?;
                Ok(self.module.metadata_string(s))
            }
            Token::LBrace | Token::MetadataVar(_) => {
                let content = self.parse_md_node_after_bang(false)?;
                Ok(own_metadata(self.module.metadata_node(content)))
            }
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32()?;
                Ok(self.resolve_md_slot(slot, loc))
            }
        }
    }

    fn parse_md_node_after_bang(&mut self, distinct: bool) -> ParseResult<MetadataKind<B>> {
        match self.peek() {
            Token::LBrace => {
                self.bump()?;
                let mut operands = Vec::new();
                if !matches!(self.peek(), Token::RBrace) {
                    loop {
                        operands.push(self.parse_md_tuple_operand()?);
                        if !self.eat_punct(PunctKind::Comma)? {
                            break;
                        }
                    }
                }
                self.expect_punct(PunctKind::RBrace, "end of metadata node")?;
                Ok(llvmkit_ir::metadata::MetadataKind::Tuple { distinct, operands })
            }
            Token::MetadataVar(bytes) => {
                let name = std::str::from_utf8(bytes.as_ref())
                    .map_err(|_| self.expected("metadata type"))?;
                let kind = llvmkit_ir::metadata::SpecializedMetadataKind::from_name(name)
                    .ok_or_else(|| self.expected("metadata type"))?;
                self.parse_specialized_metadata_body(kind, distinct)
            }
            Token::StringConstant(_) => {
                let s = self.parse_string_constant("metadata string")?;
                Ok(llvmkit_ir::metadata::MetadataKind::String(s))
            }
            _ => Err(self.expected("metadata node")),
        }
    }

    /// Parse `(field: value, ...)` for a specialized `DI*` node whose class is
    /// already resolved, with the node's leading token still current.
    ///
    /// Mirrors `LLParser::parseMDFieldsImpl` and the `PARSE_MD_FIELDS` macro
    /// (`LLParser.cpp`), including all three of its rejections: a field the
    /// class does not declare (`invalid field '...'`), a field given twice
    /// (`field '...' cannot be specified more than once`, from
    /// `LLParser::parseMDField`'s `Result.Seen` guard), and a `REQUIRED` field
    /// left out (`missing required field '...'`, reported against the closing
    /// `)` exactly as `REQUIRE_FIELD` does).
    fn parse_specialized_metadata_body(
        &mut self,
        kind: llvmkit_ir::metadata::SpecializedMetadataKind,
        distinct: bool,
    ) -> ParseResult<llvmkit_ir::metadata::MetadataKind<B>> {
        // `LLParser::parseDIAssignID` rejects a non-`distinct` node outright:
        // the class exists only to give an assignment a unique identity, so a
        // uniqued one would be meaningless.
        if kind == llvmkit_ir::metadata::SpecializedMetadataKind::DiAssignId && !distinct {
            // `parseDIAssignID` says "missing", not "expected" — routing it
            // through `Parser::expected` rendered the wrong first word.
            return Err(self.message("missing 'distinct', required for !DIAssignID()"));
        }
        // `parseDICompileUnit` opens with the same guard, before it reads a
        // single field.
        if kind == llvmkit_ir::metadata::SpecializedMetadataKind::DiCompileUnit && !distinct {
            return Err(self.message("missing 'distinct', required for !DICompileUnit"));
        }
        // The node-wide diagnostics below are reported at the `!DIFoo` token,
        // which is where upstream's `LocTy Loc = Lex.getLoc()` is taken —
        // `parseMDFieldsImpl` only eats it afterwards.
        let node_loc = self.loc();
        self.bump()?;
        if kind == llvmkit_ir::metadata::SpecializedMetadataKind::DiExpression {
            return self.parse_di_expression_body(distinct);
        }
        // `parseMDFieldsImpl` and `parseMDFieldsImplBody` label the frame; the
        // per-class `VISIT_MD_FIELDS` macro never gets a say in it.
        self.expect_punct(PunctKind::LParen, "'(' here")?;
        let mut fields: Vec<llvmkit_ir::metadata::MetadataField<B>> = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                let field_loc = DiagLoc::span(self.loc());
                let field_name = match self.peek() {
                    Token::LabelStr(bytes) => std::str::from_utf8(bytes.as_ref())
                        .map_err(|_| self.expected("valid UTF-8 metadata field name"))?
                        .to_owned(),
                    _ => return Err(self.expected("field label here")),
                };
                if !kind.accepts_field(&field_name) {
                    return Err(ParseError::InvalidMetadataField {
                        kind: kind.name(),
                        field: field_name,
                        loc: field_loc,
                    });
                }
                if fields.iter().any(|f| f.name() == field_name) {
                    return Err(ParseError::DuplicateMetadataField {
                        kind: kind.name(),
                        field: field_name,
                        loc: field_loc,
                    });
                }
                let declared = kind
                    .field(&field_name)
                    .unwrap_or_else(|| unreachable!("accepts_field just matched {field_name}"));
                self.bump()?;
                let value_loc = self.loc();
                let value = self.parse_metadata_field_value(declared.kind())?;
                self.check_metadata_field_value(declared, &value, value_loc)?;
                fields.push(llvmkit_ir::metadata::MetadataField::new(field_name, value));
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        let closing_loc = DiagLoc::span(self.loc());
        self.expect_punct(PunctKind::RParen, "')' here")?;
        for required in kind.required_fields() {
            if !fields.iter().any(|f| f.name() == required.name()) {
                return Err(ParseError::MissingRequiredMetadataField {
                    kind: kind.name(),
                    field: required.name(),
                    loc: closing_loc,
                });
            }
        }
        self.check_specialized_metadata_agreement(kind, distinct, &fields, node_loc)?;
        Ok(llvmkit_ir::metadata::MetadataKind::Specialized({
            let mut node = llvmkit_ir::metadata::SpecializedMetadataNode::new(kind);
            if distinct {
                node = node.distinct();
            }
            node.with_fields(fields)
        }))
    }

    /// Parse a `DIExpression` body — `( elem, elem, ... )` — with the `(` still
    /// current.
    ///
    /// Mirrors `LLParser::parseDIExpressionBody` (`LLParser.cpp`), which is the
    /// reason `DIExpression` is the one specialized node that never reaches
    /// `PARSE_MD_FIELDS`: its elements are positional, not `name: value` pairs.
    ///
    /// Upstream maps each `DW_OP_*` / `DW_ATE_*` through
    /// `dwarf::getOperationEncoding` / `getAttributeEncoding` and stores a
    /// `uint64_t`. llvmkit stores the written spelling — the `Dwarf.def` tables
    /// are unmodelled (`docs/future-work.md`) and `AsmWriter.cpp`'s
    /// `writeDIExpression` prints a known op back by name regardless — so an
    /// operation llvmkit does not recognise round-trips rather than being
    /// rejected. That is the one deliberate divergence here.
    fn parse_di_expression_body(
        &mut self,
        distinct: bool,
    ) -> ParseResult<llvmkit_ir::metadata::MetadataKind<B>> {
        use llvmkit_ir::metadata::DwarfExpressionOperand;

        self.expect_punct(PunctKind::LParen, "'(' here")?;
        let mut operands: Vec<DwarfExpressionOperand> = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                let operand = match self.peek() {
                    // `DW_OP_*` and `DW_ATE_*` are the two keyword families
                    // upstream accepts here, and each is looked up in its own
                    // table: a spelling the table does not carry is rejected
                    // by name rather than stored and printed straight back.
                    Token::DwarfOp(s) => {
                        let name = (*s).to_owned();
                        if llvmkit_ir::dwarf::operation_encoding(&name).is_none() {
                            return Err(ParseError::InvalidMetadataFieldValue {
                                what: "DWARF op",
                                value: name,
                                loc: DiagLoc::span(self.loc()),
                            });
                        }
                        self.bump()?;
                        DwarfExpressionOperand::Operation(name)
                    }
                    Token::DwarfAttEncoding(s) => {
                        let name = (*s).to_owned();
                        if llvmkit_ir::dwarf::attribute_encoding(&name).is_none() {
                            return Err(ParseError::InvalidMetadataFieldValue {
                                what: "DWARF attribute encoding",
                                value: name,
                                loc: DiagLoc::span(self.loc()),
                            });
                        }
                        self.bump()?;
                        DwarfExpressionOperand::Operation(name)
                    }
                    // Anything else must be an unsigned literal. This is
                    // `parseDIExpressionBody`'s **own** check, not
                    // `parseUInt64`'s — it inspects the token directly and
                    // says `expected unsigned integer`, so the site cannot
                    // share the helper.
                    _ => {
                        let Token::IntegerLit(IntLit {
                            sign: Sign::Pos,
                            base: NumBase::Dec,
                            digits,
                        }) = *self.peek()
                        else {
                            return Err(self.expected("unsigned integer"));
                        };
                        // A literal that will not fit is a *separate*
                        // diagnostic upstream: the value is read as an APSInt
                        // first and only then measured against `UINT64_MAX`.
                        let Ok(value) = digits.parse::<u64>() else {
                            return Err(
                                self.message(format!("element too large, limit is {}", u64::MAX))
                            );
                        };
                        self.bump()?;
                        DwarfExpressionOperand::Literal(value)
                    }
                };
                operands.push(operand);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(llvmkit_ir::metadata::MetadataKind::Specialized({
            let mut node = llvmkit_ir::metadata::SpecializedMetadataNode::new(
                llvmkit_ir::metadata::SpecializedMetadataKind::DiExpression,
            );
            if distinct {
                node = node.distinct();
            }
            node.with_expression_operands(operands)
        }))
    }

    /// Check a parsed field value against the grammar its class declares.
    ///
    /// Mirrors the `LLParser::parseMDField` overload set (`LLParser.cpp`): each
    /// field type is a separate overload upstream, and the overload is where
    /// the rejection lives. Applied after the value is read rather than during,
    /// which reaches the same verdicts — the token shape a value came from is
    /// recoverable from the value itself.
    ///
    /// Keyword families are validated through [`llvmkit_ir::dwarf`], whose
    /// tables are the vendored `Dwarf.def` / `DebugInfoFlags.def` (see
    /// `dwarf_def_drift.rs`). A field that upstream lets you write either as a
    /// keyword or as a raw encoding accepts both here too.
    /// The rules a specialized node applies *after* every field is read, where
    /// one field's presence or value constrains another's.
    ///
    /// Upstream writes each of these as a hand-written `if` at the bottom of
    /// the class's own `parse##CLASS`, below the `PARSE_MD_FIELDS()` macro —
    /// they are the reason those routines have a body at all beyond the macro.
    fn check_specialized_metadata_agreement(
        &self,
        kind: llvmkit_ir::metadata::SpecializedMetadataKind,
        distinct: bool,
        fields: &[llvmkit_ir::metadata::MetadataField<B>],
        node_loc: Span,
    ) -> ParseResult<()> {
        use llvmkit_ir::metadata::SpecializedMetadataKind as Kind;

        let seen = |name: &str| fields.iter().any(|field| field.name() == name);
        let value = |name: &str| {
            fields
                .iter()
                .find(|field| field.name() == name)
                .map(llvmkit_ir::metadata::MetadataField::value)
        };

        match kind {
            // `parseDICompileUnit`: `language` and `sourceLanguageName` are
            // each `OPTIONAL`, but exactly one of them is required, and the
            // version rides on the second.
            Kind::DiCompileUnit => {
                if !seen("language") && !seen("sourceLanguageName") {
                    return Err(self.message_at(
                        node_loc,
                        "missing one of 'language' or 'sourceLanguageName', required for !DICompileUnit",
                    ));
                }
                if seen("language") && seen("sourceLanguageName") {
                    return Err(self.message_at(
                        node_loc,
                        "can only specify one of 'language' and 'sourceLanguageName' on !DICompileUnit",
                    ));
                }
                if seen("sourceLanguageVersion") && !seen("sourceLanguageName") {
                    return Err(self.message_at(
                        node_loc,
                        "'sourceLanguageVersion' requires an associated 'sourceLanguageName' on !DICompileUnit",
                    ));
                }
            }
            // `parseDIEnumerator`: a `tokError`, so it anchors at whatever
            // follows the closing paren rather than at the offending field.
            Kind::DiEnumerator => {
                let is_unsigned = matches!(
                    value("isUnsigned"),
                    Some(llvmkit_ir::metadata::MetadataFieldValue::Bool(true))
                );
                let negative = matches!(
                    value("value"),
                    Some(llvmkit_ir::metadata::MetadataFieldValue::Integer(v)) if *v < 0
                );
                if is_unsigned && negative {
                    return Err(self.message("unsigned enumerator with negative value"));
                }
            }
            // `parseDIFile`: the checksum is a pair, and half a pair is an
            // error rather than a silently dropped field.
            Kind::DiFile => {
                if seen("checksumkind") != seen("checksum") {
                    return Err(
                        self.message("'checksumkind' and 'checksum' must be provided together")
                    );
                }
            }
            // `parseDISubprogram`: the guard reads the *computed* `SPFlags`, so
            // an explicit `spFlags:` carrying `DISPFlagDefinition` trips it
            // just as `isDefinition: true` does — and `spFlags:`, when given,
            // is what `toSPFlags` is skipped in favour of.
            Kind::DiSubprogram => {
                let is_definition = match value("spFlags") {
                    Some(llvmkit_ir::metadata::MetadataFieldValue::DispFlags(flags)) => {
                        flags.contains(llvmkit_ir::metadata::DispFlags::definition())
                    }
                    _ => matches!(
                        value("isDefinition"),
                        Some(llvmkit_ir::metadata::MetadataFieldValue::Bool(true))
                    ),
                };
                if is_definition && !distinct {
                    return Err(self.message_at(
                        node_loc,
                        "missing 'distinct', required for !DISubprogram that is a Definition",
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn check_metadata_field_value(
        &self,
        field: llvmkit_ir::metadata::SpecializedMetadataField,
        value: &MetadataFieldValue<B>,
        value_loc: Span,
    ) -> ParseResult<()> {
        use llvmkit_ir::dwarf;
        use llvmkit_ir::metadata::{MetadataFieldKind, MetadataFieldValue};

        // Every `parseMDField` overload opens with its token-kind check and
        // reports through `tokError`, i.e. at the value token — which llvmkit
        // has already consumed by the time this runs, so `self.loc()` would
        // name the token *after* it. `arg: -1` is the case with a vendored
        // pin: `test/Assembler/invalid-dilocalvariable-arg-negative.ll` puts
        // `expected unsigned integer` on the `-`, not on the `)` behind it.
        let loc = DiagLoc::span(value_loc);
        let name = field.name();

        // A keyword family: reject a spelling its table does not contain, and
        // let a raw unsigned encoding through as upstream's overloads do.
        let keyword = |what: &'static str, lookup: fn(&str) -> Option<u32>| -> ParseResult<()> {
            match value {
                MetadataFieldValue::Enum(spelling) => {
                    if lookup(spelling).is_none() {
                        return Err(ParseError::InvalidMetadataFieldValue {
                            what,
                            value: spelling.clone(),
                            loc,
                        });
                    }
                    Ok(())
                }
                _ => Ok(()),
            }
        };

        match field.kind() {
            MetadataFieldKind::Metadata { allow_null } => {
                if !allow_null && matches!(value, MetadataFieldValue::Null) {
                    return Err(ParseError::MetadataFieldCannotBeNull {
                        field: name.to_owned(),
                        loc,
                    });
                }
                Ok(())
            }
            MetadataFieldKind::MetadataString { empty_is_error } => {
                if empty_is_error
                    && matches!(value, MetadataFieldValue::String(text) if text.is_empty())
                {
                    return Err(ParseError::MetadataFieldCannotBeEmpty {
                        field: name.to_owned(),
                        loc,
                    });
                }
                Ok(())
            }
            MetadataFieldKind::Unsigned { max } | MetadataFieldKind::UnsignedOrMetadata { max } => {
                let MetadataFieldValue::Integer(parsed) = value else {
                    return Ok(());
                };
                if *parsed < 0 {
                    return Err(self.expected_at(value_loc, "unsigned integer"));
                }
                if u128::try_from(*parsed).is_ok_and(|v| v > u128::from(max)) {
                    return Err(ParseError::MetadataFieldValueTooLarge {
                        field: name.to_owned(),
                        limit: max,
                        loc,
                    });
                }
                Ok(())
            }
            MetadataFieldKind::Signed { min, max } => {
                let MetadataFieldValue::Integer(parsed) = value else {
                    // `parseMDField(MDSignedField&)` opens by demanding an
                    // `APSInt`; anything else never reaches the range checks.
                    return Err(self.expected_at(value_loc, "signed integer"));
                };
                if *parsed < i128::from(min) {
                    return Err(ParseError::MetadataFieldValueTooSmall {
                        field: name.to_owned(),
                        limit: min,
                        loc,
                    });
                }
                if *parsed > i128::from(max) {
                    return Err(ParseError::MetadataFieldValueTooLarge {
                        field: name.to_owned(),
                        limit: max.unsigned_abs(),
                        loc,
                    });
                }
                Ok(())
            }
            MetadataFieldKind::Bool => {
                if matches!(value, MetadataFieldValue::Bool(_)) {
                    Ok(())
                } else {
                    Err(self.expected_at(value_loc, "'true' or 'false'"))
                }
            }
            MetadataFieldKind::DwarfTag => keyword("DWARF tag", dwarf::tag),
            MetadataFieldKind::DwarfAttEncoding => {
                keyword("DWARF type attribute encoding", dwarf::attribute_encoding)
            }
            MetadataFieldKind::DwarfVirtuality => {
                keyword("DWARF virtuality code", dwarf::virtuality)
            }
            MetadataFieldKind::DwarfLang => keyword("DWARF language", dwarf::language),
            MetadataFieldKind::DwarfSourceLangName => {
                keyword("DWARF source language name", dwarf::source_language_name)
            }
            MetadataFieldKind::DwarfCc => {
                keyword("DWARF calling convention", dwarf::calling_convention)
            }
            MetadataFieldKind::DwarfMacinfoType => keyword("DWARF macinfo type", dwarf::macinfo),
            // Both flag families are validated term by term as they are
            // parsed, where `parseMDField`'s `parseFlag` validates them, so
            // there is nothing left to check once the bitfield exists.
            MetadataFieldKind::DiFlags | MetadataFieldKind::DispFlags => Ok(()),
            MetadataFieldKind::EmissionKind => keyword("emission kind", emission_kind),
            MetadataFieldKind::NameTableKind => keyword("nameTable kind", name_table_kind),
            MetadataFieldKind::ChecksumKind => keyword("checksum kind", checksum_kind),
            MetadataFieldKind::FixedPointKind => keyword("fixed-point kind", fixed_point_kind),
            // `parseMDField(DwarfEnumKindField&)` splits its rejection in two:
            // a token that is neither an integer nor a `DW_APPLE_ENUM_KIND_*`
            // keyword is `expected DWARF enum kind code`, while a keyword the
            // table does not carry is `invalid DWARF enum kind code '...'`.
            MetadataFieldKind::DwarfEnumKind => match value {
                MetadataFieldValue::Integer(_) => Ok(()),
                MetadataFieldValue::Enum(spelling) => {
                    if llvmkit_ir::dwarf::apple_enum_kind(spelling).is_some() {
                        Ok(())
                    } else {
                        Err(ParseError::InvalidMetadataFieldValue {
                            what: "DWARF enum kind code",
                            value: spelling.clone(),
                            loc,
                        })
                    }
                }
                _ => Err(self.expected_at(value_loc, "DWARF enum kind code")),
            },
            MetadataFieldKind::ApsInt
            | MetadataFieldKind::MetadataList
            | MetadataFieldKind::SignedOrMetadata => Ok(()),
        }
    }

    /// Parse one `name: value` right-hand side.
    ///
    /// `declared` is the field's declared kind, and it is here for one reason:
    /// upstream has no generic "metadata field value" production. Each
    /// `LLParser::parseMDField` overload is typed, and every one opens by
    /// checking the token kind against the one it wants — `expected DWARF
    /// tag`, `expected emission kind`, `expected nameTable kind`,
    /// `expected fixed-point kind`. A word matching no keyword lexes as
    /// `Token::Error` and lands in exactly that check, so the fallthrough
    /// below has to name the field's family rather than the syntactic
    /// category.
    fn parse_metadata_field_value(
        &mut self,
        declared: llvmkit_ir::metadata::MetadataFieldKind,
    ) -> ParseResult<MetadataFieldValue<B>> {
        use llvmkit_ir::metadata::{MetadataFieldKind, MetadataFieldValue};
        // The two flag families are dispatched on the *declared* kind rather
        // than on the token, because upstream's `parseMDField(DIFlagField&)`
        // and its `DISPFlagField` twin are separate overloads with their own
        // grammar — a `do { parseFlag } while (EatIfPresent(lltok::bar))` loop
        // over terms that may be a flag keyword *or* an unsigned integer.
        // Reading them off the token, as every other field here is read, is
        // what made `flags: 4 | DIFlagPublic` unparseable.
        match declared {
            MetadataFieldKind::DiFlags => {
                return self.parse_di_flag_field();
            }
            MetadataFieldKind::DispFlags => {
                return self.parse_disp_flag_field();
            }
            _ => {}
        }
        match self.peek() {
            Token::Kw(Keyword::Null) => {
                self.bump()?;
                Ok(MetadataFieldValue::Null)
            }
            Token::Kw(Keyword::True) => {
                self.bump()?;
                Ok(MetadataFieldValue::Bool(true))
            }
            Token::Kw(Keyword::False) => {
                self.bump()?;
                Ok(MetadataFieldValue::Bool(false))
            }
            Token::StringConstant(_) => Ok(MetadataFieldValue::String(
                self.parse_string_constant("metadata field string")?,
            )),
            Token::IntegerLit(_) => {
                let parsed = self.parse_int_literal()?;
                let value = parsed_apsint_to_i128(&parsed)
                    .ok_or_else(|| self.expected("metadata integer literal in i128 range"))?;
                Ok(MetadataFieldValue::Integer(value))
            }
            Token::MetadataVar(_) => {
                let content = self.parse_md_node_after_bang(false)?;
                Ok(MetadataFieldValue::Metadata(own_metadata(
                    self.module.metadata_node(content),
                )))
            }
            Token::Exclaim => {
                self.bump()?;
                match self.peek() {
                    Token::LBrace => {
                        self.bump()?;
                        let mut items = Vec::new();
                        if !matches!(self.peek(), Token::RBrace) {
                            loop {
                                items.push(self.parse_md_tuple_operand()?);
                                if !self.eat_punct(PunctKind::Comma)? {
                                    break;
                                }
                            }
                        }
                        self.expect_punct(PunctKind::RBrace, "'}' closing metadata list")?;
                        Ok(MetadataFieldValue::MetadataList(items))
                    }
                    Token::StringConstant(_) => {
                        let s = self.parse_string_constant("metadata string")?;
                        Ok(MetadataFieldValue::Metadata(self.module.metadata_string(s)))
                    }
                    Token::MetadataVar(_) => {
                        let content = self.parse_md_node_after_bang(false)?;
                        Ok(MetadataFieldValue::Metadata(own_metadata(
                            self.module.metadata_node(content),
                        )))
                    }
                    _ => {
                        let loc = self.loc();
                        let slot = self.parse_uint32()?;
                        Ok(MetadataFieldValue::Metadata(
                            self.resolve_md_slot(slot, loc),
                        ))
                    }
                }
            }
            Token::DwarfTag(s)
            | Token::DwarfAttEncoding(s)
            | Token::DwarfVirtuality(s)
            | Token::DwarfLang(s)
            | Token::DwarfSourceLangName(s)
            | Token::DwarfCc(s)
            | Token::DwarfOp(s)
            | Token::DwarfMacinfo(s)
            | Token::DwarfEnumKind(s)
            | Token::ChecksumKind(s)
            | Token::EmissionKind(s)
            | Token::NameTableKind(s)
            | Token::FixedPointKind(s) => {
                let value = (*s).to_owned();
                self.bump()?;
                Ok(MetadataFieldValue::Enum(value))
            }
            _ => Err(self.expected(expected_for_metadata_field_kind(declared))),
        }
    }

    /// `parseMDField(LocTy Loc, StringRef Name, DIFlagField &Result)`
    /// (`LLParser.cpp`): `do { parseFlag } while (EatIfPresent(lltok::bar))`,
    /// OR-ing the terms into one `DINode::DIFlags`.
    fn parse_di_flag_field(&mut self) -> ParseResult<MetadataFieldValue<B>> {
        use llvmkit_ir::metadata::{DiFlags, MetadataFieldValue};
        // `DINode::DIFlags Combined = DINode::FlagZero;`
        let mut combined = DiFlags::ZERO;
        loop {
            combined = combined.union(self.parse_di_flag()?);
            if !matches!(self.peek(), Token::Bar) {
                break;
            }
            self.bump()?;
        }
        Ok(MetadataFieldValue::DiFlags(combined))
    }

    /// The `parseFlag` lambda inside `parseMDField(DIFlagField&)`. An unsigned
    /// `lltok::APSInt` is read through `parseUInt32`; anything that is not a
    /// `lltok::DIFlag` is `expected debug info flag`; a `DIFlag*` the table
    /// does not carry comes back as `FlagZero` from `DINode::getFlag` and is
    /// `invalid debug info flag '…'`.
    ///
    /// A *signed* integer term falls through the first arm to the second, so
    /// `flags: -1` answers `expected debug info flag` rather than being
    /// accepted as a bitfield.
    fn parse_di_flag(&mut self) -> ParseResult<llvmkit_ir::metadata::DiFlags> {
        use llvmkit_ir::metadata::DiFlags;
        if self.peek_unsigned_apsint().is_some() {
            return Ok(DiFlags::from_bits(self.parse_uint32()?));
        }
        let Token::DiFlag(spelling) = self.peek() else {
            return Err(self.expected("debug info flag"));
        };
        let spelling = (*spelling).to_owned();
        let value = DiFlags::get_flag(&spelling);
        if value == DiFlags::ZERO {
            return Err(ParseError::InvalidMetadataFieldValue {
                what: "debug info flag",
                value: spelling,
                loc: DiagLoc::span(self.loc()),
            });
        }
        self.bump()?;
        Ok(value)
    }

    /// `parseMDField(LocTy Loc, StringRef Name, DISPFlagField &Result)`, the
    /// twin of [`Self::parse_di_flag_field`].
    fn parse_disp_flag_field(&mut self) -> ParseResult<MetadataFieldValue<B>> {
        use llvmkit_ir::metadata::{DispFlags, MetadataFieldValue};
        let mut combined = DispFlags::ZERO;
        loop {
            combined = combined.union(self.parse_disp_flag()?);
            if !matches!(self.peek(), Token::Bar) {
                break;
            }
            self.bump()?;
        }
        Ok(MetadataFieldValue::DispFlags(combined))
    }

    /// The `DISPFlagField` overload's `parseFlag`. Note that only the
    /// *invalid* message names the subprogram family — the token-kind
    /// rejection is `expected debug info flag` in both overloads.
    fn parse_disp_flag(&mut self) -> ParseResult<llvmkit_ir::metadata::DispFlags> {
        use llvmkit_ir::metadata::DispFlags;
        if self.peek_unsigned_apsint().is_some() {
            return Ok(DispFlags::from_bits(self.parse_uint32()?));
        }
        let Token::DiSpFlag(spelling) = self.peek() else {
            return Err(self.expected("debug info flag"));
        };
        let spelling = (*spelling).to_owned();
        let value = DispFlags::get_flag(&spelling);
        if value == DispFlags::ZERO {
            return Err(ParseError::InvalidMetadataFieldValue {
                what: "subprogram debug info flag",
                value: spelling,
                loc: DiagLoc::span(self.loc()),
            });
        }
        self.bump()?;
        Ok(value)
    }

    /// Consume a `!` token (Token::Exclaim). Helper for metadata parsing.
    fn expect_exclaim(&mut self, expected: &'static str) -> ParseResult<Span> {
        if matches!(self.peek(), Token::Exclaim) {
            self.bump()
        } else {
            Err(self.expected(expected))
        }
    }

    /// Parse one `#dbg_*` record operand: either a metadata node/reference or
    /// an ordinary typed value wrapped as debug metadata.
    /// `!DIArgList(i32 %a, i64 7)`.
    ///
    /// Mirrors `LLParser::parseDIArgList`, which `parseMetadata` special-cases
    /// ahead of `parseSpecializedMDNode` because its operands are a
    /// `ValueAsMetadata` list and therefore need a function state — the same
    /// reason `parseNamedMetadata` refuses one outright.
    fn parse_di_arg_list(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<MetadataId<B>> {
        self.bump()?; // eat `!DIArgList`
        self.expect_punct(PunctKind::LParen, "'(' here")?;

        let mut arguments = Vec::new();
        // An empty list is legal; upstream guards the loop with the same
        // lookahead rather than requiring one operand.
        if !matches!(self.peek(), Token::RParen) {
            loop {
                // `if (parseValueAsMetadata(MD, "expected value-as-metadata
                // operand", PFS)) return true;` — the same routine
                // `parseMetadata`'s fall-through uses, with `parseDIArgList`'s
                // own `TypeMsg`. Inlining it here cost the `isMetadataTy`
                // guard: `!DIArgList(metadata %a)` reported a type mismatch on
                // the value instead of `invalid metadata-value-metadata
                // roundtrip` on the type.
                let md = self.parse_value_as_metadata("value-as-metadata operand", Some(state))?;
                // `Args.push_back(dyn_cast<ValueAsMetadata>(MD));` — llvmkit
                // stores a `DIArgList` operand as the value itself, which is
                // what a `ValueAsMetadata` wraps, so the cast is the unwrap.
                // `parse_value_as_metadata` returns on exactly one path and it
                // builds `MetadataKind::Constant`, so the `else` is dead by
                // construction, as upstream's `dyn_cast` never fails here.
                let Some(MetadataKind::Constant(value_id)) = self.module.metadata_get(md) else {
                    unreachable!("parse_value_as_metadata yields MetadataKind::Constant")
                };
                arguments.push(value_id);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }

        self.expect_punct(PunctKind::RParen, "')' here")?;
        Ok(own_metadata(self.module.metadata_node(
            llvmkit_ir::metadata::MetadataKind::ArgList { arguments },
        )))
    }

    /// Whether the lookahead is the `!DIArgList` spelling, which is a
    /// `MetadataVar` like any other specialized node name but is dispatched
    /// before them.
    fn peek_is_di_arg_list(&self) -> bool {
        match self.peek() {
            Token::MetadataVar(bytes) => bytes.as_ref() == b"DIArgList",
            _ => false,
        }
    }

    /// `parseDebugRecord`'s `if (parseMetadata(ValLocMD, &PFS)) return true;`
    /// — the whole routine, not a re-implementation of it.
    ///
    /// Upstream holds one `Metadata *`; llvmkit's [`DebugMetadataOperand`]
    /// splits it into the `!`-spelled node and the `ValueAsMetadata` that a
    /// bare `<type> <value>` builds, because the printer spells the two
    /// differently. Which of the two `parseMetadata` will build is decided by
    /// its own dispatch token, so the fork is read off the lookahead *before*
    /// the call rather than guessed from the node afterwards — a `!N` slot
    /// that happens to resolve to a `ValueAsMetadata` is still the `!N`
    /// spelling.
    fn parse_debug_metadata_operand(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<DebugMetadataOperand<B>> {
        let is_value_as_metadata = !matches!(self.peek(), Token::Exclaim | Token::MetadataVar(_));
        let md = self.parse_metadata(Some(state))?;
        if !is_value_as_metadata {
            return Ok(DebugMetadataOperand::Metadata(md));
        }
        // `parseMetadata`'s non-`!` fall-through is
        // `parseValueAsMetadata(MD, "expected metadata operand", PFS)`, whose
        // only success statement is `MD = ValueAsMetadata::get(V);`, so the
        // `else` is dead by construction — the same unwrap
        // `parse_di_arg_list` performs on the same routine.
        let Some(MetadataKind::Constant(value_id)) = self.module.metadata_get(md) else {
            unreachable!("parse_value_as_metadata yields MetadataKind::Constant")
        };
        Ok(DebugMetadataOperand::Value(value_id))
    }

    fn parse_debug_record(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<DebugRecord<B>> {
        use llvmkit_ir::metadata::{DebugRecord, DebugVariableRecord, DebugVariableRecordKind};

        // `parseDebugRecord` frames the whole record with **capital-`E`**
        // labels — `Expected '(' here`, `Expected ',' here`, `Expected ')'
        // here` — a spelling it shares with `parseNamedMetadata` and with
        // nothing else nearby. Only the opening `expected debug record type
        // here` is lowercase, and it is an `error` at the record-type token
        // rather than a `tokError`.
        let record_loc = self.loc();
        let record_type = match self.peek() {
            Token::DbgRecordType(name) => *name,
            _ => return Err(self.message_at(record_loc, "expected debug record type here")),
        };
        self.bump()?;
        self.expect_message_punct(PunctKind::LParen, "Expected '(' here")?;

        if record_type == "label" {
            let label = self.parse_metadata_attachment_operand()?;
            self.expect_message_punct(PunctKind::Comma, "Expected ',' here")?;
            let debug_loc = self.parse_metadata_attachment_operand()?;
            self.expect_message_punct(PunctKind::RParen, "Expected ')' here")?;
            return Ok(DebugRecord::Label { label, debug_loc });
        }

        let kind = match record_type {
            "declare" => DebugVariableRecordKind::Declare,
            "value" => DebugVariableRecordKind::Value,
            "assign" => DebugVariableRecordKind::Assign,
            "declare_value" => DebugVariableRecordKind::DeclareValue,
            // The lexer produces `DbgRecordType` for exactly these five
            // spellings and no others, which is why upstream's `StringSwitch`
            // needs no `Default` arm.
            other => unreachable!("the lexer cannot produce #dbg_{other}"),
        };

        let location = self.parse_debug_metadata_operand(state)?;
        self.expect_message_punct(PunctKind::Comma, "Expected ',' here")?;
        let variable = self.parse_metadata_attachment_operand()?;
        self.expect_message_punct(PunctKind::Comma, "Expected ',' here")?;
        let expression = self.parse_metadata_attachment_operand()?;
        self.expect_message_punct(PunctKind::Comma, "Expected ',' here")?;

        let (assign_id, address_location, address_expression) =
            if kind == DebugVariableRecordKind::Assign {
                let assign_id = self.parse_metadata_attachment_operand()?;
                self.expect_message_punct(PunctKind::Comma, "Expected ',' here")?;
                let address_location = self.parse_debug_metadata_operand(state)?;
                self.expect_message_punct(PunctKind::Comma, "Expected ',' here")?;
                let address_expression = self.parse_metadata_attachment_operand()?;
                self.expect_message_punct(PunctKind::Comma, "Expected ',' here")?;
                (
                    Some(assign_id),
                    Some(address_location),
                    Some(address_expression),
                )
            } else {
                (None, None, None)
            };

        let debug_loc = self.parse_metadata_attachment_operand()?;
        self.expect_message_punct(PunctKind::RParen, "Expected ')' here")?;
        let mut record = DebugVariableRecord::new(kind, location, variable, expression, debug_loc);
        if let Some(assign_id) = assign_id {
            record = record.with_assign_id(assign_id);
        }
        if let Some(address_location) = address_location {
            record = record.with_address_location(address_location);
        }
        if let Some(address_expression) = address_expression {
            record = record.with_address_expression(address_expression);
        }
        Ok(DebugRecord::Variable(record))
    }

    /// The tail of `parseBasicBlock`'s loop body: the `, !kind !N` attachments
    /// (`parseInstructionMetadata`), the `#dbg_*` records that preceded the
    /// instruction, and then `ParserContext->addInstructionLocation`.
    ///
    /// `instruction_start` is upstream's `InstStart`; the range closes at
    /// `PrevTokEnd`, which by here is past the trailing metadata — upstream
    /// records at the same point, after the `switch` that consumed it.
    fn finish_trailing_metadata(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        bb_value: llvmkit_ir::Value<'ctx, B>,
        pending_debug_records: &mut Vec<DebugRecord<B>>,
        instruction_start: u32,
    ) -> ParseResult<()> {
        let bb = state.value_as_block_view(bb_value, self.loc())?;
        self.skip_trailing_metadata(&bb)?;
        if !pending_debug_records.is_empty() {
            let inst = bb
                .instructions()
                .last()
                .ok_or_else(|| ParseError::Expected {
                    expected: "instruction after debug record".into(),
                    loc: DiagLoc::span(self.loc()),
                })?;
            for record in pending_debug_records.drain(..) {
                own_metadata(inst.push_debug_record(self.module, record));
            }
        }
        if self.parser_context.is_some() {
            // The instruction just parsed is the one at the block's tail.
            // Upstream names it directly (`Inst`); llvmkit's parse arms hand
            // back a value rather than a lifecycle handle, so the block is
            // asked instead. A parse arm that appended nothing — none does
            // today — would record nothing rather than mislabel a neighbour.
            let instruction = bb.instructions().last();
            let range = self.file_loc_range_to_prev_token_end(instruction_start);
            if let Some(instruction) = instruction
                && let Some(context) = self.parser_context.as_mut()
            {
                // `addInstructionLocation`'s bool is discarded upstream.
                let _first_insert_won = context.add_instruction_location(&instruction, range);
            }
        }
        Ok(())
    }

    /// Mirrors the metadata-attachment loop in `LLParser::parseInstructionMetadata`.
    fn skip_trailing_metadata<S: llvmkit_ir::BlockTerminationState>(
        &mut self,
        bb: &llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, S, B>,
    ) -> ParseResult<()> {
        if matches!(self.peek(), Token::MetadataVar(_)) {
            return Err(self.expected("',' before trailing metadata"));
        }

        while matches!(self.peek(), Token::Comma) {
            self.bump()?;
            let name = match self.peek() {
                Token::MetadataVar(bytes) => std::str::from_utf8(bytes.as_ref())
                    .map_err(|_| self.expected("valid UTF-8 metadata attachment name"))?
                    .to_owned(),
                // `parseInstructionMetadata`'s only message. It is what an
                // `alloca` with a trailing comma reports, and what a clause
                // written after `addrspace(...)` reports too — the loop
                // demands metadata once a comma has been eaten, so a
                // misordered `align 4` lands here rather than on a dedicated
                // diagnostic (`alloca-addrspace-parse-error-{0,1}.ll`).
                _ => return Err(self.expected("metadata after comma")),
            };
            self.bump()?;
            let id = self.parse_metadata_attachment_operand()?;
            if let Some(inst) = bb.instructions().last() {
                let kind = MetadataAttachmentKind::from_name(&name);
                own_metadata(inst.set_metadata(self.module, kind.clone(), id));
                // `if (MDK == LLVMContext::MD_tbaa)
                //    InstsWithTBAATag.push_back(&Inst);`
                if kind == MetadataAttachmentKind::Tbaa {
                    self.insts_with_tbaa_tag.push(inst);
                }
            }
        }
        Ok(())
    }
    // ── Type definitions ─────────────────────────────────────────────────

    /// `%name = type ...` — mirrors `LLParser::parseNamedType`.
    fn parse_named_type_definition(&mut self) -> ParseResult<()> {
        let name = self
            .current_str_payload()
            .ok_or_else(|| self.expected("named type identifier"))?;
        let name_loc = self.loc();
        self.bump()?; // eat LocalVar
        // Both directives say `'=' after name`; only the *second* label differs
        // between them, and it is `parseNamedType` that says `after name`.
        self.expect_punct(PunctKind::Equal, "'=' after name")?;
        self.expect_keyword(Keyword::Type, "'type' after name")?;
        self.parse_struct_definition(Some(name), None, name_loc)
    }

    /// `%N = type ...` — mirrors `LLParser::parseUnnamedType`.
    fn parse_unnamed_type_definition(&mut self) -> ParseResult<()> {
        let id = match self.peek() {
            Token::LocalVarId(n) => *n,
            _ => return Err(self.expected("numbered type identifier")),
        };
        let loc = self.loc();
        self.bump()?;
        // `parseUnnamedType` says `after name` for the `=` and `after '='` for
        // the keyword, where `parseNamedType` says `after name` for both.
        self.expect_punct(PunctKind::Equal, "'=' after name")?;
        self.expect_keyword(Keyword::Type, "'type' after '='")?;
        self.parse_struct_definition(None, Some(id), loc)
    }

    /// Common path between `parseNamedType` and `parseUnnamedType`. The
    /// directive's RHS is restricted to a struct type (or `opaque`) per
    /// upstream `parseStructDefinition`; non-struct RHS is a typed error.
    /// `%name = type ...` / `%N = type ...`. Mirrors
    /// `LLParser::parseStructDefinition`.
    ///
    /// The table entry's `forward_ref_loc` is the whole state machine:
    /// present means "referenced, not yet defined", absent means "defined".
    /// Upstream spells the same thing as the validity of the `LocTy` half of
    /// its `std::pair<Type *, LocTy>`.
    fn parse_struct_definition(
        &mut self,
        name: Option<String>,
        slot: Option<u32>,
        decl_loc: Span,
    ) -> ParseResult<()> {
        let existing = match (&name, slot) {
            (Some(n), None) => self.named_types.get(n.as_str()).copied(),
            (None, Some(id)) => self.numbered_types.get(&id).copied(),
            _ => unreachable!("parse_struct_definition called without a name xor slot"),
        };
        // Already defined once — `Entry.first && !Entry.second.isValid()`.
        if let Some(entry) = existing
            && entry.forward_ref_loc.is_none()
        {
            return Err(self.message_at(decl_loc, "redefinition of type"));
        }

        // `%t = type opaque` counts as a definition even though it leaves the
        // body unset, which is why a later `%t = type {i32}` is a redefinition.
        if matches!(self.peek(), Token::Kw(Keyword::Opaque)) {
            self.bump()?;
            let ty = match existing {
                Some(entry) => entry.ty,
                None => self.fresh_identified_struct(&name),
            };
            self.record_type_definition(name, slot, ty);
            return Ok(());
        }

        // `<` here is either a packed struct body or a vector type alias.
        let is_packed = self.eat_punct(PunctKind::Less)?;
        if !matches!(self.peek(), Token::LBrace) {
            // A random type alias, accepted for compatibility with old files.
            // These may not be forward referenced, because there is no
            // identified struct to fill in later.
            if existing.is_some() {
                return Err(self.message_at(decl_loc, "forward references to non-struct type"));
            }
            let aliased = if is_packed {
                self.parse_array_or_vector_after_open(true)?
            } else {
                self.parse_type(false)?
            };
            self.record_type_definition(name, slot, aliased);
            return Ok(());
        }

        // `<` was already eaten above when present, so the two shapes take
        // different helpers: the brace-only one when we are inside `<{...}>`,
        // and the general one — which handles its own `<` — otherwise.
        let (elements, packed) = if is_packed {
            let (elements, _) = self.parse_struct_body_braces()?;
            self.expect_punct(PunctKind::Greater, "'>' in packed struct")?;
            (elements, true)
        } else {
            self.parse_struct_body()?
        };

        let ty = match existing {
            Some(entry) => entry.ty,
            None => self.fresh_identified_struct(&name),
        };
        let handle: StructType<'ctx, llvmkit_ir::StructBodyDyn, B> = StructType::try_from(ty)
            .map_err(|_| ParseError::Message {
                message: "redefinition of type".into(),
                loc: DiagLoc::span(decl_loc),
            })?;
        self.module
            .set_struct_body_dyn(handle, elements, packed)
            .map_err(|e| match e {
                // `parseStructDefinition` hands `setBodyOrError`'s message
                // straight to `tokError`, so it is printed verbatim.
                IrError::RecursiveStructBody { .. } => ParseError::Message {
                    message: e.to_string().into(),
                    loc: DiagLoc::span(decl_loc),
                },
                other => ParseError::Expected {
                    expected: format!("valid struct body: {other}").into(),
                    loc: DiagLoc::span(decl_loc),
                },
            })?;
        self.record_type_definition(name, slot, ty);
        Ok(())
    }

    /// The identified struct a `type` directive defines when the name has not
    /// been uttered before — named or anonymous, matching upstream's two
    /// `StructType::create` overloads.
    fn fresh_identified_struct(&self, name: &Option<String>) -> Type<'ctx, B> {
        match name {
            Some(n) => self.module.get_or_insert_named_struct(n).as_type(),
            None => self.module.anonymous_identified_struct().as_type(),
        }
    }

    /// Record a completed `type` definition, clearing the forward-reference
    /// location so a second definition is caught as a redefinition.
    fn record_type_definition(
        &mut self,
        name: Option<String>,
        slot: Option<u32>,
        ty: Type<'ctx, B>,
    ) {
        let entry = TypeEntry {
            ty,
            forward_ref_loc: None,
        };
        match (name, slot) {
            (Some(n), _) => {
                self.named_types.insert(n, entry);
            }
            (None, Some(id)) => {
                self.numbered_types.insert(id, entry);
                self.next_unnamed_type_id = self.next_unnamed_type_id.max(id.saturating_add(1));
            }
            (None, None) => unreachable!("a type definition is either named or numbered"),
        }
    }

    /// Parse a struct body: `{ T, T, ... }` or `<{ T, T, ... }>` (packed).
    fn parse_struct_body(&mut self) -> ParseResult<(Vec<Type<'ctx, B>>, bool)> {
        let packed;
        if self.eat_punct(PunctKind::Less)? {
            packed = true;
            self.expect_punct(PunctKind::LBrace, "'{' after '<' in packed struct")?;
        } else {
            packed = false;
            self.expect_punct(PunctKind::LBrace, "'{' to start struct body")?;
        }
        let mut elems = Vec::new();
        if !matches!(self.peek(), Token::RBrace) {
            loop {
                elems.push(self.parse_struct_element()?);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RBrace, "'}' to close struct body")?;
        if packed {
            self.expect_punct(PunctKind::Greater, "'>' to close packed struct")?;
        }
        Ok((elems, packed))
    }

    // ── Type grammar (`LLParser::parseType`) ─────────────────────────────

    /// Mirrors `LLParser::parseType(Type *&Result, ..., bool AllowVoid)`.
    /// `allow_void` is `true` only at function-result position.
    pub fn parse_type(&mut self, allow_void: bool) -> ParseResult<Type<'ctx, B>> {
        let type_loc = self.loc();
        let mut result: Type<'ctx, B> = {
            match *self.peek() {
                Token::PrimitiveType(p) => {
                    let ty = self.primitive_to_type(p, type_loc)?;
                    self.bump()?;
                    // `ptr` may be followed by `addrspace(N)`.
                    if matches!(p, PrimitiveTy::Ptr) {
                        let addr_space = if let Token::Kw(Keyword::Addrspace) = self.peek() {
                            self.bump()?;
                            self.parse_addr_space_paren()?
                        } else {
                            0
                        };
                        let ptr_ty: PointerType<'ctx, B> = self.module.ptr_type(addr_space);
                        if matches!(self.peek(), Token::Star) {
                            return Err(self.message("ptr* is invalid - use ptr instead"));
                        }
                        // "Fall through to parsing the type suffixes only
                        // if this 'ptr' is a function return. Otherwise,
                        // return success, implicitly rejecting other
                        // suffixes." — `LLParser::parseType`.
                        if !matches!(self.peek(), Token::LParen) {
                            return Ok(ptr_ty.as_type());
                        }
                        ptr_ty.as_type()
                    } else {
                        ty
                    }
                }
                Token::LBrace => {
                    let (elems, packed) = self.parse_struct_body()?;
                    if packed {
                        self.module.packed_struct_type(elems).as_type()
                    } else {
                        self.module.struct_type(elems).as_type()
                    }
                }
                Token::Less => {
                    // `<` introduces vector or `<{ packed-struct }>`.
                    self.bump()?; // eat `<`
                    if matches!(self.peek(), Token::LBrace) {
                        let (elems, _was_packed_redundant) = self.parse_struct_body_braces()?;
                        self.expect_punct(PunctKind::Greater, "'>' at end of packed struct")?;
                        self.module.packed_struct_type(elems).as_type()
                    } else {
                        self.parse_array_or_vector_after_open(true)?
                    }
                }
                Token::LSquare => {
                    self.bump()?; // eat `[`
                    self.parse_array_or_vector_after_open(false)?
                }
                Token::Kw(Keyword::Target) => self.parse_target_ext_type()?,
                Token::LocalVar(_) => {
                    let name = self
                        .current_str_payload()
                        .ok_or_else(|| self.expected("local identifier payload"))?;
                    let loc = self.loc();
                    self.bump()?;
                    self.lookup_or_forward_named_type(&name, loc)
                }
                Token::LocalVarId(n) => {
                    let id = n;
                    let loc = self.loc();
                    self.bump()?;
                    self.lookup_or_forward_numbered_type(id, loc)
                }
                _ => {
                    return Err(ParseError::Expected {
                        expected: "type".into(),
                        loc: DiagLoc::span(type_loc),
                    });
                }
            }
        };

        // Type suffixes, mirroring `LLParser::parseType`'s own loop. `*` and
        // `addrspace(N)*` are legacy typed-pointer syntax: the pointee is not
        // represented in llvmkit-ir, but it is still *parsed*, because the
        // rejections below are about the pointee's type. llvmkit used to skip
        // the pointee syntactically and lower straight to `ptr`, which left
        // every one of these unreachable.
        loop {
            match self.peek() {
                Token::Star => {
                    self.check_pointer_element(result, PointerSuffix::Star)?;
                    self.bump()?;
                    result = self.module.ptr_type(0).as_type();
                }
                Token::Kw(Keyword::Addrspace) => {
                    self.check_pointer_element(result, PointerSuffix::AddrSpace)?;
                    self.bump()?;
                    let addr_space = self.parse_addr_space_paren()?;
                    if !matches!(self.peek(), Token::Star) {
                        return Err(self.expected("'*' in address space"));
                    }
                    self.bump()?;
                    result = self.module.ptr_type(addr_space).as_type();
                }
                Token::LParen => {
                    result = self.parse_function_type_after_return(result)?;
                }
                _ => {
                    if !allow_void && matches!(result.into_type_enum(), AnyTypeEnum::Void(_)) {
                        // `parseType`'s `AllowVoid` guard, verbatim.
                        return Err(ParseError::Message {
                            message: "void type only allowed for function results".into(),
                            loc: DiagLoc::span(type_loc),
                        });
                    }
                    return Ok(result);
                }
            }
        }
    }

    /// The three pointee rejections in `LLParser::parseType`'s suffix loop.
    ///
    /// The `void` wording differs by one character between the two arms —
    /// `- use i8* instead` after `*`, `; use i8* instead` after `addrspace` —
    /// and that is upstream's own inconsistency, reproduced rather than
    /// smoothed (diagnostic text is contractual).
    fn check_pointer_element(
        &self,
        pointee: Type<'ctx, B>,
        suffix: PointerSuffix,
    ) -> ParseResult<()> {
        if pointee.is_label() {
            return Err(self.message("basic block pointers are invalid"));
        }
        if pointee.is_void() {
            return Err(self.message(match suffix {
                PointerSuffix::Star => "pointers to void are invalid - use i8* instead",
                PointerSuffix::AddrSpace => "pointers to void are invalid; use i8* instead",
            }));
        }
        if !pointee.is_valid_pointer_element() {
            return Err(self.message("pointer to this type is invalid"));
        }
        Ok(())
    }

    fn parse_target_ext_type(&mut self) -> ParseResult<Type<'ctx, B>> {
        self.expect_keyword(Keyword::Target, "'target'")?;
        self.expect_punct(PunctKind::LParen, "'(' in target extension type")?;
        let name = self.parse_string_constant("target extension type name")?;
        let mut type_params = Vec::new();
        let mut int_params = Vec::new();
        let mut seen_integer_param = false;
        while self.eat_punct(PunctKind::Comma)? {
            if matches!(self.peek(), Token::IntegerLit(_)) {
                seen_integer_param = true;
                int_params.push(self.parse_uint32()?);
            } else if seen_integer_param {
                // Type parameters must precede integer ones; once an integer
                // has been seen, anything else is upstream's `expected uint32
                // param`.
                return Err(self.expected("uint32 param"));
            } else {
                // `parseType(TypeParam, /*AllowVoid=*/true)` — a target
                // extension type may be parameterised by `void`.
                type_params.push(self.parse_type(true)?);
            }
        }
        self.expect_punct(PunctKind::RParen, "')' in target extension type")?;
        let loc = self.loc();
        let ty = self.module.target_ext_type(name, type_params, int_params);
        // Upstream surfaces `TargetExtType::getOrError`'s message through
        // `tokError`, so the arity complaint is the whole diagnostic.
        ty.check_params().map_err(|e| match e {
            IrError::InvalidOperation { message } => ParseError::Message {
                message: message.into(),
                loc: DiagLoc::span(loc),
            },
            other => self.builder_err("target extension type", other),
        })?;
        Ok(ty.as_type())
    }

    /// Helper: after consuming an opening `<` not followed by `{`, the
    /// remaining form is `N x T>` (vector). After consuming `[`, the form
    /// is `N x T]` (array).
    fn parse_array_or_vector_after_open(&mut self, is_vector: bool) -> ParseResult<Type<'ctx, B>> {
        // `vscale x N x T>` ?
        let scalable = if is_vector && matches!(self.peek(), Token::Kw(Keyword::Vscale)) {
            self.bump()?;
            self.expect_keyword(Keyword::X, "'x' after 'vscale'")?;
            true
        } else {
            false
        };
        // Upstream reads the count as an APSInt and only *then* range-checks
        // it, so an over-large vector count is `size too large for vector`
        // rather than a parse failure. Reading it as u64 here keeps that
        // ordering.
        let size_loc = self.loc();
        let n = self.parse_uint64()?;
        self.expect_keyword(Keyword::X, "'x' between count and element type")?;
        let elem_loc = self.loc();
        let elem = self.parse_type(false)?;
        if is_vector {
            self.expect_punct(PunctKind::Greater, "'>' at end of vector type")?;
            if n == 0 {
                return Err(self.message_at(size_loc, "zero element vector is illegal"));
            }
            let Ok(n32) = u32::try_from(n) else {
                return Err(self.message_at(size_loc, "size too large for vector"));
            };
            if !elem.is_valid_vector_element() {
                return Err(self.message_at(elem_loc, "invalid vector element type"));
            }
            let v = if scalable {
                self.module.scalable_vector_type(elem, n32)
            } else {
                self.module.vector_type(elem, n32)
            };
            Ok(v.as_type())
        } else {
            self.expect_punct(PunctKind::RSquare, "']' at end of array type")?;
            if !elem.is_valid_array_element() {
                return Err(self.message_at(elem_loc, "invalid array element type"));
            }
            let arr = self.module.array_type(elem, n);
            Ok(arr.as_type())
        }
    }

    fn parse_struct_body_braces(&mut self) -> ParseResult<(Vec<Type<'ctx, B>>, bool)> {
        // Used after `<` is already eaten; the inner `{...}` then `>`. We
        // re-use `parse_struct_body`'s logic without re-eating the `<`.
        self.expect_punct(PunctKind::LBrace, "'{' after '<' in packed struct")?;
        let mut elems = Vec::new();
        if !matches!(self.peek(), Token::RBrace) {
            loop {
                elems.push(self.parse_struct_element()?);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RBrace, "'}' in packed struct body")?;
        Ok((elems, true))
    }

    /// One struct element, checked where `LLParser::parseStructBody` checks
    /// it — per element, against the element's own location.
    fn parse_struct_element(&mut self) -> ParseResult<Type<'ctx, B>> {
        let elem_loc = self.loc();
        let ty = self.parse_type(false)?;
        if !ty.is_valid_struct_element() {
            return Err(self.message_at(elem_loc, "invalid element type for struct"));
        }
        Ok(ty)
    }

    /// `'(' (ArgType (',' ArgType)* (',' '...')?)? ')'` — mirrors
    /// `LLParser::parseArgumentList`, the one routine upstream shares between
    /// a function *type* and a function *header*.
    ///
    /// Attributes for argument `i` are installed at `AttrIndex::Param(i)` in
    /// `attrs`. `unnamed_arg_nums` collects the numbers upstream hands to
    /// `PerFunctionState`, one entry per argument with no `%name` — a *named*
    /// argument deliberately consumes no number.
    ///
    /// `argument can not have void type` is **not** reachable and so is not
    /// spelled here: upstream reads the type with `AllowVoid = false`, so a
    /// literal `void` is already refused by `parseType` as
    /// `void type only allowed for function results`. Recorded rather than
    /// invented a trigger for.
    fn parse_argument_list(
        &mut self,
        attrs: &mut AttributeStorage,
        unnamed_arg_nums: &mut Vec<u32>,
    ) -> ParseResult<(Vec<ArgInfo<'ctx, B>>, bool)> {
        // Both callers establish the `(` first — upstream records that as
        // `assert(Lex.getKind() == lltok::lparen)`, and the header path's own
        // `tokError` carries this same text.
        self.expect_punct(PunctKind::LParen, "'(' in function argument list")?;
        let mut args: Vec<ArgInfo<'ctx, B>> = Vec::new();
        let mut cur_val_id: u32 = 0;
        let mut is_var_arg = false;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                // `...` at the end of the arg list.
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    is_var_arg = true;
                    break;
                }

                // Otherwise must be an argument type.
                let type_loc = self.loc();
                let ty = self.parse_type(false)?;
                let slot = u32::try_from(args.len()).map_err(|_| ParseError::Expected {
                    expected: "parameter slot fits in u32".into(),
                    loc: DiagLoc::span(type_loc),
                })?;
                let index = AttrIndex::Param(slot);
                let parsed = self.parse_fn_attribute_value_pairs(
                    attrs,
                    index,
                    AttrListContext::ParamOrReturn,
                )?;
                if !parsed.groups.is_empty() {
                    return Err(self.expected("attribute"));
                }

                let name = if let Token::LocalVar(_) = self.peek() {
                    let name = self
                        .current_str_payload()
                        .ok_or_else(|| self.expected("local identifier payload"))?;
                    self.bump()?;
                    // `parseFunctionHeader` sets each argument's name and then
                    // notices the symbol table renamed it. For a function
                    // being built there is nothing else in that table yet, so
                    // the collision is with an earlier argument.
                    if args
                        .iter()
                        .any(|arg: &ArgInfo<'ctx, B>| arg.name.as_deref() == Some(name.as_str()))
                    {
                        return Err(self
                            .message_at(type_loc, format!("redefinition of argument '%{name}'")));
                    }
                    Some(name)
                } else {
                    let arg_id = if let Token::LocalVarId(id) = self.peek() {
                        let id = *id;
                        // Reported at the *type*, which is upstream's
                        // `TypeLoc`, not at the `%N` token itself.
                        check_value_id("argument", "%", cur_val_id, id, type_loc)?;
                        self.bump()?;
                        id
                    } else {
                        cur_val_id
                    };
                    unnamed_arg_nums.push(arg_id);
                    cur_val_id = arg_id.saturating_add(1);
                    None
                };

                if !ty.is_valid_function_argument() {
                    return Err(self.message_at(type_loc, "invalid type for function argument"));
                }

                args.push(ArgInfo {
                    loc: type_loc,
                    ty,
                    name,
                    has_attributes: has_attributes_at(attrs, index),
                });

                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' at end of argument list")?;
        Ok((args, is_var_arg))
    }

    /// `T (params...)` — mirrors `LLParser::parseFunctionType`. The opening
    /// `(` is the lookahead that triggered this arm.
    fn parse_function_type_after_return(
        &mut self,
        ret: Type<'ctx, B>,
    ) -> ParseResult<Type<'ctx, B>> {
        if !ret.is_valid_function_return() {
            return Err(self.message("invalid function return type"));
        }
        // Upstream shares `parseArgumentList` here, so attributes and a name
        // *parse* and are rejected afterwards — which is why these two
        // messages exist at all. The scratch storage is discarded: a function
        // type carries no attributes, and anything landing in it is exactly
        // what the first rejection reports.
        let mut scratch = AttributeStorage::new();
        let mut unnamed_arg_nums = Vec::new();
        let (args, var_args) = self.parse_argument_list(&mut scratch, &mut unnamed_arg_nums)?;

        // Reject names on the arguments lists.
        for arg in &args {
            if arg.name.is_some() {
                return Err(self.message_at(arg.loc, "argument name invalid in function type"));
            }
            if arg.has_attributes {
                return Err(
                    self.message_at(arg.loc, "argument attributes invalid in function type")
                );
            }
        }

        let params: Vec<Type<'ctx, B>> = args.iter().map(|arg| arg.ty).collect();
        let fn_ty = function_type_with_variadic(self.module, ret, params, var_args);
        Ok(fn_ty.as_type())
    }

    fn primitive_to_type(&self, p: PrimitiveTy, loc: Span) -> ParseResult<Type<'ctx, B>> {
        let m = self.module;
        match p {
            PrimitiveTy::Void => Ok(m.void_type().as_type()),
            PrimitiveTy::Label => Ok(m.label_type().as_type()),
            PrimitiveTy::Metadata => Ok(m.metadata_type().as_type()),
            PrimitiveTy::Token => Ok(m.token_type().as_type()),
            PrimitiveTy::X86Amx => Ok(m.x86_amx_type()),
            PrimitiveTy::WasmExnRef => Ok(m.wasm_exnref_type()),
            PrimitiveTy::Half => Ok(m.half_type().as_type()),
            PrimitiveTy::Bfloat => Ok(m.bfloat_type().as_type()),
            PrimitiveTy::Float => Ok(m.f32_type().as_type()),
            PrimitiveTy::Double => Ok(m.f64_type().as_type()),
            PrimitiveTy::X86Fp80 => Ok(m.x86_fp80_type().as_type()),
            PrimitiveTy::Fp128 => Ok(m.fp128_type().as_type()),
            PrimitiveTy::PpcFp128 => Ok(m.ppc_fp128_type().as_type()),
            PrimitiveTy::Ptr => Ok(m.ptr_type(0).as_type()),
            PrimitiveTy::Integer(n) => m
                .custom_width_int_type(n.get())
                .map(|t| t.as_type())
                .map_err(|_| ParseError::IntegerWidthOutOfRange {
                    width: u64::from(n.get()),
                    // The bound the check actually applies — not a literal
                    // that can drift away from it, as `(1 << 24) - 1` had.
                    max: llvmkit_ir::MAX_INT_BITS,
                    loc: DiagLoc::span(loc),
                }),
        }
    }

    /// Mirrors the `lltok::LocalVar` arm of `LLParser::parseType`: an
    /// undefined `%name` becomes an opaque identified struct on the spot, and
    /// the reference's location is remembered so `validateEndOfModule` can
    /// blame it if no definition ever arrives.
    fn lookup_or_forward_named_type(&mut self, name: &str, loc: Span) -> Type<'ctx, B> {
        if let Some(entry) = self.named_types.get(name) {
            return entry.ty;
        }
        let st = self.module.get_or_insert_named_struct(name);
        self.named_types.insert(
            name.to_owned(),
            TypeEntry {
                ty: st.as_type(),
                forward_ref_loc: Some(loc),
            },
        );
        st.as_type()
    }

    /// The `lltok::LocalVarID` twin of [`Self::lookup_or_forward_named_type`],
    /// mirroring upstream's `StructType::create(Context)` with no name.
    ///
    /// An *anonymous identified* struct, not a literal one: `%0 = type {i32}`
    /// and `%1 = type {i32}` must stay distinct types, and a forward-referenced
    /// `%0` has to be the same type as the `%0` its definition later fills in.
    /// A literal empty struct could be neither.
    fn lookup_or_forward_numbered_type(&mut self, id: u32, loc: Span) -> Type<'ctx, B> {
        if let Some(entry) = self.numbered_types.get(&id) {
            return entry.ty;
        }
        let st = self.module.anonymous_identified_struct();
        self.numbered_types.insert(
            id,
            TypeEntry {
                ty: st.as_type(),
                forward_ref_loc: Some(loc),
            },
        );
        st.as_type()
    }

    // ── Globals ──────────────────────────────────────────────────────────

    /// Dispatch for `@name = ...` / `@N = ...`. Routes to the global form
    /// currently supported (constructive subset: simple `@x = global
    /// TY CONST` / `@x = constant TY CONST` with optional `external`
    /// linkage). Function-level forms (`@x = ... declare ...`) are handled
    /// by [`Parser::parse_declare`] from the top-level dispatcher when the
    /// leading keyword is `declare` rather than a global identifier.
    fn parse_global_or_function(&mut self) -> ParseResult<()> {
        let (name_id, decl_loc) = match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("global identifier"))?;
                let loc = self.loc();
                self.bump()?;
                (NameOrId::Name(name), loc)
            }
            Token::GlobalId(n) => {
                let id = *n;
                let loc = self.loc();
                // `parseUnnamedGlobal`'s `checkValueID(NameLoc, "global",
                // "@", ...)`. Without it the collision reached
                // `NumberedValues::add` instead and surfaced as llvmkit's own
                // `invalid slot id 5: next unused is 11`.
                check_value_id(
                    "global",
                    "@",
                    self.numbered_globals.next_unused_id(),
                    id,
                    loc,
                )?;
                self.bump()?;
                (NameOrId::Id(id), loc)
            }
            _ => return Err(self.expected("global identifier")),
        };
        self.expect_punct(PunctKind::Equal, "'=' after global name")?;

        let (linkage, has_linkage) = match self.peek() {
            Token::Kw(keyword) => match linkage_keyword(*keyword) {
                Some(linkage) => {
                    self.bump()?;
                    (linkage, true)
                }
                None => (Linkage::External, false),
            },
            _ => (Linkage::External, false),
        };
        let (dso_locality, visibility, dll_storage_class) =
            self.parse_optional_preemption_visibility_dll()?;
        let thread_local_mode = if self.eat_keyword(Keyword::ThreadLocal)? {
            if self.eat_punct(PunctKind::LParen)? {
                let mode = if self.eat_keyword(Keyword::Localdynamic)? {
                    ThreadLocalMode::LocalDynamic
                } else if self.eat_keyword(Keyword::Initialexec)? {
                    ThreadLocalMode::InitialExec
                } else if self.eat_keyword(Keyword::Localexec)? {
                    ThreadLocalMode::LocalExec
                } else {
                    // `parseTLSModel`'s only message.
                    return Err(self.expected("localdynamic, initialexec or localexec"));
                };
                // `thread local`, unhyphenated, is upstream's spelling.
                self.expect_punct(PunctKind::RParen, "')' after thread local model")?;
                mode
            } else {
                ThreadLocalMode::GeneralDynamic
            }
        } else {
            ThreadLocalMode::NotThreadLocal
        };
        let unnamed_addr = if self.eat_keyword(Keyword::UnnamedAddr)? {
            UnnamedAddr::Global
        } else if self.eat_keyword(Keyword::LocalUnnamedAddr)? {
            UnnamedAddr::Local
        } else {
            UnnamedAddr::None
        };
        if matches!(
            self.peek(),
            Token::Kw(Keyword::Alias) | Token::Kw(Keyword::Ifunc)
        ) {
            return self.parse_alias_or_ifunc(
                name_id,
                decl_loc,
                ParsedAliasHeader {
                    linkage,
                    dso_locality,
                    visibility,
                    dll_storage_class,
                    thread_local_mode,
                    unnamed_addr,
                },
            );
        }

        let address_space = if self.eat_keyword(Keyword::Addrspace)? {
            self.parse_addr_space_paren()?
        } else {
            0
        };
        let externally_initialized = self.eat_keyword(Keyword::ExternallyInitialized)?;

        // `parseGlobal` opens with the same pair `parseAliasOrIFunc` does.
        // llvmkit checked aliases only, so `@g = private hidden global i32 0`
        // was accepted here while its alias twin was rejected.
        Self::check_linkage_agreement(linkage, visibility, dll_storage_class, decl_loc)?;

        let is_constant = if self.eat_keyword(Keyword::Global)? {
            false
        } else if self.eat_keyword(Keyword::Constant)? {
            true
        } else {
            return Err(self.expected("'global' or 'constant' after linkage"));
        };

        let type_loc = self.loc();
        let ty = self.parse_type(false)?;
        // `if (!HasLinkage || !isValidDeclarationLinkage(Linkage))` — upstream
        // simply does not *look* for an initializer when the linkage says the
        // global is a declaration. It does not parse one and reject it, and it
        // has no lookahead: `@g = external global i32 0` leaves the `0`
        // unconsumed, and the token fails at top level.
        //
        // llvmkit used to peek with `starts_global_initializer` and report an
        // invented `no initializer: a global with 'external' linkage is a
        // declaration`. Same rejection, different message and a guess in place
        // of a rule.
        let initializer = if has_linkage && is_declaration_linkage(linkage) {
            None
        } else {
            self.parse_constant(ty)?
        };
        // Checked *after* the initializer, as upstream does, and anchored at
        // the type. `PointerType::isValidElementType` is the second half:
        // a global's value type is the pointee of its own `ptr`.
        if ty.is_function() || !ty.is_valid_pointer_element() {
            return Err(self.message_at(type_loc, "invalid type for global variable"));
        }
        let mut section = None;
        let mut partition = None;
        let mut align = MaybeAlign::NONE;
        let mut comdat_name = None;
        let mut metadata = Vec::new();
        let mut code_model = None;
        let mut sanitizer: Option<llvmkit_ir::SanitizerMetadata> = None;
        while self.eat_punct(PunctKind::Comma)? {
            if self.eat_keyword(Keyword::Section)? {
                section = Some(self.parse_string_constant("section name")?);
            } else if self.eat_keyword(Keyword::Partition)? {
                partition = Some(self.parse_string_constant("partition name")?);
            } else if matches!(self.peek(), Token::Kw(Keyword::CodeModel)) {
                code_model = Some(self.parse_optional_code_model()?);
            } else if let Some(update) = self.sanitizer_for_token(self.peek()) {
                // `parseSanitizer` merges into whatever the global already
                // carries, so the four keywords accumulate rather than
                // replacing one another.
                self.bump()?;
                sanitizer = Some(update(sanitizer.unwrap_or_default()));
            } else if matches!(self.peek(), Token::Kw(Keyword::Align)) {
                align = MaybeAlign::new(self.parse_align_val()?);
            } else if self.eat_keyword(Keyword::Comdat)? {
                let name = if self.eat_punct(PunctKind::LParen)? {
                    let name = match self.peek() {
                        Token::ComdatVar(bytes) => std::str::from_utf8(bytes.as_ref())
                            .map_err(|_| self.expected("valid UTF-8 comdat name"))?
                            .to_owned(),
                        _ => return Err(self.expected("comdat variable")),
                    };
                    self.bump()?;
                    self.expect_punct(PunctKind::RParen, "')' after comdat var")?;
                    name
                } else {
                    // Bare `comdat` borrows the global's own name
                    // (`LLParser::parseOptionalComdat`). Only an *unnamed*
                    // global has no name to borrow, which is the one case
                    // upstream rejects — llvmkit used to reject the whole
                    // bare form.
                    match &name_id {
                        NameOrId::Name(n) => n.clone(),
                        NameOrId::Id(_) => {
                            return Err(self.message("comdat cannot be unnamed"));
                        }
                    }
                };
                comdat_name = Some(name);
            } else if matches!(self.peek(), Token::MetadataVar(_)) {
                metadata.push(self.parse_named_metadata_attachment()?);
            } else {
                // `parseGlobal`'s property loop falls through to
                // `parseOptionalComdat` and, when that finds no comdat,
                // reports this — bang included.
                return Err(self.message("unknown global variable property!"));
            }
        }

        let name_string = match &name_id {
            NameOrId::Name(n) => n.clone(),
            NameOrId::Id(_) => String::new(),
        };
        // `else if (M->getNamedValue(Name))` — a name already in the module is
        // a redefinition *unless* it is only there as a forward reference,
        // which this definition satisfies. Without this, the collision reached
        // the builder instead and surfaced as
        // `expected valid global definition: a global named "g" already
        // exists in this module`, so upstream's message was unreachable.
        if !name_string.is_empty()
            && !self.forward_ref_globals.contains_key(&name_string)
            && self.module.global(&name_string).is_some()
        {
            return Err(ParseError::Redefinition {
                kind: SymbolKind::Global,
                id: SymbolId::Named(name_string),
                loc: DiagLoc::span(decl_loc),
            });
        }
        let mut builder = self
            .module
            .global_builder(&name_string, ty)
            .linkage(linkage)
            .dso_locality(dso_locality)
            .visibility(visibility)
            .dll_storage_class(dll_storage_class)
            .thread_local_mode(thread_local_mode)
            .unnamed_addr(unnamed_addr)
            .address_space(address_space)
            .align(align);
        if externally_initialized {
            builder = builder.externally_initialized();
        }
        if is_constant {
            builder = builder.constant();
        }
        if let Some(c) = initializer {
            builder = builder.initializer(c);
        }
        if let Some(s) = section {
            builder = builder.section(s);
        }
        if let Some(p) = partition {
            builder = builder.partition(p);
        }
        if let Some(name) = comdat_name {
            let comdat = self.comdat_ref(&name, decl_loc);
            builder = builder.comdat(comdat);
        }
        let g = builder.build().map_err(|e| ParseError::Expected {
            expected: format!("valid global definition: {e}").into(),
            loc: DiagLoc::span(decl_loc),
        })?;
        // The parser threads borrowing handles through its deferred-fixup and
        // slot-numbering tables, so resolve the freshly minted id once here.
        let g = self.module.view(g);
        if let Some(model) = code_model {
            g.set_code_model(self.module, model);
        }
        if let Some(metadata) = sanitizer {
            g.set_sanitizer_metadata(self.module, metadata);
        }
        for (kind, id) in metadata {
            own_metadata(g.set_metadata(self.module, kind, id));
        }
        if let NameOrId::Id(id) = name_id {
            self.numbered_globals
                .add(id, GlobalRef::Variable(g))
                .map_err(|source| ParseError::InvalidSlotId {
                    source,
                    loc: DiagLoc::span(decl_loc),
                })?;
        }
        Ok(())
    }

    /// Parse a constant for use as a global initializer. Supports integer
    /// scalars, zeroinitializer, null, and aggregate constants for
    /// arrays/vectors/structs whose element fields all carry type tags.
    fn parse_alias_or_ifunc(
        &mut self,
        name_id: NameOrId,
        decl_loc: Span,
        header: ParsedAliasHeader,
    ) -> ParseResult<()> {
        let linkage = header.linkage;
        let dso_locality = header.dso_locality;
        let visibility = header.visibility;
        let dll_storage_class = header.dll_storage_class;
        let thread_local_mode = header.thread_local_mode;
        let unnamed_addr = header.unnamed_addr;
        let is_alias = if self.eat_keyword(Keyword::Alias)? {
            true
        } else if self.eat_keyword(Keyword::Ifunc)? {
            false
        } else {
            return Err(self.expected("'alias' or 'ifunc'"));
        };

        if is_alias && !llvmkit_ir::global_alias::is_valid_alias_linkage(linkage) {
            return Err(ParseError::Message {
                message: "invalid linkage type for alias".into(),
                loc: DiagLoc::span(decl_loc),
            });
        }
        // No ifunc counterpart: `parseAliasOrIFunc` guards `isValidLinkage`
        // with `if (IsAlias && ...)`, so an ifunc with a bad linkage parses
        // here and is rejected by the verifier
        // (`VerifierRule::IfuncInvalidLinkage`). llvmkit rejecting it at parse
        // time was stricter than upstream, which is a divergence in its own
        // right.
        Self::check_linkage_agreement(linkage, visibility, dll_storage_class, decl_loc)?;

        let value_type = self.parse_type(false)?;
        self.expect_punct(PunctKind::Comma, "comma after alias or ifunc's type")?;
        // `AliaseeLoc`, captured before the aliasee is read — upstream anchors
        // both the pointer-type check and `invalid aliasee` here.
        let aliasee_loc = self.loc();
        // `parseAliasOrIFunc`'s first-token branch. Four constant-expression
        // keywords name an aliasee that types *itself* and go through a bare
        // `parseValID` — upstream's comment: "The bitcast dest type is not
        // present, it is implied by the dest type". Everything else is
        // TYPE VALUE, read by `parseGlobalTypeAndValue`.
        let self_typed_aliasee = matches!(
            self.peek(),
            Token::Instruction(
                Opcode::BitCast | Opcode::GetElementPtr | Opcode::AddrSpaceCast | Opcode::IntToPtr
            )
        );
        let (target, forward_target, target_loc) = if self_typed_aliasee {
            let id = self.parse_val_id(None, None)?;
            let loc = id.loc;
            // `if (ID.Kind != ValID::t_Constant) return error(AliaseeLoc,
            // "invalid aliasee");` — ported for the routine's shape, though it
            // is defensive on both sides: each of the four `parseValID` arms
            // this branch can reach ends in `ID.Kind = ValID::t_Constant`, so
            // nothing upstream reaches the message either.
            let ValIdKind::Constant(constant) = id.kind else {
                return Err(self.message_at(aliasee_loc, "invalid aliasee"));
            };
            (constant, None, loc)
        } else {
            let written_ty = self.parse_type(false)?;
            let loc = self.loc();
            // A forward-referenced target becomes a null placeholder patched at
            // end of module, exactly as `personality` already handles the same
            // ordering problem.
            match self.parse_alias_target(written_ty) {
                Ok(c) => (c, None, loc),
                Err(ParseError::UndefinedSymbol {
                    id: SymbolId::Named(name),
                    ..
                }) => {
                    let AnyTypeEnum::Pointer(pty) = written_ty.into_type_enum() else {
                        return Err(self.expected("pointer type for alias or ifunc target"));
                    };
                    (pty.const_null().as_constant(), Some(name), loc)
                }
                Err(other) => return Err(other),
            }
        };
        // `Type *AliaseeType = Aliasee->getType(); auto *PTy =
        // dyn_cast<PointerType>(AliaseeType);` — the check and the address
        // space both come off the aliasee **value's** type, after it is read,
        // never off a type written ahead of it.
        let target_ty = target.ty();
        if !matches!(target_ty.into_type_enum(), AnyTypeEnum::Pointer(_)) {
            return Err(self.message_at(aliasee_loc, "An alias or ifunc must have pointer type"));
        }

        let mut partition = None;
        let mut ifunc_metadata = Vec::new();
        while self.eat_punct(PunctKind::Comma)? {
            if self.eat_keyword(Keyword::Partition)? {
                partition = Some(self.parse_string_constant("partition name")?);
            } else if !is_alias && matches!(self.peek(), Token::MetadataVar(_)) {
                // `} else if (!IsAlias && Lex.getKind() == lltok::MetadataVar)`
                // — an **ifunc** may carry metadata attachments and an alias
                // may not, so `@a = alias i32, ptr @g, !dbg !0` stays the
                // property error while the ifunc spelling parses.
                ifunc_metadata.push(self.parse_named_metadata_attachment()?);
            } else {
                // `parseAliasOrIFunc`'s property loop, bang included. It was
                // routed through `expected`, which rendered
                // `expected unknown alias or ifunc property` — the word
                // "expected" glued onto a message that is not one.
                return Err(self.message("unknown alias or ifunc property!"));
            }
        }

        let name_string = match &name_id {
            NameOrId::Name(n) => n.clone(),
            NameOrId::Id(_) => String::new(),
        };

        if is_alias {
            let mut builder = self
                .module
                .alias_builder(&name_string, value_type, target)
                .linkage(linkage)
                .dso_locality(dso_locality)
                .visibility(visibility)
                .dll_storage_class(dll_storage_class)
                .thread_local_mode(thread_local_mode)
                .unnamed_addr(unnamed_addr);
            if let Some(p) = partition {
                builder = builder.partition(p);
            }
            let a = builder.build().map_err(|e| ParseError::Expected {
                expected: format!("valid alias definition: {e}").into(),
                loc: DiagLoc::span(decl_loc),
            })?;
            let a_view = self.module.view(a);
            if let Some(name) = forward_target {
                self.deferred_alias_targets.push(DeferredAliasTarget {
                    object: DeferredAliasObject::Alias(a_view),
                    name,
                    ty: target_ty,
                    loc: target_loc,
                });
            }
            if let NameOrId::Id(id) = name_id {
                let a = a_view;
                self.numbered_globals
                    .add(id, GlobalRef::Alias(a))
                    .map_err(|source| ParseError::InvalidSlotId {
                        source,
                        loc: DiagLoc::span(decl_loc),
                    })?;
            }
        } else {
            let mut builder = self
                .module
                .ifunc_builder(&name_string, value_type, target)
                .linkage(linkage)
                .dso_locality(dso_locality)
                .visibility(visibility)
                // Upstream applies all three to `GV` in the ifunc branch too —
                // it simply never *prints* them, because `printIFunc` stops
                // after visibility. Storing them keeps the model faithful.
                .dll_storage_class(dll_storage_class)
                .thread_local_mode(thread_local_mode)
                .unnamed_addr(unnamed_addr);
            if let Some(p) = partition {
                builder = builder.partition(p);
            }
            let i = builder.build().map_err(|e| ParseError::Expected {
                expected: format!("valid ifunc definition: {e}").into(),
                loc: DiagLoc::span(decl_loc),
            })?;
            let i_view = self.module.view(i);
            for (kind, id) in ifunc_metadata {
                own_metadata(i_view.set_metadata(self.module, kind, id));
            }
            if let Some(name) = forward_target {
                self.deferred_alias_targets.push(DeferredAliasTarget {
                    object: DeferredAliasObject::Ifunc(i_view),
                    name,
                    ty: target_ty,
                    loc: target_loc,
                });
            }
            if let NameOrId::Id(id) = name_id {
                let i = i_view;
                self.numbered_globals
                    .add(id, GlobalRef::Ifunc(i))
                    .map_err(|source| ParseError::InvalidSlotId {
                        source,
                        loc: DiagLoc::span(decl_loc),
                    })?;
            }
        }
        Ok(())
    }

    fn parse_alias_target(
        &mut self,
        target_ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.parse_constant(target_ty)?
            .ok_or_else(|| self.expected("alias or ifunc target constant"))
    }

    fn parse_constant(
        &mut self,
        dst: Type<'ctx, B>,
    ) -> ParseResult<Option<llvmkit_ir::Constant<'ctx, B>>> {
        self.parse_global_value(dst).map(Some)
    }

    fn unsupported_constant_value_form_at(&self, loc: Span) -> ParseError {
        ParseError::Expected {
            expected: "supported constant/value form".into(),
            loc: DiagLoc::span(loc),
        }
    }

    fn unsupported_constant_expr_at(&self, loc: Span, op: Opcode) -> ParseError {
        let opcode = match op {
            Opcode::ExtractValue => "extractvalue",
            Opcode::InsertValue => "insertvalue",
            Opcode::Udiv => "udiv",
            Opcode::Sdiv => "sdiv",
            Opcode::Urem => "urem",
            Opcode::Srem => "srem",
            Opcode::Fadd => "fadd",
            Opcode::Fsub => "fsub",
            Opcode::Fmul => "fmul",
            Opcode::Fdiv => "fdiv",
            Opcode::Frem => "frem",
            Opcode::And => "and",
            Opcode::Or => "or",
            Opcode::Lshr => "lshr",
            Opcode::Ashr => "ashr",
            Opcode::Shl => "shl",
            Opcode::Mul => "mul",
            Opcode::Fneg => "fneg",
            Opcode::Select => "select",
            Opcode::Zext => "zext",
            Opcode::Sext => "sext",
            Opcode::FpTrunc => "fptrunc",
            Opcode::FpExt => "fpext",
            Opcode::UiToFp => "uitofp",
            Opcode::SiToFp => "sitofp",
            Opcode::FpToUi => "fptoui",
            Opcode::FpToSi => "fptosi",
            Opcode::Icmp => "icmp",
            Opcode::Fcmp => "fcmp",
            _ => return self.unsupported_constant_value_form_at(loc),
        };
        ParseError::Message {
            message: format!("{opcode} constexprs are no longer supported").into(),
            loc: DiagLoc::span(loc),
        }
    }

    /// `LLParser::parseValID`, whose first statement is
    /// `ID.Loc = Lex.getLoc();`. The location is recorded here, before any
    /// token is consumed, so it survives everything the body goes on to read.
    fn parse_val_id(
        &mut self,
        pfs: Option<&PerFunctionState<'ctx, B>>,
        expected_ty: Option<Type<'ctx, B>>,
    ) -> ParseResult<ValId<'ctx, B>> {
        let loc = self.loc();
        let kind = self.parse_val_id_kind(pfs, expected_ty)?;
        Ok(ValId { kind, loc })
    }

    /// The `switch (Lex.getKind())` half of `parseValID`. Split out only so
    /// that [`Self::parse_val_id`] can record `ID.Loc` around it; every arm is
    /// upstream's.
    fn parse_val_id_kind(
        &mut self,
        pfs: Option<&PerFunctionState<'ctx, B>>,
        expected_ty: Option<Type<'ctx, B>>,
    ) -> ParseResult<ValIdKind<'ctx, B>> {
        let loc = self.loc();
        match self.peek() {
            Token::Kw(Keyword::Asm) => {
                // Upstream builds a `t_InlineAsm` `ValID` here and rejects it
                // in `convertValIDToValue` when `ID.FTy` is null — that is,
                // everywhere except a call callee, where `parseCall` supplies
                // the signature. llvmkit's callee path has its own arm, so
                // reaching `parseValID` with `asm` *is* the null-`FTy` case;
                // the verdict and the text are upstream's, one layer earlier.
                let asm = self.parse_inline_asm()?;
                return Err(ParseError::Message {
                    message: "invalid type for inline asm constraint string".into(),
                    loc: DiagLoc::span(asm.loc),
                });
            }
            Token::Instruction(op) if !is_supported_constant_expr_opcode(*op) => {
                return Err(self.unsupported_constant_expr_at(loc, *op));
            }
            _ => {}
        }

        match self.peek() {
            // No `PerFunctionState` gate here, deliberately: upstream's
            // `parseValID` records `t_LocalName` / `t_LocalID` untyped and
            // unresolved, and `convertValIDToValue` is where a null `PFS`
            // becomes `invalid use of function-local name`. Rejecting early
            // made that message unreachable, and made `parseUseListOrderBB`
            // — which reads both its operands with `parseValID(…, nullptr)`
            // precisely so it can inspect `ID.Kind` itself — impossible to
            // port.
            Token::LocalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("local SSA name"))?;
                self.bump()?;
                Ok(ValIdKind::LocalName(name))
            }
            Token::LocalVarId(id) => {
                let id = *id;
                self.bump()?;
                Ok(ValIdKind::LocalId(id))
            }
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("global variable name"))?;
                self.bump()?;
                Ok(ValIdKind::GlobalName(name))
            }
            Token::GlobalId(id) => {
                let id = *id;
                self.bump()?;
                Ok(ValIdKind::GlobalId(id))
            }
            Token::IntegerLit(_) => self.parse_int_literal().map(ValIdKind::ApsInt),
            Token::FloatLit(_) => self.parse_fp_literal().map(ValIdKind::ApFloat),
            Token::Kw(Keyword::True) => {
                let ty = expected_ty.ok_or_else(|| self.expected("i1 type for boolean literal"))?;
                if ty != self.module.i1_type().as_type() {
                    return Err(self.expected("i1 type for boolean literal"));
                }
                self.bump()?;
                Ok(ValIdKind::ApsInt(ParsedApsInt {
                    value: ApInt::from_words(1, &[1]),
                    signedness: Signedness::Unsigned,
                }))
            }
            Token::Kw(Keyword::False) => {
                let ty = expected_ty.ok_or_else(|| self.expected("i1 type for boolean literal"))?;
                if ty != self.module.i1_type().as_type() {
                    return Err(self.expected("i1 type for boolean literal"));
                }
                self.bump()?;
                Ok(ValIdKind::ApsInt(ParsedApsInt {
                    value: ApInt::zero(1),
                    signedness: Signedness::Unsigned,
                }))
            }
            Token::Kw(Keyword::Null) => {
                self.bump()?;
                Ok(ValIdKind::Null)
            }
            Token::Kw(Keyword::Zeroinitializer) => {
                self.bump()?;
                Ok(ValIdKind::Zero)
            }
            Token::Kw(Keyword::Undef) => {
                self.bump()?;
                Ok(ValIdKind::Undef)
            }
            Token::Kw(Keyword::Poison) => {
                self.bump()?;
                Ok(ValIdKind::Poison)
            }
            Token::Kw(Keyword::None) => {
                let ty = expected_ty.ok_or_else(|| self.expected("type for none constant"))?;
                self.bump()?;
                match ty.into_type_enum() {
                    AnyTypeEnum::Token(_) => Ok(ValIdKind::Constant(self.module.token_none())),
                    _ => Err(self.message("invalid type for none constant")),
                }
            }
            Token::LSquare => {
                // `LLParser::parseValID`'s `lsquare` arm. The array type comes
                // from element 0, never from the demanded type.
                self.bump()?;
                let first_elt_loc = self.loc();
                let values = self.parse_global_value_vector()?;
                self.expect_punct(PunctKind::RSquare, "end of array constant")?;
                if values.is_empty() {
                    // Upstream's `t_EmptyArray`: with no elements there is no
                    // element type to derive, so the check is deferred.
                    return Ok(ValIdKind::EmptyArray);
                }
                self.check_aggregate_elements(&values, "array", first_elt_loc)?;
                let element_ty = values[0].ty();
                let len = u64::try_from(values.len()).unwrap_or(u64::MAX);
                let c = self
                    .module
                    .array_type(element_ty, len)
                    .const_array(values)
                    .map_err(|e| self.builder_err("array constant", e))?;
                Ok(ValIdKind::Constant(c.as_constant()))
            }
            Token::Less => {
                // `LLParser::parseValID`'s `less` arm: `<{ ... }>` is a packed
                // struct, `< ... >` a vector.
                self.bump()?;
                let is_packed_struct = self.eat_punct(PunctKind::LBrace)?;
                let first_elt_loc = self.loc();
                let values = self.parse_global_value_vector()?;
                if is_packed_struct {
                    self.expect_punct(PunctKind::RBrace, "end of packed struct")?;
                }
                self.expect_punct(PunctKind::Greater, "end of constant")?;
                if is_packed_struct {
                    return Ok(ValIdKind::PackedConstantStruct(values));
                }
                if values.is_empty() {
                    return Err(self.message("constant vector must not be empty"));
                }
                let element_ty = values[0].ty();
                if !element_ty.is_integer()
                    && !element_ty.is_floating_point()
                    && !element_ty.is_pointer()
                {
                    return Err(self.message_at(
                        first_elt_loc,
                        "vector elements must have integer, pointer or floating point type",
                    ));
                }
                self.check_aggregate_elements(&values, "vector", first_elt_loc)?;
                let len = u32::try_from(values.len()).unwrap_or(u32::MAX);
                let c = self
                    .module
                    .vector_type(element_ty, len)
                    .const_vector(values)
                    .map_err(|e| self.builder_err("vector constant", e))?;
                Ok(ValIdKind::Constant(c.as_constant()))
            }
            Token::LBrace => {
                // `LLParser::parseValID`'s `lbrace` arm. Every check against
                // the demanded struct type is `convertValIDToValue`'s.
                self.bump()?;
                let values = self.parse_global_value_vector()?;
                self.expect_punct(PunctKind::RBrace, "end of struct constant")?;
                Ok(ValIdKind::ConstantStruct(values))
            }
            Token::Kw(Keyword::C) => {
                // `ConstantDataArray::getString` always builds `[N x i8]`;
                // agreement with the demanded type is `convertValIDToValue`'s
                // job. Deriving the array type from the *expected* type
                // instead silently accepted `[4 x i32] c"abcd"`.
                self.bump()?;
                let bytes: Vec<u8> = match self.peek() {
                    Token::StringConstant(b) => b.as_ref().to_vec(),
                    _ => return Err(self.expected("string")),
                };
                self.bump()?;
                let i8_ty = self.module.i8_type();
                let values: Vec<_> = bytes
                    .iter()
                    .map(|byte| i8_ty.const_int(*byte).as_constant())
                    .collect();
                let c = self
                    .module
                    .array_type(i8_ty, u64::try_from(values.len()).unwrap_or(u64::MAX))
                    .const_array(values)
                    .map_err(|e| self.builder_err("c\"...\" constant", e))?;
                Ok(ValIdKind::Constant(c.as_constant()))
            }
            Token::Kw(Keyword::Blockaddress) => {
                let ty =
                    expected_ty.ok_or_else(|| self.expected("pointer type for blockaddress"))?;
                self.parse_blockaddress_constant(loc, ty, pfs)
                    .map(ValIdKind::Constant)
            }
            Token::Kw(Keyword::DsoLocalEquivalent) => self
                .parse_dso_local_equivalent_constant()
                .map(ValIdKind::Constant),
            Token::Kw(Keyword::NoCfi) => self.parse_no_cfi_constant().map(ValIdKind::Constant),
            Token::MetadataVar(_) => {
                let ty = expected_ty.ok_or_else(|| self.expected("metadata operand type"))?;
                if !ty.is_metadata() {
                    return Err(self.expected("`metadata` type for a metadata operand"));
                }
                Ok(ValIdKind::Value(self.parse_metadata_value_operand(pfs)?))
            }
            Token::Exclaim => {
                let ty = expected_ty.ok_or_else(|| self.expected("metadata operand type"))?;
                if !ty.is_metadata() {
                    return Err(self.expected("`metadata` type for a metadata operand"));
                }
                Ok(ValIdKind::Value(self.parse_metadata_value_operand(pfs)?))
            }
            Token::Kw(Keyword::Ptrauth) => self.parse_ptrauth_constant().map(ValIdKind::Constant),
            Token::Kw(Keyword::Splat) => {
                self.expect_keyword(Keyword::Splat, "'splat'")?;
                self.expect_punct(PunctKind::LParen, "'(' in splat constant")?;
                let scalar = self.parse_global_type_and_value()?;
                self.expect_punct(PunctKind::RParen, "')' in splat constant")?;
                Ok(ValIdKind::ConstantSplat(scalar))
            }
            // No `expected_ty` here, deliberately: upstream's constexpr arms
            // are reached from `parseValID(ID, /*PFS=*/nullptr)` with no type
            // in hand at all — that is how `parseAliasOrIFunc` reads a
            // `bitcast` / `getelementptr` / `addrspacecast` / `inttoptr`
            // aliasee. Demanding one made those four spellings unparseable.
            Token::Instruction(op) if is_supported_constant_expr_opcode(*op) => {
                self.parse_constant_expr().map(ValIdKind::Constant)
            }
            // `LLParser::parseValID`'s default arm.
            _ => Err(self.expected("value token")),
        }
    }

    /// `convertValIDToValue`'s `t_APFloat` arm, in upstream's three steps:
    /// validity, then the narrowing the lexer could not do, then a type
    /// comparison against what the value actually became.
    ///
    /// Upstream builds with `ConstantFP::get(Context, val)`, which types
    /// itself off the *value's* semantics, and compares that type to `Ty`.
    /// Comparing the semantics directly is the same test — two semantics are
    /// the same type exactly when they are equal — and saves mapping a
    /// semantics back to a `FloatType`.
    fn float_literal_constant(
        &self,
        loc: Span,
        ty: Type<'ctx, B>,
        value: ApFloat,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let AnyTypeEnum::Float(float_ty) = ty.into_type_enum() else {
            return Err(self.message_at(loc, "floating point constant invalid for type"));
        };
        if !llvmkit_ir::float_value_is_valid_for_type(ty, &value) {
            return Err(self.message_at(loc, "floating point constant invalid for type"));
        }

        // "The lexer has no type info, so builds all half, bfloat, float, and
        // double FP constants as double. Fix this here. Long double does not
        // need this."
        let value = if value.semantics() == ApFloatSemantics::IeeeDouble
            && matches!(
                float_ty.semantics(),
                ApFloatSemantics::IeeeHalf
                    | ApFloatSemantics::Bfloat
                    | ApFloatSemantics::IeeeSingle
            ) {
            // `convert` quiets a signalling NaN, so upstream manufactures a
            // fresh one from the converted bits afterwards.
            let was_signaling = value.is_signaling();
            let (converted, _, _) =
                value.convert(float_ty.semantics(), RoundingMode::NearestTiesToEven);
            if was_signaling {
                let payload = converted.to_bits();
                ApFloat::snan(
                    converted.semantics(),
                    if converted.is_negative() {
                        ApFloatSign::Negative
                    } else {
                        ApFloatSign::Positive
                    },
                    llvmkit_ir::NanPayload::Bits(&payload),
                )
            } else {
                converted
            }
        } else {
            value
        };

        if value.semantics() != float_ty.semantics() {
            return Err(ParseError::Message {
                message: format!("floating point constant does not have type '{ty}'").into(),
                loc: DiagLoc::span(loc),
            });
        }
        Ok(float_ty
            .const_ap_float(&value)
            .map_err(|e| self.builder_err_at(loc, "float constant", e))?
            .as_constant())
    }

    /// `convertValIDToValue`'s `t_EmptyArray` arm. Upstream materialises
    /// *poison*, not a zero-length array — nothing reads the value, and the
    /// element type is still unknown.
    fn empty_array_constant(
        &self,
        loc: Span,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let is_zero_length_array = match ty.into_type_enum() {
            AnyTypeEnum::Array(array_ty) => array_ty.is_empty(),
            _ => false,
        };
        if !is_zero_length_array {
            return Err(self.message_at(loc, "invalid empty array initializer"));
        }
        Ok(ty.poison().as_constant())
    }

    /// `convertValIDToValue`'s shared `t_ConstantStruct` /
    /// `t_PackedConstantStruct` arm, in upstream's order: element count,
    /// packedness, then per-field type. A demanded type that is not a struct
    /// at all gets the *bare* `constant expression type mismatch` — upstream
    /// words this one without the got/expected suffix its other mismatches
    /// carry.
    fn struct_initializer_constant(
        &self,
        loc: Span,
        ty: Type<'ctx, B>,
        values: &[llvmkit_ir::Constant<'ctx, B>],
        is_packed_initializer: bool,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let AnyTypeEnum::Struct(struct_ty) = ty.into_type_enum() else {
            return Err(self.message_at(loc, "constant expression type mismatch"));
        };
        if struct_ty.field_count() != values.len() {
            return Err(self.message_at(loc, "initializer with struct type has wrong # elements"));
        }
        if struct_ty.is_packed() != is_packed_initializer {
            return Err(self.message_at(loc, "packed'ness of initializer and type don't match"));
        }
        for (index, value) in values.iter().enumerate() {
            let field_ty = struct_ty.field_type(index).ok_or_else(|| {
                self.message_at(loc, "initializer with struct type has wrong # elements")
            })?;
            if value.ty() != field_ty {
                return Err(ParseError::Message {
                    message: format!(
                        "element {index} of struct initializer doesn't match struct element type"
                    )
                    .into(),
                    loc: DiagLoc::span(loc),
                });
            }
        }
        struct_ty
            .const_struct(values.to_vec())
            .map(|c| c.as_constant())
            .map_err(|e| self.builder_err_at(loc, "struct constant", e))
    }

    fn expand_splat_constant(
        &self,
        loc: Span,
        ty: Type<'ctx, B>,
        scalar: llvmkit_ir::Constant<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let AnyTypeEnum::Vector(vec_ty) = ty.into_type_enum() else {
            return Err(self.message_at(loc, "vector constant must have vector type"));
        };
        // Upstream compares against `Ty->getScalarType()`, which for a vector
        // is its element type.
        let scalar_ty = scalar.ty();
        let element_ty = vec_ty.element();
        if scalar_ty != element_ty {
            return Err(ParseError::Message {
                message: format!(
                    "constant expression type mismatch: got type '{scalar_ty}' but expected '{element_ty}'"
                )
                .into(),
                loc: DiagLoc::span(loc),
            });
        }
        let len = usize::try_from(vec_ty.min_len()).map_err(|_| ParseError::Expected {
            expected: "vector type for splat constant".into(),
            loc: DiagLoc::span(loc),
        })?;
        let elements = vec![scalar; len];
        vec_ty
            .const_vector(elements)
            .map(|c| c.as_constant())
            .map_err(|e| self.builder_err_at(loc, "splat constant", e))
    }

    /// Mirrors `Constant::getNullValue`, which `convertValIDToValue`'s
    /// `t_Zero` arm and `parseConstantValue`'s `t_Null` arm both end in.
    ///
    /// llvmkit has no `ConstantAggregateZero`, so the three arms upstream
    /// routes through it build the zero element-wise instead; every other arm
    /// is one upstream `case`. The `_` catch-all stands in for upstream's
    /// `llvm_unreachable` default, which llvmkit reports rather than traps.
    fn zero_initializer_constant(
        &self,
        loc: Span,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        match ty.into_type_enum() {
            AnyTypeEnum::Int(t) => Ok(t.const_zero().as_constant()),
            AnyTypeEnum::Pointer(t) => Ok(t.const_null().as_constant()),
            AnyTypeEnum::Float(t) => Ok(t.const_from_bits(0).as_constant()),
            AnyTypeEnum::Array(t) => {
                let len = usize::try_from(t.len()).map_err(|_| ParseError::Expected {
                    expected: "array zeroinitializer length fits in usize".into(),
                    loc: DiagLoc::span(loc),
                })?;
                let element = t.element();
                let mut elements = Vec::with_capacity(len);
                for _ in 0..len {
                    elements.push(self.zero_initializer_constant(loc, element)?);
                }
                t.const_array(elements)
                    .map(|c| c.as_constant())
                    .map_err(|e| self.builder_err_at(loc, "array zeroinitializer", e))
            }
            AnyTypeEnum::Vector(t) => {
                let len = usize::try_from(t.min_len()).map_err(|_| ParseError::Expected {
                    expected: "vector zeroinitializer length fits in usize".into(),
                    loc: DiagLoc::span(loc),
                })?;
                let element = t.element();
                let mut elements = Vec::with_capacity(len);
                for _ in 0..len {
                    elements.push(self.zero_initializer_constant(loc, element)?);
                }
                t.const_vector(elements)
                    .map(|c| c.as_constant())
                    .map_err(|e| self.builder_err_at(loc, "vector zeroinitializer", e))
            }
            AnyTypeEnum::Struct(t) => {
                if t.is_opaque() {
                    return Err(ParseError::Message {
                        message: "invalid type for null constant".into(),
                        loc: DiagLoc::span(loc),
                    });
                }
                let mut elements = Vec::with_capacity(t.field_count());
                for idx in 0..t.field_count() {
                    let field_ty = t.field_type(idx).ok_or_else(|| {
                        self.expected_at(loc, "struct field type for zeroinitializer")
                    })?;
                    elements.push(self.zero_initializer_constant(loc, field_ty)?);
                }
                t.const_struct(elements)
                    .map(|c| c.as_constant())
                    .map_err(|e| self.builder_err_at(loc, "struct zeroinitializer", e))
            }
            // `case Type::TokenTyID: return ConstantTokenNone::get(...)`. A
            // token type is first-class, is neither a label nor a
            // `TargetExtType`, and so passes both of `convertValIDToValue`'s
            // `t_Zero` guards on the way here; the constant it builds is the
            // one the `token none` spelling builds.
            AnyTypeEnum::Token(_) => Ok(self.module.token_none()),
            // `if (auto *TETy = dyn_cast<TargetExtType>(Ty)) if
            //  (!TETy->hasProperty(TargetExtType::HasZeroInit)) return
            //  error(ID.Loc, "invalid type for null constant");`
            //
            // `Module::target_ext_none` answers with upstream's complete
            // sentence already, so it goes into `ParseError::Message`, which
            // adds nothing — `Expected` prefixed it with `expected `.
            AnyTypeEnum::TargetExt(_) => self.module.target_ext_none(ty).map_err(|e| match e {
                IrError::InvalidOperation { message } => ParseError::Message {
                    message: message.into(),
                    loc: DiagLoc::span(loc),
                },
                other => self.builder_err_at(loc, "target extension none", other),
            }),
            // `default: llvm_unreachable("Cannot create a null constant of
            // that type!")`. The repo bans a runtime panic in a production
            // path, so the arm rejects instead — and it carries the message of
            // the guard that *should* have stopped the type earlier, because
            // that is the only text upstream associates with `t_Zero` at all.
            //
            // What reaches it is what upstream traps on: `metadata`, `x86_amx`
            // and `exnref` pass `convertValIDToValue`'s first-class/label
            // guard and have no `Constant::getNullValue` case. `label` reaches
            // it from neither path — the guard runs on both now. Probed at
            // this commit with `target/release/examples/parse_file.exe` on
            // `@g = global <T> zeroinitializer` and
            // `%v = freeze <T> zeroinitializer` for each of the four.
            _ => Err(self.message_at(loc, "invalid type for null constant")),
        }
    }

    /// Every element of an array or vector constant must have element 0's
    /// type. Mirrors the two agreement loops in `LLParser::parseValID`'s
    /// `lsquare` / `less` arms, which report against the *first* element's
    /// location and number the offender.
    ///
    /// `what` is the noun upstream uses — `array element #N` / `vector
    /// element #N` — so the two loops stay one function without inventing a
    /// third spelling.
    fn check_aggregate_elements(
        &self,
        values: &[llvmkit_ir::Constant<'ctx, B>],
        what: &'static str,
        first_elt_loc: Span,
    ) -> ParseResult<()> {
        let Some(first) = values.first() else {
            return Ok(());
        };
        let element_ty = first.ty();
        if what == "array" && !element_ty.is_first_class() {
            return Err(ParseError::Message {
                message: format!("invalid array element type: {element_ty}").into(),
                loc: DiagLoc::span(first_elt_loc),
            });
        }
        for (index, value) in values.iter().enumerate() {
            if value.ty() != element_ty {
                return Err(ParseError::Message {
                    message: format!("{what} element #{index} is not of type '{element_ty}").into(),
                    loc: DiagLoc::span(first_elt_loc),
                });
            }
        }
        Ok(())
    }

    /// `functions are not values, refer to them as pointers` — the guard at
    /// the very top of `LLParser::convertValIDToValue`, before any `ValID`
    /// arm runs. A function *type* in value position is always this error,
    /// whatever the value turned out to be.
    fn reject_function_typed_value(&self, loc: Span, ty: Type<'ctx, B>) -> ParseResult<()> {
        if matches!(ty.kind(), llvmkit_ir::TypeKind::Function) {
            return Err(self.message_at(loc, "functions are not values, refer to them as pointers"));
        }
        Ok(())
    }

    /// The first-class-and-not-label guard the `t_Undef`, `t_Poison` and
    /// `t_Zero` arms share. Upstream carries a `FIXME` about `LabelTy` being
    /// first-class at all, which is why the label test is separate.
    fn check_undef_like_type(
        &self,
        loc: Span,
        ty: Type<'ctx, B>,
        what: &'static str,
    ) -> ParseResult<()> {
        if !ty.is_first_class() || ty.is_label() {
            return Err(ParseError::Message {
                message: format!("invalid type for {what} constant").into(),
                loc: DiagLoc::span(loc),
            });
        }
        Ok(())
    }

    /// `constant expression type mismatch: got type 'A' but expected 'B'` —
    /// the `ValID::t_Constant` arm of `LLParser::convertValIDToValue`.
    ///
    /// A parsed constant carries its own type; nothing before this point
    /// checks it against the type the context asked for. `blockaddress` is the
    /// common way to reach it, since its type comes from the *function's*
    /// address space rather than the surrounding expression.
    fn checked_constant_type(
        &self,
        loc: Span,
        ty: Type<'ctx, B>,
        constant: llvmkit_ir::Constant<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let got = constant.ty();
        if got != ty {
            return Err(ParseError::Message {
                message: format!(
                    "constant expression type mismatch: got type '{got}' but expected '{ty}'"
                )
                .into(),
                loc: DiagLoc::span(loc),
            });
        }
        Ok(constant)
    }

    /// `LLParser::convertValIDToValue`. Every arm reports at `id.loc` —
    /// upstream's `ID.Loc`, the ValID's own **first** token — because that is
    /// what upstream passes: `getGlobalVal(ID.StrVal, Ty, ID.Loc)`,
    /// `PFS->getVal(ID.UIntVal, Ty, ID.Loc)`,
    /// `error(ID.Loc, "integer constant must have integer type")`, and so on
    /// for every arm. Reporting at `self.loc()` here anchored each of them at
    /// whatever token the lexer had already advanced to, which for a value at
    /// the end of a line is a caret on the *next* line.
    fn convert_val_id_to_value(
        &mut self,
        ty: Type<'ctx, B>,
        id: ValId<'ctx, B>,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let loc = id.loc;
        self.reject_function_typed_value(loc, ty)?;
        match id.kind {
            ValIdKind::LocalName(name) => pfs
                .ok_or_else(|| self.message_at(loc, "invalid use of function-local name"))?
                .get_val(self.module, LocalRef::Named(&name), ty, loc),
            ValIdKind::LocalId(id) => pfs
                .ok_or_else(|| self.message_at(loc, "invalid use of function-local name"))?
                .get_val(self.module, LocalRef::Numbered(id), ty, loc),
            ValIdKind::GlobalName(name) => self.resolve_global_name_as_value(loc, name, ty),
            ValIdKind::GlobalId(id) => self.resolve_global_id_as_value(loc, id, ty),
            ValIdKind::ApsInt(parsed) => {
                let int_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Int(t) => t,
                    _ => {
                        return Err(self.message_at(loc, "integer constant must have integer type"));
                    }
                };
                // `convertValIDToValue`'s `t_APSInt` arm: the literal is
                // widened *or truncated* to the demanded type, so `i8 300` is
                // 44 rather than an error.
                let bits = parsed.extend_or_truncate(int_ty.bit_width());
                let c = int_ty
                    .const_ap_int(&bits)
                    .map_err(|e| self.builder_err_at(loc, "integer constant", e))?;
                Ok(c.as_erased())
            }
            ValIdKind::ApFloat(value) => self
                .float_literal_constant(loc, ty, value)
                .map(|c| c.as_erased()),
            ValIdKind::Null => {
                let pty = match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(t) => t,
                    _ => return Err(self.message_at(loc, "null must be a pointer type")),
                };
                Ok(pty.const_null().as_erased())
            }
            ValIdKind::Zero => {
                self.check_undef_like_type(loc, ty, "null")?;
                self.zero_initializer_constant(loc, ty)
                    .map(|c| c.as_erased())
            }
            ValIdKind::Undef => {
                self.check_undef_like_type(loc, ty, "undef")?;
                Ok(ty.undef().as_erased())
            }
            ValIdKind::Poison => {
                self.check_undef_like_type(loc, ty, "poison")?;
                Ok(ty.poison().as_erased())
            }
            ValIdKind::Constant(c) => self
                .checked_constant_type(loc, ty, c)
                .map(|c| c.as_erased()),
            ValIdKind::ConstantSplat(c) => self
                .expand_splat_constant(loc, ty, c)
                .map(|c| c.as_erased()),
            ValIdKind::EmptyArray => self.empty_array_constant(loc, ty).map(|c| c.as_erased()),
            ValIdKind::ConstantStruct(values) => self
                .struct_initializer_constant(loc, ty, &values, false)
                .map(|c| c.as_erased()),
            ValIdKind::PackedConstantStruct(values) => self
                .struct_initializer_constant(loc, ty, &values, true)
                .map(|c| c.as_erased()),
            ValIdKind::Value(v) => Ok(v),
        }
    }

    /// The constant-only half of `convertValIDToValue`; same anchoring rule as
    /// [`Self::convert_val_id_to_value`].
    fn convert_val_id_to_constant(
        &mut self,
        ty: Type<'ctx, B>,
        id: ValId<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let loc = id.loc;
        self.reject_function_typed_value(loc, ty)?;
        match id.kind {
            // The pointer-type demand is `getGlobalVal`'s own opening guard,
            // so it lives in the resolver — where the *value* spelling of the
            // same arm reaches it too.
            ValIdKind::GlobalName(name) => self.resolve_global_name_as_constant(loc, name, ty),
            ValIdKind::GlobalId(id) => self.resolve_global_id_as_constant(loc, id, ty),
            ValIdKind::ApsInt(parsed) => {
                let int_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Int(t) => t,
                    _ => {
                        return Err(self.message_at(loc, "integer constant must have integer type"));
                    }
                };
                // `convertValIDToValue`'s `t_APSInt` arm: the literal is
                // widened *or truncated* to the demanded type, so `i8 300` is
                // 44 rather than an error.
                let bits = parsed.extend_or_truncate(int_ty.bit_width());
                let c = int_ty
                    .const_ap_int(&bits)
                    .map_err(|e| ParseError::Expected {
                        expected: format!("valid integer constant: {e}").into(),
                        loc: DiagLoc::span(loc),
                    })?;
                Ok(c.as_constant())
            }
            ValIdKind::ApFloat(value) => self.float_literal_constant(loc, ty, value),
            ValIdKind::Null => {
                let ptr_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(t) => t,
                    _ => return Err(self.message_at(loc, "null must be a pointer type")),
                };
                Ok(ptr_ty.const_null().as_constant())
            }
            // `parseConstantValue` routes `t_Zero` through
            // `convertValIDToValue(Ty, ID, V, /*PFS=*/nullptr)`, so the
            // `!Ty->isFirstClassType() || Ty->isLabelTy()` guard runs on the
            // constant path exactly as it does on the value path.
            ValIdKind::Zero => {
                self.check_undef_like_type(loc, ty, "null")?;
                self.zero_initializer_constant(loc, ty)
            }
            ValIdKind::Undef => {
                self.check_undef_like_type(loc, ty, "undef")?;
                Ok(ty.undef().as_constant())
            }
            ValIdKind::Poison => {
                self.check_undef_like_type(loc, ty, "poison")?;
                Ok(ty.poison().as_constant())
            }
            ValIdKind::Constant(c) => self.checked_constant_type(loc, ty, c),
            ValIdKind::ConstantSplat(c) => self.expand_splat_constant(loc, ty, c),
            ValIdKind::EmptyArray => self.empty_array_constant(loc, ty),
            ValIdKind::ConstantStruct(values) => {
                self.struct_initializer_constant(loc, ty, &values, false)
            }
            ValIdKind::PackedConstantStruct(values) => {
                self.struct_initializer_constant(loc, ty, &values, true)
            }
            ValIdKind::LocalId(_) | ValIdKind::LocalName(_) | ValIdKind::Value(_) => {
                Err(self.expected_at(loc, "constant value"))
            }
        }
    }
    fn parse_global_value(
        &mut self,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let id = self.parse_val_id(None, Some(ty))?;
        self.convert_val_id_to_constant(ty, id)
    }

    fn parse_global_type_and_value(&mut self) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let ty = self.parse_type(false)?;
        self.parse_global_value(ty)
    }

    fn parse_personality_fn(&mut self) -> ParseResult<ParsedPersonalityFn<'ctx, B>> {
        let ty = self.parse_type(false)?;
        let id = self.parse_val_id(None, Some(ty))?;
        // `ValID::Loc`, so a deferred reference is reported at the same token
        // an immediately-resolved one is.
        let value_loc = id.loc;
        if let ValIdKind::GlobalName(name) = id.kind {
            let retry = ValId {
                kind: ValIdKind::GlobalName(name.clone()),
                loc: value_loc,
            };
            match self.convert_val_id_to_constant(ty, retry) {
                Ok(constant) => Ok(ParsedPersonalityFn::Resolved(constant)),
                Err(ParseError::UndefinedSymbol { .. }) if ty.is_pointer() => {
                    Ok(ParsedPersonalityFn::ForwardName {
                        name,
                        ty,
                        loc: value_loc,
                    })
                }
                Err(err) => Err(err),
            }
        } else {
            self.convert_val_id_to_constant(
                ty,
                ValId {
                    kind: id.kind,
                    loc: value_loc,
                },
            )
            .map(ParsedPersonalityFn::Resolved)
        }
    }

    /// Mirrors `LLParser::parseGlobalValueVector`, including both of its
    /// early returns: a closing bracket yields the empty list rather than a
    /// diagnostic, and `inrange` ends the list so the caller can deal with it.
    fn parse_global_value_vector(&mut self) -> ParseResult<Vec<llvmkit_ir::Constant<'ctx, B>>> {
        let mut values = Vec::new();
        if matches!(
            self.peek(),
            Token::RBrace | Token::RSquare | Token::Greater | Token::RParen
        ) {
            return Ok(values);
        }
        loop {
            if matches!(self.peek(), Token::Kw(Keyword::Inrange)) {
                break;
            }
            values.push(self.parse_global_type_and_value()?);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }
        Ok(values)
    }

    /// `LLParser::getGlobalVal`'s opening
    /// `PointerType *PTy = dyn_cast<PointerType>(Ty); if (!PTy) …`, which runs
    /// *before* any symbol-table lookup and so fires even for a name the
    /// module already defines.
    fn check_global_reference_pointer_type(&self, loc: Span, ty: Type<'ctx, B>) -> ParseResult<()> {
        if ty.is_pointer() {
            return Ok(());
        }
        Err(ParseError::Message {
            message: "global variable reference must have pointer type".into(),
            loc: DiagLoc::span(loc),
        })
    }

    /// llvmkit's pre-emption of `validateEndOfModule`'s
    /// `intrinsic can only be used as callee` sweep — see
    /// `docs/divergences.md` entry 37. Kept where it was, after
    /// [`Self::check_global_reference_pointer_type`], so upstream's own guard
    /// still reports first.
    fn reject_intrinsic_non_callee(&self, loc: Span, name: &str) -> ParseResult<()> {
        if matches!(
            resolve_intrinsic_name(name),
            IntrinsicNameResolution::NonIntrinsic
        ) {
            return Ok(());
        }
        Err(ParseError::Message {
            message: "intrinsic can only be used as callee".into(),
            loc: DiagLoc::span(loc),
        })
    }

    /// `M->getValueSymbolTable().lookup(Name)`, narrowed to the four global
    /// kinds llvmkit keeps in separate tables.
    fn global_symbol_lookup(&self, name: &str) -> Option<GlobalRef<'ctx, B>> {
        if let Some(id) = self.module.global(name) {
            Some(GlobalRef::Variable(self.module.view(id)))
        } else if let Some(id) = self.module.function_dyn(name) {
            Some(GlobalRef::Function(self.module.view(id)))
        } else if let Some(id) = self.module.alias(name) {
            Some(GlobalRef::Alias(self.module.view(id)))
        } else {
            self.module
                .ifunc(name)
                .map(|id| GlobalRef::Ifunc(self.module.view(id)))
        }
    }

    /// `getGlobalVal`'s
    /// `if (Val) return cast_or_null<GlobalValue>(checkValidVariableType(Loc,
    /// "@" + Name, Ty, Val));`.
    ///
    /// Upstream's `Val->getType()` for a `GlobalValue` is
    /// `PointerType::get(C, GV->getAddressSpace())`; llvmkit rebuilds that
    /// pointer from the symbol's own address space because a global object's
    /// arena type here is its *value* type (`docs/divergences.md` D3). This is
    /// the same hoist `resolve_direct_callee` already performs for the callee
    /// spelling of the very same routine.
    fn check_resolved_global_type(
        &self,
        loc: Span,
        reference: &str,
        ty: Type<'ctx, B>,
        resolved: GlobalRef<'ctx, B>,
    ) -> ParseResult<()> {
        let address_space = match resolved {
            GlobalRef::Function(f) => f.address_space(),
            GlobalRef::Variable(g) => g.address_space(),
            GlobalRef::Alias(a) => a.address_space(),
            GlobalRef::Ifunc(i) => i.address_space(),
        };
        check_valid_variable_type(
            loc,
            reference,
            ty,
            self.module.ptr_type(address_space).as_type(),
        )
    }

    fn resolve_global_name_as_value(
        &mut self,
        loc: Span,
        name: String,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.check_global_reference_pointer_type(loc, ty)?;
        self.reject_intrinsic_non_callee(loc, &name)?;
        if let Some(resolved) = self.global_symbol_lookup(&name) {
            self.check_resolved_global_type(loc, &format!("@{name}"), ty, resolved)?;
            return Ok(self.global_ref_to_value(resolved));
        }
        self.global_forward_ref(Some(&name), None, ty, loc)
            .map(|c| c.as_erased())
    }

    fn resolve_global_id_as_value(
        &mut self,
        loc: Span,
        id: u32,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.check_global_reference_pointer_type(loc, ty)?;
        if let Some(resolved) = self.numbered_globals.get(id).copied() {
            self.check_resolved_global_type(loc, &format!("@{id}"), ty, resolved)?;
            return Ok(self.global_ref_to_value(resolved));
        }
        self.global_forward_ref(None, Some(id), ty, loc)
            .map(|c| c.as_erased())
    }

    fn resolve_global_name_as_constant(
        &mut self,
        loc: Span,
        name: String,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.check_global_reference_pointer_type(loc, ty)?;
        self.reject_intrinsic_non_callee(loc, &name)?;
        if let Some(resolved) = self.global_symbol_lookup(&name) {
            self.check_resolved_global_type(loc, &format!("@{name}"), ty, resolved)?;
            return Ok(self.global_ref_to_constant(resolved));
        }
        self.global_forward_ref(Some(&name), None, ty, loc)
    }

    fn resolve_global_id_as_constant(
        &mut self,
        loc: Span,
        id: u32,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.check_global_reference_pointer_type(loc, ty)?;
        if let Some(resolved) = self.numbered_globals.get(id).copied() {
            self.check_resolved_global_type(loc, &format!("@{id}"), ty, resolved)?;
            return Ok(self.global_ref_to_constant(resolved));
        }
        self.global_forward_ref(None, Some(id), ty, loc)
    }
    fn global_ref_to_value(&self, r: GlobalRef<'ctx, B>) -> llvmkit_ir::Value<'ctx, B> {
        match r {
            GlobalRef::Function(f) => f.as_erased(),
            GlobalRef::Variable(g) => g.as_erased(),
            GlobalRef::Alias(a) => a.as_erased(),
            GlobalRef::Ifunc(i) => i.as_erased(),
        }
    }

    fn global_ref_to_constant(&self, r: GlobalRef<'ctx, B>) -> llvmkit_ir::Constant<'ctx, B> {
        match r {
            GlobalRef::Function(f) => f.as_global_constant_ptr(),
            GlobalRef::Variable(g) => g.as_global_constant_ptr(),
            GlobalRef::Alias(a) => a.as_global_constant_ptr(),
            GlobalRef::Ifunc(i) => i.as_global_constant_ptr(),
        }
    }
    /// The bare `ptr` a `GlobalValue` *is* when it stands in value position.
    /// Upstream's `getGlobalVal` returns the `GlobalValue *` itself and
    /// `Value::getType` answers `PointerType::get(C, GV->getAddressSpace())`;
    /// `as_global_constant_ptr` mints that constant here (`docs/divergences.md`
    /// D3 is why the pointer is rebuilt rather than read off the arena type).
    ///
    /// The narrowing cannot fail — every arm of `global_ref_to_constant` builds
    /// its constant *at* a `PointerType`. It is spelled as a checked conversion
    /// because `PointerValue`'s unchecked constructor is private to
    /// `llvmkit-ir`, not because a pointer is in doubt.
    fn global_ref_as_pointer(
        &self,
        loc: Span,
        r: GlobalRef<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::PointerValue<'ctx, B>> {
        llvmkit_ir::PointerValue::try_from(self.global_ref_to_constant(r).as_erased())
            .map_err(|e| self.builder_err_at(loc, "global value as a pointer", e))
    }

    /// The same narrowing for the stand-in `global_forward_ref` mints. It
    /// cannot fail — `global_forward_ref` refuses a non-pointer `ty` up front
    /// and builds the placeholder *at* that type — and is spelled as a checked
    /// conversion only because `PointerValue`'s unchecked constructor is
    /// private to `llvmkit-ir`.
    fn constant_as_pointer(
        &self,
        loc: Span,
        c: llvmkit_ir::Constant<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::PointerValue<'ctx, B>> {
        llvmkit_ir::PointerValue::try_from(c.as_erased())
            .map_err(|e| self.builder_err_at(loc, "forward-referenced callee as a pointer", e))
    }

    fn resolve_global_name_as_ref(&self, name: String) -> ParseResult<GlobalRef<'ctx, B>> {
        self.global_symbol_lookup(&name)
            .ok_or_else(|| ParseError::UndefinedSymbol {
                kind: SymbolKind::Global,
                id: SymbolId::Named(name),
                loc: DiagLoc::span(self.loc()),
            })
    }

    fn parse_function_ref_for_blockaddress(
        &mut self,
        expected: &'static str,
    ) -> ParseResult<ParsedBlockAddressFunction<'ctx, B>> {
        match self.peek() {
            Token::GlobalVar(_) => {
                let loc = self.loc();
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected(expected))?;
                self.bump()?;
                if let Some(id) = self.module.function_dyn(&name) {
                    Ok(ParsedBlockAddressFunction::Resolved(self.module.view(id)))
                } else if self.module.global(&name).is_some()
                    || self.module.alias(&name).is_some()
                    || self.module.ifunc(&name).is_some()
                {
                    Err(self.expected(expected))
                } else {
                    Ok(ParsedBlockAddressFunction::Forward {
                        function: NameOrId::Name(name),
                        loc,
                    })
                }
            }
            Token::GlobalId(id) => {
                let loc = self.loc();
                let id = *id;
                self.bump()?;
                match self.numbered_globals.get(id) {
                    Some(GlobalRef::Function(function)) => {
                        Ok(ParsedBlockAddressFunction::Resolved(*function))
                    }
                    Some(_) => Err(self.expected(expected)),
                    None => Ok(ParsedBlockAddressFunction::Forward {
                        function: NameOrId::Id(id),
                        loc,
                    }),
                }
            }
            _ => Err(self.expected(expected)),
        }
    }

    /// `LLParser::parseValID`'s `kw_blockaddress` arm. `value_loc` is upstream's
    /// `ID.Loc` — the `blockaddress` keyword — which anchors the
    /// expected-type diagnostic and, later, the type comparison the deferred
    /// same-function form makes.
    fn parse_blockaddress_constant(
        &mut self,
        value_loc: Span,
        expected_ty: Type<'ctx, B>,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.expect_keyword(Keyword::Blockaddress, "'blockaddress'")?;
        self.expect_punct(PunctKind::LParen, "'(' in blockaddress")?;
        // `Fn.Loc`.
        let function_loc = self.loc();
        let function = self.parse_function_ref_for_blockaddress("function name in blockaddress")?;
        self.expect_punct(PunctKind::Comma, "',' in blockaddress")?;
        let label_loc = self.loc();
        let label = match self.peek() {
            Token::LocalVar(_) => BlockLabel::Named(
                self.current_str_payload()
                    .ok_or_else(|| self.expected("basic block name in blockaddress"))?,
            ),
            Token::LocalVarId(id) => BlockLabel::Numbered(*id),
            _ => return Err(self.expected("basic block name in blockaddress")),
        };
        self.bump()?;
        self.expect_punct(PunctKind::RParen, "')' in blockaddress")?;
        match function {
            ParsedBlockAddressFunction::Resolved(function) => {
                if function.basic_blocks().len() == 0 {
                    return Err(self.message("cannot take blockaddress inside a declaration"));
                }
                // Upstream's `BlockAddressPFS` route: a blockaddress naming the
                // function currently being parsed resolves through that
                // function's own state, where both label spellings are live.
                // A block that has not been reached yet defers to the
                // function's close, which is where upstream's
                // `resolveForwardRefBlockAddresses` would have caught it.
                let same_function =
                    pfs.filter(|state| state.func.as_erased() == function.as_dyn().as_erased());
                if let Some(state) = same_function {
                    if let Some(block) = state.defined_block(&label) {
                        let block = state.value_as_block_view(block, label_loc)?;
                        return self
                            .module
                            .block_address(function, &block)
                            .map_err(|e| self.builder_err("blockaddress", e));
                    }
                    return self.defer_block_address(
                        expected_ty,
                        DeferredBlockAddressFunction::Installed(function),
                        label,
                        BlockAddressLocs {
                            value: value_loc,
                            function: function_loc,
                            label: label_loc,
                        },
                    );
                }
                match label {
                    // The numbering is a parse-time artefact of the function's
                    // own body; once that body is closed there is nothing left
                    // to look `%5` up in.
                    BlockLabel::Numbered(_) => Err(self.message_at(
                        label_loc,
                        "cannot take address of numeric label after the function is defined",
                    )),
                    BlockLabel::Named(name) => {
                        let block = function
                            .basic_blocks()
                            .find(|bb| bb.name().as_deref() == Some(name.as_str()))
                            .ok_or_else(|| {
                                self.message_at(label_loc, "referenced value is not a basic block")
                            })?;
                        self.module
                            .block_address(function, &block)
                            .map_err(|e| self.builder_err("blockaddress", e))
                    }
                }
            }
            ParsedBlockAddressFunction::Forward { function, loc } => {
                // `if (!ExpectedTy->isPointerTy())`, inside the `!F` branch:
                // upstream only asks this when it is about to mint the
                // placeholder, because that is the only point where the
                // demanded type decides an address space. With the function in
                // hand the `blockaddress` types itself and the disagreement
                // surfaces as `convertValIDToValue`'s
                // `constant expression type mismatch` instead.
                if !matches!(expected_ty.into_type_enum(), AnyTypeEnum::Pointer(_)) {
                    return Err(self.message_at(
                        value_loc,
                        format!("type of blockaddress must be a pointer and not '{expected_ty}'"),
                    ));
                }
                self.defer_block_address(
                    expected_ty,
                    DeferredBlockAddressFunction::Forward(function),
                    label,
                    BlockAddressLocs {
                        value: value_loc,
                        function: loc,
                        label: label_loc,
                    },
                )
            }
        }
    }

    /// Park a `blockaddress` whose block is not yet available behind a
    /// placeholder. Mirrors the `ForwardRefBlockAddresses` entry upstream
    /// makes; it is drained when the named function's body closes.
    fn defer_block_address(
        &mut self,
        expected_ty: Type<'ctx, B>,
        function: DeferredBlockAddressFunction<'ctx, B>,
        label: BlockLabel,
        locs: BlockAddressLocs,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let placeholder = self
            .module
            .forward_ref_value_placeholder(expected_ty)
            .map_err(|e| self.builder_err("blockaddress placeholder", e))?;
        let constant = placeholder.as_constant();
        self.deferred_block_addresses.push(DeferredBlockAddress {
            placeholder,
            function,
            label,
            value_loc: locs.value,
            function_loc: locs.function,
            label_loc: locs.label,
        });
        Ok(constant)
    }

    /// The `@name` / `@N` operand `kw_dso_local_equivalent` and `kw_no_cfi`
    /// both read, and the `Fn.Loc` it was written at.
    ///
    /// Upstream reads it with a nested `parseValID` and then rejects every
    /// `ValID::Kind` but `t_GlobalID` / `t_GlobalName`; the rejection is what
    /// `expected` spells.
    fn parse_global_value_ref_operand(
        &mut self,
        expected: &'static str,
    ) -> ParseResult<(NameOrId, Span)> {
        let loc = self.loc();
        match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected(expected))?;
                self.bump()?;
                Ok((NameOrId::Name(name), loc))
            }
            Token::GlobalId(id) => {
                let id = *id;
                self.bump()?;
                Ok((NameOrId::Id(id), loc))
            }
            _ => Err(self.expected(expected)),
        }
    }

    /// `LLParser::parseValID`'s `kw_dso_local_equivalent` arm.
    fn parse_dso_local_equivalent_constant(
        &mut self,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.expect_keyword(Keyword::DsoLocalEquivalent, "'dso_local_equivalent'")?;
        let (reference, operand_loc) =
            self.parse_global_value_ref_operand("global value name in dso_local_equivalent")?;
        // "Try to find the function (but skip it if it's forward-referenced)":
        // a name that is currently a `ForwardRefVals` placeholder counts as
        // *not* found, because the placeholder is not the function whose value
        // type the check below asks about.
        let global = match &reference {
            NameOrId::Name(name) => {
                if self.forward_ref_globals.contains_key(name) {
                    None
                } else {
                    self.resolve_global_name_as_ref(name.clone()).ok()
                }
            }
            NameOrId::Id(id) => self.numbered_globals.get(*id).copied(),
        };
        let Some(global) = global else {
            return self.forward_ref_dso_local_equivalent(reference, operand_loc);
        };
        // `!GV->getValueType()->isFunctionTy()`, reported at the operand.
        if !global_ref_value_type_is_function(global) {
            return Err(ParseError::Message {
                message: "expected a function, alias to function, or ifunc in dso_local_equivalent"
                    .into(),
                loc: DiagLoc::span(operand_loc),
            });
        }
        self.module
            .dso_local_equivalent_global(self.global_ref_to_constant(global))
            .map_err(|e| self.builder_err("dso_local_equivalent", e))
    }

    /// Mint — or re-use — the placeholder a forward `dso_local_equivalent`
    /// stands behind. Mirrors the `FwdRefMap[Fn]` half of the arm: one
    /// placeholder per referent, and the *first* reference's location is the
    /// one `validateEndOfModule` blames, because upstream's map key is the
    /// `ValID` it was first inserted with.
    fn forward_ref_dso_local_equivalent(
        &mut self,
        reference: NameOrId,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        // Upstream's placeholder is `new GlobalVariable(*M, Int8Ty, ...)` with
        // no address space, so the reference types as a plain `ptr`; the
        // demanded type is compared against it by `convertValIDToValue`.
        let ptr_ty = self.module.ptr_type(0).as_type();
        let existing = match &reference {
            NameOrId::Name(name) => self.forward_ref_dso_local_equivalent_names.get(name),
            NameOrId::Id(id) => self.forward_ref_dso_local_equivalent_ids.get(id),
        };
        if let Some(entry) = existing {
            return Ok(entry.placeholder.as_constant());
        }
        let placeholder = self
            .module
            .forward_ref_value_placeholder(ptr_ty)
            .map_err(|e| self.builder_err("dso_local_equivalent placeholder", e))?;
        let constant = placeholder.as_constant();
        let entry = ForwardRef { placeholder, loc };
        match reference {
            NameOrId::Name(name) => {
                self.forward_ref_dso_local_equivalent_names
                    .insert(name, entry);
            }
            NameOrId::Id(id) => {
                self.forward_ref_dso_local_equivalent_ids.insert(id, entry);
            }
        }
        Ok(constant)
    }

    /// `LLParser::parseValID`'s `kw_no_cfi` arm, together with the
    /// `if (V && ID.NoCFI)` half of `convertValIDToValue`'s `t_GlobalName` /
    /// `t_GlobalID` arms that actually builds the wrapper.
    fn parse_no_cfi_constant(&mut self) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.expect_keyword(Keyword::NoCfi, "'no_cfi'")?;
        let (reference, loc) =
            self.parse_global_value_ref_operand("global value name in no_cfi")?;
        let global = match &reference {
            NameOrId::Name(name) => self.resolve_global_name_as_ref(name.clone()).ok(),
            NameOrId::Id(id) => self.numbered_globals.get(*id).copied(),
        };
        if let Some(global) = global {
            return self
                .module
                .no_cfi_global(self.global_ref_to_constant(global))
                .map_err(|e| self.builder_err("no_cfi", e));
        }
        // `getGlobalVal`: the referent joins `ForwardRefVals` exactly as a bare
        // `@name` operand would, so an undefined one is reported by that
        // sweep, in its position and with its wording. The `NoCFIValue` itself
        // cannot be built over the placeholder here — see `pending_no_cfi`.
        let ptr_ty = self.module.ptr_type(0).as_type();
        match &reference {
            NameOrId::Name(name) => self.global_forward_ref(Some(name), None, ptr_ty, loc)?,
            NameOrId::Id(id) => self.global_forward_ref(None, Some(*id), ptr_ty, loc)?,
        };
        let placeholder = self
            .module
            .forward_ref_value_placeholder(ptr_ty)
            .map_err(|e| self.builder_err("no_cfi placeholder", e))?;
        let constant = placeholder.as_constant();
        self.pending_no_cfi.push(PendingNoCfi {
            placeholder,
            reference,
            loc,
        });
        Ok(constant)
    }

    fn parse_ptrauth_operand(&mut self) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let ty = self.parse_type(false)?;
        self.parse_global_value(ty)
    }

    fn parse_ptrauth_constant(&mut self) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.expect_keyword(Keyword::Ptrauth, "'ptrauth'")?;
        self.expect_punct(PunctKind::LParen, "'(' in constant ptrauth expression")?;
        let pointer = self.parse_ptrauth_operand()?;
        self.expect_punct(PunctKind::Comma, "comma in constant ptrauth expression")?;
        let key = self.parse_ptrauth_operand()?;
        let discriminator = if self.eat_punct(PunctKind::Comma)? {
            self.parse_ptrauth_operand()?
        } else {
            self.module.i64_type().const_zero().as_constant()
        };
        let addr_discriminator = if self.eat_punct(PunctKind::Comma)? {
            self.parse_ptrauth_operand()?
        } else {
            self.module.ptr_type(0).const_null().as_constant()
        };
        let deactivation_symbol = if self.eat_punct(PunctKind::Comma)? {
            self.parse_ptrauth_operand()?
        } else {
            self.module.ptr_type(0).const_null().as_constant()
        };
        self.expect_punct(PunctKind::RParen, "')' in constant ptrauth expression")?;
        self.module
            .ptr_auth(
                pointer,
                key,
                discriminator,
                addr_discriminator,
                deactivation_symbol,
            )
            .map_err(|e| match e {
                // `Constants::ptr_auth` carries upstream's own wording
                // (`LLParser::parseValID`'s `lltok::kw_ptrauth` arm), so the
                // message is rendered verbatim rather than as `expected ...`.
                IrError::InvalidOperation { message } => self.message(message),
                other => self.builder_err("ptrauth", other),
            })
    }

    /// `LLParser::parseValID`'s constant-expression arms.
    ///
    /// **Self-typing, as upstream's are.** Every arm ends in a
    /// `ConstantExpr::get*` call whose result type comes from what was written
    /// — the destination type after `to`, the operands, the GEP's base and
    /// indices — and never from a type demanded by the surrounding context.
    /// That is what lets `parseAliasOrIFunc` read an aliasee with no type of
    /// its own, and it is where the agreement check belongs: a constant
    /// expression whose own type differs from the demanded one is
    /// `convertValIDToValue`'s `constant expression type mismatch`, one layer
    /// up, not a malformed-operands error down here.
    fn parse_constant_expr(&mut self) -> ParseResult<Constant<'ctx, B>> {
        // `parseValID` opens with `ID.Loc = Lex.getLoc();`, and every
        // *semantic* rejection in these arms is an `error(ID.Loc, …)` rather
        // than a `tokError` — so the caret sits on the opcode keyword, not on
        // whatever token the parse has reached by then. Three arms below need
        // it, and all three used to anchor at the current token: for
        // `@g = global i64 ptrtoaddr (i32 1 to i64)` that was the token *after*
        // the closing paren, i.e. end of file.
        let id_loc = self.loc();
        let op = match self.peek() {
            Token::Instruction(op) => *op,
            _ => return Err(self.expected("constant expression opcode")),
        };
        self.bump()?;
        let opcode = match op {
            Opcode::Add => ConstantExprOpcode::Add,
            Opcode::Sub => ConstantExprOpcode::Sub,
            Opcode::Xor => ConstantExprOpcode::Xor,
            Opcode::GetElementPtr => ConstantExprOpcode::GetElementPtr,
            Opcode::ShuffleVector => ConstantExprOpcode::ShuffleVector,
            Opcode::InsertElement => ConstantExprOpcode::InsertElement,
            Opcode::ExtractElement => ConstantExprOpcode::ExtractElement,
            Opcode::Trunc => ConstantExprOpcode::Trunc,
            Opcode::PtrToAddr => ConstantExprOpcode::PtrToAddr,
            Opcode::PtrToInt => ConstantExprOpcode::PtrToInt,
            Opcode::IntToPtr => ConstantExprOpcode::IntToPtr,
            Opcode::BitCast => ConstantExprOpcode::BitCast,
            Opcode::AddrSpaceCast => ConstantExprOpcode::AddrSpaceCast,
            _ => return Err(self.unsupported_constant_value_form_at(self.loc())),
        };

        match opcode {
            ConstantExprOpcode::Add | ConstantExprOpcode::Sub | ConstantExprOpcode::Xor => {
                let flags = if matches!(opcode, ConstantExprOpcode::Add | ConstantExprOpcode::Sub) {
                    self.parse_overflowing_constant_expr_flags()?
                } else {
                    ConstantExprFlags::none()
                };
                self.expect_punct(PunctKind::LParen, "'(' in binary constantexpr")?;
                let lhs = self.parse_global_type_and_value()?;
                self.expect_punct(PunctKind::Comma, "comma in binary constantexpr")?;
                let rhs = self.parse_global_type_and_value()?;
                // Upstream runs the closing `parseToken` *before* both checks,
                // in one `if (… || … || …)` chain, so a binary constantexpr
                // that is both mistyped and unterminated is reported as
                // unterminated.
                self.expect_punct(PunctKind::RParen, "')' in binary constantexpr")?;
                if lhs.ty() != rhs.ty() {
                    return Err(
                        self.message_at(id_loc, "operands of constexpr must have same type")
                    );
                }
                if !is_int_or_int_vector_type(lhs.ty()) {
                    return Err(self.message_at(
                        id_loc,
                        "constexpr requires integer or integer vector operands",
                    ));
                }
                // `ConstantExpr::get(Opc, Val0, Val1, Flags)` — an integer
                // binop's result type is its operands'.
                let result_ty = lhs.ty();
                self.build_constant_expr(result_ty, None, opcode, vec![lhs, rhs], flags)
            }
            ConstantExprOpcode::Trunc
            | ConstantExprOpcode::IntToPtr
            | ConstantExprOpcode::PtrToAddr
            | ConstantExprOpcode::PtrToInt
            | ConstantExprOpcode::BitCast
            | ConstantExprOpcode::AddrSpaceCast => {
                self.expect_punct(PunctKind::LParen, "'(' after constantexpr cast")?;
                let operand = self.parse_global_type_and_value()?;
                self.expect_keyword(Keyword::To, "'to' in constantexpr cast")?;
                let dst_ty = self.parse_type(false)?;
                self.expect_punct(PunctKind::RParen, "')' at end of constantexpr cast")?;
                // Upstream asks `CastInst::castIsValid`, not whether the
                // destination matches the initializer's type — that agreement
                // is `convertValIDToValue`'s job and is checked there.
                let src_ty = operand.ty();
                if !llvmkit_ir::cast_is_valid(cast_opcode_for(opcode), src_ty, dst_ty) {
                    return Err(ParseError::Message {
                        message: format!(
                            "invalid cast opcode for cast from '{src_ty}' to '{dst_ty}'"
                        )
                        .into(),
                        loc: DiagLoc::span(id_loc),
                    });
                }
                // `ConstantExpr::getCast(Opc, SrcVal, DestTy)` — upstream's own
                // comment on the aliasee spelling of this arm is that the
                // "dest type is not present, it is implied by the dest type",
                // i.e. the type after `to` *is* the result type.
                self.build_constant_expr(
                    dst_ty,
                    None,
                    opcode,
                    vec![operand],
                    ConstantExprFlags::none(),
                )
            }
            ConstantExprOpcode::GetElementPtr => {
                let parsed_flags = self.parse_gep_constant_expr_flags()?;
                self.expect_punct(PunctKind::LParen, "'(' in constantexpr")?;
                let source_ty = self.parse_type(false)?;
                self.expect_punct(PunctKind::Comma, "comma after getelementptr's type")?;
                let operands = self.parse_global_value_vector()?;
                self.expect_punct(PunctKind::RParen, "')' in constantexpr")?;
                let (flags, result_ty) =
                    self.validate_parsed_gep_constant_expr(source_ty, &operands, parsed_flags)?;
                self.build_constant_expr(result_ty, Some(source_ty), opcode, operands, flags)
            }
            ConstantExprOpcode::ShuffleVector
            | ConstantExprOpcode::InsertElement
            | ConstantExprOpcode::ExtractElement => {
                self.expect_punct(PunctKind::LParen, "'(' in constantexpr")?;
                let operands = self.parse_global_value_vector()?;
                self.expect_punct(PunctKind::RParen, "')' in constantexpr")?;
                let result_ty = self.validate_parsed_vector_constant_expr(opcode, &operands)?;
                self.build_constant_expr(
                    result_ty,
                    None,
                    opcode,
                    operands,
                    ConstantExprFlags::none(),
                )
            }
        }
    }

    fn parse_overflowing_constant_expr_flags(&mut self) -> ParseResult<ConstantExprFlags> {
        let mut nuw = false;
        let mut nsw = false;
        if self.eat_keyword(Keyword::Nuw)? {
            nuw = true;
        }
        if self.eat_keyword(Keyword::Nsw)? {
            nsw = true;
            if self.eat_keyword(Keyword::Nuw)? {
                nuw = true;
            }
        }
        Ok(ConstantExprFlags::overflowing(nuw, nsw))
    }

    fn parse_gep_constant_expr_flags(&mut self) -> ParseResult<ParsedGepConstantExprFlags> {
        let mut no_wrap = GepNoWrapFlags::empty();
        loop {
            if self.eat_keyword(Keyword::Inbounds)? {
                no_wrap |= GepNoWrapFlags::inbounds();
            } else if self.eat_keyword(Keyword::Nusw)? {
                no_wrap |= GepNoWrapFlags::NUSW;
            } else if self.eat_keyword(Keyword::Nuw)? {
                no_wrap |= GepNoWrapFlags::NUW;
            } else {
                break;
            }
        }

        let in_range = if self.eat_keyword(Keyword::Inrange)? {
            self.expect_punct(PunctKind::LParen, "'('")?;
            let start = self.parse_inrange_bound()?;
            self.expect_punct(PunctKind::Comma, "','")?;
            let end = self.parse_inrange_bound()?;
            self.expect_punct(PunctKind::RParen, "')'")?;
            Some((start, end))
        } else {
            None
        };

        Ok(ParsedGepConstantExprFlags { no_wrap, in_range })
    }

    /// The `inrange` half of the `getelementptr` arm: upstream widens both
    /// bounds to the base pointer's index width before comparing them, so a
    /// bound that only overflows at the narrower width is legal.
    /// The `inrange` half of `LLParser::parseValID`'s `getelementptr` arm,
    /// after both bounds have been lexed:
    ///
    /// ```text
    /// InRangeStart = InRangeStart->extOrTrunc(IndexWidth);
    /// InRangeEnd = InRangeEnd->extOrTrunc(IndexWidth);
    /// if (InRangeStart->sge(*InRangeEnd))
    ///   return error(..., "expected end to be larger than start");
    /// ```
    fn gep_constant_expr_flags(
        &self,
        parsed: ParsedGepConstantExprFlags,
        address_space: u32,
    ) -> ParseResult<ConstantExprFlags> {
        let Some((start, end)) = parsed.in_range else {
            return Ok(ConstantExprFlags::gep(parsed.no_wrap));
        };
        let bit_width = self.module.data_layout().index_size_in_bits(address_space);
        let in_range = ConstantExprInRange::new(
            start.extend_or_truncate(bit_width),
            end.extend_or_truncate(bit_width),
        );
        if !in_range.is_non_empty() {
            return Err(self.expected("end to be larger than start"));
        }
        Ok(ConstantExprFlags::gep_with_in_range(
            parsed.no_wrap,
            in_range,
        ))
    }

    /// One `inrange` bound. Upstream reads it as `Lex.getAPSIntVal()`, the
    /// single `APSInt` every integer token carries, so this is
    /// [`Self::parse_int_literal`] and nothing else — the `[us]0x` active-bit
    /// truncation and the signed/unsigned stamp are that one lexer rule's job.
    fn parse_inrange_bound(&mut self) -> ParseResult<ParsedApsInt> {
        match self.peek() {
            Token::IntegerLit(_) => self.parse_int_literal(),
            _ => Err(self.expected("integer")),
        }
    }

    /// Everything `LLParser::parseValID`'s `getelementptr` arm does after the
    /// closing paren, in upstream's order: base type, `inrange` bounds, index
    /// agreement, sizedness, constant-expression support, index walk.
    ///
    /// The order *is* the behaviour — several of these overlap, so which one
    /// fires decides the message. `getelementptr({<vscale x 2 x i32>, i32},
    /// ptr @g, i32 0)` is both unsized and unsupported, and upstream reports
    /// it unsized.
    /// The `Opc == Instruction::GetElementPtr` half of `parseValID`'s
    /// `getelementptr` arm, up to and including the `inrange` bounds.
    ///
    /// Returns the flags **and** the result type
    /// `ConstantExpr::getGetElementPtr` would give the expression:
    /// `GetElementPtrInst::getGEPReturnType`'s answer, a `ptr` in the base's
    /// address space, made a vector of the first vector shape found among the
    /// base and then the indices.
    fn validate_parsed_gep_constant_expr(
        &self,
        source_ty: Type<'ctx, B>,
        operands: &[llvmkit_ir::Constant<'ctx, B>],
        parsed_flags: ParsedGepConstantExprFlags,
    ) -> ParseResult<(ConstantExprFlags, Type<'ctx, B>)> {
        // Upstream's `Elts.size() == 0 || !isPtrOrPtrVectorTy()`; asking for
        // the address space answers both at once, and the `inrange` bounds
        // need it next anyway.
        let Some((base, indices)) = operands.split_first() else {
            return Err(self.message("base of getelementptr must be a pointer"));
        };
        let Some(address_space) = pointer_address_space_or_vector_element(base.ty()) else {
            return Err(self.message("base of getelementptr must be a pointer"));
        };

        let flags = self.gep_constant_expr_flags(parsed_flags, address_space)?;

        let mut gep_width = vector_shape_type(base.ty());
        for index in indices {
            if !is_int_or_int_vector_type(index.ty()) {
                return Err(self.message("getelementptr index must be an integer"));
            }
            if let Some(index_shape) = vector_shape_type(index.ty()) {
                if let Some(pointer_shape) = gep_width
                    && index_shape != pointer_shape
                {
                    return Err(
                        self.message("getelementptr vector index has a wrong number of elements")
                    );
                }
                // The base may have been a scalar, so the width is known only
                // now — upstream's own comment.
                gep_width = Some(index_shape);
            }
        }

        if !indices.is_empty() && !source_ty.is_sized() {
            return Err(self.message("base element of getelementptr must be sized"));
        }
        // `ConstantExpr::isSupportedGetElementPtr`.
        if source_ty.is_scalable() {
            return Err(self.message("invalid base element for constant getelementptr"));
        }
        let index_values: Vec<_> = indices.iter().map(|index| index.as_erased()).collect();
        if llvmkit_ir::indexed_gep_type(source_ty, &index_values).is_none() {
            return Err(self.message("invalid getelementptr indices"));
        }
        let scalar_ptr = self.module.ptr_type(address_space).as_type();
        let result_ty = match gep_width {
            None => scalar_ptr,
            Some((lanes, true)) => self
                .module
                .scalable_vector_type(scalar_ptr, lanes)
                .as_type(),
            Some((lanes, false)) => self.module.vector_type(scalar_ptr, lanes).as_type(),
        };
        Ok((flags, result_ty))
    }

    /// The three non-GEP arms of `parseValID`'s
    /// `getelementptr`/`shufflevector`/`insertelement`/`extractelement` case,
    /// each an `isValidOperands` guard followed by a `ConstantExpr::get*` whose
    /// result type falls out of the operands. Returns that type.
    fn validate_parsed_vector_constant_expr(
        &self,
        opcode: ConstantExprOpcode,
        operands: &[Constant<'ctx, B>],
    ) -> ParseResult<Type<'ctx, B>> {
        match opcode {
            ConstantExprOpcode::ShuffleVector => {
                let [lhs, rhs, mask] = operands else {
                    return Err(self.expected("three operands to shufflevector"));
                };
                if !is_valid_shufflevector(lhs.ty(), rhs.ty(), mask.ty()) {
                    return Err(self.message("invalid operands to shufflevector"));
                }
                // `ConstantExpr::getShuffleVector`: `VectorType::get(EltTy,
                // Mask.size(), TypeIsScalable)` — the element type and
                // scalability from `V1`, the length from the mask.
                let (AnyTypeEnum::Vector(lhs_ty), AnyTypeEnum::Vector(mask_ty)) =
                    (AnyTypeEnum::from(lhs.ty()), AnyTypeEnum::from(mask.ty()))
                else {
                    return Err(self.message("invalid operands to shufflevector"));
                };
                let element = lhs_ty.element();
                Ok(if lhs_ty.is_scalable() {
                    self.module
                        .scalable_vector_type(element, mask_ty.min_len())
                        .as_type()
                } else {
                    self.module
                        .vector_type(element, mask_ty.min_len())
                        .as_type()
                })
            }
            ConstantExprOpcode::ExtractElement => {
                let [vector, index] = operands else {
                    return Err(self.expected("two operands to extractelement"));
                };
                if !is_valid_extractelement(vector.ty(), index.ty()) {
                    return Err(self.message("invalid extractelement operands"));
                }
                // `ConstantExpr::getExtractElement` types itself off the
                // vector's element type.
                let AnyTypeEnum::Vector(vector_ty) = AnyTypeEnum::from(vector.ty()) else {
                    return Err(self.message("invalid extractelement operands"));
                };
                Ok(vector_ty.element())
            }
            ConstantExprOpcode::InsertElement => {
                let [vector, value, index] = operands else {
                    return Err(self.expected("three operands to insertelement"));
                };
                if !is_valid_insertelement(vector.ty(), value.ty(), index.ty()) {
                    return Err(self.message("invalid insertelement operands"));
                }
                // `ConstantExpr::getInsertElement` gives back the vector's own
                // type.
                Ok(vector.ty())
            }
            _ => unreachable!(
                "only shufflevector / extractelement / insertelement reach this helper"
            ),
        }
    }

    fn build_constant_expr(
        &self,
        result_ty: Type<'ctx, B>,
        source_ty: Option<Type<'ctx, B>>,
        opcode: ConstantExprOpcode,
        operands: Vec<Constant<'ctx, B>>,
        flags: ConstantExprFlags,
    ) -> ParseResult<Constant<'ctx, B>> {
        let options = ConstantExprOptions::new().flags(flags);
        let options = match source_ty {
            Some(source_ty) => options.source_ty(source_ty),
            None => options,
        };
        self.module
            .constant_expr_with_options(
                result_ty,
                opcode,
                operands.into_iter().map(|c| c.as_erased()),
                [],
                [],
                options,
            )
            .map_err(|e| match e {
                IrError::InvalidOperation { message }
                    if matches!(opcode, ConstantExprOpcode::ShuffleVector)
                        && message == "invalid shufflevector constant expression" =>
                {
                    ParseError::Message {
                        message: "invalid operands to shufflevector".into(),
                        loc: DiagLoc::span(self.loc()),
                    }
                }
                IrError::InvalidOperation { message } => ParseError::Expected {
                    expected: message.into(),
                    loc: DiagLoc::span(self.loc()),
                },
                other => self.builder_err("constant expression", other),
            })
    }

    fn parse_optional_function_linkage(&mut self) -> ParseResult<Linkage> {
        let linkage = match self.peek() {
            Token::Kw(keyword) => match linkage_keyword(*keyword) {
                Some(linkage) => {
                    self.bump()?;
                    linkage
                }
                None => Linkage::External,
            },
            _ => Linkage::External,
        };
        Ok(linkage)
    }

    /// `parseFunctionHeader`'s last act when `IsDefine` is false: a
    /// `blockaddress` naming this function can never be satisfied, because a
    /// declaration has no blocks to address.
    ///
    /// Upstream looks the function up in `ForwardRefBlockAddresses` and
    /// reports at the *reference*, not at the declaration.
    fn check_no_blockaddress_for_declaration(
        &self,
        f: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>,
        name_id: &NameOrId,
    ) -> ParseResult<()> {
        for item in &self.deferred_block_addresses {
            let is_ours = match &item.function {
                DeferredBlockAddressFunction::Installed(installed) => {
                    installed.as_erased() == f.as_erased()
                }
                DeferredBlockAddressFunction::Forward(reference) => reference == name_id,
            };
            if is_ours {
                // `error(Blocks->first.Loc, ...)` — the *function* reference's
                // location, since `Blocks->first` is the `Fn` `ValID`.
                return Err(self.message_at(
                    item.function_loc,
                    "cannot take blockaddress inside a declaration",
                ));
            }
        }
        Ok(())
    }

    /// The two rules `parseFunctionHeader` applies once the whole clause
    /// chain has parsed and the `AttributeList` is assembled.
    ///
    /// `builtin` is a real attribute — a *call site* may carry it — so the
    /// rejection lives here rather than in the attribute loop, anchored at
    /// the attribute's own location. The `sret` rule asks parameter 0 only,
    /// and reports at the return type.
    fn check_function_attribute_rules(
        &self,
        suffix: &FunctionSuffix<'ctx, B>,
        attrs: &AttributeStorage,
        ret_ty: Type<'ctx, B>,
        ret_ty_loc: Span,
    ) -> ParseResult<()> {
        if let Some(loc) = suffix.builtin_loc {
            return Err(self.message_at(loc, "'builtin' attribute not valid on function"));
        }
        if attrs.has_kind(AttrIndex::Param(0), AttrKind::StructRet) && !ret_ty.is_void() {
            return Err(self.message_at(
                ret_ty_loc,
                "functions with 'sret' argument must return void",
            ));
        }
        Ok(())
    }

    /// The scope-token guard `parseCatchSwitch` and `parseCleanupPad` each run
    /// right after their `within`: the next token must be `none` or a local,
    /// and anything else is `expected scope value for <pad>` rather than
    /// whatever reading a value would have said. `parseCatchPad` runs a
    /// narrower version of the same guard — see
    /// [`check_catchpad_scope_token`](Self::check_catchpad_scope_token).
    fn check_pad_scope_token(&self, pad: &'static str) -> ParseResult<()> {
        if matches!(
            self.peek(),
            Token::Kw(Keyword::None) | Token::LocalVar(_) | Token::LocalVarId(_)
        ) {
            return Ok(());
        }
        Err(self.message(format!("expected scope value for {pad}")))
    }

    /// `LLParser::parseCatchPad`'s scope guard, which is *narrower* than the
    /// one above: `none` is not a legal catchpad scope, because a catchpad's
    /// parent is always a `catchswitch`, never the function. Upstream tests
    /// `Lex.getKind() != lltok::LocalVar && Lex.getKind() != lltok::LocalVarID`
    /// where `parseCleanupPad` and `parseCatchSwitch` also admit `kw_none`.
    fn check_catchpad_scope_token(&self) -> ParseResult<()> {
        if matches!(self.peek(), Token::LocalVar(_) | Token::LocalVarId(_)) {
            return Ok(());
        }
        Err(self.message("expected scope value for catchpad"))
    }

    /// The argument-agreement walk `parseCall`, `parseInvoke` and
    /// `parseCallBr` each carry *verbatim* — three copies of the same loop in
    /// upstream, and the reason its three diagnostics appear three times each
    /// in `LLParser.cpp`.
    ///
    /// It only bites when the callee was written with an explicit function
    /// type (`call i32 (i32, i32) @f(i32 1)`): otherwise `resolveFunctionType`
    /// *builds* the signature from the arguments, so they agree by
    /// construction. llvmkit deferred all of this to the verifier, which the
    /// no-divergence directive reverses.
    fn check_call_argument_agreement(
        &self,
        fn_ty: llvmkit_ir::FunctionType<'ctx, B>,
        arg_tys: &[Type<'ctx, B>],
        arg_locs: &[Span],
        call_loc: Span,
    ) -> ParseResult<()> {
        let params: Vec<Type<'ctx, B>> = fn_ty.params().collect();
        let mut expected = params.iter();
        for (arg_ty, arg_loc) in arg_tys.iter().zip(arg_locs) {
            let expected_ty = match expected.next() {
                Some(expected_ty) => Some(*expected_ty),
                // A varargs signature simply stops checking once its declared
                // parameters run out.
                None if fn_ty.is_var_arg() => None,
                None => return Err(self.message_at(*arg_loc, "too many arguments specified")),
            };
            if let Some(expected_ty) = expected_ty
                && expected_ty != *arg_ty
            {
                return Err(self.message_at(
                    *arg_loc,
                    format!("argument is not of expected type '{expected_ty}'"),
                ));
            }
        }
        if expected.next().is_some() {
            return Err(self.message_at(call_loc, "not enough parameters specified for call"));
        }
        Ok(())
    }

    /// `parseFunctionHeader`'s redefinition arm.
    ///
    /// Upstream always creates a **fresh** `Function` for a header; only a
    /// *forward reference* is reused (its placeholder is RAUW'd and erased),
    /// and anything else already carrying the name is an error. llvmkit used
    /// to reuse any function whose signature happened to match, so a repeated
    /// `declare` — or a `declare` followed by a `define` — was accepted
    /// silently.
    ///
    /// The two messages differ by namespace *and* by one `@`: a pre-existing
    /// **function** is `invalid redefinition of function 'f'`, while any other
    /// named value is `redefinition of function '@f'`.
    fn check_function_redefinition(&self, name: &str, loc: Span) -> ParseResult<()> {
        // An empty name is the `@N` / `@""` form, which upstream routes
        // through `ForwardRefValIDs` instead.
        //
        // The forward-reference case never reaches here at all: it is the
        // `if (FRVI != ForwardRefVals.end())` arm of the same `else if` chain,
        // handled by [`Self::claim_function_forward_ref`].
        if name.is_empty() {
            return Ok(());
        }
        if self.module.function_dyn(name).is_some() {
            return Err(self.message_at(loc, format!("invalid redefinition of function '{name}'")));
        }
        if self.module.global(name).is_some()
            || self.module.alias(name).is_some()
            || self.module.ifunc(name).is_some()
        {
            return Err(self.message_at(loc, format!("redefinition of function '@{name}'")));
        }
        Ok(())
    }

    /// `parseFunctionHeader`'s `if (!FunctionName.empty()) { … } else { … }`
    /// block: the `else if` chain that decides whether this header *claims* a
    /// pending forward-reference placeholder, and rejects a name already
    /// taken. Returns upstream's `GlobalValue *FwdFn` — the placeholder the
    /// caller RAUWs once the fresh `Function` exists.
    ///
    /// Both branches compare `FwdFn->getType() != PFT`, which after opaque
    /// pointers is nothing but the address space, and neither looks at the
    /// signature: a call site's arguments never constrain the definition.
    /// llvmkit used to *reuse* a function whose signature happened to match
    /// and reject the header otherwise, because its forward-referenced callee
    /// was a real `Function` built at the call site's type.
    ///
    /// The two messages differ in wording **and** in anchor: the named form is
    /// `error(FRVI->second.second, …)`, on the reference that created the
    /// placeholder, while the numbered form is `error(NameLoc, …)`, on the
    /// header's own `@N`.
    fn claim_function_forward_ref(
        &mut self,
        name: &str,
        name_id: &NameOrId,
        address_space: u32,
        name_loc: Span,
    ) -> ParseResult<Option<ForwardRef<'ctx, B>>> {
        // `PointerType *PFT = PointerType::get(Context, AddrSpace);`
        let pft = self.module.ptr_type(address_space).as_type();
        if !name.is_empty() {
            let Some(entry) = self.forward_ref_globals.remove(name) else {
                // `else if ((Fn = M->getFunction(FunctionName)))` / `else if
                // (M->getNamedValue(FunctionName))`.
                self.check_function_redefinition(name, name_loc)?;
                return Ok(None);
            };
            let placeholder_ty = entry.placeholder.ty();
            if placeholder_ty != pft {
                return Err(self.message_at(
                    entry.loc,
                    format!(
                        "invalid forward reference to function '{name}' with wrong type: \
                         expected '{pft}' but was '{placeholder_ty}'"
                    ),
                ));
            }
            return Ok(Some(entry));
        }
        // The `@N` half.
        //
        // **Divergence:** `@""` — a name syntactically present but semantically
        // missing — reaches this branch upstream too, where `FunctionNumber ==
        // (unsigned)-1` is replaced by `NumberedVals.getNext()` so the header
        // claims that slot. llvmkit lexes `@""` as an empty `GlobalVar` and
        // carries it as `NameOrId::Name("")`, which takes no number at all, so
        // the arm below never fires for it. That is the *unnamed global takes
        // no slot* gap, catalogued as **G15** in `docs/fixture-coverage.md`
        // with `test/Assembler/skip-value-numbers-globals.ll` behind it; it is
        // not introduced here, and an empty name reached
        // `check_function_redefinition`'s own early return before.
        let NameOrId::Id(id) = name_id else {
            return Ok(None);
        };
        let Some(entry) = self.forward_ref_global_ids.remove(id) else {
            return Ok(None);
        };
        let placeholder_ty = entry.placeholder.ty();
        if placeholder_ty != pft {
            return Err(self.message_at(
                name_loc,
                format!(
                    "type of definition and forward reference of '@{id}' disagree: \
                     expected '{pft}' but was '{placeholder_ty}'"
                ),
            ));
        }
        Ok(Some(entry))
    }

    /// `parseFunctionHeader`'s "Verify that the linkage is ok" switch.
    ///
    /// Upstream runs it **after** the return type has parsed, and anchors it
    /// at `LinkageLoc` — so a malformed return type is reported before a bad
    /// linkage, and the caret sits on the linkage keyword rather than on the
    /// function name.
    fn check_function_linkage(linkage: Linkage, is_define: bool, loc: Span) -> ParseResult<()> {
        match linkage {
            Linkage::Appending | Linkage::Common => Err(ParseError::Message {
                message: "invalid function linkage type".into(),
                loc: DiagLoc::span(loc),
            }),
            Linkage::ExternalWeak if is_define => Err(ParseError::Message {
                message: "invalid linkage for function definition".into(),
                loc: DiagLoc::span(loc),
            }),
            Linkage::Private
            | Linkage::Internal
            | Linkage::AvailableExternally
            | Linkage::LinkOnceAny
            | Linkage::LinkOnceOdr
            | Linkage::WeakAny
            | Linkage::WeakOdr
                if !is_define =>
            {
                Err(ParseError::Message {
                    message: "invalid linkage for function declaration".into(),
                    loc: DiagLoc::span(loc),
                })
            }
            _ => Ok(()),
        }
    }

    fn parse_optional_visibility(&mut self) -> ParseResult<Visibility> {
        if self.eat_keyword(Keyword::Default)? {
            Ok(Visibility::Default)
        } else if self.eat_keyword(Keyword::Hidden)? {
            Ok(Visibility::Hidden)
        } else if self.eat_keyword(Keyword::Protected)? {
            Ok(Visibility::Protected)
        } else {
            Ok(Visibility::Default)
        }
    }

    fn parse_optional_dll_storage_class(&mut self) -> ParseResult<DllStorageClass> {
        if self.eat_keyword(Keyword::Dllimport)? {
            Ok(DllStorageClass::DllImport)
        } else if self.eat_keyword(Keyword::Dllexport)? {
            Ok(DllStorageClass::DllExport)
        } else {
            Ok(DllStorageClass::Default)
        }
    }

    fn parse_optional_dso_locality(&mut self) -> ParseResult<llvmkit_ir::DsoLocality> {
        if self.eat_keyword(Keyword::DsoLocal)? {
            Ok(llvmkit_ir::DsoLocality::Local)
        } else if self.eat_keyword(Keyword::DsoPreemptable)? {
            Ok(llvmkit_ir::DsoLocality::Preemptable)
        } else {
            Ok(llvmkit_ir::DsoLocality::Default)
        }
    }

    /// The `dso_local` / visibility / DLL-storage-class run that follows a
    /// linkage keyword, in upstream's order.
    ///
    /// Mirrors the tail of `LLParser::parseOptionalLinkage`, including its
    /// cross-clause rejection: `dllimport` names a symbol that lives in
    /// another module, which is exactly what `dso_local` denies.
    ///
    /// The order is load-bearing rather than cosmetic. `AsmWriter.cpp`'s
    /// `printFunction` and `printGlobal` emit these three in the same
    /// sequence, so reading them in any other one makes LLVM's own output
    /// unparseable — `define dso_local hidden void @f()` is the canonical
    /// spelling, not an exotic one.
    fn parse_optional_preemption_visibility_dll(
        &mut self,
    ) -> ParseResult<(llvmkit_ir::DsoLocality, Visibility, DllStorageClass)> {
        let dso_locality = self.parse_optional_dso_locality()?;
        let visibility = self.parse_optional_visibility()?;
        let dll_storage_class = self.parse_optional_dll_storage_class()?;
        if dso_locality == llvmkit_ir::DsoLocality::Local
            && dll_storage_class == DllStorageClass::DllImport
        {
            return Err(self.message("dso_location and DLL-StorageClass mismatch"));
        }
        Ok((dso_locality, visibility, dll_storage_class))
    }

    fn parse_optional_function_unnamed_addr(&mut self) -> ParseResult<UnnamedAddr> {
        if self.eat_keyword(Keyword::UnnamedAddr)? {
            Ok(UnnamedAddr::Global)
        } else if self.eat_keyword(Keyword::LocalUnnamedAddr)? {
            Ok(UnnamedAddr::Local)
        } else {
            Ok(UnnamedAddr::None)
        }
    }

    /// `attributes #N = { … }`. Ports `LLParser::parseUnnamedAttrGrp`.
    ///
    /// `AttrGrpLoc` is captured at the `attributes` keyword, *before* it is
    /// consumed, and is what both the redefinition error and
    /// `attribute group has no attributes` report against — llvmkit used to
    /// anchor them one token later, on the `#N`.
    fn parse_unnamed_attr_group(&mut self) -> ParseResult<()> {
        let loc = self.loc();
        self.expect_keyword(Keyword::Attributes, "'attributes'")?;
        let id = match self.peek() {
            Token::AttrGrpId(id) => {
                let id = *id;
                self.bump()?;
                id
            }
            _ => return Err(self.expected("attribute group id")),
        };
        self.expect_punct(PunctKind::Equal, "'=' here")?;
        self.expect_punct(PunctKind::LBrace, "'{' here")?;
        let mut storage = AttributeStorage::new();
        // A `#N` reference inside the group is rejected by the loop itself, so
        // it can never return group ids here.
        self.parse_fn_attribute_value_pairs(
            &mut storage,
            AttrIndex::Function,
            AttrListContext::AttributeGroup,
        )?;
        self.expect_punct(PunctKind::RBrace, "end of attribute group")?;
        if storage.is_empty() {
            return Err(ParseError::Message {
                message: "attribute group has no attributes".into(),
                loc: DiagLoc::span(loc),
            });
        }
        self.numbered_attr_groups
            .add(id, storage.clone())
            .map_err(|source| ParseError::InvalidSlotId {
                source,
                loc: DiagLoc::span(loc),
            })?;
        self.module.set_attribute_group(id, storage);
        Ok(())
    }

    fn attr_kind_for_keyword(keyword: Keyword) -> Option<AttrKind> {
        Some(match keyword {
            Keyword::Zeroext => AttrKind::Zext,
            Keyword::Signext => AttrKind::Sext,
            Keyword::Noundef => AttrKind::NoUndef,
            Keyword::Nonnull => AttrKind::NonNull,
            Keyword::Noalias => AttrKind::NoAlias,
            Keyword::Nounwind => AttrKind::NoUnwind,
            Keyword::Nocreateundeforpoison => AttrKind::NoCreateUndefOrPoison,
            Keyword::Nocallback => AttrKind::NoCallback,
            Keyword::Noduplicate => AttrKind::NoDuplicate,
            Keyword::Nomerge => AttrKind::NoMerge,
            Keyword::Convergent => AttrKind::Convergent,
            Keyword::Cold => AttrKind::Cold,
            Keyword::Strictfp => AttrKind::StrictFp,
            Keyword::Immarg => AttrKind::ImmArg,
            Keyword::Readnone => AttrKind::ReadNone,
            Keyword::Readonly => AttrKind::ReadOnly,
            Keyword::Alwaysinline => AttrKind::AlwaysInline,
            Keyword::Noinline => AttrKind::NoInline,
            Keyword::Writeonly => AttrKind::WriteOnly,
            Keyword::Returned => AttrKind::Returned,
            Keyword::Nofree => AttrKind::NoFree,
            Keyword::Writable => AttrKind::Writable,
            Keyword::Noreturn => AttrKind::NoReturn,
            Keyword::Willreturn => AttrKind::WillReturn,
            Keyword::Mustprogress => AttrKind::MustProgress,
            Keyword::Nosync => AttrKind::NoSync,
            Keyword::Optnone => AttrKind::OptimizeNone,
            Keyword::Optsize => AttrKind::OptimizeForSize,
            Keyword::Speculatable => AttrKind::Speculatable,
            Keyword::Inreg => AttrKind::InReg,
            Keyword::Nest => AttrKind::Nest,
            Keyword::Swiftself => AttrKind::SwiftSelf,
            Keyword::Norecurse => AttrKind::NoRecurse,
            Keyword::Hot => AttrKind::Hot,
            Keyword::Inlinehint => AttrKind::InlineHint,
            Keyword::SanitizeAddress => AttrKind::SanitizeAddress,
            Keyword::Nonlazybind => AttrKind::NonLazyBind,
            Keyword::Minsize => AttrKind::MinSize,
            Keyword::Ssp => AttrKind::StackProtect,
            Keyword::Sspstrong => AttrKind::StackProtectStrong,
            Keyword::Sspreq => AttrKind::StackProtectReq,
            // Every remaining plain enum attribute in `Attributes.td`. The
            // lexer already had each keyword and `llvmkit-ir` most of the
            // kinds; only this mapping — upstream's `tokenToAttribute` — was
            // missing them, so each parsed as "not an attribute" and ended
            // whatever list it appeared in.
            Keyword::Allocalign => AttrKind::AllocAlign,
            Keyword::Allocptr => AttrKind::AllocatedPointer,
            Keyword::Builtin => AttrKind::Builtin,
            Keyword::DisableSanitizerInstrumentation => AttrKind::DisableSanitizerInstrumentation,
            Keyword::CoroElideSafe => AttrKind::CoroElideSafe,
            Keyword::CoroOnlyDestroyWhenComplete => AttrKind::CoroDestroyOnlyWhenComplete,
            Keyword::DeadOnReturn => AttrKind::DeadOnReturn,
            Keyword::DeadOnUnwind => AttrKind::DeadOnUnwind,
            Keyword::FnRetThunkExtern => AttrKind::FnRetThunkExtern,
            Keyword::HybridPatchable => AttrKind::HybridPatchable,
            Keyword::Jumptable => AttrKind::JumpTable,
            Keyword::Naked => AttrKind::Naked,
            Keyword::Nobuiltin => AttrKind::NoBuiltin,
            Keyword::NocfCheck => AttrKind::NoCfCheck,
            Keyword::Nodivergencesource => AttrKind::NoDivergenceSource,
            Keyword::Noext => AttrKind::NoExt,
            Keyword::Noimplicitfloat => AttrKind::NoImplicitFloat,
            Keyword::Noprofile => AttrKind::NoProfile,
            Keyword::Noredzone => AttrKind::NoRedZone,
            Keyword::NosanitizeBounds => AttrKind::NoSanitizeBounds,
            Keyword::NosanitizeCoverage => AttrKind::NoSanitizeCoverage,
            Keyword::NullPointerIsValid => AttrKind::NullPointerIsValid,
            Keyword::Optdebug => AttrKind::OptimizeForDebugging,
            Keyword::Optforfuzzing => AttrKind::OptForFuzzing,
            Keyword::Presplitcoroutine => AttrKind::PresplitCoroutine,
            Keyword::ReturnsTwice => AttrKind::ReturnsTwice,
            Keyword::Safestack => AttrKind::SafeStack,
            Keyword::SanitizeAllocToken => AttrKind::SanitizeAllocToken,
            Keyword::SanitizeHwaddress => AttrKind::SanitizeHwAddress,
            Keyword::SanitizeMemory => AttrKind::SanitizeMemory,
            Keyword::SanitizeMemtag => AttrKind::SanitizeMemTag,
            Keyword::SanitizeNumericalStability => AttrKind::SanitizeNumericalStability,
            Keyword::SanitizeRealtime => AttrKind::SanitizeRealtime,
            Keyword::SanitizeRealtimeBlocking => AttrKind::SanitizeRealtimeBlocking,
            Keyword::SanitizeThread => AttrKind::SanitizeThread,
            Keyword::SanitizeType => AttrKind::SanitizeType,
            Keyword::Shadowcallstack => AttrKind::ShadowCallStack,
            Keyword::Skipprofile => AttrKind::SkipProfile,
            Keyword::SpeculativeLoadHardening => AttrKind::SpeculativeLoadHardening,
            Keyword::Swiftasync => AttrKind::SwiftAsync,
            Keyword::Swifterror => AttrKind::SwiftError,
            _ => return None,
        })
    }
    /// `parseFunctionHeader`'s `parseFnAttributeValuePairs(FuncAttrs,
    /// FwdRefAttrGrps, false, BuiltinLoc)` term: **one** call, entered
    /// unconditionally, ended by `tokenToAttribute` answering
    /// `Attribute::None` on the first token that is not an attribute.
    ///
    /// llvmkit used to gate this on `is_attr_start`, a hand-maintained second
    /// copy of the loop's arm list, and to call the loop repeatedly until the
    /// predicate went false. Both are gone: a keyword missing from a lookahead
    /// is not rejected, it makes the whole list invisible — `define void @f()
    /// uwtable {` failed with `expected '{' to open function body` for exactly
    /// that reason — and a re-entered loop restarts
    /// `parse_fn_attribute_value_pairs`'s `legacy_memory` accumulator, which
    /// upstream intersects across the *whole* list.
    ///
    /// `align N` is parsed here as an `AttributeList` entry, exactly as
    /// upstream does, and moved to the alignment field by
    /// `parse_optional_function_suffix`. llvmkit used to exclude it from this
    /// loop and leave it to the suffix, which is invisible while the suffix is
    /// order-free but breaks `align 8 section "x"` once the clause chain is a
    /// fixed sequence.
    fn parse_optional_function_header_attrs(
        &mut self,
        attrs: &mut AttributeStorage,
    ) -> ParseResult<ParsedAttrList> {
        self.parse_fn_attribute_value_pairs(
            attrs,
            AttrIndex::Function,
            AttrListContext::FunctionHeader,
        )
    }

    /// The fixed clause chain `parseFunctionHeader` runs after the argument
    /// list, in its order and with its arity: each clause is tried **once**,
    /// in sequence.
    ///
    /// That makes the order contractual. `define void @f() gc "x" section "y"`
    /// is rejected upstream — `section` is looked for before `gc`, so by the
    /// time `gc` has been eaten the `section` arm is already behind us, and
    /// the leftover `section` fails against the `{`. llvmkit looped over the
    /// clauses instead, in any order and any number of times, so it also
    /// accepted `section "a" section "b"`.
    ///
    /// `align` appears in two places on purpose. The attribute loop parses it
    /// first (the `Alignment` exemption in
    /// [`Self::check_attribute_position`] is upstream's own hack, comment
    /// included) and it is then *moved* to the alignment field, which is what
    /// makes `define void @f() align 8 section "x"` legal; the clause below
    /// covers the post-`comdat` position. The two spellings differ in one
    /// detail — `parseOptionalAlignment` is called with `AllowParens = true`
    /// from the attribute loop and `false` here, so `align(8)` is an
    /// attribute-position-only form.
    ///
    /// Metadata is deliberately absent: upstream reads a *declaration*'s
    /// attachments before the header (`parseDeclare`) and a *definition*'s
    /// after it (`parseOptionalFunctionMetadata`), and neither eats a comma.
    fn parse_optional_function_suffix(
        &mut self,
        attrs: &mut AttributeStorage,
    ) -> ParseResult<FunctionSuffix<'ctx, B>> {
        let parsed_attrs = self.parse_optional_function_header_attrs(attrs)?;
        let mut suffix = FunctionSuffix {
            attr_groups: parsed_attrs.groups,
            builtin_loc: parsed_attrs.builtin_loc,
            ..FunctionSuffix::default()
        };
        // "If the alignment was parsed as an attribute, move to the alignment
        // field." — `parseFunctionHeader`, verbatim.
        if let Some(value) = attrs.int_value(AttrIndex::Function, AttrKind::Alignment) {
            suffix.align = MaybeAlign::new(
                Align::new(value).map_err(|e| self.builder_err("function align", e))?,
            );
            attrs.remove(AttrIndex::Function, AttrKind::Alignment);
        }
        if self.eat_keyword(Keyword::Section)? {
            suffix.section = Some(self.parse_string_constant("section name")?);
        }
        if self.eat_keyword(Keyword::Partition)? {
            suffix.partition = Some(self.parse_string_constant("partition name")?);
        }
        if self.eat_keyword(Keyword::Comdat)? {
            suffix.comdat = if self.eat_punct(PunctKind::LParen)? {
                let name = match self.peek() {
                    Token::ComdatVar(bytes) => std::str::from_utf8(bytes.as_ref())
                        .map_err(|_| self.expected("valid UTF-8 comdat name"))?
                        .to_owned(),
                    _ => return Err(self.expected("comdat variable")),
                };
                self.bump()?;
                self.expect_punct(PunctKind::RParen, "')' after comdat")?;
                Some(Some(name))
            } else {
                Some(None)
            };
        }
        if matches!(self.peek(), Token::Kw(Keyword::Align)) {
            let value = self.parse_optional_alignment_value(false)?;
            suffix.align = MaybeAlign::new(
                Align::new(value).map_err(|e| self.builder_err("function align", e))?,
            );
        }
        if self.eat_keyword(Keyword::Gc)? {
            suffix.gc = Some(self.parse_string_constant("gc name")?);
        }
        if self.eat_keyword(Keyword::Prefix)? {
            suffix.prefix_data = Some(self.parse_global_type_and_value()?);
        }
        if self.eat_keyword(Keyword::Prologue)? {
            suffix.prologue_data = Some(self.parse_global_type_and_value()?);
        }
        if self.eat_keyword(Keyword::Personality)? {
            suffix.personality_fn = Some(self.parse_personality_fn()?);
        }
        Ok(suffix)
    }

    /// `(!dbg !57)*` after a function *definition*'s header. Mirrors
    /// `LLParser::parseOptionalFunctionMetadata`, which loops on `MetadataVar`
    /// directly and never eats a comma — `define i32 @f(), !dbg !3 {` is
    /// invalid, and llvmkit used to accept it.
    fn parse_optional_function_metadata(
        &mut self,
    ) -> ParseResult<Vec<(MetadataAttachmentKind, MetadataId<B>)>> {
        let mut attachments = Vec::new();
        while matches!(self.peek(), Token::MetadataVar(_)) {
            attachments.push(self.parse_named_metadata_attachment()?);
        }
        Ok(attachments)
    }

    fn parse_fn_attribute_value_pairs(
        &mut self,
        out: &mut AttributeStorage,
        index: AttrIndex,
        context: AttrListContext,
    ) -> ParseResult<ParsedAttrList> {
        let mut builtin_loc = None;
        let mut groups = Vec::new();
        // The legacy memory keywords **intersect** into one accumulator —
        // `upgradeMemoryAttr` is `ME &= MemoryEffects::X()` per keyword, with
        // `ME` starting at `unknown()` and emitted once after the whole list.
        // `readonly writeonly` is one `memory(none)`, not two attributes.
        let mut legacy_memory = MemoryEffects::unknown();
        loop {
            // `Loc` in both upstream loops: the attribute's own keyword, which
            // is what the position diagnostics report against.
            let attr_loc = self.loc();
            match self.peek() {
                // `}` is the loop's only unconditional exit, in both contexts.
                Token::RBrace => break,
                Token::LBrace | Token::Comma | Token::Eof if !context.in_attr_group() => break,
                Token::AttrGrpId(id) if context == AttrListContext::FunctionHeader => {
                    let id = *id;
                    self.bump()?;
                    groups.push(id);
                }
                Token::AttrGrpId(_) if context == AttrListContext::AttributeGroup => {
                    return Err(self.message(
                        "cannot have an attribute group reference in an attribute group",
                    ));
                }
                // `parseOptionalParamOrReturnAttrs` has no `#N` arm at all, so
                // the token simply is not an attribute and ends the list.
                Token::AttrGrpId(_) => break,
                Token::StringConstant(_) => {
                    let key = self.parse_string_constant("attribute string key")?;
                    let value = if self.eat_punct(PunctKind::Equal)? {
                        self.parse_string_constant("attribute string value")?
                    } else {
                        String::new()
                    };
                    out.add(index, Attribute::<B>::string(key, value));
                }
                // `parseOptionalParamOrReturnAttrs`'s `if (Token ==
                // lltok::kw_nocapture) { Lex.Lex();
                // B.addCapturesAttr(CaptureInfo::none()); continue; }`, which
                // sits between the string-attribute arm and `tokenToAttribute`.
                // LLVM 22 has no `Attribute::NoCapture`: `nocapture` is spelled
                // `captures(none)` in the IR and prints that way, which
                // `test/Assembler/auto_upgrade_intrinsics.ll`'s
                // `CHECK: declare void @llvm.lifetime.start.p0(ptr captures(none))`
                // pins on `llvm-as | llvm-dis` output.
                //
                // The arm is `ParamOrReturn` only, exactly as upstream's is:
                // `tokenToAttribute` has no `nocapture` case, so a `nocapture`
                // in a function-attribute list or an attribute group is not an
                // attribute at all and falls through to the loop's end / to
                // `unterminated attribute group`.
                //
                // **One half of the arm is deliberately not ported.** Upstream
                // `continue`s before the `canUseAsParamAttr` /
                // `canUseAsRetAttr` checks, so its parser accepts `nocapture`
                // on a *return* value and leaves the rejection to
                // `Verifier::verifyFunctionAttrs` (`Attribute
                // 'captures(none)' does not apply to function return values`).
                // llvmkit has no `verifyFunctionAttrs` — `docs/divergences.md`
                // entry 23 — so bypassing the position check here would trade
                // a wrong-layer rejection for an accepts-invalid. The check
                // stays; `Captures` is `[ParamAttr]` in `Attributes.td`, so the
                // verdict matches and only the layer and the wording do not.
                Token::Kw(Keyword::Nocapture) if context == AttrListContext::ParamOrReturn => {
                    self.bump()?;
                    let attr = Attribute::<B>::Captures(llvmkit_ir::CaptureInfo::none());
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Align) => {
                    // Inside a group the grammar is `align = N`, read with
                    // `parseUInt32` and given no `error()` at all: upstream's
                    // `parseEnumAttribute` case `Attribute::Alignment` hands the
                    // value straight to `Align(Value)`, whose `assert(Value >
                    // 0)` / `assert(isPowerOf2_64(Value))` are the only
                    // rejections, and `AttrBuilder::addAlignmentAttr` adds
                    // `assert(*Align <= Value::MaximumAlignment)`.
                    //
                    // llvmkit raises no runtime panics in production paths, so
                    // an assert is ported as a diagnostic, not as a crash:
                    // `check_alignment_value` reuses `parseOptionalAlignment`'s
                    // two `error()` texts for the same three values. Against an
                    // assertions-enabled `llvm-as` that is the same accept /
                    // reject set with a diagnostic instead of an abort; against
                    // a release one, where the asserts are compiled out and
                    // `align = 3` is silently rounded to 2 by `Log2_64`, it is
                    // deliberate hardening. `docs/divergences.md` carried it
                    // as a rejects-valid row until this comment said so; ids in
                    // that file are re-used, so the row is named, not numbered.
                    let value = if context.in_attr_group() {
                        self.bump()?;
                        self.expect_punct(PunctKind::Equal, "'=' here")?;
                        let value_loc = self.loc();
                        let value = u64::from(self.parse_uint32()?);
                        self.check_alignment_value(value, value_loc)?;
                        value
                    } else {
                        self.parse_optional_alignment_value(true)?
                    };
                    let attr = Attribute::<B>::int(AttrKind::Alignment, value)
                        .ok_or_else(|| self.expected("attribute"))?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Alignstack) => {
                    let value = if context.in_attr_group() {
                        self.bump()?;
                        self.expect_punct(PunctKind::Equal, "'=' here")?;
                        let value_loc = self.loc();
                        let value = u64::from(self.parse_uint32()?);
                        // The group spelling reaches `MaybeAlign(uint64_t)`,
                        // whose `assert(Value == 0 || isPowerOf2_64(Value))`
                        // makes zero *well defined*: it yields `nullopt` and
                        // `addStackAlignmentAttr` returns without adding
                        // anything. Upstream's `parseOptionalStackAlignment`,
                        // reached only by the `alignstack(N)` spelling below,
                        // is the one that rejects zero outright.
                        if value == 0 {
                            continue;
                        }
                        if !value.is_power_of_two() {
                            return Err(
                                self.message_at(value_loc, "stack alignment is not a power of two")
                            );
                        }
                        value
                    } else {
                        // `parseOptionalStackAlignment` runs
                        // `!isPowerOf2_32(Alignment)`, which is false for zero,
                        // so the `alignstack(0)` spelling never reaches the
                        // `MaybeAlign` arm above.
                        self.parse_stack_alignment_value()?
                    };
                    // `assert(*Align <= 0x100 && "Alignment too large.")` in
                    // `AttrBuilder::addStackAlignmentAttr`, which both spellings
                    // reach. Ported as a diagnostic for the reason given on the
                    // `align` arm above, anchored — like every other position
                    // diagnostic in this loop — at the attribute's own keyword;
                    // the text is llvmkit's own, because upstream states this
                    // one only as an assertion string.
                    if value > 0x100 {
                        return Err(self.message_at(attr_loc, "stack alignment is too large"));
                    }
                    let attr = Attribute::<B>::int(AttrKind::StackAlignment, value)
                        .ok_or_else(|| self.expected("attribute"))?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Memory) => {
                    let attr = self.parse_memory_attribute()?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Nofpclass) => {
                    let attr = self.parse_nofpclass_attribute()?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(keyword)
                    if index == AttrIndex::Function
                        && Self::legacy_memory_effects(*keyword).is_some() =>
                {
                    let effects = Self::legacy_memory_effects(*keyword)
                        .ok_or_else(|| self.expected("memory attribute"))?;
                    self.bump()?;
                    legacy_memory &= effects;
                }
                Token::Kw(Keyword::Uwtable) => {
                    self.bump()?;
                    let kind = if self.eat_punct(PunctKind::LParen)? {
                        let kind = if self.eat_keyword(Keyword::Sync)? {
                            1
                        } else if self.eat_keyword(Keyword::Async)? {
                            2
                        } else {
                            return Err(self.message("expected unwind table kind"));
                        };
                        self.expect_punct(PunctKind::RParen, "')'")?;
                        kind
                    } else {
                        2
                    };
                    let attr = Attribute::<B>::int(AttrKind::UwTable, kind)
                        .ok_or_else(|| self.expected("attribute"))?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(kw @ (Keyword::Dereferenceable | Keyword::DereferenceableOrNull)) => {
                    let kind = if *kw == Keyword::Dereferenceable {
                        AttrKind::Dereferenceable
                    } else {
                        AttrKind::DereferenceableOrNull
                    };
                    self.bump()?;
                    self.expect_punct(PunctKind::LParen, "'('")?;
                    let bytes_loc = self.loc();
                    let bytes = self.parse_uint64()?;
                    // The non-zero check runs *after* the closing paren, so a
                    // malformed `dereferenceable(0` reports `expected ')'`.
                    self.expect_punct(PunctKind::RParen, "')'")?;
                    if bytes == 0 {
                        return Err(
                            self.message_at(bytes_loc, "dereferenceable bytes must be non-zero")
                        );
                    }
                    let attr = Attribute::<B>::int(kind, bytes)
                        .ok_or_else(|| self.expected("attribute"))?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(
                    kw @ (Keyword::Byval
                    | Keyword::Byref
                    | Keyword::Inalloca
                    | Keyword::Sret
                    | Keyword::Preallocated
                    | Keyword::Elementtype),
                ) => {
                    let kind = match kw {
                        Keyword::Byval => AttrKind::ByVal,
                        Keyword::Byref => AttrKind::ByRef,
                        Keyword::Inalloca => AttrKind::InAlloca,
                        Keyword::Sret => AttrKind::StructRet,
                        Keyword::Preallocated => AttrKind::Preallocated,
                        _ => AttrKind::ElementType,
                    };
                    self.bump()?;
                    // `parseRequiredTypeAttr`'s three steps, with its bare
                    // texts: `byref-parse-error-{0,5}.ll` pin the `'('` and
                    // `-{1,3}.ll` pin `parse_type`'s own `expected type`.
                    self.expect_punct(PunctKind::LParen, "'('")?;
                    let ty = self.parse_type(false)?;
                    self.expect_punct(PunctKind::RParen, "')'")?;
                    let attr = Attribute::<B>::type_attr(kind, ty)
                        .ok_or_else(|| self.expected("attribute"))?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Captures) => {
                    let attr = self.parse_captures_attribute()?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Range) => {
                    let attr = self.parse_range_attribute()?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Initializes) => {
                    let attr = self.parse_initializes_attribute()?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Allocsize) => {
                    let attr = self.parse_alloc_size_attribute()?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::VscaleRange) => {
                    let attr = self.parse_vscale_range_attribute()?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Allockind) => {
                    let attr = self.parse_alloc_kind_attribute()?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                Token::Kw(keyword) => {
                    let Some(kind) = Self::attr_kind_for_keyword(*keyword) else {
                        if context.in_attr_group() {
                            return Err(self.message("unterminated attribute group"));
                        }
                        break;
                    };
                    self.bump()?;
                    // `BuiltinLoc = Lex.getLoc()` in upstream's `kw_builtin`
                    // arm. The rejection itself belongs to the *caller*: a
                    // call site may legally carry `builtin`.
                    if kind == AttrKind::Builtin {
                        builtin_loc = Some(attr_loc);
                    }
                    let attr = Attribute::<B>::enum_attr(kind)
                        .ok_or_else(|| self.expected("attribute"))?;
                    self.check_attribute_position(index, &attr, attr_loc)?;
                    out.add(index, attr);
                }
                // `tokenToAttribute` answers `Attribute::None` for anything
                // that is not an attribute keyword. Inside a group that is a
                // hard error; elsewhere it just ends the list.
                _ if context.in_attr_group() => {
                    return Err(self.message("unterminated attribute group"));
                }
                _ => break,
            }
        }
        // `if (ME != MemoryEffects::unknown()) B.addMemoryAttr(ME);` — and
        // because `addAttributeImpl` replaces by kind, a legacy keyword
        // anywhere in the list overwrites an explicit `memory(...)` from the
        // same list, in either source order.
        if legacy_memory != MemoryEffects::unknown() {
            out.add(index, Attribute::<B>::memory(legacy_memory));
        }
        Ok(ParsedAttrList {
            groups,
            builtin_loc,
        })
    }

    /// `range(<ty> <n>,<n>)`. Ports `LLParser::parseRangeAttr`.
    ///
    /// Two placements are contractual and were both off by one token before:
    /// `the range must have integer type!` is `error(TyLoc, …)`, anchored at
    /// the **first** token of the type rather than the one after it; and the
    /// empty-set check runs *before* the closing `)` is required, so it
    /// anchors on that `)`.
    ///
    /// The type check is `Type::isIntegerTy`, which is false for a vector of
    /// integers — `range(<4 x i32> 0, 0)` is the fixture that says so.
    fn parse_range_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Range, "'range'")?;
        self.expect_punct(PunctKind::LParen, "'('")?;
        let type_loc = self.loc();
        let ty = self.parse_type(false)?;
        let TypeKind::Integer { bits } = ty.kind() else {
            return Err(self.message_at(type_loc, "the range must have integer type!"));
        };
        let lower = self.parse_sized_apsint(bits)?;
        self.expect_punct(PunctKind::Comma, "','")?;
        let upper = self.parse_sized_apsint(bits)?;
        if lower.eq_ap_int(&upper) && !lower.is_zero() {
            return Err(self.message("the range represent the empty set but limits aren't 0!"));
        }
        self.expect_punct(PunctKind::RParen, "')'")?;
        let Some(attr) = Attribute::<B>::range(ty, lower, upper) else {
            // `Attribute::range` refuses a non-integer type, bounds whose width
            // disagrees with it, and the empty set. The first and third were
            // just checked, and both bounds are built at `bits`.
            unreachable!("range bounds are built at the type's width and are not the empty set")
        };
        Ok(attr)
    }

    /// `initializes((Lo1,Hi1),(Lo2,Hi2),...)`. Ports
    /// `LLParser::parseInitializesAttr`.
    ///
    /// Three details of the loop are contractual:
    ///
    /// - the list is one-or-more with no trailing comma, so `initializes()`
    ///   fails on the *inner* `(` and `initializes((0,4),)` does too;
    /// - `Lower == Upper` is rejected **before** the inner `)` is required,
    ///   which puts that diagnostic on the token after `Upper`;
    /// - the ordering invariant runs only after the outer `)` is consumed, so
    ///   `Invalid (unordered or overlapping) range list` lands on whatever
    ///   follows the whole attribute.
    ///
    /// Unlike `parseRangeAttr` there is **no** width check: upstream calls
    /// `APSInt::extend(64)` directly, which asserts in a debug build on a
    /// literal wider than 64 bits and misbehaves in a release one. No fixture
    /// covers that input and llvmkit raises no runtime panics, so the bound
    /// is truncated to 64 bits — the closest defined reading of upstream's
    /// release behaviour.
    fn parse_initializes_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Initializes, "'initializes'")?;
        self.expect_punct(PunctKind::LParen, "'('")?;

        let mut ranges = Vec::new();
        loop {
            self.expect_punct(PunctKind::LParen, "'('")?;
            let lower = self.parse_apsint_at_64_bits()?;
            self.expect_punct(PunctKind::Comma, "','")?;
            let upper = self.parse_apsint_at_64_bits()?;
            if lower.eq_ap_int(&upper) {
                return Err(self.message("the range should not represent the full or empty set!"));
            }
            self.expect_punct(PunctKind::RParen, "')'")?;
            let Ok(range) = llvmkit_ir::ConstantRange::new(lower, upper) else {
                // `ConstantRange::new` rejects only mismatched widths and an
                // equal pair that is neither the minimum nor the maximum.
                // Both bounds are 64 bits, and equality was just rejected.
                unreachable!("initializes bounds are 64-bit and unequal")
            };
            ranges.push(range);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }

        self.expect_punct(PunctKind::RParen, "')'")?;
        Attribute::<B>::initializes(ranges)
            .ok_or_else(|| self.message("Invalid (unordered or overlapping) range list"))
    }

    /// `parseInitializesAttr`'s local `ParseAPSInt` lambda.
    fn parse_apsint_at_64_bits(&mut self) -> ParseResult<ApInt> {
        if !matches!(self.peek(), Token::IntegerLit(_)) {
            return Err(self.expected("integer"));
        }
        Ok(self.parse_int_literal()?.extend_or_truncate(64))
    }

    /// `parseRangeAttr`'s local `ParseAPSInt` lambda: the token must fit the
    /// declared type, then widens by its own signedness.
    ///
    /// The width check reads the *token's* width, which is why
    /// [`Self::parse_int_literal`] must not build the literal at the
    /// destination width — doing so makes this question unaskable, and
    /// `range(i8 300, 0)` silently means `range(i8 44, 0)`.
    fn parse_sized_apsint(&mut self, bit_width: u32) -> ParseResult<ApInt> {
        if !matches!(self.peek(), Token::IntegerLit(_)) {
            return Err(self.expected("integer"));
        }
        // Upstream checks the width before `Lex.Lex()`, so its `tokError`
        // names the literal; the span is captured here for the same reason.
        let literal_loc = self.loc();
        let parsed = self.parse_int_literal()?;
        if parsed.bit_width() > bit_width {
            return Err(self.message_at(
                literal_loc,
                "integer is too large for the bit width of specified type",
            ));
        }
        Ok(parsed.extend_or_truncate(bit_width))
    }

    /// The position check both attribute loops run on every attribute they
    /// parse: `parseFnAttributeValuePairs`'s `canUseAsFnAttr` guard and
    /// `parseOptionalParamOrReturnAttrs`' `canUseAsParamAttr` /
    /// `canUseAsRetAttr` pair.
    ///
    /// Upstream accumulates these with `HaveError |= error(...)` and keeps
    /// parsing, so a list with several misplaced attributes reports several
    /// times. llvmkit reports the first and stops — same verdict for the
    /// module, one diagnostic instead of several.
    fn check_attribute_position(
        &self,
        index: AttrIndex,
        attr: &Attribute<'ctx, B>,
        loc: Span,
    ) -> ParseResult<()> {
        let Some(kind) = attr.kind() else {
            // A string attribute; upstream's check is on the enum kind only.
            return Ok(());
        };
        let positions = kind.positions();
        let (allowed, message) = match index {
            // "As a hack, we allow function alignment to be initially parsed
            // as an attribute on a function declaration/definition or added to
            // an attribute group and later moved to the alignment field."
            AttrIndex::Function => (
                positions.function || kind == AttrKind::Alignment,
                "this attribute does not apply to functions",
            ),
            AttrIndex::Return => (
                positions.return_value,
                "this attribute does not apply to return values",
            ),
            AttrIndex::Param(_) => (
                positions.parameter,
                "this attribute does not apply to parameters",
            ),
        };
        if allowed {
            Ok(())
        } else {
            Err(self.message_at(loc, message))
        }
    }

    /// `captures(address, ret: provenance)`. Ports `LLParser::parseCapturesAttr`.
    ///
    /// The loop shape is contractual, not incidental:
    ///
    /// - `ret:` may appear at **any** position, not only first, and at most
    ///   once. Once seen, every later component accumulates into the return
    ///   bucket — there is no way back.
    /// - The "no `none` beside a component" guard is per *bucket*: it resets
    ///   at `ret:`, so `captures(address, ret: none)` is legal.
    /// - Components accumulate with `|=`, so `captures(address, address)`
    ///   collapses silently.
    /// - A missing `ret:` means the return bucket **equals** the other bucket
    ///   (`Ret.value_or(Other)`), not that it is empty.
    /// - `captures()` is rejected: the first iteration finds `)` where a
    ///   component keyword must be, so it is the "expected one of …" message.
    fn parse_captures_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        // `ret:` would otherwise lex as a *label*. Upstream sets
        // `setIgnoreColonInIdentifiers(true)` before the first token inside
        // the attribute and clears it with a `scope_exit`; the split here is
        // that reset, so an early `?` cannot leave the flag on. Same shape as
        // `parse_memory_attribute`.
        self.lex.ignore_colon_in_idents = true;
        let result = self.parse_captures_attribute_body();
        self.lex.ignore_colon_in_idents = false;
        result
    }

    fn parse_captures_attribute_body(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Captures, "'captures'")?;
        self.expect_punct(PunctKind::LParen, "'('")?;
        let mut other = llvmkit_ir::CaptureComponents::NONE;
        let mut ret: Option<llvmkit_ir::CaptureComponents> = None;
        let mut seen_component = false;
        loop {
            // `ret` is the return *opcode* token, here and upstream: LLVM's
            // `INSTKEYWORD(ret, Ret)` is what `lltok::kw_ret` names, and
            // `parseCapturesAttr` eats that same token.
            if matches!(self.peek(), Token::Instruction(Opcode::Ret)) {
                self.bump()?;
                // The colon is consumed *before* the duplicate check, so the
                // diagnostic lands on the token after it.
                self.expect_punct(PunctKind::Colon, "':'")?;
                if ret.is_some() {
                    return Err(self.message("duplicate 'ret' location"));
                }
                ret = Some(llvmkit_ir::CaptureComponents::NONE);
                seen_component = false;
            }
            let current = match ret.as_mut() {
                Some(ret) => ret,
                None => &mut other,
            };
            if self.eat_keyword(Keyword::None)? {
                if seen_component {
                    return Err(self.message("cannot use 'none' with other component"));
                }
                *current = llvmkit_ir::CaptureComponents::NONE;
            } else {
                if seen_component && current.captures_nothing() {
                    return Err(self.message("cannot use 'none' with other component"));
                }
                let component = match self.peek() {
                    Token::Kw(Keyword::AddressIsNull) => {
                        llvmkit_ir::CaptureComponents::ADDRESS_IS_NULL
                    }
                    Token::Kw(Keyword::Address) => llvmkit_ir::CaptureComponents::ADDRESS,
                    Token::Kw(Keyword::Provenance) => llvmkit_ir::CaptureComponents::PROVENANCE,
                    Token::Kw(Keyword::ReadProvenance) => {
                        llvmkit_ir::CaptureComponents::READ_PROVENANCE
                    }
                    _ => {
                        return Err(self.message(
                            "expected one of 'none', 'address', 'address_is_null', 'provenance' or 'read_provenance'",
                        ));
                    }
                };
                self.bump()?;
                *current |= component;
            }
            seen_component = true;
            if self.eat_punct(PunctKind::RParen)? {
                break;
            }
            self.expect_punct(PunctKind::Comma, "',' or ')'")?;
        }
        Ok(Attribute::Captures(llvmkit_ir::CaptureInfo {
            other,
            ret: ret.unwrap_or(other),
        }))
    }

    /// `allocsize(N)` / `allocsize(N, M)`. Ports
    /// `LLParser::parseAllocSizeArguments`, whose one diagnostic of its own
    /// catches an attribute that names the same parameter twice.
    fn parse_alloc_size_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Allocsize, "'allocsize'")?;
        self.expect_punct(PunctKind::LParen, "'('")?;
        let element_size = self.parse_uint32()?;
        let element_count = if self.eat_punct(PunctKind::Comma)? {
            let count_loc = self.loc();
            let count = self.parse_uint32()?;
            if count == element_size {
                return Err(self.message_at(
                    count_loc,
                    "'allocsize' indices can't refer to the same parameter",
                ));
            }
            Some(count)
        } else {
            None
        };
        self.expect_punct(PunctKind::RParen, "')'")?;
        Ok(Attribute::AllocSize {
            element_size,
            element_count,
        })
    }

    /// `vscale_range(min)` / `vscale_range(min, max)`. Ports
    /// `LLParser::parseVScaleRangeArguments`, including its default: a missing
    /// max is *min*, not unbounded.
    fn parse_vscale_range_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::VscaleRange, "'vscale_range'")?;
        self.expect_punct(PunctKind::LParen, "'('")?;
        let min = self.parse_uint32()?;
        let max = if self.eat_punct(PunctKind::Comma)? {
            self.parse_uint32()?
        } else {
            min
        };
        self.expect_punct(PunctKind::RParen, "')'")?;
        // `addVScaleRangeAttr(Min, Max > 0 ? Max : std::nullopt)`: zero is how
        // upstream spells unbounded, so it never survives as a maximum.
        Ok(Attribute::VScaleRange {
            min,
            max: (max > 0).then_some(max),
        })
    }

    /// `allockind("alloc,zeroed")`. Ports `LLParser::parseAllocKind`, whose
    /// argument is one *string* holding a comma-separated list — so an empty
    /// or all-unknown list is `expected allockind value`, reported twice for
    /// two different reasons.
    fn parse_alloc_kind_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Allockind, "'allockind'")?;
        self.expect_punct(PunctKind::LParen, "'('")?;
        let kind_loc = self.loc();
        let Ok(spelled) = self.parse_string_constant("allockind value") else {
            return Err(self.message_at(kind_loc, "expected allockind value"));
        };
        let mut kind = llvmkit_ir::AllocFnKind::UNKNOWN;
        for word in spelled.split(',') {
            let Some(one) = llvmkit_ir::AllocFnKind::from_keyword(word) else {
                return Err(ParseError::Message {
                    message: format!("unknown allockind {word}").into(),
                    loc: DiagLoc::span(kind_loc),
                });
            };
            kind |= one;
        }
        self.expect_punct(PunctKind::RParen, "')'")?;
        if kind.is_unknown() {
            return Err(self.message_at(kind_loc, "expected allockind value"));
        }
        Ok(Attribute::AllocKind(kind))
    }

    /// The class mask a single `nofpclass` component keyword contributes.
    ///
    /// Ports `keywordToFPClassTest` (`LLParser.cpp`). Two spellings do not have
    /// a dedicated token: `ninf` is also a fast-math flag, and `sub` is also
    /// the instruction, so both arrive as the token they share.
    fn nofpclass_component(&self) -> Option<FpClassTest> {
        Some(match self.peek() {
            Token::Kw(Keyword::All) => FpClassTest::ALL,
            Token::Kw(Keyword::Nan) => FpClassTest::NAN,
            Token::Kw(Keyword::Snan) => FpClassTest::SIGNALING_NAN,
            Token::Kw(Keyword::Qnan) => FpClassTest::QUIET_NAN,
            Token::Kw(Keyword::Inf) => FpClassTest::INFINITY,
            Token::Kw(Keyword::Ninf) => FpClassTest::NEGATIVE_INFINITY,
            Token::Kw(Keyword::Pinf) => FpClassTest::POSITIVE_INFINITY,
            Token::Kw(Keyword::Norm) => FpClassTest::NORMAL,
            Token::Kw(Keyword::Nnorm) => FpClassTest::NEGATIVE_NORMAL,
            Token::Kw(Keyword::Pnorm) => FpClassTest::POSITIVE_NORMAL,
            Token::Instruction(Opcode::Sub) => FpClassTest::SUBNORMAL,
            Token::Kw(Keyword::Nsub) => FpClassTest::NEGATIVE_SUBNORMAL,
            Token::Kw(Keyword::Psub) => FpClassTest::POSITIVE_SUBNORMAL,
            Token::Kw(Keyword::Zero) => FpClassTest::ZERO,
            Token::Kw(Keyword::Nzero) => FpClassTest::NEGATIVE_ZERO,
            Token::Kw(Keyword::Pzero) => FpClassTest::POSITIVE_ZERO,
            _ => return None,
        })
    }

    /// `nofpclass(<class list>)` or `nofpclass(<mask>)`.
    ///
    /// Mirrors `LLParser::parseNoFPClassAttr`. Components may repeat and may
    /// overlap — upstream carries a `TODO` to reject overlap and does not — and
    /// the single-integer spelling is accepted only as the very first token,
    /// must be non-zero, and must fit inside `fcAllFlags`.
    fn parse_nofpclass_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Nofpclass, "'nofpclass'")?;
        self.expect_punct(PunctKind::LParen, "'('")?;

        let mut mask = FpClassTest::NONE;
        loop {
            if let Some(component) = self.nofpclass_component() {
                mask |= component;
                self.bump()?;
            } else if mask.is_none() {
                // The integer spelling, which replaces the whole list.
                let value = self.parse_uint64()?;
                let bits = u32::try_from(value)
                    .ok()
                    .filter(|bits| *bits != 0)
                    .and_then(|bits| {
                        FpClassTest::from_bits(bits)
                            .filter(|_| bits & !FpClassTest::ALL.bits() == 0)
                    });
                let Some(bits) = bits else {
                    return Err(self.expected("valid mask value for 'nofpclass'"));
                };
                self.expect_punct(PunctKind::RParen, "')'")?;
                return Ok(Attribute::NoFpClass(bits));
            } else {
                return Err(self.expected("nofpclass test mask"));
            }

            if self.eat_punct(PunctKind::RParen)? {
                return Ok(Attribute::NoFpClass(mask));
            }
        }
    }

    /// `memory(...)`. Mirrors `LLParser::parseMemoryAttr`.
    fn parse_memory_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        // `memory(argmem: read)` writes a colon as a *separator*, so the
        // lexer must not read `argmem:` as a label. Upstream sets the same
        // flag and resets it with a `scope_exit`; the split here is that
        // reset, so an early `?` cannot leave it on.
        //
        // It has to be set before the token *inside* the parens is lexed,
        // and the parser holds one token of lookahead — hence at entry,
        // before the `memory` keyword is consumed, exactly as upstream.
        self.lex.ignore_colon_in_idents = true;
        let result = self.parse_memory_attribute_body();
        self.lex.ignore_colon_in_idents = false;
        result
    }

    fn parse_memory_attribute_body(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Memory, "'memory'")?;
        if !self.eat_punct(PunctKind::LParen)? {
            return Err(self.expected("'('"));
        }
        let mut effects = MemoryEffects::none();
        let mut seen_location = false;
        // Upstream's `do { ... } while (EatIfPresent(comma))`: at least one
        // component is required, so `memory()` is rejected by the access-kind
        // arm below rather than accepted as an empty set.
        loop {
            let location = Self::memory_location_for_token(self.peek());
            if location.is_some() {
                self.bump()?;
                if !self.eat_punct(PunctKind::Colon)? {
                    return Err(self.expected("':' after location"));
                }
            }
            // Upstream's stale location list — it omits target_mem0 and
            // target_mem1 even though `keywordToLoc` accepts both. The
            // staleness is contractual; do not "fix" it.
            let Some(mod_ref) = Self::memory_access_kind_for_token(self.peek()) else {
                return Err(self.expected(if location.is_none() {
                    "memory location (argmem, inaccessiblemem, errnomem) or access kind (none, read, write, readwrite)"
                } else {
                    "access kind (none, read, write, readwrite)"
                }));
            };
            self.bump()?;
            match location {
                Some(location) => {
                    seen_location = true;
                    effects = effects.with_mod_ref(location, mod_ref);
                }
                None => {
                    if seen_location {
                        return Err(self.message("default access kind must be specified first"));
                    }
                    effects = Self::memory_effects_for_mod_ref(mod_ref);
                }
            }
            if self.eat_punct(PunctKind::RParen)? {
                return Ok(Attribute::<B>::memory(effects));
            }
            if !self.eat_punct(PunctKind::Comma)? {
                return Err(self.message("unterminated memory attribute"));
            }
        }
    }

    /// Mirrors `keywordToLoc` (`LLParser.cpp`).
    fn memory_location_for_token(token: &Token<'_>) -> Option<MemoryLocation> {
        Some(match token {
            Token::Kw(Keyword::Argmem) => MemoryLocation::ArgMem,
            Token::Kw(Keyword::Inaccessiblemem) => MemoryLocation::InaccessibleMem,
            Token::Kw(Keyword::Errnomem) => MemoryLocation::ErrnoMem,
            Token::Kw(Keyword::TargetMem0) => MemoryLocation::TargetMem0,
            Token::Kw(Keyword::TargetMem1) => MemoryLocation::TargetMem1,
            _ => return None,
        })
    }

    /// Mirrors `keywordToModRef` (`LLParser.cpp`).
    fn memory_access_kind_for_token(token: &Token<'_>) -> Option<ModRefInfo> {
        Some(match token {
            Token::Kw(Keyword::None) => ModRefInfo::NoModRef,
            Token::Kw(Keyword::Read) => ModRefInfo::Ref,
            Token::Kw(Keyword::Write) => ModRefInfo::Mod,
            Token::Kw(Keyword::Readwrite) => ModRefInfo::ModRef,
            _ => return None,
        })
    }

    fn legacy_memory_effects(keyword: Keyword) -> Option<MemoryEffects> {
        Some(match keyword {
            Keyword::Readnone => MemoryEffects::none(),
            Keyword::Readonly => MemoryEffects::read_only(),
            Keyword::Writeonly => MemoryEffects::write_only(),
            Keyword::Argmemonly => MemoryEffects::arg_mem_only(),
            Keyword::Inaccessiblememonly => MemoryEffects::inaccessible_mem_only(),
            Keyword::InaccessiblememOrArgmemonly => MemoryEffects::inaccessible_or_arg_mem_only(),
            _ => return None,
        })
    }

    fn memory_effects_for_mod_ref(mod_ref: ModRefInfo) -> MemoryEffects {
        match mod_ref {
            ModRefInfo::NoModRef => MemoryEffects::none(),
            ModRefInfo::Ref => MemoryEffects::read_only(),
            ModRefInfo::Mod => MemoryEffects::write_only(),
            ModRefInfo::ModRef => MemoryEffects::unknown(),
        }
    }

    fn parse_optional_param_attrs(&mut self) -> ParseResult<AttributeStorage> {
        let mut storage = AttributeStorage::new();
        let parsed = self.parse_fn_attribute_value_pairs(
            &mut storage,
            AttrIndex::Param(0),
            AttrListContext::ParamOrReturn,
        )?;
        if !parsed.groups.is_empty() {
            return Err(self.expected("attribute"));
        }
        Ok(storage)
    }

    fn parse_optional_return_attrs(&mut self) -> ParseResult<AttributeStorage> {
        let mut storage = AttributeStorage::new();
        let parsed = self.parse_fn_attribute_value_pairs(
            &mut storage,
            AttrIndex::Return,
            AttrListContext::ParamOrReturn,
        )?;
        if !parsed.groups.is_empty() {
            return Err(self.expected("attribute"));
        }
        Ok(storage)
    }

    fn parse_optional_fn_attrs(&mut self) -> ParseResult<(AttributeStorage, Vec<u32>)> {
        let mut storage = AttributeStorage::new();
        // A call site's `builtin` is legal, so its location is discarded here
        // — only `parseFunctionHeader` rejects one.
        let parsed = self.parse_fn_attribute_value_pairs(
            &mut storage,
            AttrIndex::Function,
            AttrListContext::FunctionHeader,
        )?;
        Ok((storage, parsed.groups))
    }

    /// Mirrors `knownBundleName` (`lib/IR/LLVMContext.cpp`), the spelling
    /// table `LLVMContext::LLVMContext` registers for the `OB_*` tags.
    fn operand_bundle_tag_from_name(name: String) -> llvmkit_ir::instr_types::OperandBundleTag {
        match name.as_str() {
            "deopt" => llvmkit_ir::instr_types::OperandBundleTag::Deopt,
            "funclet" => llvmkit_ir::instr_types::OperandBundleTag::Funclet,
            "gc-transition" => llvmkit_ir::instr_types::OperandBundleTag::GcTransition,
            "cfguardtarget" => llvmkit_ir::instr_types::OperandBundleTag::CfGuardTarget,
            "preallocated" => llvmkit_ir::instr_types::OperandBundleTag::Preallocated,
            "gc-live" => llvmkit_ir::instr_types::OperandBundleTag::GcLive,
            "clang.arc.attachedcall" => {
                llvmkit_ir::instr_types::OperandBundleTag::ClangArcAttachedCall
            }
            "ptrauth" => llvmkit_ir::instr_types::OperandBundleTag::PtrAuth,
            "kcfi" => llvmkit_ir::instr_types::OperandBundleTag::Kcfi,
            "convergencectrl" => llvmkit_ir::instr_types::OperandBundleTag::ConvergenceCtrl,
            "align" => llvmkit_ir::instr_types::OperandBundleTag::Align,
            "deactivation-symbol" => llvmkit_ir::instr_types::OperandBundleTag::DeactivationSymbol,
            _ => llvmkit_ir::instr_types::OperandBundleTag::Custom(name),
        }
    }

    fn parse_optional_operand_bundles(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<Box<[llvmkit_ir::instr_types::OperandBundleData]>> {
        let begin_loc = self.loc();
        if !self.eat_punct(PunctKind::LSquare)? {
            return Ok(Box::new([]));
        }
        let mut bundles = Vec::new();
        if !matches!(self.peek(), Token::RSquare) {
            loop {
                let tag = self.parse_string_constant("operand bundle tag")?;
                self.expect_punct(PunctKind::LParen, "'(' in operand bundle")?;
                let mut inputs = Vec::new();
                if !matches!(self.peek(), Token::RParen) {
                    loop {
                        let ty = self.parse_type(false)?;
                        // `parseOptionalOperandBundles` branches on the input
                        // type, exactly as `parseParameterList` and
                        // `parseExceptionArgs` do: a `metadata`-typed input is
                        // read by `parseMetadataAsValue`, everything else by
                        // `parseValue`. Without the branch the `ValueAsMetadata`
                        // spelling `metadata i32 %a` never reaches
                        // `parseValueAsMetadata` and dies in `parseValue`.
                        let value = if ty.is_metadata() {
                            self.parse_metadata_as_value(state)?
                        } else {
                            self.parse_value(state, ty)?
                        };
                        inputs.push(value.slot());
                        if !self.eat_punct(PunctKind::Comma)? {
                            break;
                        }
                    }
                }
                self.expect_punct(PunctKind::RParen, "')' in operand bundle")?;
                bundles.push(llvmkit_ir::instr_types::OperandBundleData::new(
                    Self::operand_bundle_tag_from_name(tag),
                    inputs,
                ));
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        // Checked *before* the `]` is eaten, and anchored at the `[` — a
        // written-but-empty bundle set is an error, where an absent one is
        // fine. llvmkit accepted `[]`.
        if bundles.is_empty() {
            return Err(self.message_at(begin_loc, "operand bundle set must not be empty"));
        }
        self.expect_punct(PunctKind::RSquare, "']' to close operand bundles")?;
        Ok(bundles.into_boxed_slice())
    }

    // ── declare ─────────────────────────────────────────────────────────

    /// `declare [linkage] RET @name(PARAMS) [unnamed_addr]`.
    /// Mirrors the `LLParser::parseFunctionHeader` linkage and
    /// unnamed-address arms that have concrete `FunctionData` storage today.
    fn parse_declare(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Declare, "'declare'")?;
        // `LLParser::parseDeclare` collects metadata attachments written
        // *before* the header — `declare !dbg !0 void @f()` — and applies
        // them once the function exists. `define` has no such prefix form;
        // its attachments come after the header instead.
        let mut leading_metadata = Vec::new();
        while matches!(self.peek(), Token::MetadataVar(_)) {
            leading_metadata.push(self.parse_named_metadata_attachment()?);
        }
        let linkage_loc = self.loc();
        let linkage = self.parse_optional_function_linkage()?;
        let (dso_locality, visibility, dll_storage_class) =
            self.parse_optional_preemption_visibility_dll()?;
        let calling_conv = self.parse_optional_calling_conv()?;
        let mut attrs = AttributeStorage::new();
        self.parse_fn_attribute_value_pairs(
            &mut attrs,
            AttrIndex::Return,
            AttrListContext::ParamOrReturn,
        )?;
        let ret_ty_loc = self.loc();
        let ret_ty = self.parse_type(true)?;
        // `parseFunctionHeader`'s three post-type checks, in its order and at
        // its anchors: the linkage switch and the linkage/visibility pair sit
        // on `LinkageLoc`, the return type on `RetTypeLoc`. A declaration
        // reaches all three, because `parseDeclare` calls the same routine.
        Self::check_function_linkage(linkage, false, linkage_loc)?;
        Self::check_linkage_agreement(linkage, visibility, dll_storage_class, linkage_loc)?;
        if !ret_ty.is_valid_function_return() {
            return Err(self.message_at(ret_ty_loc, "invalid function return type"));
        }
        let decl_loc = self.loc();
        let (name_id, name) = match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("function name"))?;
                (NameOrId::Name(name.clone()), name)
            }
            Token::GlobalId(n) => {
                let n = *n;
                check_value_id(
                    "function",
                    "@",
                    self.numbered_globals.next_unused_id(),
                    n,
                    decl_loc,
                )?;
                (NameOrId::Id(n), String::new())
            }
            _ => return Err(self.expected("function name")),
        };
        self.bump()?;
        // `parseFunctionHeader` shares `parseArgumentList` with the function
        // *type* production, so a declaration gets the same numbering check a
        // definition does — llvmkit used to *discard* `%N` argument ids here.
        let mut unnamed_arg_nums = Vec::new();
        let (args, var_args) = self.parse_argument_list(&mut attrs, &mut unnamed_arg_nums)?;
        let params: Vec<Type<'ctx, B>> = args.iter().map(|arg| arg.ty).collect();
        let param_names: Vec<Option<String>> = args.iter().map(|arg| arg.name.clone()).collect();
        let unnamed_addr = self.parse_optional_function_unnamed_addr()?;
        // A function with no explicit `addrspace` lives in the *program*
        // address space, not 0. Mirrors `parseOptionalProgramAddrSpace`, which
        // is `parseOptionalAddrSpace` with `DefaultAS =
        // getProgramAddressSpace()` — the `DefaultAS` parameter llvmkit had no
        // equivalent of.
        let address_space = self.parse_optional_program_addr_space()?;
        let suffix = self.parse_optional_function_suffix(&mut attrs)?;
        self.check_function_attribute_rules(&suffix, &attrs, ret_ty, ret_ty_loc)?;

        let fn_ty = function_type_with_variadic(self.module, ret_ty, params, var_args);
        match resolve_intrinsic_name(&name) {
            IntrinsicNameResolution::NonIntrinsic => {}
            IntrinsicNameResolution::UnknownIntrinsic => {
                return Err(ParseError::Expected {
                    expected: "unknown intrinsic".into(),
                    loc: DiagLoc::span(decl_loc),
                });
            }
            IntrinsicNameResolution::Known(_) => {
                if linkage != Linkage::External
                    || visibility != Visibility::Default
                    || dll_storage_class != DllStorageClass::Default
                    || dso_locality != llvmkit_ir::DsoLocality::Default
                    || calling_conv != CallingConv::default()
                    || unnamed_addr != UnnamedAddr::None
                    || address_space != 0
                    || suffix.section.is_some()
                    || suffix.partition.is_some()
                    || suffix.comdat.is_some()
                    || suffix.align != MaybeAlign::NONE
                    || suffix.gc.is_some()
                    || suffix.prefix_data.is_some()
                    || suffix.prologue_data.is_some()
                    || suffix.personality_fn.is_some()
                    // A declaration's attachments are the ones read *before*
                    // the header; there is no trailing form to check.
                    || !leading_metadata.is_empty()
                {
                    return Err(self.intrinsic_modifier_error(decl_loc));
                }
                let descriptor = self
                    .module
                    .intrinsic_descriptor_from_signature(&name, fn_ty)
                    .map_err(|e| self.intrinsic_parse_error(decl_loc, e))?;
                let expected_attrs = descriptor
                    .declaration_attributes(fn_ty)
                    .map_err(|e| self.intrinsic_parse_error(decl_loc, e))?;
                if !attrs.is_subset_of(&expected_attrs) {
                    return Err(self.intrinsic_attribute_error(decl_loc));
                }
                if self.intrinsic_declaration_attrs_are_pending(&suffix.attr_groups) {
                    if Self::has_duplicate_attr_groups(&suffix.attr_groups) {
                        return Err(self.intrinsic_attribute_error(decl_loc));
                    }
                    self.deferred_intrinsic_attribute_checks.push(
                        DeferredIntrinsicAttributeCheck {
                            attrs: attrs.clone(),
                            attr_groups: suffix.attr_groups.clone(),
                            expected_attrs: expected_attrs.clone(),
                            loc: decl_loc,
                        },
                    );
                } else if !self
                    .intrinsic_declaration_attr_groups_match(&suffix.attr_groups, &expected_attrs)?
                {
                    return Err(self.intrinsic_attribute_error(decl_loc));
                }
                let f = self
                    .module
                    .get_or_insert_intrinsic_declaration(&descriptor)
                    .map_err(|e| self.intrinsic_parse_error(decl_loc, e))?;
                let f = self.module.view(f);
                for (slot, name) in param_names.into_iter().enumerate() {
                    if let Some(name) = name {
                        let slot = u32::try_from(slot).map_err(|_| ParseError::Expected {
                            expected: "parameter slot fits in u32".into(),
                            loc: DiagLoc::span(decl_loc),
                        })?;
                        let arg = f.param(slot).map_err(|e| ParseError::Expected {
                            expected: format!("function parameter slot {slot}: {e}").into(),
                            loc: DiagLoc::span(decl_loc),
                        })?;
                        arg.set_name(self.module, &name);
                    }
                }
                return Ok(());
            }
        }
        let forward_ref =
            self.claim_function_forward_ref(&name, &name_id, address_space, decl_loc)?;
        // `Fn = Function::Create(FT, ExternalLinkage, AddrSpace, FunctionName,
        // M);` — unconditional. A header never re-uses an existing `Function`;
        // the only thing a pending forward reference contributes is the
        // placeholder RAUW'd below.
        let f = self
            .module
            .add_function_dyn(&name, fn_ty, linkage)
            .map_err(|e| ParseError::Expected {
                expected: format!("valid function declaration: {e}").into(),
                loc: DiagLoc::span(decl_loc),
            })?;
        let f = self.module.view(f);
        f.set_visibility(self.module, visibility);
        f.set_dll_storage_class(self.module, dll_storage_class);
        f.set_dso_locality(self.module, dso_locality);
        f.set_calling_conv(self.module, calling_conv);
        f.set_unnamed_addr(self.module, unnamed_addr);
        f.set_address_space(self.module, address_space);
        f.set_attributes(self.module, attrs);
        for (slot, name) in param_names.into_iter().enumerate() {
            if let Some(name) = name {
                let slot = u32::try_from(slot).map_err(|_| ParseError::Expected {
                    expected: "parameter slot fits in u32".into(),
                    loc: DiagLoc::span(decl_loc),
                })?;
                let arg = f.param(slot).map_err(|e| ParseError::Expected {
                    expected: format!("function parameter slot {slot}: {e}").into(),
                    loc: DiagLoc::span(decl_loc),
                })?;
                arg.set_name(self.module, &name);
            }
        }
        for group in suffix.attr_groups {
            f.add_function_attr_group(self.module, group);
        }
        if let Some(section) = suffix.section {
            f.set_section(self.module, section);
        }
        if let Some(partition) = suffix.partition {
            f.set_partition(self.module, partition);
        }
        if let Some(comdat_name) = suffix.comdat {
            // Bare `comdat` borrows the function's own name
            // (`LLParser::parseOptionalComdat`); an unnamed function has none
            // to borrow. `test/Assembler/unnamed-comdat.ll` pins this on
            // `define void @0() comdat`.
            let name = match comdat_name {
                Some(name) => name,
                None if f.name().is_empty() => {
                    return Err(self.message("comdat cannot be unnamed"));
                }
                None => f.name().to_owned(),
            };
            let comdat = self.comdat_ref(&name, decl_loc);
            f.set_comdat(self.module, comdat)
                .map_err(|e| self.builder_err("function comdat", e))?;
        }
        f.set_align(self.module, suffix.align);
        if let Some(gc) = suffix.gc {
            f.set_gc(self.module, gc);
        }
        if let Some(prefix_data) = suffix.prefix_data {
            f.set_prefix_data(self.module, prefix_data)
                .map_err(|e| self.builder_err("function prefix", e))?;
        }
        if let Some(prologue_data) = suffix.prologue_data {
            f.set_prologue_data(self.module, prologue_data)
                .map_err(|e| self.builder_err("function prologue", e))?;
        }
        if let Some(personality_fn) = suffix.personality_fn {
            match personality_fn {
                ParsedPersonalityFn::Resolved(personality_fn) => {
                    f.set_personality_fn(self.module, personality_fn)
                        .map_err(|e| self.builder_err("function personality", e))?;
                }
                ParsedPersonalityFn::ForwardName { name, ty, loc } => {
                    self.deferred_personality_fns.push(DeferredPersonalityFn {
                        function: f,
                        name,
                        ty,
                        loc,
                    });
                }
            }
        }
        // `if (FwdFn) { FwdFn->replaceAllUsesWith(Fn); FwdFn->eraseFromParent(); }`
        // — the last statement of `parseFunctionHeader`'s common tail, after
        // every setter and the argument-name loop, and before `parseDeclare`
        // resumes with the attachments it read ahead of the header.
        if let Some(entry) = forward_ref {
            Self::resolve_global_forward_ref(entry, f.as_global_constant_ptr())?;
        }
        // `parseDeclare` applies the attachments it read *before* the header,
        // in the order they were written. There is no trailing form: a
        // declaration's metadata comes first or not at all, so
        // `declare void @f() !dbg !0` is invalid — which llvmkit used to
        // accept, because its clause loop had a `MetadataVar` arm.
        for (kind, id) in leading_metadata {
            own_metadata(f.set_metadata(self.module, kind, id));
        }
        if let NameOrId::Id(id) = &name_id
            && self.numbered_globals.get(*id).is_none()
        {
            self.numbered_globals
                .add(*id, GlobalRef::Function(f))
                .map_err(|source| ParseError::InvalidSlotId {
                    source,
                    loc: DiagLoc::span(decl_loc),
                })?;
        }
        // `parseFunctionHeader`'s `if (IsDefine) return false;` tail: only a
        // declaration reaches the blockaddress check.
        self.check_no_blockaddress_for_declaration(f, &name_id)?;
        Ok(())
    }

    // ── define ──────────────────────────────────────────────────────────

    /// `define RET @name(PARAMS) { ... }` — full function definition with
    /// a body. Mirrors `LLParser::parseDefine` for the constructive
    /// instruction subset currently shipped (ret / unreachable / br /
    /// cond_br / icmp / add / sub / mul). Function linkage and
    /// unnamed-address markers are preserved when present.
    fn parse_define(&mut self) -> ParseResult<()> {
        // `FileLoc FunctionStart(Lex.getTokLineColumnPos());` — the `define`
        // keyword itself, read before it is eaten.
        let function_start = self.loc().start;
        self.expect_keyword(Keyword::Define, "'define'")?;
        let linkage_loc = self.loc();
        let linkage = self.parse_optional_function_linkage()?;
        let (dso_locality, visibility, dll_storage_class) =
            self.parse_optional_preemption_visibility_dll()?;
        let calling_conv = self.parse_optional_calling_conv()?;
        let mut attrs = AttributeStorage::new();
        self.parse_fn_attribute_value_pairs(
            &mut attrs,
            AttrIndex::Return,
            AttrListContext::ParamOrReturn,
        )?;
        let ret_ty_loc = self.loc();
        let ret_ty = self.parse_type(true)?;
        // `parseFunctionHeader`'s three post-type checks, in its order and at
        // its anchors. Note the pair anchors on `LinkageLoc` here while the
        // global and alias sites anchor on `NameLoc` — one shared predicate,
        // three call sites, and the caret is *not* shared.
        Self::check_function_linkage(linkage, true, linkage_loc)?;
        Self::check_linkage_agreement(linkage, visibility, dll_storage_class, linkage_loc)?;
        if !ret_ty.is_valid_function_return() {
            return Err(self.message_at(ret_ty_loc, "invalid function return type"));
        }
        let decl_loc = self.loc();
        let (name_id, name) = match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("function name"))?;
                (NameOrId::Name(name.clone()), name)
            }
            Token::GlobalId(n) => {
                let n = *n;
                check_value_id(
                    "function",
                    "@",
                    self.numbered_globals.next_unused_id(),
                    n,
                    decl_loc,
                )?;
                (NameOrId::Id(n), String::new())
            }
            _ => return Err(self.expected("function name")),
        };
        self.bump()?;
        match resolve_intrinsic_name(&name) {
            IntrinsicNameResolution::NonIntrinsic => {}
            IntrinsicNameResolution::UnknownIntrinsic => {
                return Err(ParseError::Expected {
                    expected: "unknown intrinsic".into(),
                    loc: DiagLoc::span(decl_loc),
                });
            }
            IntrinsicNameResolution::Known(_) => {
                return Err(ParseError::Expected {
                    expected: "intrinsic functions should never be defined".into(),
                    loc: DiagLoc::span(decl_loc),
                });
            }
        }
        // One `parseArgumentList` for both header paths and the function
        // *type* production, exactly as upstream shares it.
        let mut unnamed_arg_nums = Vec::new();
        let (args, var_args) = self.parse_argument_list(&mut attrs, &mut unnamed_arg_nums)?;
        let param_types: Vec<Type<'ctx, B>> = args.iter().map(|arg| arg.ty).collect();
        let param_names: Vec<Option<String>> = args.iter().map(|arg| arg.name.clone()).collect();
        let unnamed_addr = self.parse_optional_function_unnamed_addr()?;
        // A function with no explicit `addrspace` lives in the *program*
        // address space, not 0. Mirrors `parseOptionalProgramAddrSpace`, which
        // is `parseOptionalAddrSpace` with `DefaultAS =
        // getProgramAddressSpace()` — the `DefaultAS` parameter llvmkit had no
        // equivalent of.
        let address_space = self.parse_optional_program_addr_space()?;
        let suffix = self.parse_optional_function_suffix(&mut attrs)?;
        self.check_function_attribute_rules(&suffix, &attrs, ret_ty, ret_ty_loc)?;
        let function_metadata = self.parse_optional_function_metadata()?;

        let fn_ty = function_type_with_variadic(self.module, ret_ty, param_types, var_args);
        let forward_ref =
            self.claim_function_forward_ref(&name, &name_id, address_space, decl_loc)?;
        // `Fn = Function::Create(FT, ExternalLinkage, AddrSpace, FunctionName,
        // M);` — unconditional, exactly as in `parse_declare`; `IsDefine` does
        // not reach this far into `parseFunctionHeader`.
        let f = self
            .module
            .add_function_dyn(&name, fn_ty, linkage)
            .map_err(|e| ParseError::Expected {
                expected: format!("valid function definition: {e}").into(),
                loc: DiagLoc::span(decl_loc),
            })?;
        let f = self.module.view(f);
        f.set_visibility(self.module, visibility);
        f.set_dll_storage_class(self.module, dll_storage_class);
        f.set_dso_locality(self.module, dso_locality);
        f.set_calling_conv(self.module, calling_conv);
        f.set_unnamed_addr(self.module, unnamed_addr);
        f.set_address_space(self.module, address_space);
        f.set_attributes(self.module, attrs);
        for (slot, p) in param_names.iter().enumerate() {
            if let Some(n) = p {
                let slot_u32 = u32::try_from(slot).map_err(|_| ParseError::Expected {
                    expected: "parameter slot fits in u32".into(),
                    loc: DiagLoc::span(decl_loc),
                })?;
                let arg = f.param(slot_u32).map_err(|e| ParseError::Expected {
                    expected: format!("function parameter slot {slot}: {e}").into(),
                    loc: DiagLoc::span(decl_loc),
                })?;
                arg.set_name(self.module, n);
            }
        }
        for group in suffix.attr_groups {
            f.add_function_attr_group(self.module, group);
        }
        if let Some(section) = suffix.section {
            f.set_section(self.module, section);
        }
        if let Some(partition) = suffix.partition {
            f.set_partition(self.module, partition);
        }
        if let Some(comdat_name) = suffix.comdat {
            // Bare `comdat` borrows the function's own name
            // (`LLParser::parseOptionalComdat`); an unnamed function has none
            // to borrow. `test/Assembler/unnamed-comdat.ll` pins this on
            // `define void @0() comdat`.
            let name = match comdat_name {
                Some(name) => name,
                None if f.name().is_empty() => {
                    return Err(self.message("comdat cannot be unnamed"));
                }
                None => f.name().to_owned(),
            };
            let comdat = self.comdat_ref(&name, decl_loc);
            f.set_comdat(self.module, comdat)
                .map_err(|e| self.builder_err("function comdat", e))?;
        }
        f.set_align(self.module, suffix.align);
        if let Some(gc) = suffix.gc {
            f.set_gc(self.module, gc);
        }
        if let Some(prefix_data) = suffix.prefix_data {
            f.set_prefix_data(self.module, prefix_data)
                .map_err(|e| self.builder_err("function prefix", e))?;
        }
        if let Some(prologue_data) = suffix.prologue_data {
            f.set_prologue_data(self.module, prologue_data)
                .map_err(|e| self.builder_err("function prologue", e))?;
        }
        if let Some(personality_fn) = suffix.personality_fn {
            match personality_fn {
                ParsedPersonalityFn::Resolved(personality_fn) => {
                    f.set_personality_fn(self.module, personality_fn)
                        .map_err(|e| self.builder_err("function personality", e))?;
                }
                ParsedPersonalityFn::ForwardName { name, ty, loc } => {
                    self.deferred_personality_fns.push(DeferredPersonalityFn {
                        function: f,
                        name,
                        ty,
                        loc,
                    });
                }
            }
        }
        // `if (FwdFn) { FwdFn->replaceAllUsesWith(Fn); FwdFn->eraseFromParent(); }`
        // — the last statement of `parseFunctionHeader`'s common tail, so a
        // recursive call in the body below sees the real `Function`, not the
        // placeholder.
        if let Some(entry) = forward_ref {
            Self::resolve_global_forward_ref(entry, f.as_global_constant_ptr())?;
        }
        // `parseDefine` reads the attachments *after* the header, through
        // `parseOptionalFunctionMetadata`, and before the body's `{`.
        for (kind, id) in function_metadata {
            own_metadata(f.set_metadata(self.module, kind, id));
        }
        if let NameOrId::Id(id) = name_id
            && self.numbered_globals.get(id).is_none()
        {
            self.numbered_globals
                .add(id, GlobalRef::Function(f))
                .map_err(|source| ParseError::InvalidSlotId {
                    source,
                    loc: DiagLoc::span(decl_loc),
                })?;
        }

        self.expect_punct(PunctKind::LBrace, "'{' in function body")?;

        let mut state = PerFunctionState::new(f);
        // Mirrors `PerFunctionState::PerFunctionState`, which walks the
        // created function's arguments and hands every *unnamed* one the next
        // entry of `UnnamedArgNums`. The numbering rule itself lives in
        // `parseArgumentList`'s `checkValueID`, so there is nothing to
        // re-check here.
        let mut unnamed = unnamed_arg_nums.into_iter();
        for (slot, name) in param_names.into_iter().enumerate() {
            let slot_u32 = u32::try_from(slot).map_err(|_| ParseError::Expected {
                expected: "parameter slot fits in u32".into(),
                loc: DiagLoc::span(decl_loc),
            })?;
            let arg = f.param(slot_u32).map_err(|e| ParseError::Expected {
                expected: format!("function parameter slot {slot}: {e}").into(),
                loc: DiagLoc::span(decl_loc),
            })?;
            let v = arg.as_erased();
            match name {
                Some(n) => {
                    state.local_named.insert(n, v);
                }
                None => {
                    let Some(id) = unnamed.next() else {
                        unreachable!(
                            "parse_argument_list pushes one number per unnamed argument, \
                             and param_names is that same list"
                        )
                    };
                    state.local_numbered.insert(id, v);
                    state.next_unnamed_value_id = id.saturating_add(1);
                }
            }
        }

        self.parse_function_body(&mut state)?;
        // Upstream drains `ForwardRefBlockAddresses` for this function as the
        // body *opens* (`PerFunctionState::resolveForwardRefBlockAddresses`),
        // because its own `PerFunctionState` will forward-declare whatever
        // blocks the addresses name. llvmkit resolves at the close instead —
        // by then every block exists for real, so no placeholder blocks are
        // needed, and the labels are still numbered.
        self.resolve_block_addresses_for_function(&state, &name_id)?;
        // Upstream eats the `}` *then* runs `PFS.finishFunction()`, so a body
        // that both ends early and leaves a value undefined reports the
        // missing brace first.
        self.expect_punct(PunctKind::RBrace, "'}' to close function body")?;
        // `ParserContext->addFunctionLocation(F, FileLocRange(FunctionStart,
        // Lex.getPrevTokEndLineColumnPos()))`, the `}` being the previous
        // token by now.
        //
        // Upstream records the range even when the body failed to parse — it
        // collects `RetValue` first and only then calls in. llvmkit propagates
        // with `?` instead, so a failed `define` records nothing; the parse has
        // failed either way and the registry never reaches a caller.
        self.record_function_location(f, function_start);
        state.finish(self.module)?;
        Ok(())
    }

    /// `ParserContext->addFunctionLocation(...)`, a no-op when the parser was
    /// built without a registry (upstream's null `AsmParserContext *`).
    fn record_function_location(
        &mut self,
        function: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>,
        start: u32,
    ) {
        if self.parser_context.is_none() {
            return;
        }
        let range = self.file_loc_range_to_prev_token_end(start);
        if let Some(context) = self.parser_context.as_mut() {
            // Upstream discards `addFunctionLocation`'s bool: a second insert
            // for the same handle leaves the first range in place rather than
            // failing the parse.
            let _first_insert_won = context.add_function_location(function, range);
        }
    }

    // ── Function body driver ─────────────────────────────────────────────

    /// `'{' BasicBlock+ UseListOrderDirective* '}'` — mirrors
    /// `LLParser::parseFunctionBody`, whose two loops are what make the
    /// grammar's `+` and its ordering real: every basic block comes first,
    /// then every `uselistorder` directive, and neither may be empty of
    /// blocks. llvmkit ran one loop that took either at any point, so
    /// `define void @f() { }` parsed as a body with no blocks, and a block
    /// after a `uselistorder` was accepted.
    fn parse_function_body(&mut self, state: &mut PerFunctionState<'ctx, B>) -> ParseResult<()> {
        // "We need at least one basic block."
        if matches!(
            self.peek(),
            Token::RBrace | Token::Kw(Keyword::Uselistorder)
        ) {
            return Err(self.message("function body requires at least one basic block"));
        }
        while !matches!(
            self.peek(),
            Token::RBrace | Token::Kw(Keyword::Uselistorder)
        ) {
            match self.peek() {
                // `parseBasicBlock`'s `(LabelStr|LabelID)?` prologue: a
                // `LabelStr` fills `Name`, a `LabelID` fills `NameID`, and
                // `defineBB` is handed whichever one the lexer produced. The
                // split is the lexer's, so `"42":` — a `LabelStr` out of
                // `LLLexer::LexQuote` — names a block `42` while bare `42:`
                // numbers one.
                Token::LabelStr(_) => {
                    let label_loc = self.loc();
                    let label = self
                        .current_label_str()
                        .ok_or_else(|| self.expected("basic-block label"))?;
                    self.bump()?;
                    self.parse_basic_block(state, BlockHeader::Named(label), label_loc)?;
                }
                Token::LabelId(id) => {
                    let label_loc = self.loc();
                    let id = *id;
                    self.bump()?;
                    self.parse_basic_block(state, BlockHeader::Numbered(id), label_loc)?;
                }
                _ => {
                    // LLVM defines an unlabeled block with the next shared
                    // function-local numbered value slot.
                    let loc = self.loc();
                    self.parse_basic_block(state, BlockHeader::Implicit, loc)?;
                }
            }
        }
        // Then, and only then, the `uselistorder` directives.
        while !matches!(self.peek(), Token::RBrace) {
            self.parse_use_list_order(Some(state))?;
        }
        Ok(())
    }

    fn current_label_str(&self) -> Option<String> {
        match self.peek() {
            Token::LabelStr(bytes) => std::str::from_utf8(bytes.as_ref()).ok().map(str::to_owned),
            _ => None,
        }
    }

    /// `parseBasicBlock`, minus its instruction loop — which lives in
    /// [`Self::parse_basic_block_instructions`] because upstream's single
    /// `addBlockLocation` call sits after a `do`/`while` that llvmkit spells as
    /// fifteen `return`s.
    ///
    /// `header_loc` is upstream's `BBStart`: the block's first token, which is
    /// the label when there is one and the first instruction otherwise —
    /// `parseFunctionBody` hands it in already read, where upstream reads it
    /// inside `parseBasicBlock`.
    fn parse_basic_block(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        header: BlockHeader,
        header_loc: Span,
    ) -> ParseResult<()> {
        let bb = state.define_basic_block(self.module, header, header_loc)?;
        let bb_value = bb.to_erased();
        // Drive the typed builder for this block.
        let builder = IrBuilder::with_folder(self.module, NoFolder).position_at_end(bb);
        self.parse_basic_block_instructions(state, Some(builder), bb_value)?;
        // `ParserContext->addBlockLocation(BB, FileLocRange(BBStart,
        // Lex.getPrevTokEndLineColumnPos()))`.
        self.record_block_location(state, bb_value, header_loc.start)?;
        Ok(())
    }

    /// `ParserContext->addBlockLocation(...)`, a no-op without a registry.
    ///
    /// Fallible only in the block-view narrowing: `bb_value` is the erased
    /// handle the caller has just built the block from, so the narrowing is the
    /// same one [`Self::finish_trailing_metadata`] already performs on it.
    fn record_block_location(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        bb_value: llvmkit_ir::Value<'ctx, B>,
        start: u32,
    ) -> ParseResult<()> {
        if self.parser_context.is_none() {
            return Ok(());
        }
        let bb = state.value_as_block_view(bb_value, self.loc())?;
        let range = self.file_loc_range_to_prev_token_end(start);
        if let Some(context) = self.parser_context.as_mut() {
            // `addBlockLocation`'s bool is discarded upstream, as it is for
            // functions and instructions.
            let _first_insert_won = context.add_block_location(&bb, range);
        }
        Ok(())
    }

    fn parse_basic_block_instructions(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        builder: Option<ParsedBlockBuilder<'ctx, 'ctx, B>>,
        bb_value: llvmkit_ir::Value<'ctx, B>,
    ) -> ParseResult<()> {
        // Emit instructions until a terminator consumes `builder`.
        let mut builder = builder;
        let mut pending_debug_records = Vec::new();
        // Track whether any non-phi instruction has been emitted in this block.
        // A `phi` appearing after one is ill-formed `.ll`: the auto-hoisting phi
        // builders would silently reorder it into valid position, so reject it
        // at parse time instead of laundering bad input into valid IR.
        let mut seen_non_phi = false;
        loop {
            while matches!(self.peek(), Token::Hash) {
                // `parseBasicBlock`'s debug-record loop is one half of the
                // format-intermix guard; the `llvm.dbg.*` call site in
                // `parseCall` is the other.
                if self.seen_old_dbg_info_format {
                    return Err(self.message(
                        "debug record should not appear in a module containing debug info intrinsics",
                    ));
                }
                self.seen_new_dbg_info_format = true;
                self.bump()?;
                pending_debug_records.push(self.parse_debug_record(state)?);
            }

            // `FileLoc InstStart(Lex.getTokLineColumnPos());` — taken after the
            // `#dbg_*` records and *before* the optional `%name =`, so the
            // recorded range opens at the result name when there is one.
            let instruction_start = self.loc().start;

            // `LocTy NameLoc = Lex.getLoc();` — `parseBasicBlock` takes it
            // *before* stripping the optional `%name =` / `%N =`, and hands
            // that one location to `setInstName`. Every diagnostic that
            // routine raises therefore points at the result name, not at the
            // opcode behind it.
            let result_loc = self.loc();
            let result_name = self.parse_lhs_assignment()?;

            // `parseInstruction`'s first statement, which upstream reaches
            // only after `parseBasicBlock` has stripped the result name: a
            // block that runs off the end of the file gets its own message
            // rather than whatever the enclosing production would have said
            // next. Input ending at `%x =` is end-of-file, not a missing
            // opcode.
            if matches!(self.peek(), Token::Eof) {
                return Err(self.message("found end of file when expecting more instructions"));
            }

            // `parseInstruction`'s switch, first half: the arms whose
            // instruction ends the block or produces no value. They take the
            // builder by value or reject a result name outright, so Rust's
            // ownership rules keep them in their own `match`; upstream's is
            // one switch reached at this same point, with the result name
            // already in hand.
            match self.peek() {
                Token::Instruction(Opcode::Ret) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_ret(state, b)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::Unreachable) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    let _ = b.unreachable();
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::Br) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_br(state, b)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::Store) => {
                    let b_ref = borrow_live_builder(&builder, self.loc())?;
                    self.bump()?;
                    self.parse_store(state, b_ref)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    seen_non_phi = true;
                    continue;
                }
                Token::Instruction(Opcode::Fence) => {
                    let b_ref = borrow_live_builder(&builder, self.loc())?;
                    self.bump()?;
                    self.parse_fence(b_ref)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    seen_non_phi = true;
                    continue;
                }
                Token::Instruction(Opcode::Switch) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_switch(state, b)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::IndirectBr) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_indirectbr(state, b)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::Invoke) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    // `parseInvoke` is entered with the keyword already eaten
                    // — upstream's `Lex.Lex()` sits in `parseInstruction`,
                    // ahead of the switch. The return type it reads next is
                    // `parseType`'s, so a `%`-sigil token there is a named
                    // type and nothing else.
                    self.bump()?;
                    let v = self.parse_invoke(state, b, &result_name)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    match v {
                        Some(val) => state.bind_local(&result_name, val, result_loc)?,
                        None => reject_named_void(&result_name, result_loc)?,
                    }
                    return Ok(());
                }
                Token::Instruction(Opcode::Resume) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_resume(state, b)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::CleanupRet) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_cleanupret(state, b)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::CatchRet) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_catchret(state, b)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    reject_named_void(&result_name, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::CatchSwitch) => {
                    // `parse_catchswitch` eats the keyword itself, so it is
                    // called with the opcode still unconsumed.
                    let b = take_live_builder(&mut builder, self.loc())?;
                    let v = self.parse_catchswitch(state, b, &result_name)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    state.bind_local(&result_name, v, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::CallBr) => {
                    // `parse_callbr` eats the keyword itself, as above.
                    let b = take_live_builder(&mut builder, self.loc())?;
                    let v = self.parse_callbr(state, b, &result_name)?;
                    self.finish_trailing_metadata(
                        state,
                        bb_value,
                        &mut pending_debug_records,
                        instruction_start,
                    )?;
                    match v {
                        Some(val) => state.bind_local(&result_name, val, result_loc)?,
                        None => reject_named_void(&result_name, result_loc)?,
                    }
                    return Ok(());
                }
                _ => {}
            }
            if matches!(
                self.peek(),
                Token::Kw(Keyword::Tail | Keyword::Musttail | Keyword::Notail)
            ) {
                let b_ref = borrow_live_builder(&builder, self.loc())?;
                let value = self.parse_call(state, b_ref, &result_name)?;
                self.finish_trailing_metadata(
                    state,
                    bb_value,
                    &mut pending_debug_records,
                    instruction_start,
                )?;
                state.bind_local(&result_name, value, result_loc)?;
                seen_non_phi = true;
                continue;
            }
            // `parseInstruction`'s switch, second half: the arms that mint a
            // value and only borrow the builder.
            let opcode = match self.peek() {
                Token::Instruction(op) => *op,
                _ => return Err(self.expected("instruction opcode")),
            };
            // A `phi` must be grouped at the top of its block: reject one that
            // follows any non-phi instruction. Every other (non-terminator)
            // opcode marks the boundary past which phis are no longer allowed.
            if matches!(opcode, Opcode::Phi) {
                if seen_non_phi {
                    return Err(self.expected("phi must be grouped at the top of its basic block"));
                }
            } else {
                seen_non_phi = true;
            }
            // `LocTy Loc = Lex.getLoc();` — `parseInstruction` takes it before
            // `Lex.Lex()` eats the keyword, and the `kw_select` and `kw_phi`
            // fast-math guards anchor their diagnostics on it.
            let opcode_loc = self.loc();
            self.bump()?;
            let b_ref = borrow_live_builder(&builder, self.loc())?;
            let value = match opcode {
                Opcode::Add => self.parse_int_binop(state, b_ref, IntBinOp::Add, &result_name)?,
                Opcode::Sub => self.parse_int_binop(state, b_ref, IntBinOp::Sub, &result_name)?,
                Opcode::Mul => self.parse_int_binop(state, b_ref, IntBinOp::Mul, &result_name)?,
                Opcode::Udiv => self.parse_int_binop(state, b_ref, IntBinOp::Udiv, &result_name)?,
                Opcode::Sdiv => self.parse_int_binop(state, b_ref, IntBinOp::Sdiv, &result_name)?,
                Opcode::Urem => self.parse_int_binop(state, b_ref, IntBinOp::Urem, &result_name)?,
                Opcode::Srem => self.parse_int_binop(state, b_ref, IntBinOp::Srem, &result_name)?,
                Opcode::Shl => self.parse_int_binop(state, b_ref, IntBinOp::Shl, &result_name)?,
                Opcode::Lshr => self.parse_int_binop(state, b_ref, IntBinOp::Lshr, &result_name)?,
                Opcode::Ashr => self.parse_int_binop(state, b_ref, IntBinOp::Ashr, &result_name)?,
                Opcode::And => self.parse_int_binop(state, b_ref, IntBinOp::And, &result_name)?,
                Opcode::Or => self.parse_int_binop(state, b_ref, IntBinOp::Or, &result_name)?,
                Opcode::Xor => self.parse_int_binop(state, b_ref, IntBinOp::Xor, &result_name)?,
                Opcode::Icmp => self.parse_icmp(state, b_ref, &result_name)?,
                Opcode::Trunc => self.parse_int_cast(state, b_ref, IntCast::Trunc, &result_name)?,
                Opcode::Zext => self.parse_int_cast(state, b_ref, IntCast::Zext, &result_name)?,
                Opcode::Sext => self.parse_int_cast(state, b_ref, IntCast::Sext, &result_name)?,
                Opcode::PtrToInt => self.parse_ptr_to_int(state, b_ref, &result_name)?,
                Opcode::IntToPtr => self.parse_int_to_ptr(state, b_ref, &result_name)?,
                Opcode::Fneg => self.parse_fneg(state, b_ref, &result_name)?,
                Opcode::Fadd => self.parse_fp_binop(state, b_ref, FpBinOp::Add, &result_name)?,
                Opcode::Fsub => self.parse_fp_binop(state, b_ref, FpBinOp::Sub, &result_name)?,
                Opcode::Fmul => self.parse_fp_binop(state, b_ref, FpBinOp::Mul, &result_name)?,
                Opcode::Fdiv => self.parse_fp_binop(state, b_ref, FpBinOp::Div, &result_name)?,
                Opcode::Frem => self.parse_fp_binop(state, b_ref, FpBinOp::Rem, &result_name)?,
                Opcode::Fcmp => self.parse_fcmp(state, b_ref, &result_name)?,
                Opcode::Alloca => self.parse_alloca(state, b_ref, &result_name)?,
                Opcode::Load => self.parse_load(state, b_ref, &result_name)?,
                Opcode::GetElementPtr => self.parse_gep(state, b_ref, &result_name)?,
                Opcode::Select => self.parse_select(state, b_ref, &result_name, opcode_loc)?,
                Opcode::FpToUi => {
                    self.parse_fp_to_int(state, b_ref, FpToInt::FpToUi, &result_name)?
                }
                Opcode::FpToSi => {
                    self.parse_fp_to_int(state, b_ref, FpToInt::FpToSi, &result_name)?
                }
                Opcode::UiToFp => {
                    self.parse_int_to_fp(state, b_ref, IntToFp::UiToFp, &result_name)?
                }
                Opcode::SiToFp => {
                    self.parse_int_to_fp(state, b_ref, IntToFp::SiToFp, &result_name)?
                }
                Opcode::AddrSpaceCast => self.parse_addrspace_cast(state, b_ref, &result_name)?,
                Opcode::BitCast => self.parse_bitcast(state, b_ref, &result_name)?,
                Opcode::FpTrunc => self.parse_fptrunc(state, b_ref, &result_name)?,
                Opcode::FpExt => self.parse_fpext(state, b_ref, &result_name)?,
                Opcode::PtrToAddr => self.parse_ptrtoaddr(state, b_ref, &result_name)?,
                Opcode::ExtractElement => self.parse_extractelement(state, b_ref, &result_name)?,
                Opcode::InsertElement => self.parse_insertelement(state, b_ref, &result_name)?,
                Opcode::ShuffleVector => self.parse_shufflevector(state, b_ref, &result_name)?,
                Opcode::ExtractValue => self.parse_extractvalue(state, b_ref, &result_name)?,
                Opcode::InsertValue => self.parse_insertvalue(state, b_ref, &result_name)?,
                Opcode::Phi => self.parse_phi(state, b_ref, &result_name, opcode_loc)?,
                Opcode::Call => self.parse_call(state, b_ref, &result_name)?,
                Opcode::VaArg => self.parse_vaarg(state, b_ref, &result_name)?,
                Opcode::Freeze => self.parse_freeze(state, b_ref, &result_name)?,
                Opcode::AtomicCmpXchg => self.parse_cmpxchg(state, b_ref, &result_name)?,
                Opcode::AtomicRmw => self.parse_atomicrmw(state, b_ref, &result_name)?,
                Opcode::LandingPad => self.parse_landingpad(state, b_ref, &result_name)?,
                Opcode::CleanupPad => self.parse_cleanuppad(state, b_ref, &result_name)?,
                Opcode::CatchPad => self.parse_catchpad(state, b_ref, &result_name)?,
                // The first half of the dispatch returned or continued the
                // loop for each of these, on the same unadvanced token, so
                // none reaches here. Listing them keeps this `match`
                // exhaustive: a new `Opcode` variant is then a compile error
                // rather than a runtime diagnostic no upstream arm emits.
                Opcode::Ret
                | Opcode::Br
                | Opcode::Switch
                | Opcode::IndirectBr
                | Opcode::Invoke
                | Opcode::Resume
                | Opcode::Unreachable
                | Opcode::CleanupRet
                | Opcode::CatchRet
                | Opcode::CatchSwitch
                | Opcode::CallBr
                | Opcode::Store
                | Opcode::Fence => unreachable!(
                    "terminator and void-result opcodes leave the loop in the first half of the dispatch"
                ),
            };
            self.finish_trailing_metadata(
                state,
                bb_value,
                &mut pending_debug_records,
                instruction_start,
            )?;
            state.bind_local(&result_name, value, result_loc)?;
        }
    }

    /// Parse an optional `%name = ` / `%N = ` LHS introduction. Mirrors the
    /// `lltok::LocalVarID` / `lltok::LocalVar` pair in
    /// `LLParser::parseBasicBlock`'s instruction loop, including which of the
    /// two `parseToken(lltok::equal, …)` messages each spelling raises. An
    /// instruction with no result name is upstream's fall-through, which
    /// leaves `NameStr` empty and `NameID` at `-1`.
    fn parse_lhs_assignment(&mut self) -> ParseResult<LocalLhs> {
        match self.peek() {
            Token::LocalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("local SSA name"))?;
                self.bump()?;
                self.expect_punct(PunctKind::Equal, "'=' after instruction name")?;
                Ok(LocalLhs::Named(name))
            }
            Token::LocalVarId(id) => {
                let id = *id;
                self.bump()?;
                self.expect_punct(PunctKind::Equal, "'=' after instruction id")?;
                Ok(LocalLhs::Numbered(id))
            }
            _ => Ok(LocalLhs::None),
        }
    }

    /// `ret void` or `ret TYPE VALUE`. Mirrors `LLParser::parseRet`.
    fn parse_ret(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `ret`
        // `parseRet` compares against the *enclosing function's* return type
        // and reports at the returned type's own token, in both arms.
        let type_loc = self.loc();
        let result_ty = state.func.signature().return_type();
        if let Token::PrimitiveType(PrimitiveTy::Void) = self.peek() {
            self.bump()?;
            if !result_ty.is_void() {
                return Err(self.message_at(
                    type_loc,
                    format!("value doesn't match function result type '{result_ty}'"),
                ));
            }
            let _ = b.ret_void().map_err(|e| ParseError::Expected {
                expected: format!("valid ret void: {e}").into(),
                loc: DiagLoc::span(self.loc()),
            })?;
            return Ok(());
        }
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        if result_ty != v.ty() {
            return Err(self.message_at(
                type_loc,
                format!("value doesn't match function result type '{result_ty}'"),
            ));
        }
        let _ = b.ret(v).map_err(|e| ParseError::Expected {
            expected: format!("valid ret: {e}").into(),
            loc: DiagLoc::span(self.loc()),
        })?;
        Ok(())
    }

    /// `br label %t` or `br i1 %c, label %t, label %f`. Mirrors
    /// `LLParser::parseBr`.
    fn parse_br(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `br`
        // `if (parseTypeAndValue(Op0, Loc, PFS)) return true;` — the
        // unconditional form is *not* a `label` keyword lookahead upstream:
        // the operand is read as a type-and-value like any other, and it is
        // the `dyn_cast` below that decides which `br` this is.
        let cond_loc = self.loc();
        let (cond_ty, cond_v) = self.parse_type_and_value(state)?;
        // `if (BasicBlock *BB = dyn_cast<BasicBlock>(Op0)) { Inst =
        //  BranchInst::Create(BB); return false; }`
        if let Some(target) = state.block_label_for_value(cond_v) {
            let _ = b.br(target).map_err(|e| ParseError::Expected {
                expected: format!("valid br: {e}").into(),
                loc: DiagLoc::span(self.loc()),
            })?;
            return Ok(());
        }
        // `if (Op0->getType() != Type::getInt1Ty(Context)) return error(Loc,
        //  "branch condition must have 'i1' type");` — after the block test,
        // not before it, and after the operand has been read.
        if !matches!(
            cond_ty.into_type_enum(),
            AnyTypeEnum::Int(t) if t.bit_width() == 1
        ) {
            return Err(self.message_at(cond_loc, "branch condition must have 'i1' type"));
        }
        self.expect_punct(PunctKind::Comma, "',' after branch condition")?;
        let then_bb = self.parse_type_and_basic_block(state)?;
        self.expect_punct(PunctKind::Comma, "',' after true destination")?;
        let else_bb = self.parse_type_and_basic_block(state)?;
        let cond_iv: IntValue<'ctx, IntDyn, B> = cond_v
            .try_into()
            .map_err(|_| self.expected("i1 condition"))?;
        let cond_i1: IntValue<'ctx, bool, B> = cond_iv
            .try_into()
            .map_err(|_| self.expected("i1 condition"))?;
        let _ = b
            .cond_br(cond_i1, then_bb, else_bb)
            .map_err(|e| ParseError::Expected {
                expected: format!("valid cond_br: {e}").into(),
                loc: DiagLoc::span(self.loc()),
            })?;
        Ok(())
    }

    /// `OP [nuw] [nsw] TYPE LHS, RHS` or `OP [exact] TYPE LHS, RHS` or `OP [disjoint] TYPE LHS, RHS`.
    /// Mirrors `LLParser::parseArithmetic` / `parseLogical` (LLParser.cpp ~8132 / 8152).
    fn parse_int_binop(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        op: IntBinOp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        use llvmkit_ir::instr_types::{
            AddFlags, AshrFlags, LshrFlags, MulFlags, OrFlags, SdivFlags, ShlFlags, SubFlags,
            UdivFlags,
        };
        // Parse optional flags before the type: upstream grammar accepts
        //   add/sub/mul/shl [nuw] [nsw] TYPE LHS, RHS   (nuw/nsw in either order)
        //   udiv/sdiv/lshr/ashr [exact] TYPE LHS, RHS
        //   or [disjoint] TYPE LHS, RHS
        // The retry after `nsw` mirrors the kw_add/sub/mul/shl instruction arm
        // (LLParser.cpp ~7323): `nsw nuw` parses and prints canonically as
        // `nuw nsw`.
        let is_overflowing_binop = matches!(
            op,
            IntBinOp::Add | IntBinOp::Sub | IntBinOp::Mul | IntBinOp::Shl
        );
        let mut nuw = is_overflowing_binop && self.eat_keyword(Keyword::Nuw)?;
        let nsw = is_overflowing_binop && self.eat_keyword(Keyword::Nsw)?;
        if !nuw {
            nuw = is_overflowing_binop && self.eat_keyword(Keyword::Nuw)?;
        }
        let exact = matches!(
            op,
            IntBinOp::Udiv | IntBinOp::Sdiv | IntBinOp::Lshr | IntBinOp::Ashr
        ) && self.eat_keyword(Keyword::Exact)?;
        let disjoint_or = matches!(op, IntBinOp::Or) && self.eat_keyword(Keyword::Disjoint)?;

        let operand_loc = self.loc();
        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between binop operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;

        // `parseArithmetic`'s operand rule for the integer opcodes, and
        // `parseLogical`'s — the two differ only in wording, and upstream
        // routes `and` / `or` / `xor` through the second. Neither existed
        // here: a non-integer operand reached the builder.
        if !is_int_or_int_vector_type(ty) {
            let message = if matches!(op, IntBinOp::And | IntBinOp::Or | IntBinOp::Xor) {
                "instruction requires integer or integer vector operands"
            } else {
                "invalid operand type for instruction"
            };
            return Err(self.message_at(operand_loc, message));
        }

        // Vector operands take the erased builder. The typed `int_*`
        // family routes both operands through `IntoIntValue<W>`, whose
        // `IntWidth` marker describes a *scalar* width, so `<N x iM>` cannot
        // convert. Upstream has one path for both (`LLParser::parseArithmetic`
        // hands the operands straight to `BinaryOperator::Create`); the split
        // here is llvmkit's typed-handle layer, not a grammar difference.
        if is_vector_type(ty) {
            let name = result_name.as_str();
            let flags = int_binop_flags(nuw, nsw, exact, disjoint_or);
            let v = b
                .int_binop_erased(op.opcode(), lhs_v, rhs_v, flags, name)
                .map_err(|e| self.builder_err(op.mnemonic(), e))?;
            return Ok(b.view(v));
        }

        let lhs: IntValue<'ctx, IntDyn, B> = lhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed lhs"))?;
        let rhs: IntValue<'ctx, IntDyn, B> = rhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed rhs"))?;
        let name = result_name.as_str();
        let v = match op {
            IntBinOp::Add => {
                let mut flags = AddFlags::new();
                if nuw {
                    flags = flags.nuw();
                }
                if nsw {
                    flags = flags.nsw();
                }
                b.int_add_with_flags::<IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("add", e))?
            }
            IntBinOp::Sub => {
                let mut flags = SubFlags::new();
                if nuw {
                    flags = flags.nuw();
                }
                if nsw {
                    flags = flags.nsw();
                }
                b.int_sub_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("sub", e))?
            }
            IntBinOp::Mul => {
                let mut flags = MulFlags::new();
                if nuw {
                    flags = flags.nuw();
                }
                if nsw {
                    flags = flags.nsw();
                }
                b.int_mul_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("mul", e))?
            }
            IntBinOp::Shl => {
                let mut flags = ShlFlags::new();
                if nuw {
                    flags = flags.nuw();
                }
                if nsw {
                    flags = flags.nsw();
                }
                b.int_shl_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("shl", e))?
            }
            IntBinOp::Udiv => {
                let mut flags = UdivFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.int_udiv_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("udiv", e))?
            }
            IntBinOp::Sdiv => {
                let mut flags = SdivFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.int_sdiv_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("sdiv", e))?
            }
            IntBinOp::Lshr => {
                let mut flags = LshrFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.int_lshr_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("lshr", e))?
            }
            IntBinOp::Ashr => {
                let mut flags = AshrFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.int_ashr_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("ashr", e))?
            }
            IntBinOp::Urem => b
                .int_urem::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("urem", e))?,
            IntBinOp::Srem => b
                .int_srem::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("srem", e))?,
            IntBinOp::And => b
                .int_and::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("and", e))?,
            IntBinOp::Or => {
                let flags = if disjoint_or {
                    OrFlags::new().disjoint()
                } else {
                    OrFlags::new()
                };
                b.int_or_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("or", e))?
            }
            IntBinOp::Xor => b
                .int_xor::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("xor", e))?,
        };
        Ok(b.view(v).as_erased())
    }

    /// `icmp [samesign] PRED TYPE LHS, RHS`. Mirrors `LLParser::parseCompare`.
    fn parse_icmp(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let samesign = self.eat_keyword(Keyword::Samesign)?;
        let pred = match self.peek() {
            Token::Kw(Keyword::Eq) => llvmkit_ir::IntPredicate::Eq,
            Token::Kw(Keyword::Ne) => llvmkit_ir::IntPredicate::Ne,
            Token::Kw(Keyword::Slt) => llvmkit_ir::IntPredicate::Slt,
            Token::Kw(Keyword::Sle) => llvmkit_ir::IntPredicate::Sle,
            Token::Kw(Keyword::Sgt) => llvmkit_ir::IntPredicate::Sgt,
            Token::Kw(Keyword::Sge) => llvmkit_ir::IntPredicate::Sge,
            Token::Kw(Keyword::Ult) => llvmkit_ir::IntPredicate::Ult,
            Token::Kw(Keyword::Ule) => llvmkit_ir::IntPredicate::Ule,
            Token::Kw(Keyword::Ugt) => llvmkit_ir::IntPredicate::Ugt,
            Token::Kw(Keyword::Uge) => llvmkit_ir::IntPredicate::Uge,
            // `parseCmpPredicate`'s default arm, which names an example.
            _ => return Err(self.message("expected icmp predicate (e.g. 'eq')")),
        };
        self.bump()?;
        let operand_loc = self.loc();
        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' after compare value")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;

        // `LLParser::parseCompare` accepts integers **and pointers** here
        // (`isIntOrIntVectorTy() || isPtrOrPtrVectorTy()`); `icmp eq ptr %a,
        // %b` is ordinary IR that a scalar-integer-only path rejected.
        if !is_int_or_int_vector_type(ty) && !is_ptr_or_ptr_vector_type(ty) {
            return Err(self.message_at(operand_loc, "icmp requires integer operands"));
        }

        // Vector operands take the erased builder, as in `parse_int_binop`:
        // the result is `<N x i1>`, which no `IntValueId<bool, B>` describes.
        // Pointers take it for the same reason in the other direction —
        // `IntValue<IntDyn>` cannot name a `ptr`.
        if is_vector_type(ty) || ty.is_pointer() {
            let name = result_name.as_str();
            let flags = if samesign {
                llvmkit_ir::instr_types::IcmpFlags::new().samesign()
            } else {
                llvmkit_ir::instr_types::IcmpFlags::new()
            };
            let r = b
                .int_cmp_erased(pred, lhs_v, rhs_v, flags, name)
                .map_err(|e| self.builder_err("icmp", e))?;
            return Ok(b.view(r));
        }

        let lhs: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = lhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed lhs"))?;
        let rhs: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = rhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed rhs"))?;
        let name = result_name.as_str();
        let flags = if samesign {
            llvmkit_ir::instr_types::IcmpFlags::new().samesign()
        } else {
            llvmkit_ir::instr_types::IcmpFlags::new()
        };
        let r = b
            .int_cmp_with_flags_dyn(pred, lhs, rhs, flags, name)
            .map_err(|e| self.builder_err("icmp", e))?;
        Ok(b.view(r).as_erased())
    }

    /// `trunc [nuw] [nsw] TYPE VALUE to TYPE` / `zext [nneg] TYPE VALUE to TYPE` / `sext TYPE VALUE to TYPE`.
    /// Mirrors `LLParser::parseCast`'s integer-cast arm.
    fn parse_int_cast(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        op: IntCast,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // nuw/nsw parse in either order (retry mirrors the kw_trunc arm,
        // LLParser.cpp ~7405); print order is canonically `nuw nsw`.
        let is_trunc = matches!(op, IntCast::Trunc);
        let mut trunc_nuw = is_trunc && self.eat_keyword(Keyword::Nuw)?;
        let trunc_nsw = is_trunc && self.eat_keyword(Keyword::Nsw)?;
        if !trunc_nuw {
            trunc_nuw = is_trunc && self.eat_keyword(Keyword::Nuw)?;
        }
        let zext_nneg = matches!(op, IntCast::Zext) && self.eat_keyword(Keyword::Nneg)?;
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(
            Keyword::To,
            "'to' between cast operand and destination type",
        )?;
        let dst_ty = self.parse_type(false)?;

        // Vector operands take the erased builder, as in `parse_int_binop`.
        // The typed `*_dyn` cast family routes the source through
        // `IntoIntValue<IntDyn>` and takes an `IntType` destination, both of
        // which describe a *scalar* width, so `<N x iM>` converts to neither.
        // Upstream has one path for both — `LLParser::parseCast` hands the
        // operand straight to `CastInst::Create`.
        if is_vector_type(src_ty) || is_vector_type(dst_ty) {
            let flags = IntCastFlags::new();
            let flags = if trunc_nuw { flags.nuw() } else { flags };
            let flags = if trunc_nsw { flags.nsw() } else { flags };
            let flags = if zext_nneg { flags.nneg() } else { flags };
            let v = b
                .int_cast_erased(op.cast_opcode(), src_v, dst_ty, flags, result_name.as_str())
                .map_err(|e| self.builder_err(op.mnemonic(), e))?;
            return Ok(b.view(v));
        }

        let src_int: IntValue<'ctx, IntDyn, B> = src_v
            .try_into()
            .map_err(|_| self.expected("integer-typed cast source"))?;
        let dst_int = match dst_ty.into_type_enum() {
            AnyTypeEnum::Int(t) => t,
            _ => return Err(self.expected("integer destination type for trunc/zext/sext")),
        };
        let name = result_name.as_str();
        let v = match op {
            IntCast::Trunc => {
                let flags = llvmkit_ir::instr_types::TruncFlags::new();
                let flags = if trunc_nuw { flags.nuw() } else { flags };
                let flags = if trunc_nsw { flags.nsw() } else { flags };
                if trunc_nuw || trunc_nsw {
                    b.trunc_with_flags_dyn(src_int, dst_int, flags, name)
                } else {
                    b.trunc_dyn(src_int, dst_int, name)
                }
                .map_err(|e| self.builder_err("trunc", e))?
            }
            IntCast::Zext => if zext_nneg {
                b.zext_with_flags_dyn(
                    src_int,
                    dst_int,
                    llvmkit_ir::instr_types::ZextFlags::new().nneg(),
                    name,
                )
            } else {
                b.zext_dyn(src_int, dst_int, name)
            }
            .map_err(|e| self.builder_err("zext", e))?,
            IntCast::Sext => b
                .sext_dyn(src_int, dst_int, name)
                .map_err(|e| self.builder_err("sext", e))?,
        };
        Ok(b.view(v).as_erased())
    }

    /// `ptrtoint TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// `Instruction::PtrToInt` arm.
    fn parse_ptr_to_int(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in ptrtoint")?;
        let dst_ty = self.parse_type(false)?;
        let src_ptr: PointerValue<'ctx, B> = src_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed ptrtoint source"))?;
        let dst_int = match dst_ty.into_type_enum() {
            AnyTypeEnum::Int(t) => t,
            _ => return Err(self.expected("integer destination type for ptrtoint")),
        };
        let v = b
            .ptr_to_int(src_ptr, dst_int, result_name.as_str())
            .map_err(|e| self.builder_err("ptrtoint", e))?;
        Ok(b.view(v).as_erased())
    }

    /// `inttoptr TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// `Instruction::IntToPtr` arm.
    fn parse_int_to_ptr(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in inttoptr")?;
        let dst_ty = self.parse_type(false)?;
        let src_int: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = src_v
            .try_into()
            .map_err(|_| self.expected("integer-typed inttoptr source"))?;
        let dst_ptr = match dst_ty.into_type_enum() {
            AnyTypeEnum::Pointer(t) => t,
            _ => return Err(self.expected("pointer destination type for inttoptr")),
        };
        let v = b
            .int_to_ptr(src_int, dst_ptr, result_name.as_str())
            .map_err(|e| self.builder_err("inttoptr", e))?;
        Ok(b.view(v).as_erased())
    }

    /// `fneg [nnan ninf ...] TYPE VALUE`. Mirrors `LLParser::parseUnaryOp` for `Instruction::FNeg`.
    fn parse_fneg(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        // Erased for the same reason as the binary operators below: a float
        // *vector* has no typed handle, `FloatKind` being scalar.
        let r = b
            .fp_neg_erased(v, fmf, result_name.as_str())
            .map_err(|e| self.builder_err("fneg", e))?;
        Ok(b.view(r))
    }

    /// `OP [nnan ninf ...] TYPE LHS, RHS` for fadd/fsub/fmul/fdiv/frem.
    /// Mirrors `LLParser::parseArithmetic` FP arm.
    fn parse_fp_binop(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        op: FpBinOp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between FP binop operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;
        // Erased because a float *vector* has no typed handle — llvmkit's
        // `FloatKind` markers are scalar, so `<N x double>` cannot route
        // through `IntoFloatValue`. Upstream needs no split:
        // `LLParser::parseArithmetic` hands the operands to
        // `BinaryOperator::Create`.
        let (opcode, what) = match op {
            FpBinOp::Add => (llvmkit_ir::BinaryOpcode::Fadd, "fadd"),
            FpBinOp::Sub => (llvmkit_ir::BinaryOpcode::Fsub, "fsub"),
            FpBinOp::Mul => (llvmkit_ir::BinaryOpcode::Fmul, "fmul"),
            FpBinOp::Div => (llvmkit_ir::BinaryOpcode::Fdiv, "fdiv"),
            FpBinOp::Rem => (llvmkit_ir::BinaryOpcode::Frem, "frem"),
        };
        let v = b
            .fp_binop_erased(opcode, lhs_v, rhs_v, fmf, result_name.as_str())
            .map_err(|e| self.builder_err(what, e))?;
        Ok(b.view(v))
    }

    /// `fcmp [nnan ninf ...] PRED TYPE LHS, RHS`. Mirrors `LLParser::parseCompare` FP arm.
    fn parse_fcmp(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let fmf = self.parse_optional_fmf()?;
        use FloatPredicate as P;
        let pred = match self.peek() {
            Token::Kw(Keyword::Oeq) => P::Oeq,
            Token::Kw(Keyword::Ogt) => P::Ogt,
            Token::Kw(Keyword::Oge) => P::Oge,
            Token::Kw(Keyword::Olt) => P::Olt,
            Token::Kw(Keyword::Ole) => P::Ole,
            Token::Kw(Keyword::One) => P::One,
            Token::Kw(Keyword::Ord) => P::Ord,
            Token::Kw(Keyword::Uno) => P::Uno,
            Token::Kw(Keyword::Ueq) => P::Ueq,
            Token::Kw(Keyword::Ugt) => P::Ugt,
            Token::Kw(Keyword::Uge) => P::Uge,
            Token::Kw(Keyword::Ult) => P::Ult,
            Token::Kw(Keyword::Ule) => P::Ule,
            Token::Kw(Keyword::Une) => P::Une,
            Token::Kw(Keyword::True) => P::True,
            Token::Kw(Keyword::False) => P::False,
            // The `Instruction::FCmp` half of `parseCmpPredicate`'s default
            // arm — a different example predicate from the icmp twin.
            _ => return Err(self.message("expected fcmp predicate (e.g. 'oeq')")),
        };
        self.bump()?;
        let operand_loc = self.loc();
        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' after compare value")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;
        // `LLParser::parseCompare`'s `FCmp` arm.
        if !is_fp_or_fp_vector_type(ty) {
            return Err(self.message_at(operand_loc, "fcmp requires floating point operands"));
        }
        // Erased: a vector compare has neither a typed float operand nor a
        // typed `i1` result, so it can use neither half of the typed builder.
        let r = b
            .fp_cmp_erased(pred, lhs_v, rhs_v, fmf, result_name.as_str())
            .map_err(|e| self.builder_err("fcmp", e))?;
        Ok(b.view(r))
    }

    /// `alloca TYPE [, TYPE COUNT] [, align N]`.
    /// Mirrors `LLParser::parseAlloc` (LLParser.cpp ~8540).
    fn parse_alloca(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `inalloca` / `swifterror` markers precede the type
        // (`LLParser::parseAlloc`).
        let inalloca = self.eat_keyword(Keyword::Inalloca)?;
        let swifterror = self.eat_keyword(Keyword::Swifterror)?;
        let ty_loc = self.loc();
        let ty = self.parse_type(false)?;
        if ty.is_function() || !ty.is_valid_pointer_element() {
            return Err(self.message_at(ty_loc, "invalid type for alloca"));
        }
        // Upstream parses size, then alignment, then address space.
        let size = self.parse_optional_comma_array_size(state)?;
        let align = self.parse_optional_comma_align()?;
        let addr_space = self.parse_optional_comma_addrspace()?;
        // `if (!Alignment && !Ty->isSized(&Visited))` — an explicit alignment
        // is what makes an unsized allocation legal, because the alignment is
        // otherwise derived from the type's layout. Upstream's capital `C`.
        if align.is_none() && !ty.is_sized() {
            return Err(self.message_at(ty_loc, "Cannot allocate unsized type"));
        }
        // Runtime-clause dispatch: every optional slot is an `Option` /
        // `bool` decided by the source text, so the builder chain is
        // assembled with explicit ifs (the same shape
        // [`Self::function_type_with_variadic`] uses for its runtime split).
        let mut alloca = b.alloca_builder(ty).name(result_name.as_str());
        if let Some(size) = size {
            alloca = alloca.array(size);
        }
        if let Some(align) = align {
            alloca = alloca.align(align);
        }
        if let Some(addr_space) = addr_space {
            alloca = alloca.addr_space(addr_space);
        }
        if inalloca {
            alloca = alloca.inalloca();
        }
        if swifterror {
            alloca = alloca.swifterror();
        }
        let r = alloca.build().map_err(|e| self.builder_err("alloca", e))?;
        Ok(b.view(r).as_erased())
    }

    /// Optional `, <intty> <size>` array-size operand for `alloca`, present
    /// when the token after the comma is a type rather than the `align`
    /// keyword (mirrors `LLParser::parseAlloc`'s size branch). Uses the same
    /// save/restore peek as [`Self::parse_optional_comma_align`], so a
    /// `, align N` clause is left intact for that method.
    fn parse_optional_comma_array_size(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<Option<llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B>>> {
        if !matches!(self.peek(), Token::Comma) {
            return Ok(None);
        }
        let saved_lex = self.lex.clone();
        let saved_current = self.current.clone();
        let saved_prev_token_end = self.prev_token_end;
        self.bump()?;
        // A `, align N`, `, addrspace(N)`, or `, !dbg !N` (trailing metadata)
        // clause is not an array size — restore the comma for the align /
        // addrspace / metadata handlers. Mirrors `LLParser::parseAlloc`, which
        // branches on `kw_align` / `kw_addrspace` / `MetadataVar` before
        // attempting the size parse.
        if matches!(
            self.peek(),
            Token::Kw(Keyword::Align) | Token::Kw(Keyword::Addrspace) | Token::MetadataVar(_)
        ) {
            self.lex = saved_lex;
            self.current = saved_current;
            self.prev_token_end = saved_prev_token_end;
            return Ok(None);
        }
        // Upstream reads a general `parseTypeAndValue` here and rejects a
        // non-integer *afterwards*, at the operand's own location.
        let size_loc = self.loc();
        let size_ty = self.parse_type(false)?;
        let size_v = self.parse_value(state, size_ty)?;
        let n: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = size_v
            .try_into()
            .map_err(|_| self.message_at(size_loc, "element count must have integer type"))?;
        Ok(Some(n))
    }

    /// Optional `, addrspace(N)` clause for `alloca` (after any align),
    /// mirroring `LLParser::parseAlloc`. Uses the same save/restore peek so a
    /// trailing `, !dbg` metadata comma is left intact.
    /// Mirrors `LLParser::parseOptionalCommaAddrSpace`: after a comma only an
    /// address space or trailing metadata may follow, and anything else is an
    /// error rather than a silent stop. Metadata is the early exit, which
    /// llvmkit spells by putting the comma back so the caller's own metadata
    /// loop reads it — upstream instead keeps the comma eaten and reports it
    /// through `AteExtraComma`.
    fn parse_optional_comma_addrspace(&mut self) -> ParseResult<Option<u32>> {
        if !matches!(self.peek(), Token::Comma) {
            return Ok(None);
        }
        let saved_lex = self.lex.clone();
        let saved_current = self.current.clone();
        let saved_prev_token_end = self.prev_token_end;
        self.bump()?;
        if matches!(self.peek(), Token::MetadataVar(_)) {
            self.lex = saved_lex;
            self.current = saved_current;
            self.prev_token_end = saved_prev_token_end;
            return Ok(None);
        }
        if !matches!(self.peek(), Token::Kw(Keyword::Addrspace)) {
            return Err(self.expected("metadata or 'addrspace'"));
        }
        self.bump()?;
        Ok(Some(self.parse_addr_space_paren()?))
    }

    /// Mirrors `LLParser::parseOptionalCommaAlign`, the load/store/cmpxchg/
    /// atomicrmw form: after a comma only `align` or trailing metadata may
    /// follow.
    ///
    /// `parseAlloc` deliberately does **not** use it — it hand-rolls a nested
    /// dispatch because an `addrspace` clause may follow the comma too, which
    /// is why [`parse_optional_comma_align`] keeps its backtracking shape for
    /// that one caller.
    fn parse_optional_comma_align_strict(&mut self) -> ParseResult<Option<Align>> {
        let mut alignment = None;
        loop {
            if !matches!(self.peek(), Token::Comma) {
                return Ok(alignment);
            }
            let saved_lex = self.lex.clone();
            let saved_current = self.current.clone();
            let saved_prev_token_end = self.prev_token_end;
            self.bump()?;
            if matches!(self.peek(), Token::MetadataVar(_)) {
                self.lex = saved_lex;
                self.current = saved_current;
                self.prev_token_end = saved_prev_token_end;
                return Ok(alignment);
            }
            if !matches!(self.peek(), Token::Kw(Keyword::Align)) {
                return Err(self.expected("metadata or 'align'"));
            }
            alignment = Some(self.parse_align_val()?);
        }
    }

    /// `load [volatile] TYPE, ptr PTR [, align N]` or
    /// `load atomic [volatile] TYPE, ptr PTR [syncscope("...")] ORDERING, align N`.
    /// Mirrors `LLParser::parseLoad` (LLParser.cpp ~8608).
    fn parse_load(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let is_atomic = self.eat_keyword(Keyword::Atomic)?;
        let volatile = self.eat_keyword(Keyword::Volatile)?;
        let explicit_type_loc = self.loc();
        let ty = self.parse_type(false)?;
        self.expect_punct(PunctKind::Comma, "comma after load's type")?;
        let operand_loc = self.loc();
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;

        // `parseScopeAndOrdering` is a no-op when the load is not atomic, and
        // the align clause is read the *same* way in both cases — optionally.
        // llvmkit used to demand a comma and an alignment on the atomic path,
        // so a missing one was `expected ',' after atomic ordering` instead of
        // upstream's own diagnostic below.
        let scope_and_ordering = if is_atomic {
            let sync_scope = self.parse_optional_syncscope()?;
            Some((sync_scope, self.parse_atomic_ordering()?))
        } else {
            None
        };
        let align = self.parse_optional_comma_align_strict()?;

        // `parseLoad`'s checks, in its order and at its anchors.
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v.try_into().map_err(|_| {
            self.message_at(
                operand_loc,
                "load operand must be a pointer to a first class type",
            )
        })?;
        if !ty.is_first_class() {
            return Err(self.message_at(
                operand_loc,
                "load operand must be a pointer to a first class type",
            ));
        }
        if is_atomic && align.is_none() {
            return Err(self.message_at(
                operand_loc,
                "atomic load must have explicit non-zero alignment",
            ));
        }
        if let Some((_, ordering)) = scope_and_ordering
            && matches!(
                ordering,
                AtomicOrdering::Release | AtomicOrdering::AcquireRelease
            )
        {
            return Err(self.message_at(operand_loc, "atomic load cannot use Release ordering"));
        }
        if align.is_none() && !ty.is_sized() {
            return Err(self.message_at(explicit_type_loc, "loading unsized types is not allowed"));
        }

        // Runtime-clause dispatch: `volatile` / `atomic` / the align and
        // syncscope clauses are all decided by the source text, so the
        // builder chain is assembled with explicit ifs (the same shape
        // [`Self::function_type_with_variadic`] uses for its runtime split).
        let mut load = b.load_from(ptr);
        if volatile {
            load = load.volatile();
        }
        if let Some((sync_scope, ordering)) = scope_and_ordering {
            load = load.atomic(ordering).sync_scope(sync_scope);
        }
        if let Some(align) = align {
            load = load.align(align);
        }
        let v = load
            .erased(ty, result_name.as_str())
            .map_err(|e| self.builder_err("load", e))?;
        Ok(b.view(v))
    }

    /// `store [volatile] TYPE VALUE, ptr PTR [, align N]` or
    /// `store atomic [volatile] TYPE VALUE, ptr PTR [syncscope("...")] ORDERING, align N`.
    /// Mirrors `LLParser::parseStore` (LLParser.cpp ~8658). Returns no value.
    fn parse_store(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        let is_atomic = self.eat_keyword(Keyword::Atomic)?;
        let volatile = self.eat_keyword(Keyword::Volatile)?;
        let value_loc = self.loc();
        let val_ty = self.parse_type(false)?;
        let val_v = self.parse_value(state, val_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after store operand")?;
        let ptr_loc = self.loc();
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;

        // As in `parse_load`: the align clause is optional on both paths, and
        // its absence on an atomic store is a *diagnostic*, not a parse
        // failure.
        let scope_and_ordering = if is_atomic {
            let sync_scope = self.parse_optional_syncscope()?;
            Some((sync_scope, self.parse_atomic_ordering()?))
        } else {
            None
        };
        let align = self.parse_optional_comma_align_strict()?;

        // `parseStore`'s checks, in its order. Note the anchors differ: the
        // pointer rule reports at `PtrLoc`, everything else at the *value*.
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v
            .try_into()
            .map_err(|_| self.message_at(ptr_loc, "store operand must be a pointer"))?;
        if !val_ty.is_first_class() {
            return Err(self.message_at(value_loc, "store operand must be a first class value"));
        }
        if is_atomic && align.is_none() {
            return Err(self.message_at(
                value_loc,
                "atomic store must have explicit non-zero alignment",
            ));
        }
        if let Some((_, ordering)) = scope_and_ordering
            && matches!(
                ordering,
                AtomicOrdering::Acquire | AtomicOrdering::AcquireRelease
            )
        {
            return Err(self.message_at(value_loc, "atomic store cannot use Acquire ordering"));
        }
        if align.is_none() && !val_ty.is_sized() {
            return Err(self.message_at(value_loc, "storing unsized types is not allowed"));
        }

        // Runtime-clause dispatch, mirroring [`Self::parse_load`].
        let mut store = b.store_to(val_v, ptr);
        if volatile {
            store = store.volatile();
        }
        if let Some((sync_scope, ordering)) = scope_and_ordering {
            store = store.atomic(ordering).sync_scope(sync_scope);
        }
        if let Some(align) = align {
            store = store.align(align);
        }
        store.build().map_err(|e| self.builder_err("store", e))?;
        Ok(())
    }

    /// `getelementptr FLAGS SOURCE_TY, ptr P, INDEX, INDEX, ...` where
    /// FLAGS is any-order `inbounds` / `nusw` / `nuw`.
    /// Mirrors `LLParser::parseGetElementPtr` (LLParser.cpp).
    fn parse_gep(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // Upstream loops over the flag keywords in any order
        // (`test/Assembler/flags.ll` has both `nusw nuw` and
        // `nuw nusw inbounds`); AsmWriter's canonical print order is
        // `inbounds` / `nusw` / `nuw`, so a fixed eat order could not even
        // re-parse this crate's own output. Same loop as
        // `Self::parse_gep_constant_expr_flags`.
        let mut flags = GepNoWrapFlags::empty();
        loop {
            if self.eat_keyword(Keyword::Inbounds)? {
                flags |= GepNoWrapFlags::inbounds();
            } else if self.eat_keyword(Keyword::Nusw)? {
                flags |= GepNoWrapFlags::NUSW;
            } else if self.eat_keyword(Keyword::Nuw)? {
                flags |= GepNoWrapFlags::NUW;
            } else {
                break;
            }
        }
        let source_ty = self.parse_type(false)?;
        self.expect_punct(PunctKind::Comma, "comma after getelementptr's type")?;
        let base_loc = self.loc();
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        // `dyn_cast<PointerType>(BaseType->getScalarType())` — a *vector* of
        // pointers is a legal base, which is why upstream asks the scalar
        // type rather than the type itself.
        if pointer_address_space_or_vector_element(ptr_v.ty()).is_none() {
            return Err(self.message_at(base_loc, "base of getelementptr must be a pointer"));
        }
        let mut index_values: Vec<llvmkit_ir::Value<'ctx, B>> = Vec::new();
        // `ElementCount GEPWidth = BaseType->isVectorTy()
        //      ? cast<VectorType>(BaseType)->getElementCount()
        //      : ElementCount::getFixed(0);` — `None` is the `getFixed(0)`
        // sentinel, which no real vector type can produce upstream either
        // (`VectorType::get` asserts a non-zero element count).
        let mut gep_width = vector_shape_type(ptr_v.ty());
        while matches!(self.peek(), Token::Comma) {
            let saved_lex = self.lex.clone();
            let saved_current = self.current.clone();
            let saved_prev_token_end = self.prev_token_end;
            self.bump()?;
            // A trailing `, !dbg !N` attachment is not an index. Upstream
            // breaks out of the index loop on `MetadataVar` and reports the
            // comma as already eaten (`InstExtraComma`); llvmkit restores it
            // so `skip_trailing_metadata` sees the comma it expects, the same
            // backtrack `parse_optional_comma_array_size` uses for alloca.
            if matches!(self.peek(), Token::MetadataVar(_)) {
                self.lex = saved_lex;
                self.current = saved_current;
                self.prev_token_end = saved_prev_token_end;
                break;
            }
            let elt_loc = self.loc();
            let idx_ty = self.parse_type(false)?;
            let idx_v = self.parse_value(state, idx_ty)?;
            if !is_int_or_int_vector_type(idx_v.ty()) {
                return Err(self.message_at(elt_loc, "getelementptr index must be an integer"));
            }
            if let Some(index_shape) = vector_shape_type(idx_v.ty()) {
                if gep_width.is_some_and(|width| width != index_shape) {
                    return Err(self.message_at(
                        elt_loc,
                        "getelementptr vector index has a wrong number of elements",
                    ));
                }
                gep_width = Some(index_shape);
            }
            index_values.push(idx_v);
        }

        // `parseGetElementPtr`'s three tail checks. Note the scalable rule
        // differs from the constant-expression arm's: an *instruction* asks
        // only whether the source type is a struct containing a scalable
        // vector, where `ConstantExpr::isSupportedGetElementPtr` refuses any
        // scalable source outright.
        if !index_values.is_empty() && !source_ty.is_sized() {
            return Err(self.message_at(base_loc, "base element of getelementptr must be sized"));
        }
        if source_ty.is_struct() && source_ty.is_scalable() {
            return Err(self.message_at(
                base_loc,
                "getelementptr cannot target structure that contains scalable vector type",
            ));
        }
        if llvmkit_ir::indexed_gep_type(source_ty, &index_values).is_none() {
            return Err(self.message_at(base_loc, "invalid getelementptr indices"));
        }

        // `GetElementPtrInst *GEP = GetElementPtrInst::Create(Ty, Ptr, Indices);
        //  Inst = GEP; GEP->setNoWrapFlags(NW);` — the erased entry point,
        // because a `<N x ptr>` base and `<N x iM>` indices are both legal
        // here and neither is a `PointerValue` / `IntValue`. Every one of
        // upstream's rules has already answered above, in upstream's order.
        let name = result_name.as_str();
        let v = b
            .gep_erased(source_ty, ptr_v, index_values, flags, name)
            .map_err(|e| self.builder_err("getelementptr", e))?;
        Ok(b.view(v))
    }

    /// `select i1 COND, TYPE TRUE, TYPE FALSE`, and the `<N x i1>` condition
    /// form alongside it. Mirrors `LLParser::parseSelect`.
    ///
    /// Construction goes through
    /// [`llvmkit_ir::IrBuilder::select_erased`] for every arm category.
    /// The typed [`llvmkit_ir::IrBuilder::select`] cannot express two of
    /// the shapes LLVM allows — a `<N x i1>` condition, which is no
    /// `IntValue<bool>`, and a vector arm, which no `SelectArm` marker
    /// describes — and its narrowing would be discarded here anyway, since
    /// this returns an erased value whichever arm ran.
    ///
    /// The condition and token checks below run *before* constant folding, and
    /// that ordering is the point: the folder answers for two equal arms
    /// without inspecting the condition, so a malformed `select` whose arms
    /// agree would otherwise fold away instead of being rejected. Everything
    /// else is left to the builder, which ports `areInvalidOperands` whole.
    fn parse_select(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
        opcode_loc: Span,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `LLParser::parseInstruction`'s `kw_select` arm eats fast-math flags
        // before calling `parseSelect`, then applies them to the result --
        // rejecting them outright when the result is not floating-point.
        let fmf = self.parse_optional_fmf()?;
        let cond_ty = self.parse_type(false)?;
        let cond_v = self.parse_value(state, cond_ty)?;
        let cond_value = cond_v;
        self.expect_punct(PunctKind::Comma, "',' after select condition")?;
        let true_ty = self.parse_type(false)?;
        let true_v = self.parse_value(state, true_ty)?;
        self.expect_punct(PunctKind::Comma, "',' between select arms")?;
        let false_ty = self.parse_type(false)?;
        let false_v = self.parse_value(state, false_ty)?;
        if true_ty != false_ty {
            return Err(self.expected("matching arm types in select"));
        }
        let valid_condition = match cond_ty.into_type_enum() {
            AnyTypeEnum::Int(ty) => ty.bit_width() == 1,
            AnyTypeEnum::Vector(ty) => IntType::<IntDyn, B>::try_from(ty.element())
                .is_ok_and(|element| element.bit_width() == 1),
            _ => false,
        };
        if !valid_condition {
            return Err(self.expected("i1 select condition"));
        }
        // `"select values cannot have token type"`, checked here rather than
        // left to the builder because constant folding runs first below and
        // would answer for two equal token arms before anything rejected them.
        // `areInvalidOperands` names no other arm restriction: a struct or
        // array arm is valid LLVM and parses.
        if matches!(true_ty.into_type_enum(), AnyTypeEnum::Token(_)) {
            return Err(self.expected("select arms of a type other than token"));
        }
        // `if (!isa<FPMathOperator>(Inst))`, whose `Select` arm is
        // `FPMathOperator::isSupportedFloatingPointType(V->getType())` — wider
        // than `isFPOrFPVectorTy`, which is why this is not
        // `is_fp_or_fp_vector_type`. The anchor is upstream's `Loc`, taken in
        // `parseInstruction` *before* the opcode keyword is eaten.
        if !fmf.is_empty() && !llvmkit_ir::is_supported_floating_point_type(true_ty) {
            return Err(self.message_at(
                opcode_loc,
                "fast-math-flags specified for select without floating-point scalar or vector return type",
            ));
        }
        // No constant folding here. `LLParser::parseSelect` ends in an
        // unconditional `SelectInst::Create`, and the parser's builder is
        // already a `NoFolder` one, so an all-constant `select` must still
        // become an instruction. Folding it away made `%r = select i1 true,
        // i32 5, i32 5` vanish from the printed module and left a trailing
        // `!dbg` to attach to whatever instruction preceded it — the
        // attachment path takes the block's last instruction, and a folded
        // select adds none. LLVM 22 removed `select` constexprs outright
        // (`select constexprs are no longer supported`), so there is no
        // constant form for this to fold *into*.
        let id = b
            .select_erased_with_fmf(cond_value, true_v, false_v, fmf, result_name.as_str())
            .map_err(|e| self.builder_err("select", e))?;
        Ok(b.view(id))
    }

    /// `fptosi`/`fptoui TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// for `Instruction::FPToSI` / `FPToUI`.
    fn parse_fp_to_int(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        op: FpToInt,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in fp->int cast")?;
        let dst_ty = self.parse_type(false)?;
        let src_fp: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> = src_v
            .try_into()
            .map_err(|_| self.expected("float-typed source for fp->int cast"))?;
        let dst_int = match dst_ty.into_type_enum() {
            AnyTypeEnum::Int(t) => t,
            _ => return Err(self.expected("integer destination for fp->int cast")),
        };
        let name = result_name.as_str();
        let v = match op {
            FpToInt::FpToSi => b
                .fp_to_si(src_fp, dst_int, name)
                .map_err(|e| self.builder_err("fptosi", e))?,
            FpToInt::FpToUi => b
                .fp_to_ui(src_fp, dst_int, name)
                .map_err(|e| self.builder_err("fptoui", e))?,
        };
        Ok(b.view(v).as_erased())
    }

    /// `sitofp`/`uitofp TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// for `Instruction::SIToFP` / `UIToFP`.
    fn parse_int_to_fp(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        op: IntToFp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let nneg = matches!(op, IntToFp::UiToFp) && self.eat_keyword(Keyword::Nneg)?;
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in int->fp cast")?;
        let dst_ty = self.parse_type(false)?;
        let src_int: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = src_v
            .try_into()
            .map_err(|_| self.expected("integer-typed source for int->fp cast"))?;
        let dst_fp = match dst_ty.into_type_enum() {
            AnyTypeEnum::Float(t) => t,
            _ => return Err(self.expected("float destination for int->fp cast")),
        };
        let name = result_name.as_str();
        let v = match op {
            IntToFp::SiToFp => b
                .si_to_fp(src_int, dst_fp, name)
                .map_err(|e| self.builder_err("sitofp", e))?,
            IntToFp::UiToFp => {
                if nneg {
                    b.ui_to_fp_with_flags_dyn(src_int, dst_fp, UiToFpFlags::new().nneg(), name)
                        .map_err(|e| self.builder_err("uitofp", e))?
                } else {
                    b.ui_to_fp(src_int, dst_fp, name)
                        .map_err(|e| self.builder_err("uitofp", e))?
                }
            }
        };
        Ok(b.view(v).as_erased())
    }

    /// `addrspacecast ptr VALUE to ptr`. Mirrors `LLParser::parseCast`
    /// for `Instruction::AddrSpaceCast`.
    fn parse_addrspace_cast(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in addrspacecast")?;
        let dst_ty = self.parse_type(false)?;
        let src_ptr: llvmkit_ir::PointerValue<'ctx, B> = src_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed source for addrspacecast"))?;
        let dst_ptr = match dst_ty.into_type_enum() {
            AnyTypeEnum::Pointer(t) => t,
            _ => return Err(self.expected("ptr destination for addrspacecast")),
        };
        let v = b
            .addrspace_cast(src_ptr, dst_ptr, result_name.as_str())
            .map_err(|e| self.builder_err("addrspacecast", e))?;
        Ok(b.view(v).as_erased())
    }

    // ── S3.2: new opcode parsers ──────────────────────────────────────────

    /// `bitcast <src-ty> <src-val> to <dst-ty>`. Mirrors `LLParser::parseCast`
    /// `Instruction::BitCast` arm. Uses `bitcast_dyn` for the parser's
    /// runtime-typed path.
    ///
    /// Upstream: `test/Assembler/bitcast.ll`.
    fn parse_bitcast(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in bitcast")?;
        let dst_ty = self.parse_type(false)?;
        let name = result_name.as_str();
        let v = b
            .bitcast_dyn(src_v, dst_ty, name)
            .map_err(|e| self.builder_err("bitcast", e))?;
        Ok(b.view(v))
    }

    /// `fptrunc <fp-ty> <val> to <fp-ty>`. Mirrors `LLParser::parseCast`
    /// `Instruction::FPTrunc` arm. Uses `fp_trunc_dyn`.
    ///
    /// Upstream: `test/Assembler/fptrunc.ll`.
    fn parse_fptrunc(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `LLParser::parseInstruction` eats fast-math flags for `fptrunc` and
        // `fpext` -- the two cast opcodes that are `FPMathOperator`s -- before
        // dispatching to `parseCast`.
        let fmf = self.parse_optional_fmf()?;
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in fptrunc")?;
        let dst_ty = self.parse_type(false)?;
        let sv: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> = src_v
            .try_into()
            .map_err(|_| self.expected("float-typed source for fptrunc"))?;
        let df = match dst_ty.into_type_enum() {
            AnyTypeEnum::Float(t) => t,
            _ => return Err(self.expected("float destination type for fptrunc")),
        };
        let v = b
            .fp_trunc_dyn_with_fmf(sv, df, fmf, result_name.as_str())
            .map_err(|e| self.builder_err("fptrunc", e))?;
        Ok(b.view(v).as_erased())
    }

    /// `fpext <fp-ty> <val> to <fp-ty>`. Mirrors `LLParser::parseCast`
    /// `Instruction::FPExt` arm. Uses `fp_ext_dyn`.
    ///
    /// Upstream: `test/Assembler/fpext.ll`.
    fn parse_fpext(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `LLParser::parseInstruction` eats fast-math flags for `fptrunc` and
        // `fpext` -- the two cast opcodes that are `FPMathOperator`s -- before
        // dispatching to `parseCast`.
        let fmf = self.parse_optional_fmf()?;
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in fpext")?;
        let dst_ty = self.parse_type(false)?;
        let sv: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> = src_v
            .try_into()
            .map_err(|_| self.expected("float-typed source for fpext"))?;
        let df = match dst_ty.into_type_enum() {
            AnyTypeEnum::Float(t) => t,
            _ => return Err(self.expected("float destination type for fpext")),
        };
        let v = b
            .fp_ext_dyn_with_fmf(sv, df, fmf, result_name.as_str())
            .map_err(|e| self.builder_err("fpext", e))?;
        Ok(b.view(v).as_erased())
    }

    /// `ptrtoaddr <ptr-or-vector-ty> <val> to <int-or-vector-ty>`. Mirrors
    /// `LLParser::parseCast` for `Instruction::PtrToAddr`.
    ///
    /// Upstream: `test/Assembler/ptrtoaddr.ll`.
    fn parse_ptrtoaddr(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in ptrtoaddr")?;
        let dst_ty = self.parse_type(false)?;
        let v = b
            .ptr_to_addr_dyn(src_v, dst_ty, result_name.as_str())
            .map_err(|e| self.builder_err("ptrtoaddr", e))?;
        Ok(b.view(v))
    }

    /// `extractelement <vec-ty> <vec>, <idx-ty> <idx>`.
    /// Mirrors `LLParser::parseExtractElement`.
    ///
    /// Upstream: `test/Assembler/extractelement.ll`.
    fn parse_extractelement(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `ExtractElementInst::isValidOperands` is one predicate over both
        // operands, so upstream has one message for every way it can fail,
        // anchored on the *vector*.
        let operand_loc = self.loc();
        let vec_ty = self.parse_type(false)?;
        let vec_v = self.parse_value(state, vec_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after extract value")?;
        let idx_ty = self.parse_type(false)?;
        let idx_v = self.parse_value(state, idx_ty)?;
        if !is_vector_type(vec_ty) || !idx_ty.is_integer() {
            return Err(self.message_at(operand_loc, "invalid extractelement operands"));
        }
        let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = idx_v
            .try_into()
            .map_err(|_| self.message_at(operand_loc, "invalid extractelement operands"))?;
        let v = b
            .extract_element(vec_v, idx, result_name.as_str())
            .map_err(|e| self.builder_err("extractelement", e))?;
        Ok(b.view(v))
    }

    /// `insertelement <vec-ty> <vec>, <elt-ty> <elt>, <idx-ty> <idx>`.
    /// Mirrors `LLParser::parseInsertElement`.
    ///
    /// Upstream: `test/Assembler/insertelement.ll`.
    fn parse_insertelement(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `InsertElementInst::isValidOperands`: a vector, an element matching
        // its element type, and an integer index — one message for all three.
        let operand_loc = self.loc();
        let vec_ty = self.parse_type(false)?;
        let vec_v = self.parse_value(state, vec_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after insertelement value")?;
        let elt_ty = self.parse_type(false)?;
        let elt_v = self.parse_value(state, elt_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after insertelement value")?;
        let idx_ty = self.parse_type(false)?;
        let idx_v = self.parse_value(state, idx_ty)?;
        let element_matches = match AnyTypeEnum::from(vec_ty) {
            AnyTypeEnum::Vector(vector) => vector.element() == elt_ty,
            _ => false,
        };
        if !element_matches || !idx_ty.is_integer() {
            return Err(self.message_at(operand_loc, "invalid insertelement operands"));
        }
        let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = idx_v
            .try_into()
            .map_err(|_| self.message_at(operand_loc, "invalid insertelement operands"))?;
        let v = b
            .insert_element(vec_v, elt_v, idx, result_name.as_str())
            .map_err(|e| self.builder_err("insertelement", e))?;
        Ok(b.view(v))
    }

    /// `shufflevector <ty> <v1>, <ty> <v2>, <mask-ty> <mask>`. Mirrors
    /// `LLParser::parseShuffleVector`.
    ///
    /// The only `test/Assembler` fixture naming this opcode is
    /// `constant-splat.ll`, and it writes the *constant-expression* form;
    /// `test/Verifier` names it nowhere, and nothing under `test/Assembler`,
    /// `test/Verifier` or `test/Bitcode` pins this routine's diagnostic. The
    /// instruction form is exercised by `test/Bitcode/vscale-round-trip.ll`
    /// (`@non_const_shufflevector`) and `test/Bitcode/compatibility.ll`; the
    /// rule itself is `ShuffleVectorInst::isValidOperands`
    /// (`Instructions.cpp`).
    fn parse_shufflevector(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `LocTy Loc;` — `parseTypeAndValue(Op0, Loc, PFS)` records the span of
        // the FIRST operand, and the routine's one error is anchored there.
        let operand_loc = self.loc();
        let v1_ty = self.parse_type(false)?;
        let v1 = self.parse_value(state, v1_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after shuffle mask")?;
        let v2_ty = self.parse_type(false)?;
        let v2 = self.parse_value(state, v2_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after shuffle value")?;
        // `parseTypeAndValue(Op2, PFS)`. The mask is read as an ordinary
        // value, not as a constant: upstream lets `%m` through here and lets
        // `isValidOperands` refuse it, so the diagnostic stays the operand one.
        // Whatever this parse says propagates untouched — upstream re-words
        // nothing.
        let mask_ty = self.parse_type(false)?;
        let mask = self.parse_value(state, mask_ty)?;

        // `if (!ShuffleVectorInst::isValidOperands(Op0, Op1, Op2))
        //    return error(Loc, "invalid shufflevector operands");`
        // One check, one message, anchored at the first operand — the mask's
        // type and shape are part of it.
        if !llvmkit_ir::ShuffleVectorInst::is_valid_operands_with_constant_mask(v1, v2, mask) {
            return Err(self.message_at(operand_loc, "invalid shufflevector operands"));
        }

        // `Inst = new ShuffleVectorInst(Op0, Op1, Op2);`, whose body runs
        // `getShuffleMask(cast<Constant>(Mask), MaskArr)` before constructing.
        // Both of that step's failure modes are assertions upstream — the
        // `cast<Constant>`, and `getShuffleMask`'s "Scalable vector shuffle
        // mask must be undef or zeroinitializer" — and the check above has
        // already made each unreachable. llvmkit does not port a crash: the
        // decode answers upstream's own message for this routine at upstream's
        // own anchor, so no text is invented and no failure is swallowed.
        let decoded = llvmkit_ir::Constant::try_from(mask)
            .ok()
            .and_then(shufflevector_mask_from_constant)
            .ok_or_else(|| self.message_at(operand_loc, "invalid shufflevector operands"))?;
        // `setShuffleMask(MaskArr)` plus the `Value *Mask` constructor's own
        // `VectorType::get(V1 element type, cast<VectorType>(Mask->getType())
        // ->getElementCount())`. `IrBuilder::shuffle_vector` is the
        // `ArrayRef<int>` constructor, which spells the same type as
        // `VectorType::get(EltTy, Mask.size(), isa<ScalableVectorType>(V1->
        // getType()))`; the two agree here because `getShuffleMask` yields one
        // entry per mask-type lane, and the check above proved the mask's
        // scalability equal to V1's.
        let v = b
            .shuffle_vector(v1, v2, &decoded, result_name.as_str())
            .map_err(|e| self.builder_err("shufflevector", e))?;
        Ok(b.view(v))
    }

    /// `extractvalue <agg-ty> <agg>, <idx>, ...`. Mirrors
    /// `LLParser::parseExtractValue`.
    ///
    /// Upstream: `test/Assembler/extractvalue.ll`.
    fn parse_extractvalue(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let operand_loc = self.loc();
        let agg_ty = self.parse_type(false)?;
        let agg_v = self.parse_value(state, agg_ty)?;
        let indices = self.parse_index_list()?;
        if !agg_ty.is_aggregate() {
            return Err(self.message_at(operand_loc, "extractvalue operand must be aggregate type"));
        }
        if llvmkit_ir::indexed_aggregate_type(agg_ty, &indices).is_none() {
            return Err(self.message_at(operand_loc, "invalid indices for extractvalue"));
        }
        let v = b
            .extract_value_dyn(agg_v, &indices, result_name.as_str())
            .map_err(|e| self.builder_err("extractvalue", e))?;
        Ok(b.view(v))
    }

    /// `(',' uint32)+` — mirrors `LLParser::parseIndexList`, which
    /// `parseExtractValue` and `parseInsertValue` share.
    ///
    /// The **first** comma is required, so `extractvalue {i32} %a` with no
    /// index at all is rejected; llvmkit's two copies of this loop simply
    /// produced an empty index list and accepted it. A `, !dbg !N` after at
    /// least one index ends the list, and with *no* index it is
    /// `expected index` rather than a silent empty list.
    fn parse_index_list(&mut self) -> ParseResult<Vec<u32>> {
        if !matches!(self.peek(), Token::Comma) {
            return Err(self.message("expected ',' as start of index list"));
        }
        let mut indices = Vec::new();
        while matches!(self.peek(), Token::Comma) {
            let saved_lex = self.lex.clone();
            let saved_current = self.current.clone();
            let saved_prev_token_end = self.prev_token_end;
            self.bump()?;
            if matches!(self.peek(), Token::MetadataVar(_)) {
                if indices.is_empty() {
                    return Err(self.message("expected index"));
                }
                // Upstream reports the comma as already eaten
                // (`InstExtraComma`); llvmkit restores it so
                // `skip_trailing_metadata` sees the comma it expects — the
                // same backtrack `parse_optional_comma_array_size` uses.
                self.lex = saved_lex;
                self.current = saved_current;
                self.prev_token_end = saved_prev_token_end;
                return Ok(indices);
            }
            indices.push(self.parse_uint32()?);
        }
        Ok(indices)
    }

    /// `insertvalue <agg-ty> <agg>, <elt-ty> <elt>, <idx>, ...`. Mirrors
    /// `LLParser::parseInsertValue`.
    ///
    /// Upstream: `test/Assembler/insertvalue.ll`.
    fn parse_insertvalue(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let agg_loc = self.loc();
        let agg_ty = self.parse_type(false)?;
        let agg_v = self.parse_value(state, agg_ty)?;
        self.expect_punct(PunctKind::Comma, "comma after insertvalue operand")?;
        let elt_loc = self.loc();
        let elt_ty = self.parse_type(false)?;
        let elt_v = self.parse_value(state, elt_ty)?;
        let indices = self.parse_index_list()?;
        // `parseInsertValue`'s three checks. Note the anchors differ: the
        // aggregate rules report at the *aggregate*, the disagreement at the
        // inserted value.
        if !agg_ty.is_aggregate() {
            return Err(self.message_at(agg_loc, "insertvalue operand must be aggregate type"));
        }
        let Some(indexed_ty) = llvmkit_ir::indexed_aggregate_type(agg_ty, &indices) else {
            return Err(self.message_at(agg_loc, "invalid indices for insertvalue"));
        };
        if indexed_ty != elt_ty {
            return Err(self.message_at(
                elt_loc,
                format!(
                    "insertvalue operand and field disagree in type: '{elt_ty}' instead of '{indexed_ty}'"
                ),
            ));
        }
        let v = b
            .insert_value_dyn(agg_v, elt_v, &indices, result_name.as_str())
            .map_err(|e| self.builder_err("insertvalue", e))?;
        Ok(b.view(v))
    }

    /// `phi <ty> [ <val>, <label> ], ...`. Handles any first-class *data*
    /// result type — int, float, pointer, vector, array, or struct; other
    /// first-class types (`label` / `metadata` / `token`) and non-first-class
    /// types are rejected. Forward-referenced incoming values are stored in
    /// `state.deferred_phi` and resolved by `PerFunctionState::finish`.
    /// Mirrors `LLParser::parsePhi` (LLParser.cpp ~7990).
    ///
    /// Upstream: `test/Assembler/phi.ll`.
    fn parse_phi(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
        opcode_loc: Span,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `LLParser::parseInstruction`'s `kw_phi` arm eats fast-math flags
        // before calling `parsePHI`, then applies them -- rejecting them when
        // the phi's result type is not floating-point. They used to be parsed
        // and dropped here, so `phi fast float ...` round-tripped without its
        // flags.
        let fmf = self.parse_optional_fmf()?;
        let type_loc = self.loc();
        let ty = self.parse_type(false)?;
        // `parsePHI`'s own first check, before any incoming is read.
        if !ty.is_first_class() {
            return Err(self.message_at(type_loc, "phi node must have first class type"));
        }
        // `if (!isa<FPMathOperator>(Inst))`, whose `PHI` arm is
        // `FPMathOperator::isSupportedFloatingPointType(V->getType())`. The
        // anchor is upstream's `Loc`, taken in `parseInstruction` *before* the
        // opcode keyword is eaten.
        if !fmf.is_empty() && !llvmkit_ir::is_supported_floating_point_type(ty) {
            return Err(self.message_at(
                opcode_loc,
                "fast-math-flags specified for phi without floating-point scalar or vector return type",
            ));
        }
        let name = result_name.as_str();
        // Build the phi and extract its value ID for deferred edge resolution.
        let phi_val = match ty.into_type_enum() {
            AnyTypeEnum::Int(int_ty) => {
                let phi = b
                    .int_phi_dyn(int_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                b.view(phi).to_erased()
            }
            AnyTypeEnum::Float(fp_ty) => {
                let phi = b
                    .fp_phi_dyn(fp_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                let phi = b.view(phi);
                phi.set_fast_math_flags(fmf)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.to_erased()
            }
            AnyTypeEnum::Pointer(ptr_ty) => {
                let phi = b
                    .pointer_phi_in_addrspace(ptr_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                b.view(phi).to_erased()
            }
            // The remaining first-class *data* types — vector, array, and
            // non-opaque struct — are legal phi result types. Route them through
            // the erased `phi_dyn`; the type-checked incoming-add path is
            // unchanged. The `is_first_class` guard is what excludes an *opaque*
            // struct (no body, hence unsized): it is `AnyTypeEnum::Struct` but
            // not a valid phi result, and `Module::verify()` rejects it, so
            // reject it here rather than parse IR that cannot verify.
            AnyTypeEnum::Vector(_) | AnyTypeEnum::Array(_) | AnyTypeEnum::Struct(_)
                if ty.is_first_class() =>
            {
                let phi = b
                    .phi_dyn(ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                let phi = b.view(phi);
                // A `<N x float>` phi is an `FPMathOperator` too; the guard
                // above already rejected flags on any non-FP result.
                phi.set_fast_math_flags(fmf)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.to_erased()
            }
            // Everything else is rejected here. `label`, `metadata`, and
            // `token` are first-class per `Type::is_first_class` yet are not
            // valid phi result types (LLVM rejects e.g. `phi token`, and the
            // llvmkit verifier does not catch it); function / void /
            // opaque-struct types are likewise invalid (`void` is already
            // caught earlier by `parse_type`). Gating on `is_first_class`
            // would wrongly admit the label/metadata/token cases, so the
            // acceptable result types are enumerated explicitly instead.
            _ => {
                return Err(self.expected(
                    "phi result type must be int, float, pointer, vector, array, or non-opaque struct",
                ));
            }
        };
        // Record the phi's source location, keyed by its arena id, so the
        // end-of-function coherence check can anchor a diagnostic here — a
        // numbered/anonymous phi has no matchable textual name.
        state.phi_locs.push((phi_val.slot(), self.loc()));
        // Parse incoming pairs: `[ val, label ], ...`
        // First pair has no leading comma; subsequent pairs have one.
        let mut first = true;
        loop {
            if !first {
                let saved_lex = self.lex.clone();
                let saved_current = self.current.clone();
                let saved_prev_token_end = self.prev_token_end;
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
                // A trailing `, !dbg !N` attachment is not another incoming
                // pair. Upstream breaks out of the pair loop on `MetadataVar`
                // and reports the comma as already eaten (`InstExtraComma`);
                // llvmkit restores it so `skip_trailing_metadata` sees the
                // comma it expects, the same backtrack the index loops use.
                if matches!(self.peek(), Token::MetadataVar(_)) {
                    self.lex = saved_lex;
                    self.current = saved_current;
                    self.prev_token_end = saved_prev_token_end;
                    break;
                }
                if !matches!(self.peek(), Token::LSquare) {
                    // Comma was consumed by something else — error.
                    return Err(self.expected("'[' to start phi incoming pair after ','"));
                }
            }
            first = false;
            if !matches!(self.peek(), Token::LSquare) {
                break;
            }
            self.bump()?; // eat `[`
            let val_loc = self.loc();
            // Upstream reads the incoming value with the general
            // `parseValue(Ty, Op0, PFS)`, which mints a forward reference for
            // a name that is not yet defined — the loop-carried
            // `%next` of `[ %next, %loop ]` — and accepts every other value
            // form besides. The edge is added immediately; predecessor
            // coherence is checked once, for the whole function, by
            // `check_function_phi_coherence` at `finish`.
            let val = self.parse_value(state, ty)?;
            self.expect_punct(PunctKind::Comma, "',' in phi incoming pair")?;
            let bb_ref = self.parse_phi_label(state)?;
            self.expect_punct(PunctKind::RSquare, "']' to close phi incoming pair")?;
            let bb = state.resolve_block_ref(self.module, &bb_ref, val_loc)?;
            b.phi_add_incoming_from_value(phi_val, val, bb)
                .map_err(|e| self.builder_err("phi.add_incoming", e))?;
        }
        Ok(phi_val)
    }

    /// Parse the label in a `[ val, label %name ]` phi pair.
    fn parse_phi_label(&mut self, state: &mut PerFunctionState<'ctx, B>) -> ParseResult<BlockRef> {
        let loc = self.loc();
        match self.peek() {
            Token::LocalVar(_) => {
                let n = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("block label in phi pair"))?;
                self.bump()?;
                // A phi predecessor may already be terminated (the common
                // merge-block case), so ensure the block through the
                // state-agnostic label path, never the unterminated-only
                // construction path.
                state.ensure_block_label(self.module, &n, loc)?;
                Ok(BlockRef::Named(n))
            }
            Token::LocalVarId(id) => {
                let id = *id;
                self.bump()?;
                state.get_or_create_numbered_block_label(self.module, id, loc)?;
                Ok(BlockRef::Numbered(id))
            }
            _ => Err(self.expected("block label in phi incoming pair")),
        }
    }

    /// `call [tail] [cc] [ret-attrs] <ret-ty> @func(<args>) [fn-attrs]`.
    /// Handles both void and value-returning calls. Mirrors
    /// `LLParser::parseCall` (LLParser.cpp ~8250).
    ///
    /// Upstream: `test/Assembler/call.ll`.
    fn parse_call(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let tail_kind = if self.eat_keyword(Keyword::Tail)? {
            llvmkit_ir::instr_types::TailCallKind::Tail
        } else if self.eat_keyword(Keyword::Musttail)? {
            llvmkit_ir::instr_types::TailCallKind::MustTail
        } else if self.eat_keyword(Keyword::Notail)? {
            llvmkit_ir::instr_types::TailCallKind::NoTail
        } else {
            llvmkit_ir::instr_types::TailCallKind::None
        };
        // `LocTy CallLoc = Lex.getLoc();` is `LLParser::parseCall`'s last
        // statement before `parseToken(lltok::kw_call, …)`, and so before
        // `EatFastMathFlagsIfPresent()` too. `parseInstruction` has already
        // eaten the opcode keyword, so for a plain `call` this is the token
        // after `call`, and for `tail`/`musttail`/`notail` it is the `call`
        // token itself. llvmkit eats the tail keyword here rather than in the
        // dispatcher, so this is the equivalent point. Three diagnostics are
        // anchored on it: `not enough parameters specified for call`, the
        // fast-math guard and the `llvm.dbg` guard.
        let call_loc = self.loc();
        // `if (TCK != CallInst::TCK_None && parseToken(lltok::kw_call, …))
        // return true;` — after a tail keyword the `call` keyword is
        // mandatory, and its absence is a diagnostic rather than a silent
        // continue. For a plain `call` there is nothing to eat here: llvmkit's
        // instruction dispatch has already consumed it, which is upstream's
        // `parseInstruction` `Lex.Lex(); // Eat the keyword.`
        if !matches!(tail_kind, llvmkit_ir::instr_types::TailCallKind::None) {
            if !matches!(self.peek(), Token::Instruction(Opcode::Call)) {
                return Err(self.expected("'tail call', 'musttail call', or 'notail call'"));
            }
            self.bump()?;
        }
        // `LLParser::parseCall` eats the flags here, before the calling
        // convention, and rejects them below when the return type is not
        // floating-point.
        let fmf = self.parse_optional_fmf()?;
        let calling_conv = self.parse_optional_calling_conv()?;
        let return_attrs = self.parse_optional_return_attrs()?;
        // `LLParser::parseCall` reads the call site's address space here, in
        // the `||` chain between the return attributes and the callee type.
        // The position is load-bearing: it decides whether a malformed
        // `addrspace(...)` or a missing return type is reported first. Absent,
        // the address space is the datalayout's *program* address space, not 0
        // (`parseOptionalProgramAddrSpace`).
        let call_addr_space = self.parse_optional_program_addr_space()?;
        let ret_ty_loc = self.loc();
        let callee_ty = self.parse_type(true)?;
        let parsed_callee = self.parse_direct_callee_ref(state, call_addr_space)?;
        self.expect_punct(PunctKind::LParen, "'(' in call argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx, B>> = Vec::new();
        let mut arg_tys: Vec<Type<'ctx, B>> = Vec::new();
        let mut arg_locs: Vec<Span> = Vec::new();
        let mut arg_attrs = Vec::new();
        let musttail = matches!(tail_kind, llvmkit_ir::instr_types::TailCallKind::MustTail);
        let enclosing_varargs = state.func.signature().is_var_arg();
        let mut var_args = false;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    // Musttail forwarding ellipsis (`LLParser::parseParameterList`):
                    // valid only in a musttail call inside a varargs function.
                    if !musttail {
                        return Err(self.expected(
                            "unexpected ellipsis in argument list for non-musttail call",
                        ));
                    }
                    if !enclosing_varargs {
                        return Err(self.expected(
                            "unexpected ellipsis in argument list for musttail call in non-varargs function",
                        ));
                    }
                    self.bump()?;
                    var_args = true;
                    break;
                }
                let arg_loc = self.loc();
                let arg_ty = self.parse_type(false)?;
                let one_arg_attrs = self.parse_optional_param_attrs()?;
                // `parseParameterList` branches on the argument type: a
                // `metadata` parameter takes `parseMetadataAsValue`, not
                // `parseValue`, which is what makes `metadata i32 %a` — every
                // old-format debug intrinsic operand — legal.
                let arg_v = if arg_ty.is_metadata() {
                    self.parse_metadata_as_value(state)?
                } else {
                    self.parse_value(state, arg_ty)?
                };
                arg_tys.push(arg_ty);
                arg_locs.push(arg_loc);
                arg_attrs.push(one_arg_attrs);
                args.push(arg_v);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close call argument list")?;
        // Reciprocal rule: a musttail call in a varargs function must forward
        // the varargs with a trailing `...`.
        if musttail && enclosing_varargs && !var_args {
            return Err(self
                .expected("'...' at end of argument list for musttail call in varargs function"));
        }
        let (function_attrs, function_attr_groups) = self.parse_optional_fn_attrs()?;
        let operand_bundles = self.parse_optional_operand_bundles(state)?;
        let call_attrs = llvmkit_ir::instr_types::CallAttributeData::new(
            return_attrs,
            arg_attrs.into_boxed_slice(),
            function_attrs,
        )
        .function_attr_groups(function_attr_groups.into_boxed_slice())
        .operand_bundles(operand_bundles)
        .fast_math_flags(fmf);
        // `resolveFunctionType`: an explicit function type is used as written;
        // anything else is a bare *return* type and the signature is built
        // from the arguments — which is why the walk below only bites on the
        // explicit form.
        //
        // `FunctionType::get(RetType, ParamTypes, false)` — the variadic bit
        // is hardcoded off. A musttail forwarding `...` is consumed by
        // `parseParameterList` and contributes no `ParamInfo`, so it never
        // reaches `ParamTypes`; threading `var_args` in here built a call-site
        // type upstream never builds. `Verifier::verifyMustTailCall`'s
        // `CallerTy->isVarArg() == CalleeTy->isVarArg()` is what then rejects
        // the module.
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => {
                if !callee_ty.is_valid_function_return() {
                    return Err(
                        self.message_at(ret_ty_loc, "Invalid result type for LLVM function")
                    );
                }
                function_type_with_variadic(self.module, callee_ty, arg_tys.clone(), false)
            }
        };
        // `CalleeID.StrVal` survives `convertValIDToValue` upstream because
        // `CalleeID` is still live at the dbg guard below; llvmkit's
        // `ParsedDirectCallee` is consumed by resolution, so the one field that
        // guard reads is taken first.
        let callee_global_name = match &parsed_callee {
            ParsedDirectCallee::Name { name, .. } => Some(name.clone()),
            _ => None,
        };
        // Upstream resolves the callee here — `convertValIDToValue` runs
        // immediately after `CalleeID.FTy = Ty` and *before* the argument loop
        // — so a bad callee is reported ahead of a bad argument.
        let callee = self.resolve_direct_callee(parsed_callee, parsed_fn_ty, call_addr_space)?;
        self.check_call_argument_agreement(parsed_fn_ty, &arg_tys, &arg_locs, call_loc)?;
        // `LLParser::parseCall`'s FMF guard. Upstream builds the `CallInst`
        // first and runs `if (FMF.any()) { if (!isa<FPMathOperator>(CI)) {
        // CI->deleteValue(); return error(CallLoc, …); } }`. llvmkit has no
        // orphan-then-delete — `append_instruction` attaches — so the guard
        // runs before construction instead. Observably identical: nothing
        // between the two points can fail, and this guard still precedes the
        // `llvm.dbg` guard exactly as upstream's does. The anchor is
        // upstream's `CallLoc`, not the current token.
        // `isa<FPMathOperator>(CI)`'s `Call` arm is
        // `FPMathOperator::isSupportedFloatingPointType(V->getType())`, so a
        // homogeneous floating-point aggregate return type is an
        // `FPMathOperator` and may carry the flags.
        if !fmf.is_empty()
            && !llvmkit_ir::is_supported_floating_point_type(parsed_fn_ty.return_type())
        {
            return Err(self.message_at(
                call_loc,
                "fast-math-flags specified for call without floating-point scalar or vector return type",
            ));
        }
        // The old-format half of the debug-info intermix guard. It keys on the
        // callee's *ValID* being a global name, so an indirect call through a
        // pointer that happens to hold `llvm.dbg.value` does not trip it.
        if let Some(name) = &callee_global_name
            && is_old_dbg_format_intrinsic(name)
        {
            if self.seen_new_dbg_info_format {
                return Err(self.message_at(
                    call_loc,
                    "llvm.dbg intrinsic should not appear in a module using non-intrinsic debug info",
                ));
            }
            self.seen_old_dbg_info_format = true;
        }
        // `CallInst::Create(Ty, Callee, Args, BundleList)` — ONE construction
        // site, because `convertValIDToValue` hands `parseCall` one
        // `Value *Callee` and `parseCall` has no direct/indirect fork. The
        // three `ParsedCallee` variants differ only in how the operand is
        // erased, which is `convertValIDToValue`'s switch, not a second
        // instruction shape. `setTailCallKind`, `setCallingConv` and
        // `setAttributes` ride along.
        let call = b
            .call_erased::<llvmkit_ir::Dyn, _, _>(
                parsed_fn_ty,
                callee.as_erased(),
                args,
                tail_kind,
                llvmkit_ir::CallSiteConfig::new(result_name.as_str())
                    .calling_conv(calling_conv)
                    .attrs(call_attrs),
            )
            .map_err(|e| self.builder_err("call", e))?;
        Ok(b.view(call).to_erased())
    }

    /// `LLParser::parseOptionalCallingConv`, whole. An absent convention is
    /// `ccc`, not an error, so this never fails on a token it does not know.
    ///
    /// The keyword table is checked against upstream's switch by
    /// `calling_conv_drift.rs`; the printed spellings live on
    /// [`llvmkit_ir::CallingConv`] and mirror `printCallingConv`.
    fn parse_optional_calling_conv(&mut self) -> ParseResult<CallingConv> {
        let cc = match self.peek() {
            Token::Kw(Keyword::Ccc) => Some(CallingConv::C),
            Token::Kw(Keyword::Fastcc) => Some(CallingConv::FAST),
            Token::Kw(Keyword::Coldcc) => Some(CallingConv::COLD),
            Token::Kw(Keyword::Anyregcc) => Some(CallingConv::ANY_REG),
            Token::Kw(Keyword::PreserveMostcc) => Some(CallingConv::PRESERVE_MOST),
            Token::Kw(Keyword::PreserveAllcc) => Some(CallingConv::PRESERVE_ALL),
            Token::Kw(Keyword::PreserveNonecc) => Some(CallingConv::PRESERVE_NONE),
            Token::Kw(Keyword::Ghccc) => Some(CallingConv::GHC),
            Token::Kw(Keyword::Swiftcc) => Some(CallingConv::SWIFT),
            Token::Kw(Keyword::Swifttailcc) => Some(CallingConv::SWIFT_TAIL),
            Token::Kw(Keyword::CxxFastTlscc) => Some(CallingConv::CXX_FAST_TLS),
            Token::Kw(Keyword::X86Stdcallcc) => Some(CallingConv::X86_STD_CALL),
            Token::Kw(Keyword::X86Fastcallcc) => Some(CallingConv::X86_FAST_CALL),
            Token::Kw(Keyword::X86Thiscallcc) => Some(CallingConv::X86_THIS_CALL),
            Token::Kw(Keyword::X86Vectorcallcc) => Some(CallingConv::X86_VECTOR_CALL),
            Token::Kw(Keyword::X86Regcallcc) => Some(CallingConv::X86_REG_CALL),
            Token::Kw(Keyword::X86Intrcc) => Some(CallingConv::X86_INTR),
            Token::Kw(Keyword::IntelOclBicc) => Some(CallingConv::INTEL_OCL_BI),
            Token::Kw(Keyword::Win64cc) => Some(CallingConv::WIN64),
            Token::Kw(Keyword::X86_64Sysvcc) => Some(CallingConv::X86_64_SYS_V),
            Token::Kw(Keyword::Hhvmcc) => Some(CallingConv::DUMMY_HHVM),
            Token::Kw(Keyword::HhvmCcc) => Some(CallingConv::DUMMY_HHVM_C),
            Token::Kw(Keyword::ArmApcscc) => Some(CallingConv::ARM_APCS),
            Token::Kw(Keyword::ArmAapcscc) => Some(CallingConv::ARM_AAPCS),
            Token::Kw(Keyword::ArmAapcsVfpcc) => Some(CallingConv::ARM_AAPCS_VFP),
            Token::Kw(Keyword::Aarch64VectorPcs) => Some(CallingConv::AARCH64_VECTOR_CALL),
            Token::Kw(Keyword::Aarch64SveVectorPcs) => Some(CallingConv::AARCH64_SVE_VECTOR_CALL),
            Token::Kw(Keyword::Aarch64SmePreservemostFromX0) => {
                Some(CallingConv::AARCH64_SME_PRESERVE_MOST_FROM_X0)
            }
            Token::Kw(Keyword::Aarch64SmePreservemostFromX1) => {
                Some(CallingConv::AARCH64_SME_PRESERVE_MOST_FROM_X1)
            }
            Token::Kw(Keyword::Aarch64SmePreservemostFromX2) => {
                Some(CallingConv::AARCH64_SME_PRESERVE_MOST_FROM_X2)
            }
            Token::Kw(Keyword::Msp430Intrcc) => Some(CallingConv::MSP430_INTR),
            Token::Kw(Keyword::AvrIntrcc) => Some(CallingConv::AVR_INTR),
            Token::Kw(Keyword::AvrSignalcc) => Some(CallingConv::AVR_SIGNAL),
            Token::Kw(Keyword::PtxKernel) => Some(CallingConv::PTX_KERNEL),
            Token::Kw(Keyword::PtxDevice) => Some(CallingConv::PTX_DEVICE),
            Token::Kw(Keyword::SpirKernel) => Some(CallingConv::SPIR_KERNEL),
            Token::Kw(Keyword::SpirFunc) => Some(CallingConv::SPIR_FUNC),
            Token::Kw(Keyword::AmdgpuVs) => Some(CallingConv::AMDGPU_VS),
            Token::Kw(Keyword::AmdgpuGfx) => Some(CallingConv::AMDGPU_GFX),
            Token::Kw(Keyword::AmdgpuLs) => Some(CallingConv::AMDGPU_LS),
            Token::Kw(Keyword::AmdgpuHs) => Some(CallingConv::AMDGPU_HS),
            Token::Kw(Keyword::AmdgpuEs) => Some(CallingConv::AMDGPU_ES),
            Token::Kw(Keyword::AmdgpuGs) => Some(CallingConv::AMDGPU_GS),
            Token::Kw(Keyword::AmdgpuPs) => Some(CallingConv::AMDGPU_PS),
            Token::Kw(Keyword::AmdgpuCs) => Some(CallingConv::AMDGPU_CS),
            Token::Kw(Keyword::AmdgpuCsChain) => Some(CallingConv::AMDGPU_CS_CHAIN),
            Token::Kw(Keyword::AmdgpuCsChainPreserve) => {
                Some(CallingConv::AMDGPU_CS_CHAIN_PRESERVE)
            }
            Token::Kw(Keyword::AmdgpuKernel) => Some(CallingConv::AMDGPU_KERNEL),
            Token::Kw(Keyword::AmdgpuGfxWholeWave) => Some(CallingConv::AMDGPU_GFX_WHOLE_WAVE),
            Token::Kw(Keyword::Tailcc) => Some(CallingConv::TAIL),
            Token::Kw(Keyword::CfguardCheckcc) => Some(CallingConv::CF_GUARD_CHECK),
            Token::Kw(Keyword::M68kRtdcc) => Some(CallingConv::M68K_RTD),
            Token::Kw(Keyword::Graalcc) => Some(CallingConv::GRAAL),
            Token::Kw(Keyword::RiscvVectorCc) => Some(CallingConv::RISCV_VECTOR_CALL),
            Token::Kw(Keyword::CheriotCompartmentcallcc) => {
                Some(CallingConv::CHERIOT_COMPARTMENT_CALL)
            }
            Token::Kw(Keyword::CheriotCompartmentcalleecc) => {
                Some(CallingConv::CHERIOT_COMPARTMENT_CALLEE)
            }
            Token::Kw(Keyword::CheriotLibrarycallcc) => Some(CallingConv::CHERIOT_LIBRARY_CALL),
            Token::Kw(Keyword::RiscvVlsCc) => return self.parse_riscv_vls_calling_conv(),
            Token::Kw(Keyword::Cc) => {
                self.bump()?;
                // `parseUInt32(CC)` and nothing else — upstream validates no
                // range here. `MaxID` (1023) bounds the *bitcode* encoding,
                // and neither the parser nor the Verifier consults it, so
                // `cc 5000` is legal.
                return Ok(CallingConv::from_raw(self.parse_uint32()?));
            }
            _ => None,
        };
        if let Some(cc) = cc {
            self.bump()?;
            Ok(cc)
        } else {
            Ok(CallingConv::C)
        }
    }

    /// `riscv_vls_cc` / `riscv_vls_cc(<ABI_VLEN>)`.
    ///
    /// **This reproduces an upstream bug, deliberately.** The `kw_riscv_vls_cc`
    /// arm consumes its own keyword with `Lex.Lex()` and then, when no `(`
    /// follows, `break`s to the switch's common tail — which calls `Lex.Lex()`
    /// a *second* time. A bare `riscv_vls_cc` therefore swallows the token
    /// after it, so `define riscv_vls_cc void @f()` loses its return type.
    /// Every other arm reaches that tail without having consumed anything.
    ///
    /// It is unreachable from printed IR: `printCallingConv` writes these
    /// twelve conventions only as `riscv_vls_cc(<N>)`, never bare. Reproduced
    /// rather than fixed because the program's contract is upstream's
    /// behaviour, not upstream's intent; recorded in `docs/future-work.md`.
    fn parse_riscv_vls_calling_conv(&mut self) -> ParseResult<CallingConv> {
        self.expect_keyword(Keyword::RiscvVlsCc, "'riscv_vls_cc'")?;
        if !self.eat_punct(PunctKind::LParen)? {
            // The upstream double-`Lex.Lex()` described above.
            self.bump()?;
            return Ok(CallingConv::RISCV_VLS_CALL_128);
        }
        let vlen_loc = self.loc();
        let vlen = self.parse_uint32()?;
        self.expect_punct(PunctKind::RParen, "')'")?;
        let cc = match vlen {
            32 => CallingConv::RISCV_VLS_CALL_32,
            64 => CallingConv::RISCV_VLS_CALL_64,
            128 => CallingConv::RISCV_VLS_CALL_128,
            256 => CallingConv::RISCV_VLS_CALL_256,
            512 => CallingConv::RISCV_VLS_CALL_512,
            1024 => CallingConv::RISCV_VLS_CALL_1024,
            2048 => CallingConv::RISCV_VLS_CALL_2048,
            4096 => CallingConv::RISCV_VLS_CALL_4096,
            8192 => CallingConv::RISCV_VLS_CALL_8192,
            16384 => CallingConv::RISCV_VLS_CALL_16384,
            32768 => CallingConv::RISCV_VLS_CALL_32768,
            65536 => CallingConv::RISCV_VLS_CALL_65536,
            _ => return Err(self.message_at(vlen_loc, "unknown RISC-V ABI VLEN")),
        };
        Ok(cc)
    }

    /// `ValID ::= 'asm' SideEffect? AlignStack? IntelDialect? Unwind?
    /// STRINGCONSTANT ',' STRINGCONSTANT`. Mirrors `LLParser::parseValID`'s
    /// `kw_asm` arm, keyword order included — upstream reads them in a fixed
    /// sequence, so `asm alignstack sideeffect ""` does not parse.
    fn parse_inline_asm(&mut self) -> ParseResult<ParsedInlineAsm> {
        let loc = self.loc();
        self.expect_keyword(Keyword::Asm, "'asm'")?;
        let has_side_effects = self.eat_keyword(Keyword::Sideeffect)?;
        let is_align_stack = self.eat_keyword(Keyword::Alignstack)?;
        let dialect = if self.eat_keyword(Keyword::Inteldialect)? {
            llvmkit_ir::AsmDialect::Intel
        } else {
            llvmkit_ir::AsmDialect::Att
        };
        let can_unwind = self.eat_keyword(Keyword::Unwind)?;
        let asm = self.parse_string_constant("string constant")?;
        self.expect_punct(PunctKind::Comma, "comma in inline asm expression")?;
        let constraints = self.parse_string_constant("constraint string")?;
        Ok(ParsedInlineAsm {
            asm,
            constraints,
            has_side_effects,
            is_align_stack,
            dialect,
            can_unwind,
            loc,
        })
    }

    /// Parse the callee operand of `call` / `invoke` / `callbr`. Global
    /// callees (`@f`, `@42`) and inline asm keep dedicated arms so direct
    /// resolution (forward declarations, intrinsics) still sees names; any
    /// other token parses as a general pointer-typed value (`%fp`, `null`,
    /// `undef`, constants), mirroring `LLParser::parseCall`'s
    /// `parseValID` + `convertValIDToValue(PointerType::get(Context,
    /// CallAddrSpace))` callee handling. `LLParser::parseCallBr` is the one
    /// caller that demands `PointerType::getUnqual(Context)` instead, so
    /// `callee_addr_space` is the call site's address space and `parse_callbr`
    /// passes a literal `0`.
    fn parse_direct_callee_ref(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        callee_addr_space: u32,
    ) -> ParseResult<ParsedDirectCallee<'ctx, B>> {
        let loc = self.loc();
        match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("callee function name"))?;
                self.bump()?;
                Ok(ParsedDirectCallee::Name { name, loc })
            }
            Token::GlobalId(id) => {
                let id = *id;
                self.bump()?;
                Ok(ParsedDirectCallee::Id { id, loc })
            }
            Token::Kw(Keyword::Asm) => Ok(ParsedDirectCallee::InlineAsm(self.parse_inline_asm()?)),
            _ => {
                // `convertValIDToValue(PointerType::get(Context,
                // CallAddrSpace), …)`: the callee is looked up *at* the call
                // site's address space, which is what makes
                // `PerFunctionState::getVal`'s `checkValidVariableType` reject
                // `call i8 %fnptr42(…)` under a zero program address space and
                // accept it under `P42`.
                let ptr_ty = self.module.ptr_type(callee_addr_space).as_type();
                let v = self.parse_value(state, ptr_ty)?;
                Ok(ParsedDirectCallee::Value { v, loc })
            }
        }
    }

    /// Mirrors `convertValIDToValue`'s callee arms: `t_GlobalName` /
    /// `t_GlobalID` go through `LLParser::getGlobalVal`, `t_InlineAsm` builds
    /// an `InlineAsm` and ignores `Ty`, and the local arms have already been
    /// resolved by [`Self::parse_direct_callee_ref`].
    ///
    /// `callee_addr_space` is the address space of the `PointerType` upstream
    /// demands — `PointerType::get(Context, CallAddrSpace)` in `parseCall` /
    /// `parseInvoke`, `PointerType::getUnqual(Context)` in `parseCallBr`.
    fn resolve_direct_callee(
        &mut self,
        parsed: ParsedDirectCallee<'ctx, B>,
        parsed_fn_ty: llvmkit_ir::FunctionType<'ctx, B>,
        callee_addr_space: u32,
    ) -> ParseResult<ParsedCallee<'ctx, B>> {
        match parsed {
            ParsedDirectCallee::Name { name, loc } => {
                // `getGlobalVal(Name, Ty, Loc)` is **one** lookup, in
                // `M->getValueSymbolTable()`, and it accepts any
                // `GlobalValue` — function, global variable, alias or ifunc.
                // A symbol-table (or forward-ref-table) hit goes through
                // `checkValidVariableType(Loc, "@" + Name, Ty, Val)` before
                // anything else looks at it. `Val->getType()` is
                // `GlobalValue::getType` — `PointerType::get(C,
                // GV->getAddressSpace())` — which llvmkit rebuilds from the
                // symbol's own address space, because a global's arena type
                // here is its *value* type (`docs/divergences.md` D3); that
                // hoist is `check_resolved_global_type`, shared with
                // `resolve_global_name_as_value`, llvmkit's port of the same
                // routine for an ordinary operand.
                let ptr_ty = self.module.ptr_type(callee_addr_space).as_type();
                if let Some(resolved) = self.global_symbol_lookup(&name) {
                    self.check_resolved_global_type(loc, &format!("@{name}"), ptr_ty, resolved)?;
                    let GlobalRef::Function(f) = resolved else {
                        // A non-function `GlobalValue` callee stays the bare
                        // pointer `getGlobalVal` handed back: the call's own
                        // `FunctionType` lives on the `CallBase`, not on the
                        // callee, so nothing downstream needs a `Function`.
                        return Ok(ParsedCallee::Indirect(
                            self.global_ref_as_pointer(loc, resolved)?,
                        ));
                    };
                    match resolve_intrinsic_name(&name) {
                        // A non-intrinsic direct callee resolves to the
                        // function regardless of whether the call-site type
                        // matches the declaration: upstream `parseCall`
                        // looks the callee up as a bare pointer and the call
                        // carries its own `FunctionType` (`CallBase`), which
                        // the build site applies via `call_site_type`. The
                        // verifier — not the parser — owns the eventual
                        // call-vs-declaration check.
                        IntrinsicNameResolution::NonIntrinsic => {}
                        IntrinsicNameResolution::UnknownIntrinsic => {
                            return Err(ParseError::Expected {
                                expected: "unknown intrinsic".into(),
                                loc: DiagLoc::span(loc),
                            });
                        }
                        IntrinsicNameResolution::Known(_) => {
                            // Intrinsics are the exception: a call whose type
                            // disagrees with the intrinsic declaration is
                            // invalid IR upstream too — `getCalledFunction`
                            // returns null on the mismatch and
                            // `Verifier::visitFunction` reports "Invalid user
                            // of intrinsic instruction" — so rejecting it at
                            // parse reaches the same verdict.
                            if f.signature() != parsed_fn_ty {
                                return Err(ParseError::Expected {
                                    expected: "intrinsic signature mismatch".into(),
                                    loc: DiagLoc::span(loc),
                                });
                            }
                            let descriptor = self
                                .module
                                .intrinsic_descriptor_from_signature(&name, parsed_fn_ty)
                                .map_err(|e| self.intrinsic_parse_error(loc, e))?;
                            if f.intrinsic_descriptor() != Some(descriptor) {
                                return Err(ParseError::Expected {
                                    expected: "intrinsic signature mismatch".into(),
                                    loc: DiagLoc::span(loc),
                                });
                            }
                        }
                    }
                    return Ok(ParsedCallee::Function(f));
                }
                match resolve_intrinsic_name(&name) {
                    IntrinsicNameResolution::Known(_) => {
                        let descriptor = self
                            .module
                            .intrinsic_descriptor_from_signature(&name, parsed_fn_ty)
                            .map_err(|e| self.intrinsic_parse_error(loc, e))?;
                        let f = self
                            .module
                            .get_or_insert_intrinsic_declaration(&descriptor)
                            .map_err(|e| self.intrinsic_parse_error(loc, e))?;
                        let f = self.module.view(f);
                        // See the non-intrinsic miss arm below:
                        // `createGlobalFwdRef(M, PTy)` mints the placeholder at
                        // the *demanded* pointer type's address space.
                        f.set_address_space(self.module, callee_addr_space);
                        Ok(ParsedCallee::Function(f))
                    }
                    IntrinsicNameResolution::UnknownIntrinsic => Err(ParseError::Expected {
                        expected: "unknown intrinsic".into(),
                        loc: DiagLoc::span(loc),
                    }),
                    IntrinsicNameResolution::NonIntrinsic => {
                        // `getGlobalVal`'s miss path, reached through the very
                        // same `global_forward_ref` an ordinary `@`-operand
                        // takes: `createGlobalFwdRef(M, PTy)` mints an
                        // **untyped** stand-in — an `i8` `GlobalVariable` with
                        // `ExternalWeakLinkage` whose only meaningful property
                        // is `PTy->getAddressSpace()` — records it in
                        // `ForwardRefVals`, and hands it back as a bare `ptr`.
                        //
                        // Nothing about the callee's eventual signature is
                        // decided here: the call carries its own `FunctionType`
                        // on the `CallBase`, and `parseFunctionHeader` RAUWs the
                        // placeholder with the real `Function` when the
                        // `declare` / `define` arrives. llvmkit used to mint a
                        // real `Function` at the *call site's* signature
                        // instead, which no later definition could re-type.
                        //
                        // The placeholder is minted at the *demanded* pointer
                        // type's address space, so a later reference at a
                        // different one mismatches. Under `target datalayout =
                        // "P42"` a `call void @f()` with no `addrspace` keyword
                        // therefore forward-references `@f` at 42, not at 0.
                        let placeholder =
                            self.global_forward_ref(Some(&name), None, ptr_ty, loc)?;
                        Ok(ParsedCallee::Indirect(
                            self.constant_as_pointer(loc, placeholder)?,
                        ))
                    }
                }
            }
            ParsedDirectCallee::Id { id, loc } => {
                // `getGlobalVal(unsigned ID, Ty, Loc)` reads `NumberedVals`,
                // which holds every `GlobalValue` kind, exactly as the named
                // overload reads the symbol table.
                let ptr_ty = self.module.ptr_type(callee_addr_space).as_type();
                let Some(resolved) = self.numbered_globals.get(id).copied() else {
                    // …and, exactly as in the named overload, a miss is not an
                    // error: `ForwardRefValIDs` gets a `createGlobalFwdRef`
                    // placeholder that `parseFunctionHeader` RAUWs at the
                    // definition. llvmkit used to answer `use of undefined
                    // value` outright, so `call void @0()` above `define void
                    // @0()` was rejected.
                    let placeholder = self.global_forward_ref(None, Some(id), ptr_ty, loc)?;
                    return Ok(ParsedCallee::Indirect(
                        self.constant_as_pointer(loc, placeholder)?,
                    ));
                };
                // `checkValidVariableType(Loc, "@" + Twine(ID), Ty, Val)`. See
                // the named arm above for why this reduces to an address-space
                // comparison.
                self.check_resolved_global_type(loc, &format!("@{id}"), ptr_ty, resolved)?;
                match resolved {
                    GlobalRef::Function(f) => Ok(ParsedCallee::Function(f)),
                    other => Ok(ParsedCallee::Indirect(
                        self.global_ref_as_pointer(loc, other)?,
                    )),
                }
            }
            ParsedDirectCallee::InlineAsm(data) => Ok(ParsedCallee::InlineAsm({
                // `convertValIDToValue`'s `t_InlineAsm` arm verifies before it
                // constructs, and prints `InlineAsm::verify`'s message as-is.
                llvmkit_ir::verify_inline_asm(parsed_fn_ty, &data.constraints).map_err(|e| {
                    ParseError::Message {
                        message: e.to_string().into(),
                        loc: DiagLoc::span(data.loc),
                    }
                })?;
                self.module
                    .inline_asm(parsed_fn_ty, data.asm, data.constraints, {
                        let mut options =
                            llvmkit_ir::InlineAsmOptions::new().with_dialect(data.dialect);
                        if data.has_side_effects {
                            options = options.side_effects();
                        }
                        if data.is_align_stack {
                            options = options.align_stack();
                        }
                        if data.can_unwind {
                            options = options.unwind();
                        }
                        options
                    })
            })),
            ParsedDirectCallee::Value { v, loc } => {
                // Mirrors `PerFunctionState::getVal`'s type check: whatever
                // value form the callee took, it must be pointer-typed.
                let callee =
                    llvmkit_ir::PointerValue::try_from(v).map_err(|e| ParseError::Expected {
                        expected: format!("pointer callee: {e}").into(),
                        loc: DiagLoc::span(loc),
                    })?;
                Ok(ParsedCallee::Indirect(callee))
            }
        }
    }

    /// `va_arg <list-ptr>, <ty>`. Mirrors `LLParser::parseVA_Arg`.
    ///
    /// Upstream: `test/Assembler/vaarg.ll`.
    fn parse_vaarg(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let list_ty = self.parse_type(false)?;
        let list_v = self.parse_value(state, list_ty)?;
        let list_ptr: llvmkit_ir::PointerValue<'ctx, B> = list_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed va_arg list operand"))?;
        self.expect_punct(PunctKind::Comma, "',' after vaarg operand")?;
        let result_type_loc = self.loc();
        let result_ty = self.parse_type(false)?;
        if !result_ty.is_first_class() {
            return Err(self.message_at(
                result_type_loc,
                "va_arg requires operand with first class type",
            ));
        }
        let v = b
            .va_arg(list_ptr, result_ty, result_name.as_str())
            .map_err(|e| self.builder_err("va_arg", e))?;
        Ok(b.view(v).to_erased())
    }

    /// `freeze <ty> <val>`. Mirrors `LLParser::parseFreeze`.
    ///
    /// Upstream: `test/Assembler/freeze.ll`.
    fn parse_freeze(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let r = b
            .freeze(v, result_name.as_str())
            .map_err(|e| self.builder_err("freeze", e))?;
        Ok(b.view(r).to_erased())
    }

    /// `switch <ty> <val>, label %default [ <ty> N, label %case ... ]`.
    /// Mirrors `LLParser::parseSwitch` (LLParser.cpp ~7640).
    ///
    /// Upstream: `test/Assembler/switch.ll`.
    fn parse_switch(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `switch`
        let cond_loc = self.loc();
        let (cond_ty, cond_v) = self.parse_type_and_value(state)?;
        self.expect_punct(PunctKind::Comma, "',' after switch condition")?;
        let default_bb = self.parse_type_and_basic_block(state)?;
        // Case list: `[ ty N, label %bb, ... ]`. Upstream demands the `[`
        // *before* checking the condition's type, so a malformed table is
        // reported ahead of a bad condition.
        self.expect_punct(PunctKind::LSquare, "'[' with switch table")?;
        if !cond_ty.is_integer() {
            return Err(self.message_at(cond_loc, "switch condition must have integer type"));
        }
        let (_, mut sw) = b
            .switch_dyn(cond_v, default_bb, "")
            .map_err(|e| self.builder_err("switch", e))?;
        // `SmallPtrSet<Value*, 32> SeenCases` — duplicate *values*, compared
        // by identity, which for uniqued constants is equality.
        let mut seen_cases: Vec<llvmkit_ir::Value<'ctx, B>> = Vec::new();
        loop {
            if matches!(self.peek(), Token::RSquare) {
                self.bump()?;
                break;
            }
            // `parseTypeAndValue(Constant, CondLoc, PFS) ||
            //  parseToken(lltok::comma, "expected ',' after case value") ||
            //  parseTypeAndBasicBlock(DestBB, PFS)` — the whole pair is read
            // before either case-value rule is applied, so a malformed
            // destination is reported ahead of a duplicate or non-constant
            // case value. `CondLoc` is re-taken by `parseTypeAndValue`, which
            // is what anchors both rules at the case value rather than at the
            // condition.
            let case_loc = self.loc();
            let (_, case_v) = self.parse_type_and_value(state)?;
            self.expect_punct(PunctKind::Comma, "',' after case value")?;
            let case_bb = self.parse_type_and_basic_block(state)?;
            if seen_cases.contains(&case_v) {
                return Err(self.message_at(case_loc, "duplicate case value in switch"));
            }
            seen_cases.push(case_v);
            // `!isa<ConstantInt>(Constant)` — an *integer* is not enough; an
            // `i32 %arg` is an integer value and still not a case value.
            // Converting straight to `IntValue` would accept one.
            if llvmkit_ir::Constant::try_from(case_v).is_err() {
                return Err(self.message_at(case_loc, "case value is not a constant integer"));
            }
            let case_int: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = case_v
                .try_into()
                .map_err(|_| self.message_at(case_loc, "case value is not a constant integer"))?;
            sw = sw
                .add_case(case_int, case_bb)
                .map_err(|e| self.builder_err("switch.add_case", e))?;
        }
        let _ = sw.finish();
        Ok(())
    }

    /// `indirectbr <ptr-ty> <addr>, [ label %dest1, ... ]`.
    /// Mirrors `LLParser::parseIndirectBr` (LLParser.cpp ~7685).
    ///
    /// Upstream: `test/Assembler/indirectbr.ll`.
    fn parse_indirectbr(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `indirectbr`
        let addr_loc = self.loc();
        let (_, addr_v) = self.parse_type_and_value(state)?;
        self.expect_punct(PunctKind::Comma, "',' after indirectbr address")?;
        // As in `parseSwitch`, the `[` is demanded before the address type is
        // checked.
        self.expect_punct(PunctKind::LSquare, "'[' with indirectbr")?;
        let addr: PointerValue<'ctx, B> = addr_v
            .try_into()
            .map_err(|_| self.message_at(addr_loc, "indirectbr address must have pointer type"))?;
        let (_, mut ibr) = b
            .indirectbr(addr, "")
            .map_err(|e| self.builder_err("indirectbr", e))?;
        // `if (Lex.getKind() != lltok::rsquare) { parseTypeAndBasicBlock(…);
        //  while (EatIfPresent(lltok::comma)) parseTypeAndBasicBlock(…); }`
        // — the first iteration is unrolled, and the shape is observable: a
        // trailing comma runs the loop body against the `]`, and a missing
        // comma falls out of the loop into the `]` demand below. A single
        // `while (peek != ']')` loop accepted `[label %a,]` and
        // `[label %a label %b]`.
        if !matches!(self.peek(), Token::RSquare) {
            let dest_bb = self.parse_type_and_basic_block(state)?;
            ibr = ibr
                .add_destination(dest_bb)
                .map_err(|e| self.builder_err("indirectbr.add_destination", e))?;
            while self.eat_punct(PunctKind::Comma)? {
                let dest_bb = self.parse_type_and_basic_block(state)?;
                ibr = ibr
                    .add_destination(dest_bb)
                    .map_err(|e| self.builder_err("indirectbr.add_destination", e))?;
            }
        }
        self.expect_punct(PunctKind::RSquare, "']' at end of block list")?;
        let _ = ibr.finish();
        Ok(())
    }

    /// `fence [syncscope("...")] <ordering>`. Void instruction.
    /// Mirrors `LLParser::parseFence` (LLParser.cpp ~8476).
    ///
    /// Upstream: `test/Assembler/fence.ll`.
    fn parse_fence(&mut self, b: &ParsedBlockBuilder<'ctx, 'ctx, B>) -> ParseResult<()> {
        let sync_scope = self.parse_optional_syncscope()?;
        let ordering = self.parse_atomic_ordering()?;
        // `parseFence`'s two rules, both `tokError` so both anchor at the
        // token after the ordering. llvmkit had neither: the orderings
        // reached the builder.
        if ordering == AtomicOrdering::Unordered {
            return Err(self.message("fence cannot be unordered"));
        }
        if ordering == AtomicOrdering::Monotonic {
            return Err(self.message("fence cannot be monotonic"));
        }
        let _ = b
            .fence(ordering, sync_scope, "")
            .map_err(|e| self.builder_err("fence", e))?;
        Ok(())
    }

    /// `cmpxchg [weak] [volatile] ptr <ptr>, <ty> <cmp>, <ty> <new>
    ///         [syncscope("...")] <success-ord> <fail-ord> [, align N]`.
    /// Returns `{ ty, i1 }`. Mirrors `LLParser::parseAtomicCmpXchg`.
    ///
    /// Upstream: `test/Assembler/cmpxchg.ll`.
    fn parse_cmpxchg(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let weak = self.eat_keyword(Keyword::Weak)?;
        let volatile = self.eat_keyword(Keyword::Volatile)?;
        let ptr_loc = self.loc();
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after cmpxchg address")?;
        let cmp_ty = self.parse_type(false)?;
        let cmp_v = self.parse_value(state, cmp_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after cmpxchg cmp operand")?;
        let new_loc = self.loc();
        let new_ty = self.parse_type(false)?;
        let new_v = self.parse_value(state, new_ty)?;
        let sync_scope = self.parse_optional_syncscope()?;
        let success_ord = self.parse_atomic_ordering()?;
        let failure_ord = self.parse_atomic_ordering()?;
        let align = self.parse_optional_comma_align_strict()?;

        // `parseCmpXchg`'s five checks, in its order. The two ordering rules
        // are `tokError` and come *first*, before the operand types are even
        // looked at; llvmkit reached none of them, since every rejection came
        // from the builder.
        if !cmpxchg_success_ordering_is_valid(success_ord) {
            return Err(self.message("invalid cmpxchg success ordering"));
        }
        if !cmpxchg_failure_ordering_is_valid(failure_ord) {
            return Err(self.message("invalid cmpxchg failure ordering"));
        }
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v
            .try_into()
            .map_err(|_| self.message_at(ptr_loc, "cmpxchg operand must be a pointer"))?;
        if cmp_ty != new_ty {
            return Err(self.message_at(new_loc, "compare value and new value type do not match"));
        }
        if !new_ty.is_first_class() {
            return Err(self.message_at(new_loc, "cmpxchg operand must be a first class value"));
        }
        let align = match align {
            Some(value) => llvmkit_ir::align::MaybeAlign::from(value),
            None => llvmkit_ir::align::MaybeAlign::NONE,
        };
        let mut config =
            llvmkit_ir::instr_types::AtomicCmpXchgConfig::new(success_ord, failure_ord, sync_scope)
                .align(align);
        if weak {
            config = config.weak();
        }
        if volatile {
            config = config.volatile();
        }
        let v = b
            .atomic_cmpxchg(ptr, cmp_v, new_v, config, result_name.as_str())
            .map_err(|e| self.builder_err("cmpxchg", e))?;
        Ok(b.view(v).to_erased())
    }

    /// `atomicrmw [volatile] <op> ptr <ptr>, <ty> <val>
    ///           [syncscope("...")] <ordering> [, align N]`.
    /// Returns the old value. Mirrors `LLParser::parseAtomicRMW`.
    ///
    /// Upstream: `test/Assembler/atomicrmw.ll`.
    fn parse_atomicrmw(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let volatile = self.eat_keyword(Keyword::Volatile)?;
        let op = self.parse_atomicrmw_op()?;
        let ptr_loc = self.loc();
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after atomicrmw address")?;
        let val_loc = self.loc();
        let val_ty = self.parse_type(false)?;
        let val_v = self.parse_value(state, val_ty)?;
        let sync_scope = self.parse_optional_syncscope()?;
        let ordering = self.parse_atomic_ordering()?;
        let align = self.parse_optional_comma_align_strict()?;

        // `parseAtomicRMW`'s checks, in its order. llvmkit had none of them.
        if ordering == AtomicOrdering::Unordered {
            return Err(self.message("atomicrmw cannot be unordered"));
        }
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v
            .try_into()
            .map_err(|_| self.message_at(ptr_loc, "atomicrmw operand must be a pointer"))?;
        if val_ty.is_scalable() {
            return Err(self.message_at(val_loc, "atomicrmw operand may not be scalable"));
        }
        // The operand rule is three-way, and the operation's own name is part
        // of every message — `AtomicRMWInst::getOperationName`, which is the
        // same spelling the AsmWriter prints.
        if op == AtomicRmwBinOp::Xchg {
            if !val_ty.is_integer() && !val_ty.is_floating_point() && !val_ty.is_pointer() {
                return Err(self.message_at(
                    val_loc,
                    format!(
                        "atomicrmw {op} operand must be an integer, floating point, or pointer type"
                    ),
                ));
            }
        } else if atomicrmw_op_is_floating_point(op) {
            if !is_fp_or_fp_vector_type(val_ty) {
                return Err(self.message_at(
                    val_loc,
                    format!("atomicrmw {op} operand must be a floating point type"),
                ));
            }
        } else if !val_ty.is_integer() {
            return Err(self.message_at(
                val_loc,
                format!("atomicrmw {op} operand must be an integer"),
            ));
        }
        let size = self.module.data_layout().type_store_size_in_bits(val_ty);
        if size < 8 || !size.is_power_of_two() {
            return Err(self.message_at(
                val_loc,
                "atomicrmw operand must be power-of-two byte-sized integer",
            ));
        }
        let align = match align {
            Some(value) => llvmkit_ir::align::MaybeAlign::from(value),
            None => llvmkit_ir::align::MaybeAlign::NONE,
        };
        let mut config =
            llvmkit_ir::instr_types::AtomicRmwConfig::new(ordering, sync_scope).align(align);
        if volatile {
            config = config.volatile();
        }
        let v = b
            .atomicrmw(op, ptr, val_v, config, result_name.as_str())
            .map_err(|e| self.builder_err("atomicrmw", e))?;
        Ok(b.view(v).to_erased())
    }

    /// Parse an `atomicrmw` operation keyword.
    fn parse_atomicrmw_op(&mut self) -> ParseResult<AtomicRmwBinOp> {
        use AtomicRmwBinOp as Op;
        let op = match self.peek() {
            Token::Kw(Keyword::Xchg) => Op::Xchg,
            Token::Instruction(Opcode::Add) => Op::Add,
            Token::Instruction(Opcode::Sub) => Op::Sub,
            Token::Instruction(Opcode::And) => Op::And,
            Token::Kw(Keyword::Nand) => Op::Nand,
            Token::Instruction(Opcode::Or) => Op::Or,
            Token::Instruction(Opcode::Xor) => Op::Xor,
            Token::Kw(Keyword::Max) => Op::Max,
            Token::Kw(Keyword::Min) => Op::Min,
            Token::Kw(Keyword::Umax) => Op::Umax,
            Token::Kw(Keyword::Umin) => Op::Umin,
            Token::Instruction(Opcode::Fadd) => Op::Fadd,
            Token::Instruction(Opcode::Fsub) => Op::Fsub,
            Token::Kw(Keyword::Fmax) => Op::Fmax,
            Token::Kw(Keyword::Fmin) => Op::Fmin,
            Token::Kw(Keyword::Fmaximum) => Op::Fmaximum,
            Token::Kw(Keyword::Fminimum) => Op::Fminimum,
            Token::Kw(Keyword::UincWrap) => Op::UincWrap,
            Token::Kw(Keyword::UdecWrap) => Op::UdecWrap,
            Token::Kw(Keyword::UsubCond) => Op::UsubCond,
            Token::Kw(Keyword::UsubSat) => Op::UsubSat,
            _ => return Err(self.message("expected binary operation in atomicrmw")),
        };
        self.bump()?;
        Ok(op)
    }

    // ── S3.3: EH/funclet opcodes ──────────────────────────────────────────

    /// `landingpad <type> [cleanup] [catch/filter ...]`.
    /// Non-terminator. Mirrors `LLParser::parseLandingPad` (LLParser.cpp ~7820).
    ///
    /// Upstream: `test/Assembler/landingpad.ll`.
    fn parse_landingpad(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let result_ty = self.parse_type(false)?;
        let cleanup = self.eat_keyword(Keyword::Cleanup)?;
        let mut lp = b
            .landingpad(result_ty, cleanup, result_name.as_str())
            .map_err(|e| self.builder_err("landingpad", e))?;
        // Parse clauses: `catch <ty> <val>` | `filter <array-ty> <val>`
        loop {
            match self.peek() {
                Token::Kw(Keyword::Catch) => {
                    self.bump()?;
                    let clause_loc = self.loc();
                    let clause_ty = self.parse_type(false)?;
                    let clause_v = self.parse_value(state, clause_ty)?;
                    // "A 'catch' type expects a non-array constant. A filter
                    // clause expects an array constant." — upstream's comment.
                    if clause_ty.is_array() {
                        return Err(
                            self.message_at(clause_loc, "'catch' clause has an invalid type")
                        );
                    }
                    if llvmkit_ir::Constant::try_from(clause_v).is_err() {
                        return Err(
                            self.message_at(clause_loc, "clause argument must be a constant")
                        );
                    }
                    lp = lp
                        .add_catch_clause(clause_v)
                        .map_err(|e| self.builder_err("landingpad.catch", e))?;
                }
                Token::Kw(Keyword::Filter) => {
                    self.bump()?;
                    let clause_loc = self.loc();
                    let filter_ty = self.parse_type(false)?;
                    let filter_v = self.parse_value(state, filter_ty)?;
                    if !filter_ty.is_array() {
                        return Err(
                            self.message_at(clause_loc, "'filter' clause has an invalid type")
                        );
                    }
                    if llvmkit_ir::Constant::try_from(filter_v).is_err() {
                        return Err(
                            self.message_at(clause_loc, "clause argument must be a constant")
                        );
                    }
                    lp = lp
                        .add_filter_clause(filter_v)
                        .map_err(|e| self.builder_err("landingpad.filter", e))?;
                }
                _ => break,
            }
        }
        Ok(lp.finish().to_erased())
    }

    /// `cleanuppad within <token-or-none> [<args>]`. Non-terminator.
    /// Mirrors `LLParser::parseCleanupPad`.
    ///
    /// Upstream: `test/Bitcode/compatibility.ll` `@instructions.win_eh.2`.
    fn parse_cleanuppad(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.expect_keyword(Keyword::Within, "'within' after cleanuppad")?;
        // `parseCleanupPad` checks the scope token itself before reading a
        // value, so anything that is not `none` or a local gets its own
        // message rather than a generic value error.
        self.check_pad_scope_token("cleanuppad")?;
        let parent_pad = self.parse_optional_pad_token(state)?;
        let args = self.parse_bracket_value_list(state)?;
        let v = match parent_pad {
            Some(parent) => b.cleanup_pad(parent, args, result_name.as_str()),
            None => b.cleanup_pad_within_none(args, result_name.as_str()),
        }
        .map_err(|e| self.builder_err("cleanuppad", e))?;
        Ok(v.to_erased())
    }

    /// `catchpad within <catchswitch> [<args>]`. Non-terminator.
    /// Mirrors `LLParser::parseCatchPad`.
    ///
    /// Upstream: `test/Bitcode/compatibility.ll` `@instructions.win_eh.1`.
    fn parse_catchpad(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.expect_keyword(Keyword::Within, "'within' after catchpad")?;
        self.check_catchpad_scope_token()?;
        // `parseValue(Type::getTokenTy(Context), CatchSwitch, PFS)` — implied
        // type, no type token in the syntax.
        let token_ty = self.module.token_type().as_type();
        let parent_v = self.parse_value(state, token_ty)?;
        let args = self.parse_bracket_value_list(state)?;
        let v = b
            .catch_pad(parent_v, args, result_name.as_str())
            .map_err(|e| self.builder_err("catchpad", e))?;
        Ok(v.to_erased())
    }

    /// `resume <ty> <val>`. Terminator.
    /// Mirrors `LLParser::parseResume`.
    ///
    /// Upstream: `test/Verifier/resume.ll`.
    fn parse_resume(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let _ = b.resume(v, "").map_err(|e| self.builder_err("resume", e))?;
        Ok(())
    }

    /// `cleanupret from Value unwind ('to' 'caller' | TypeAndValue)`.
    /// Terminator. Mirrors `LLParser::parseCleanupRet`.
    ///
    /// Upstream: `test/Bitcode/compatibility.ll` `@instructions.win_eh.2`.
    fn parse_cleanupret(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        self.expect_keyword(Keyword::From, "'from' after cleanupret")?;
        // `parseValue(Type::getTokenTy(Context), CleanupPad, PFS)` — the pad
        // operand carries no type token, so reading one consumed `%clean` as a
        // named type.
        let token_ty = self.module.token_type().as_type();
        let pad_v = self.parse_value(state, token_ty)?;
        // `parseToken(lltok::kw_unwind, "expected 'unwind' in cleanupret")` —
        // mandatory upstream, and `unwind to caller` is the only way to spell
        // the absent destination.
        self.expect_keyword(Keyword::Unwind, "'unwind' in cleanupret")?;
        let unwind_dest = if self.eat_keyword(Keyword::To)? {
            self.expect_keyword(Keyword::Caller, "'caller' in cleanupret")?;
            None
        } else {
            Some(self.parse_type_and_basic_block(state)?)
        };
        let _ = match unwind_dest {
            Some(dest) => b.cleanup_ret(pad_v, dest, ""),
            None => b.cleanup_ret_to_caller(pad_v, ""),
        }
        .map_err(|e| self.builder_err("cleanupret", e))?;
        Ok(())
    }

    /// `catchret from Parent Value 'to' TypeAndValue`. Terminator.
    /// Mirrors `LLParser::parseCatchRet`.
    ///
    /// Upstream: `test/Bitcode/compatibility.ll` `@instructions.win_eh.2`.
    fn parse_catchret(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        self.expect_keyword(Keyword::From, "'from' after catchret")?;
        // `parseValue(Type::getTokenTy(Context), CatchPad, PFS)` — implied type.
        let token_ty = self.module.token_type().as_type();
        let pad_v = self.parse_value(state, token_ty)?;
        self.expect_keyword(Keyword::To, "'to' in catchret")?;
        let dest = self.parse_type_and_basic_block(state)?;
        let _ = b
            .catch_ret(pad_v, dest, "")
            .map_err(|e| self.builder_err("catchret", e))?;
        Ok(())
    }

    /// `catchswitch within Parent [<handlers>] unwind ('to' 'caller' | TypeAndValue)`.
    /// Terminator. Returns the catchswitch value.
    /// Mirrors `LLParser::parseCatchSwitch`.
    ///
    /// Upstream: `test/Bitcode/compatibility.ll` `@instructions.win_eh.1`.
    fn parse_catchswitch(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.bump()?; // eat `catchswitch`
        self.expect_keyword(Keyword::Within, "'within' after catchswitch")?;
        self.check_pad_scope_token("catchswitch")?;
        let parent_pad = self.parse_optional_pad_token(state)?;
        self.expect_punct(PunctKind::LSquare, "'[' with catchswitch labels")?;
        // `do { parseTypeAndBasicBlock } while (EatIfPresent(lltok::comma));`
        // — at least one handler is required, and the `]` is consumed by the
        // `parseToken` *after* the loop, so a trailing comma is rejected.
        let mut handlers: Vec<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> = Vec::new();
        loop {
            let bb = self.parse_type_and_basic_block(state)?;
            handlers.push(bb);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }
        self.expect_punct(PunctKind::RSquare, "']' after catchswitch labels")?;
        self.expect_keyword(Keyword::Unwind, "'unwind' after catchswitch scope")?;
        let unwind_dest = if self.eat_keyword(Keyword::To)? {
            self.expect_keyword(Keyword::Caller, "'caller' in catchswitch")?;
            None
        } else {
            Some(self.parse_type_and_basic_block(state)?)
        };
        let name = result_name.as_str();
        let (_, mut cs) = match (parent_pad, unwind_dest) {
            (Some(parent), Some(dest)) => b.catch_switch(parent, dest, name),
            (Some(parent), None) => b.catch_switch_to_caller(parent, name),
            (None, Some(dest)) => b.catch_switch_within_none(dest, name),
            (None, None) => b.catch_switch_within_none_to_caller(name),
        }
        .map_err(|e| self.builder_err("catchswitch", e))?;
        for h in handlers {
            cs = cs
                .add_handler(h)
                .map_err(|e| self.builder_err("catchswitch.add_handler", e))?;
        }
        Ok(cs.finish().to_erased())
    }

    /// `invoke [cc] [ret-attrs] <ret-ty> @func(<args>) to label %normal
    ///        unwind label %unwind`. Terminator.
    /// Mirrors `LLParser::parseInvoke`.
    ///
    /// Upstream: `test/Bitcode/compatibility.ll` `@instructions.terminators`
    /// (`invoke fastcc void @f.fastcc() to label %defaultdest unwind label
    /// %exc`). There is no `test/Assembler/invoke.ll` in LLVM 22.1.4.
    fn parse_invoke(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<Option<llvmkit_ir::Value<'ctx, B>>> {
        // The dispatch has already eaten the `invoke` keyword, as
        // `parseInstruction`'s `Lex.Lex()` does ahead of its switch.
        let call_loc = self.loc();
        let calling_conv = self.parse_optional_calling_conv()?;
        let return_attrs = self.parse_optional_return_attrs()?;
        // `LLParser::parseInvoke` carries its own `parseOptionalProgramAddrSpace`
        // (upstream's `InvokeAddrSpace`) in the same slot `parseCall` does:
        // between the return attributes and the callee type.
        let invoke_addr_space = self.parse_optional_program_addr_space()?;
        let ret_ty_loc = self.loc();
        let callee_ty = self.parse_type(true)?;
        let parsed_callee = self.parse_direct_callee_ref(state, invoke_addr_space)?;
        self.expect_punct(PunctKind::LParen, "'(' in invoke argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx, B>> = Vec::new();
        let mut arg_tys: Vec<Type<'ctx, B>> = Vec::new();
        let mut arg_locs: Vec<Span> = Vec::new();
        let mut arg_attrs = Vec::new();
        // invoke can never be varargs-forwarding (only musttail calls are).
        let var_args = false;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    // An invoke can never be musttail, so a forwarding
                    // ellipsis is always invalid here (`parseParameterList`
                    // is called with `IsMustTailCall = false`).
                    return Err(
                        self.expected("unexpected ellipsis in argument list for non-musttail call")
                    );
                }
                let arg_loc = self.loc();
                let arg_ty = self.parse_type(false)?;
                let one_arg_attrs = self.parse_optional_param_attrs()?;
                // `parseParameterList` branches on the argument type: a
                // `metadata` parameter takes `parseMetadataAsValue`, not
                // `parseValue`, which is what makes `metadata i32 %a` — every
                // old-format debug intrinsic operand — legal.
                let arg_v = if arg_ty.is_metadata() {
                    self.parse_metadata_as_value(state)?
                } else {
                    self.parse_value(state, arg_ty)?
                };
                arg_attrs.push(one_arg_attrs);
                arg_tys.push(arg_ty);
                arg_locs.push(arg_loc);
                args.push(arg_v);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close invoke argument list")?;
        let (function_attrs, function_attr_groups) = self.parse_optional_fn_attrs()?;
        let operand_bundles = self.parse_optional_operand_bundles(state)?;
        let call_attrs = llvmkit_ir::instr_types::CallAttributeData::new(
            return_attrs,
            arg_attrs.into_boxed_slice(),
            function_attrs,
        )
        .function_attr_groups(function_attr_groups.into_boxed_slice())
        .operand_bundles(operand_bundles);
        self.expect_keyword(Keyword::To, "'to' in invoke")?;
        let normal_bb = self.parse_type_and_basic_block(state)?;
        self.expect_keyword(Keyword::Unwind, "'unwind' in invoke")?;
        let unwind_bb = self.parse_type_and_basic_block(state)?;
        // Upstream `resolveFunctionType`: an explicitly written function
        // type IS the call-site type; otherwise infer from the arguments.
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => {
                if !callee_ty.is_valid_function_return() {
                    return Err(
                        self.message_at(ret_ty_loc, "Invalid result type for LLVM function")
                    );
                }
                function_type_with_variadic(self.module, callee_ty, arg_tys.clone(), var_args)
            }
        };
        // `parseInvoke` resolves the callee before the argument loop, exactly
        // as `parseCall` does.
        let callee = self.resolve_direct_callee(parsed_callee, parsed_fn_ty, invoke_addr_space)?;
        self.check_call_argument_agreement(parsed_fn_ty, &arg_tys, &arg_locs, call_loc)?;
        let name = result_name.as_str();
        let (_, inst) = match callee {
            ParsedCallee::Function(callee) => b
                .invoke_dyn_with_config(
                    callee,
                    args,
                    normal_bb,
                    unwind_bb,
                    llvmkit_ir::CallSiteConfig::new(name)
                        .calling_conv(calling_conv)
                        .attrs(call_attrs)
                        .call_site_type(parsed_fn_ty),
                )
                .map_err(|e| self.builder_err("invoke", e))?,
            ParsedCallee::InlineAsm(asm) => b
                .inline_asm_invoke_with_config::<llvmkit_ir::Dyn, _, _, _, _>(
                    asm,
                    args,
                    normal_bb,
                    unwind_bb,
                    llvmkit_ir::CallSiteConfig::new(name)
                        .calling_conv(calling_conv)
                        .attrs(call_attrs),
                )
                .map_err(|e| self.builder_err("invoke", e))?,
            ParsedCallee::Indirect(callee_ptr) => b
                .indirect_invoke_dyn_with_config::<llvmkit_ir::Dyn, _, _, _, _, _>(
                    callee_ptr,
                    parsed_fn_ty,
                    args,
                    normal_bb,
                    unwind_bb,
                    llvmkit_ir::CallSiteConfig::new(name)
                        .calling_conv(calling_conv)
                        .attrs(call_attrs),
                )
                .map_err(|e| self.builder_err("invoke", e))?,
        };
        let ret_is_void = matches!(
            parsed_fn_ty.return_type().into_type_enum(),
            AnyTypeEnum::Void(_)
        );
        // For void-returning invokes, don't bind a result. Non-void unnamed
        // invokes still consume the next numbered local slot, matching
        // `LLParser::setInstName(NameID=-1, NameStr="")`.
        if ret_is_void {
            Ok(None)
        } else {
            Ok(Some(inst.to_erased()))
        }
    }

    /// `callbr [cc] <ret-ty> @func(<args>) [other label targets]
    ///        to label %normal [, label %indirect ...]`. Terminator.
    /// Mirrors `LLParser::parseCallBr`.
    ///
    /// Upstream: `test/Assembler/callbr.ll`.
    fn parse_callbr(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<Option<llvmkit_ir::Value<'ctx, B>>> {
        self.bump()?; // eat `callbr`
        let call_loc = self.loc();
        let calling_conv = self.parse_optional_calling_conv()?;
        let return_attrs = self.parse_optional_return_attrs()?;
        let ret_ty_loc = self.loc();
        let callee_ty = self.parse_type(true)?;
        // `LLParser::parseCallBr` has no `parseOptionalProgramAddrSpace` — its
        // `||` chain goes return-attrs -> `parseType` — and resolves the callee
        // with `convertValIDToValue(PointerType::getUnqual(Context), …)`, i.e.
        // address space 0 whatever the datalayout says. Written out here so the
        // asymmetry with `parseCall` / `parseInvoke` is visible at the call
        // site rather than hidden in a callee helper's default.
        let callbr_addr_space = 0;
        let parsed_callee = self.parse_direct_callee_ref(state, callbr_addr_space)?;
        self.expect_punct(PunctKind::LParen, "'(' in callbr argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx, B>> = Vec::new();
        let mut arg_tys: Vec<Type<'ctx, B>> = Vec::new();
        let mut arg_locs: Vec<Span> = Vec::new();
        let mut arg_attrs = Vec::new();
        // callbr can never be varargs-forwarding (only musttail calls are).
        let var_args = false;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    // A callbr can never be musttail, so a forwarding
                    // ellipsis is always invalid here.
                    return Err(
                        self.expected("unexpected ellipsis in argument list for non-musttail call")
                    );
                }
                let arg_loc = self.loc();
                let arg_ty = self.parse_type(false)?;
                let one_arg_attrs = self.parse_optional_param_attrs()?;
                // `parseParameterList` branches on the argument type: a
                // `metadata` parameter takes `parseMetadataAsValue`, not
                // `parseValue`, which is what makes `metadata i32 %a` — every
                // old-format debug intrinsic operand — legal.
                let arg_v = if arg_ty.is_metadata() {
                    self.parse_metadata_as_value(state)?
                } else {
                    self.parse_value(state, arg_ty)?
                };
                arg_attrs.push(one_arg_attrs);
                arg_tys.push(arg_ty);
                arg_locs.push(arg_loc);
                args.push(arg_v);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close callbr argument list")?;
        let (function_attrs, function_attr_groups) = self.parse_optional_fn_attrs()?;
        let operand_bundles = self.parse_optional_operand_bundles(state)?;
        let call_attrs = llvmkit_ir::instr_types::CallAttributeData::new(
            return_attrs,
            arg_attrs.into_boxed_slice(),
            function_attrs,
        )
        .function_attr_groups(function_attr_groups.into_boxed_slice())
        .operand_bundles(operand_bundles);
        self.expect_keyword(Keyword::To, "'to' in callbr")?;
        let fallthrough = self.parse_type_and_basic_block(state)?;
        // Optional `[ label %ind1, ... ]`
        // The indirect-destination list is **mandatory**, and no comma
        // precedes it: `parseCallBr` ends its `||` chain with
        // `parseToken(lltok::lsquare, "expected '[' in callbr")`. llvmkit made
        // the whole list optional and tolerated a leading comma, so
        // `callbr void @f() to label %x` and `... to label %x, [...]` both
        // parsed.
        let mut indirect: Vec<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> = Vec::new();
        self.expect_punct(PunctKind::LSquare, "'[' in callbr")?;
        // `parseCallBr` unrolls the first iteration of its destination list
        // exactly as `parseIndirectBr` does, and for the same observable
        // reason: `[label %a,]` and `[label %a label %b]` are both rejected.
        if !matches!(self.peek(), Token::RSquare) {
            indirect.push(self.parse_type_and_basic_block(state)?);
            while self.eat_punct(PunctKind::Comma)? {
                indirect.push(self.parse_type_and_basic_block(state)?);
            }
        }
        self.expect_punct(PunctKind::RSquare, "']' at end of block list")?;
        // Upstream `resolveFunctionType`: an explicitly written function
        // type IS the call-site type; otherwise infer from the arguments.
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => {
                if !callee_ty.is_valid_function_return() {
                    return Err(
                        self.message_at(ret_ty_loc, "Invalid result type for LLVM function")
                    );
                }
                function_type_with_variadic(self.module, callee_ty, arg_tys.clone(), var_args)
            }
        };
        // `parseCallBr` resolves the callee before the argument loop, exactly
        // as `parseCall` does.
        let callee = self.resolve_direct_callee(parsed_callee, parsed_fn_ty, callbr_addr_space)?;
        self.check_call_argument_agreement(parsed_fn_ty, &arg_tys, &arg_locs, call_loc)?;
        let name = result_name.as_str();
        let (_, inst) = match callee {
            ParsedCallee::Function(callee) => b
                .callbr_with_config(
                    callee,
                    args,
                    fallthrough,
                    indirect,
                    llvmkit_ir::CallSiteConfig::new(name)
                        .calling_conv(calling_conv)
                        .attrs(call_attrs)
                        .call_site_type(parsed_fn_ty),
                )
                .map_err(|e| self.builder_err("callbr", e))?,
            ParsedCallee::InlineAsm(asm) => b
                .inline_asm_callbr_with_config::<llvmkit_ir::Dyn, _, _, _, _, _>(
                    asm,
                    args,
                    fallthrough,
                    indirect,
                    llvmkit_ir::CallSiteConfig::new(name)
                        .calling_conv(calling_conv)
                        .attrs(call_attrs),
                )
                .map_err(|e| self.builder_err("callbr", e))?,
            // `parseCallBr` stores whatever `Value *` its callee resolved to,
            // exactly as `parseCall` does; a non-function operand — a function
            // pointer, or a `@name` still standing on its forward-reference
            // placeholder — is `Verifier::visitCallBrInst`'s to reject
            // ("Callbr: indirect function / invalid signature"), not the
            // parser's. llvmkit used to reject it here because its callbr
            // builder had no indirect form.
            ParsedCallee::Indirect(callee_ptr) => b
                .indirect_callbr_with_config(
                    callee_ptr,
                    parsed_fn_ty,
                    args,
                    fallthrough,
                    indirect,
                    llvmkit_ir::CallSiteConfig::new(name)
                        .calling_conv(calling_conv)
                        .attrs(call_attrs),
                )
                .map_err(|e| self.builder_err("callbr", e))?,
        };
        let ret_is_void = matches!(
            parsed_fn_ty.return_type().into_type_enum(),
            AnyTypeEnum::Void(_)
        );
        if ret_is_void {
            Ok(None)
        } else {
            Ok(Some(inst.to_erased()))
        }
    }

    /// Parse a parent-pad scope operand for `cleanuppad` and `catchswitch`.
    ///
    /// `LLParser::parseCleanupPad` and `LLParser::parseCatchSwitch` both spell
    /// this `parseValue(Type::getTokenTy(Context), ParentPad, PFS)`: the
    /// `token` type is *implied*, and there is no type in the syntax. Reading
    /// one here consumed `%cs` as a named-type reference and then read the
    /// `[…]` argument list as an array constant.
    ///
    /// The `none` early return is llvmkit's ADT spelling of upstream's
    /// `ConstantTokenNone`: the instruction payloads store the parent pad as an
    /// `Option<ValueSlot>` and select a `*_within_none` builder where upstream
    /// stores the constant. The accept/reject set is identical, because the
    /// caller's `check_pad_scope_token` has already narrowed the token to
    /// `none`, `%name` or `%N`.
    fn parse_optional_pad_token(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<Option<llvmkit_ir::Value<'ctx, B>>> {
        if matches!(self.peek(), Token::Kw(Keyword::None)) {
            self.bump()?;
            return Ok(None);
        }
        let token_ty = self.module.token_type().as_type();
        let v = self.parse_value(state, token_ty)?;
        Ok(Some(v))
    }

    /// Parse `[ ty val, ty val, ... ]` — a bracket-enclosed value list
    /// used by `cleanuppad` / `catchpad` argument lists.
    fn parse_bracket_value_list(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<Vec<llvmkit_ir::Value<'ctx, B>>> {
        self.expect_punct(PunctKind::LSquare, "'[' to open pad argument list")?;
        let mut args = Vec::new();
        if !matches!(self.peek(), Token::RSquare) {
            loop {
                let ty = self.parse_type(false)?;
                // `parseExceptionArgs` branches on the argument type the same
                // way `parseParameterList` and `parseOptionalOperandBundles`
                // do: `metadata` goes to `parseMetadataAsValue`, everything
                // else to `parseValue`.
                let v = if ty.is_metadata() {
                    self.parse_metadata_as_value(state)?
                } else {
                    self.parse_value(state, ty)?
                };
                args.push(v);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RSquare, "']' to close pad argument list")?;
        Ok(args)
    }

    /// [`Self::builder_err`] at an explicit location — upstream's
    /// `error(Loc, …)` beside its `tokError(…)`.
    fn builder_err_at(&self, loc: Span, label: &str, e: IrError) -> ParseError {
        ParseError::Expected {
            expected: format!("{label}: {e}").into(),
            loc: DiagLoc::span(loc),
        }
    }

    fn builder_err(&self, label: &str, e: IrError) -> ParseError {
        ParseError::Expected {
            expected: format!("valid {label}: {e}").into(),
            loc: DiagLoc::span(self.loc()),
        }
    }

    /// Mirrors `LLParser::parseTypeAndValue` — `parseType(Ty) ||
    /// parseValue(Ty, V, PFS)`. The type is handed back alongside the value
    /// because two of upstream's callers (`parseBr`, `parseSwitch`) test it
    /// after the fact.
    fn parse_type_and_value(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<(Type<'ctx, B>, llvmkit_ir::Value<'ctx, B>)> {
        let ty = self.parse_type(false)?;
        let value = self.parse_value(state, ty)?;
        Ok((ty, value))
    }

    /// Mirrors `LLParser::parseTypeAndBasicBlock`, the one routine every
    /// terminator's block operand goes through.
    ///
    /// It is `parseTypeAndValue` plus an `isa<BasicBlock>` guard, so the token
    /// that is *not* a `label` decides the message: one that cannot begin a
    /// type gives `parseType`'s `expected type`, and a well-formed
    /// type-and-value that is not a block gives `expected a basic block`
    /// anchored at the **start of the type** — which is why `Loc` is taken
    /// before the type is read and not re-taken afterwards.
    ///
    /// Upstream's second overload (`parseTypeAndBasicBlock(BB, PFS)`) only
    /// discards the out-parameter `Loc`; every in-tree caller of the
    /// three-argument form discards it too, so there is one routine here.
    fn parse_type_and_basic_block(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> {
        // `Loc = Lex.getLoc();`
        let loc = self.loc();
        // `if (parseTypeAndValue(V, PFS)) return true;`
        let (_, value) = self.parse_type_and_value(state)?;
        // `if (!isa<BasicBlock>(V)) return error(Loc, "expected a basic
        //  block"); BB = cast<BasicBlock>(V);`
        state
            .block_label_for_value(value)
            .ok_or_else(|| self.message_at(loc, "expected a basic block"))
    }

    /// Parse a value of the given type. Accepts local SSA references,
    /// integer literals, and `null`/`zeroinitializer`/`true`/`false`.
    fn parse_value(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.parse_value_no_type(state, ty)
    }

    fn parse_value_no_type(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let id = self.parse_val_id(Some(state), Some(ty))?;
        self.convert_val_id_to_value(ty, id, Some(state))
    }

    /// Parse a floating-point literal and perform APFloat semantic conversion.
    /// Read a floating-point literal with **no** reference to the type it will
    /// be used at, as `LLLexer` does — it has none. Every decimal literal
    /// becomes an `IEEEdouble`, and each `0x` form its own fixed semantics;
    /// narrowing to the demanded type, and rejecting when that loses
    /// information, is `convertValIDToValue`'s `t_APFloat` arm.
    fn parse_fp_literal(&mut self) -> ParseResult<ApFloat> {
        use super::ll_token::FpLit;
        let value = match self.peek() {
            Token::FloatLit(fp) => match *fp {
                FpLit::Decimal(s) => ApFloat::from_string(
                    ApFloatSemantics::IeeeDouble,
                    s,
                    RoundingMode::NearestTiesToEven,
                )
                .map(|(value, _status)| value)
                .map_err(|_| self.expected("valid decimal float literal"))?,
                FpLit::HexDouble(s) => parse_hex_apfloat(ApFloatSemantics::IeeeDouble, s)
                    .map_err(|_| self.expected("valid hex double literal"))?,
                FpLit::HexHalf(s) => parse_hex_apfloat(ApFloatSemantics::IeeeHalf, s)
                    .map_err(|_| self.expected("valid hex half literal"))?,
                FpLit::HexBfloat(s) => parse_hex_apfloat(ApFloatSemantics::Bfloat, s)
                    .map_err(|_| self.expected("valid hex bfloat literal"))?,
                FpLit::HexX87(s) => parse_hex_apfloat(ApFloatSemantics::X87DoubleExtended, s)
                    .map_err(|_| self.expected("valid hex x87 literal"))?,
                FpLit::HexQuad(s) => parse_hex_apfloat_pair(ApFloatSemantics::IeeeQuad, s)
                    .map_err(|_| self.expected("valid hex quad literal"))?,
                // NOT the pair reader, because llvmkit's `PpcDoubleDouble` bit
                // layout is itself the mirror of upstream's: `ppc_words`
                // (`ap_float.rs`) reads the *high* word as the leading double,
                // where `DoubleAPFloat::bitcastToAPInt` puts the leading double
                // in the low word. The two mirrorings cancel, so reading the
                // digits big-endian assigns the halves exactly as upstream's
                // `HexToIntPair` does. See `parse_hex_apfloat_pair` for why
                // `fp128`, which has no component pair, needs the other reader.
                FpLit::HexPpc128(s) => parse_hex_apfloat(ApFloatSemantics::PpcDoubleDouble, s)
                    .map_err(|_| self.expected("valid hex ppc128 literal"))?,
            },
            _ => return Err(self.expected("floating-point literal")),
        };
        self.bump()?;
        Ok(value)
    }
}

/// `0xH`, `0xR`, `0x…`, and `0xK` — forms whose hex digits are a plain
/// big-endian integer of the semantics' own width.
///
/// `0xK` (`x86_fp80`) belongs here even though upstream reads it through
/// `LLLexer::FP80HexToIntPair`: that helper takes the first four
/// digits as the high word and the next sixteen as the low word, which for an
/// 80-bit value is exactly big-endian order. `AsmWriter` prints it the same
/// way, `getHiBits(16)` then `getLoBits(64)`, in `WriteConstantInternal`.
fn parse_hex_apfloat(semantics: ApFloatSemantics, digits: &str) -> IrResult<ApFloat> {
    let bits = ApInt::from_string(semantics.bit_width(), digits, 16)?;
    ApFloat::from_bits(semantics, &bits)
}

/// `0xL` (`fp128`) and `0xM` (`ppc_fp128`) — **not** big-endian.
///
/// Ports `LLLexer::HexToIntPair` exactly: the first sixteen
/// hex digits are the *low* 64-bit word and the next sixteen are the high
/// word, which is also the order `AsmWriter` prints them in — `getLoBits(64)`
/// then `getHiBits(64)`, in `AsmWriter`'s `WriteConstantInternal`.
/// Reading these as one
/// big-endian 128-bit number transposes the halves, which both changes the
/// value and makes `parse → print` non-idempotent against llvmkit's own
/// printer.
///
/// Upstream's fewer-than-sixteen-digit behaviour is mirrored too, quirk
/// included: the low word is only filled when at least sixteen digits are
/// present, so a short literal such as `0xL1` lands entirely in the *high*
/// word.
fn parse_hex_apfloat_pair(semantics: ApFloatSemantics, digits: &str) -> IrResult<ApFloat> {
    let bytes = digits.as_bytes();
    let (low_digits, high_digits) = if bytes.len() >= 16 {
        digits.split_at(16)
    } else {
        ("", digits)
    };
    let low = hex_word(low_digits)?;
    let high = hex_word(high_digits)?;
    let bits = ApInt::from_words(semantics.bit_width(), &[low, high]);
    ApFloat::from_bits(semantics, &bits)
}

/// One 64-bit word of a `0xL` / `0xM` literal. At most sixteen hex digits, so
/// the accumulation cannot overflow; more than that is upstream's
/// "constant bigger than 128 bits detected".
fn hex_word(digits: &str) -> IrResult<u64> {
    if digits.len() > 16 {
        return Err(IrError::InvalidOperation {
            message: "hexadecimal float word is longer than 16 digits",
        });
    }
    let mut word = 0u64;
    for byte in digits.bytes() {
        let digit = char::from(byte)
            .to_digit(16)
            .ok_or(IrError::InvalidOperation {
                message: "hexadecimal float literal has a non-hex digit",
            })?;
        word = word * 16 + u64::from(digit);
    }
    Ok(word)
}

// ── Helper enums ────────────────────────────────────────────────────────────

// ── Function-body helper types ──────────────────────────────────────────────

#[derive(Clone, Debug)]
enum BlockRef {
    Named(String),
    Numbered(u32),
}

/// A function-local name used before it was defined: the placeholder minted
/// at the first use, and that use's span.
///
/// The pair is what `PerFunctionState::ForwardRefVals` stores
/// (`std::pair<Value*, LocTy>`). The span is not decoration — upstream
/// reports `use of undefined value` at the *first* reference, not at the end
/// of the function.
struct ForwardRef<'ctx, B: ModuleBrand> {
    placeholder: llvmkit_ir::ForwardRefValue<'ctx, B>,
    loc: Span,
}

/// How a function-local value was spelled at a use site. Upstream keeps the
/// two spellings in separate maps and passes `"%" + Name` or `"%" + Twine(ID)`
/// to `checkValidVariableType`; this carries the same distinction.
#[derive(Clone, Copy)]
enum LocalRef<'a> {
    Named(&'a str),
    Numbered(u32),
}

impl LocalRef<'_> {
    /// The sigil-prefixed spelling upstream quotes in its diagnostics.
    fn display(self) -> String {
        match self {
            LocalRef::Named(name) => format!("%{name}"),
            LocalRef::Numbered(id) => format!("%{id}"),
        }
    }
}

/// Mirrors `LLParser::checkValidVariableType`: a name that resolves to a
/// value of the wrong type is an error, worded one way when a `label` was
/// wanted and another way otherwise.
///
/// Two spelling changes from upstream, both forced:
///
/// - upstream takes the `Value *` and opens with `Type *ValTy =
///   Val->getType();`. That statement is hoisted to the caller here, because a
///   global object's arena type in llvmkit is its **value** type
///   (`GlobalValue::getValueType`) while upstream's `Val->getType()` for a
///   `GlobalValue` is the pointer `GlobalValue::getType` builds —
///   `PointerType::get(C, GV->getAddressSpace())`. Callers holding a global
///   rebuild that pointer; see `docs/divergences.md` D3.
/// - upstream returns `Val` or `nullptr`, and every caller turns the null into
///   `return true`. Here the sentinel is the `Err`, and the caller keeps the
///   value it already had.
///
/// `name` is the sigil-prefixed spelling upstream quotes: `"%" + Name` /
/// `"%" + Twine(ID)` from `PerFunctionState::getVal`, `"@" + Name` /
/// `"@" + Twine(ID)` from `getGlobalVal`.
fn check_valid_variable_type<'ctx, B: ModuleBrand + 'ctx>(
    loc: Span,
    name: &str,
    ty: Type<'ctx, B>,
    value_ty: Type<'ctx, B>,
) -> ParseResult<()> {
    if value_ty == ty {
        return Ok(());
    }
    if ty.is_label() {
        return Err(ParseError::NotABasicBlock {
            name: name.to_owned(),
            loc: DiagLoc::span(loc),
        });
    }
    Err(ParseError::DefinedWithWrongType {
        name: name.to_owned(),
        defined: value_ty.to_string(),
        expected: ty.to_string(),
        loc: DiagLoc::span(loc),
    })
}

/// Per-function symbol tables. Mirrors `LLParser::PerFunctionState`'s
/// named/numbered value tables and the basic-block lookup map.
struct PerFunctionState<'ctx, B: ModuleBrand> {
    func: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>,
    /// `%name` to the bound SSA value.
    local_named: std::collections::HashMap<String, llvmkit_ir::Value<'ctx, B>>,
    /// `%N` to the bound function-local value: argument, instruction result,
    /// or unnamed basic block. LLVM keeps these in one NumberedVals table.
    local_numbered: std::collections::HashMap<u32, llvmkit_ir::Value<'ctx, B>>,
    /// Slot id of the next anonymous function-local value.
    next_unnamed_value_id: u32,
    /// `label` to the named basic-block identity. Created on first reference
    /// to support `br label %later` forward references; re-materialize a
    /// linear insertion handle only at the construction use site.
    ///
    /// `RefCell` for the reason [`Self::forward_ref_named`] carries one: a
    /// block forward reference is minted by *reading* an operand, inside
    /// `getVal`'s `Ty->isLabelTy()` arm, and every value-parsing path reaches
    /// that through `&PerFunctionState`.
    blocks: RefCell<std::collections::HashMap<String, llvmkit_ir::Value<'ctx, B>>>,
    block_refs: RefCell<std::collections::HashMap<String, Span>>,
    defined_blocks: std::collections::HashSet<String>,
    /// `%N` block placeholder identities and definitions, keyed by the shared
    /// local numbered-value slot.
    numbered_blocks: RefCell<std::collections::HashMap<u32, llvmkit_ir::Value<'ctx, B>>>,
    numbered_block_refs: RefCell<std::collections::HashMap<u32, Span>>,
    defined_numbered_blocks: std::collections::HashSet<u32>,
    /// `%name` referenced before it was defined, holding the placeholder
    /// minted at the first use and that use's span. Mirrors
    /// `PerFunctionState::ForwardRefVals`.
    ///
    /// A `BTreeMap`, not a `HashMap`, because `finishFunction` reports
    /// `ForwardRefVals.begin()` — the lexicographically smallest name — and
    /// which of several undefined names is named is part of the diagnostic.
    /// `RefCell` because a forward reference is created by *reading* an
    /// operand: every value-parsing path would otherwise have to thread a
    /// `&mut` down through `parse_val_id` / `convert_val_id_to_value`.
    forward_ref_named: RefCell<BTreeMap<String, ForwardRef<'ctx, B>>>,
    /// `%N` referenced before it was defined. Mirrors
    /// `PerFunctionState::ForwardRefValIDs`; ordered for the same reason.
    forward_ref_numbered: RefCell<BTreeMap<u32, ForwardRef<'ctx, B>>>,
    /// Source span of each parsed phi, keyed by its result name, so the
    /// end-of-function coherence check in `finish()` can point a diagnostic
    /// at the offending phi instead of at `Module::verify()`.
    phi_locs: Vec<(llvmkit_ir::value::ValueSlot, Span)>,
}

impl<'ctx, B: ModuleBrand + 'ctx> PerFunctionState<'ctx, B> {
    fn new(func: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>) -> Self {
        let mut blocks = std::collections::HashMap::new();
        for bb in func.basic_blocks() {
            let name = bb.name().unwrap_or_default();
            blocks.insert(name, bb.to_erased());
        }
        Self {
            func,
            local_named: std::collections::HashMap::new(),
            local_numbered: std::collections::HashMap::new(),
            next_unnamed_value_id: 0,
            blocks: RefCell::new(blocks),
            block_refs: RefCell::new(std::collections::HashMap::new()),
            defined_blocks: std::collections::HashSet::new(),
            numbered_blocks: RefCell::new(std::collections::HashMap::new()),
            numbered_block_refs: RefCell::new(std::collections::HashMap::new()),
            defined_numbered_blocks: std::collections::HashSet::new(),
            forward_ref_named: RefCell::new(BTreeMap::new()),
            forward_ref_numbered: RefCell::new(BTreeMap::new()),
            phi_locs: Vec::new(),
        }
    }

    fn invalid_numbered_slot(&self, id: u32, loc: Span) -> ParseError {
        ParseError::InvalidSlotId {
            source: AddError::StaleId {
                id,
                next: self.next_unnamed_value_id,
            },
            loc: DiagLoc::span(loc),
        }
    }

    /// Mirrors `PerFunctionState::getVal(Name, Type::getLabelTy(…), Loc)` —
    /// the whole of `getBB(const std::string &, LocTy)` bar its
    /// `dyn_cast_or_null`, and the `Ty->isLabelTy()` arm of `getVal`'s
    /// placeholder minting (`FwdVal = BasicBlock::Create(F.getContext(),
    /// Name, &F)`).
    ///
    /// A name already bound to a non-block local reaches
    /// `checkValidVariableType` with `Ty->isLabelTy()`, so it fails with
    /// `'%name' is not a basic block`. That is the message `br label %x`
    /// carries when `%x` is an instruction result, and the one
    /// [`Self::get_basic_block_named`] overwrites with `unable to create
    /// block named '<n>'` when the same name is being *defined* as a label.
    fn get_val_as_block_named(
        &self,
        module: &'ctx Module<B, Unverified>,
        name: &str,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.get_val(
            module,
            LocalRef::Named(name),
            module.label_type().as_type(),
            loc,
        )
    }

    /// Look up or lazily create the named basic block, as a label identity.
    fn ensure_block_label(
        &self,
        module: &'ctx Module<B, Unverified>,
        name: &str,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> {
        let value = self.get_val_as_block_named(module, name, loc)?;
        self.value_as_block_label(value, loc)
    }

    /// Mirrors `LLParser::PerFunctionState::defineBB`.
    ///
    /// One function for one upstream routine: the first `Name.empty()` branch
    /// picks the block, the `F.splice` step moves it to the end of the
    /// function, and the second `Name.empty()` branch drops it from the
    /// forward-ref sets. The three [`BlockHeader`] arms are upstream's three
    /// cases — a textual label, a numbered label (`NameID != -1`), and an
    /// unlabeled block (`NameID == -1`, so `NameID = NumberedVals.getNext()`).
    fn define_basic_block(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        header: BlockHeader,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        // `BasicBlock *BB;` — upstream's `if (Name.empty()) { … } else { … }`.
        let (block, defined_name) = match header {
            // `} else { NameID = NumberedVals.getNext(); }`
            BlockHeader::Implicit => {
                let id = self.next_unnamed_value_id;
                (
                    self.get_basic_block_numbered(module, id, loc)?,
                    DefinedBlockName::Numbered(id),
                )
            }
            // `if (P.checkValueID(Loc, "label", "", NumberedVals.getNext(),
            //                     NameID)) return nullptr;`
            BlockHeader::Numbered(id) => {
                check_value_id("label", "", self.next_unnamed_value_id, id, loc)?;
                (
                    self.get_basic_block_numbered(module, id, loc)?,
                    DefinedBlockName::Numbered(id),
                )
            }
            // `BB = getBB(Name, Loc); if (!BB) { P.error(Loc, "unable to
            //  create block named '" + Name + "'"); return nullptr; }`
            BlockHeader::Named(name) => {
                let block = self.get_basic_block_named(module, &name, loc)?;
                (block, DefinedBlockName::Named(name))
            }
        };

        // `F.splice(F.end(), &F, BB->getIterator());`
        //
        // "Move the block to the end of the function.  Forward ref'd blocks
        // are inserted wherever they happen to be referenced." — so a block
        // reached by a forward reference prints where it is *defined*, not
        // where it was first mentioned. The handle is linear, so the erased
        // value is taken first and the block view re-derived at the end.
        //
        // The failure arm is unreachable: the handle was minted by
        // `append_basic_block` or `basic_block_for_construction` on *this*
        // function, so all three of `move_basic_block_to_end`'s refusals are
        // dead by construction — but the ban on runtime panics keeps the
        // mapping. It spells the same `valid <label>: <e>` shape
        // `LLParser::builder_err` uses; that helper itself is out of reach
        // here because it hangs off `LLParser` and keys off the *current*
        // token, where upstream's `error(Loc, …)` keys off the block's own.
        let block_value = block.to_erased();
        self.func
            .move_basic_block_to_end(module, block)
            .map_err(|e| ParseError::Expected {
                expected: format!("valid basic block definition: {e}").into(),
                loc: DiagLoc::span(loc),
            })?;

        // "Remove the block from forward ref sets."
        match defined_name {
            DefinedBlockName::Numbered(id) => {
                // `ForwardRefValIDs.erase(NameID);`
                self.numbered_block_refs.borrow_mut().remove(&id);
                // `NumberedVals.add(NameID, BB);` — `add` also advances
                // `NextUnusedID` to `ID + 1`; `checkValueID` has already
                // proved `id >= next`, so `max` and `id + 1` agree.
                self.local_numbered.insert(id, block_value);
                self.defined_numbered_blocks.insert(id);
                self.next_unnamed_value_id = self.next_unnamed_value_id.max(id.saturating_add(1));
            }
            // `// BB forward references are already in the function symbol
            //  table.
            //  ForwardRefVals.erase(Name);` — llvmkit keeps blocks in a map of
            // their own and reports leftovers by comparing `block_refs`
            // against `defined_blocks`, so marking the name defined *is* the
            // erase.
            DefinedBlockName::Named(name) => {
                self.defined_blocks.insert(name);
            }
        }

        // `return BB;`
        self.value_as_block(module, block_value, loc)
    }

    /// Mirrors `PerFunctionState::getVal(ID, Type::getLabelTy(…), Loc)` — the
    /// whole of `getBB(unsigned ID, LocTy)` bar its `dyn_cast_or_null`,
    /// including the `Ty->isLabelTy()` arm of the placeholder minting.
    ///
    /// The numbered twin of [`Self::get_val_as_block_named`], and reachable
    /// for the same reason: `checkValueID` only rejects `ID < NextID`, so an
    /// id at or above it that already carries a pending **non-label** forward
    /// reference passes that guard and fails here instead, with
    /// `'%N' is not a basic block`.
    fn get_val_as_block_numbered(
        &self,
        module: &'ctx Module<B, Unverified>,
        id: u32,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.get_val(
            module,
            LocalRef::Numbered(id),
            module.label_type().as_type(),
            loc,
        )
    }

    /// Mirrors `PerFunctionState::getBB(unsigned ID, LocTy)` together with
    /// `defineBB`'s `unable to create block numbered '<N>'` arm — the numbered
    /// twin of [`Self::get_basic_block_named`], and collapsed the same way.
    fn get_basic_block_numbered(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        id: u32,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        let value = self
            .get_val_as_block_numbered(module, id, loc)
            .map_err(|_| ParseError::Message {
                message: format!("unable to create block numbered '{id}'").into(),
                loc: DiagLoc::span(loc),
            })?;
        self.value_as_block(module, value, loc)
    }

    /// Mirrors `PerFunctionState::getBB(const std::string &Name, LocTy)`
    /// together with `defineBB`'s `unable to create block named '<n>'` arm.
    ///
    /// `getBB` reaches the block through `getVal(Name, LabelTy)`, so the name
    /// is looked up in the function's **value** symbol table: a name already
    /// bound to a non-block local makes the block uncreatable, and `getVal`'s
    /// own `'%x' is not a basic block` is then overwritten by `defineBB`'s
    /// message — upstream's `error()` keeps only the last one at equal
    /// priority, so that is what a user sees. Discarding the inner error is
    /// that overwrite; both carry the same `Loc`.
    fn get_basic_block_named(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        name: &str,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        let value = self
            .get_val_as_block_named(module, name, loc)
            .map_err(|_| ParseError::Message {
                message: format!("unable to create block named '{name}'").into(),
                loc: DiagLoc::span(loc),
            })?;
        self.value_as_block(module, value, loc)
    }

    fn value_as_block(
        &self,
        module: &'ctx Module<B, Unverified>,
        value: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        self.func
            .basic_block_for_construction(module, value)
            .map_err(|_| ParseError::Expected {
                expected: "referenced value is not an unterminated basic block".into(),
                loc: DiagLoc::span(loc),
            })
    }

    /// The block this `blockaddress` label names, if the body has defined it.
    ///
    /// Deliberately non-creating: upstream's `getBB` *would* forward-declare,
    /// but every caller here runs either mid-body (where a miss defers) or at
    /// the close (where a miss is a real error), so inventing a block would
    /// only hide the second case.
    fn defined_block(&self, label: &BlockLabel) -> Option<llvmkit_ir::Value<'ctx, B>> {
        match label {
            BlockLabel::Named(name) => self
                .defined_blocks
                .contains(name)
                .then(|| self.blocks.borrow().get(name).copied())
                .flatten(),
            BlockLabel::Numbered(id) => self
                .defined_numbered_blocks
                .contains(id)
                .then(|| self.numbered_blocks.borrow().get(id).copied())
                .flatten(),
        }
    }

    /// `isa<BasicBlock>(V)`, as a projection rather than a predicate —
    /// upstream spells the test and the narrowing as one `dyn_cast`
    /// (`parseBr`) or as `isa` followed by `cast`
    /// (`parseTypeAndBasicBlock`), and both need the block afterwards.
    fn block_view_for_value(
        &self,
        value: llvmkit_ir::Value<'ctx, B>,
    ) -> Option<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Terminated, B>> {
        self.func.basic_blocks().find(|bb| bb.to_erased() == value)
    }

    /// [`Self::block_view_for_value`] as a label id.
    fn block_label_for_value(
        &self,
        value: llvmkit_ir::Value<'ctx, B>,
    ) -> Option<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> {
        self.block_view_for_value(value).map(|bb| bb.id())
    }

    fn value_as_block_view(
        &self,
        value: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Terminated, B>> {
        self.block_view_for_value(value)
            .ok_or_else(|| ParseError::Message {
                message: "referenced value is not a basic block".into(),
                loc: DiagLoc::span(loc),
            })
    }

    fn value_as_block_label(
        &self,
        value: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> {
        Ok(self.value_as_block_view(value, loc)?.id())
    }

    fn get_or_create_numbered_block_label(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        id: u32,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> {
        // `getVal(ID, LabelTy, Loc)` — no `checkValueID` here: upstream runs
        // that guard at the *definition* (`defineBB`), and a reference to a
        // backward slot forward-declares a block that the later label header
        // then rejects.
        let value = self.get_val_as_block_numbered(module, id, loc)?;
        self.value_as_block_label(value, loc)
    }

    /// Resolve a phi-incoming predecessor block reference for an edge-add.
    ///
    /// Unlike block *construction*, a phi predecessor is a label reference and
    /// is usually already terminated (the common merge-block / diamond-tail
    /// case), so this resolves through the state-agnostic label path and
    /// returns a view rather than an [`Unterminated`] construction handle. The
    /// block was ensured to exist when the phi incoming pair was parsed
    /// (`parse_phi_label`). Only phi resolution uses this; branch/switch
    /// targets go through `parse_type_and_basic_block`.
    fn resolve_block_ref(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        block_ref: &BlockRef,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Terminated, B>> {
        let label = match block_ref {
            BlockRef::Named(name) => self.ensure_block_label(module, name, loc)?,
            BlockRef::Numbered(id) => self.get_or_create_numbered_block_label(module, *id, loc)?,
        };
        self.value_as_block_view(module.view(label).to_erased(), loc)
    }

    /// The lookup half of both `PerFunctionState::getVal` overloads:
    /// `F.getValueSymbolTable()->lookup(Name)` / `NumberedVals.get(ID)`, then
    /// `ForwardRefVals` / `ForwardRefValIDs`.
    ///
    /// Blocks are consulted alongside values because upstream keeps both in
    /// the function's one value symbol table, which is what makes
    /// `'%x' is not a basic block` and `'%x' defined with type 'label'`
    /// reachable. One routine, because every caller of either `getVal`
    /// overload — including both `getBB` overloads — reads the same two
    /// tables in the same order.
    fn lookup_local(&self, reference: LocalRef<'_>) -> Option<llvmkit_ir::Value<'ctx, B>> {
        match reference {
            LocalRef::Named(name) => self
                .local_named
                .get(name)
                .copied()
                .or_else(|| self.blocks.borrow().get(name).copied())
                .or_else(|| {
                    self.forward_ref_named
                        .borrow()
                        .get(name)
                        .map(|entry| entry.placeholder.as_value())
                }),
            LocalRef::Numbered(id) => self
                .local_numbered
                .get(&id)
                .copied()
                .or_else(|| self.numbered_blocks.borrow().get(&id).copied())
                .or_else(|| {
                    self.forward_ref_numbered
                        .borrow()
                        .get(&id)
                        .map(|entry| entry.placeholder.as_value())
                }),
        }
    }

    /// Look up a function-local value, minting a forward-reference
    /// placeholder when the name has not been defined yet. Mirrors
    /// `LLParser::PerFunctionState::getVal`: symbol table, then the
    /// forward-reference map, then a fresh sentinel of the demanded type.
    ///
    /// Both of upstream's placeholder arms live here —
    /// `if (Ty->isLabelTy()) FwdVal = BasicBlock::Create(…); else FwdVal =
    /// new Argument(Ty, Name);` — because upstream has one `getVal` per
    /// spelling and `getBB` is `dyn_cast_or_null<BasicBlock>(getVal(Name,
    /// LabelTy, Loc))`. Splitting the label arm into a second routine is what
    /// made `parseTypeAndBasicBlock` unportable: `parseTypeAndValue` at a
    /// `label` type has to reach the block-minting arm, and it reaches it
    /// through `convertValIDToValue` -> `getVal`, not through `getBB`.
    fn get_val(
        &self,
        module: &'ctx Module<B, Unverified>,
        reference: LocalRef<'_>,
        ty: Type<'ctx, B>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        if let Some(value) = self.lookup_local(reference) {
            // `checkValidVariableType(Loc, "%" + Name, Ty, Val)` —
            // `LocalRef::display` produces upstream's `%name` / `%N` spelling.
            check_valid_variable_type(loc, &reference.display(), ty, value.ty())?;
            return Ok(value);
        }
        // "Don't make placeholders with invalid type" — upstream refuses a
        // sentinel it could not give a type to.
        if !ty.is_first_class() {
            return Err(ParseError::Message {
                message: "invalid use of a non-first-class type".into(),
                loc: DiagLoc::span(loc),
            });
        }
        // `if (Ty->isLabelTy()) FwdVal = BasicBlock::Create(F.getContext(),
        //  Name, &F);` — the numbered overload passes `""` for the name.
        // Upstream's `ForwardRefVals[Name] = std::make_pair(FwdVal, Loc);`
        // then records it; llvmkit keeps blocks in tables of their own, so
        // the identity and the reference span are recorded side by side and
        // `finishFunction` merges them back with the value tables.
        if ty.is_label() {
            let name = match reference {
                LocalRef::Named(name) => name,
                LocalRef::Numbered(_) => "",
            };
            let value = self.func.append_basic_block(module, name).to_erased();
            match reference {
                LocalRef::Named(name) => {
                    self.blocks.borrow_mut().insert(name.to_owned(), value);
                    self.block_refs
                        .borrow_mut()
                        .entry(name.to_owned())
                        .or_insert(loc);
                }
                LocalRef::Numbered(id) => {
                    self.numbered_blocks.borrow_mut().insert(id, value);
                    self.numbered_block_refs
                        .borrow_mut()
                        .entry(id)
                        .or_insert(loc);
                }
            }
            return Ok(value);
        }
        let placeholder =
            module
                .forward_ref_value_placeholder(ty)
                .map_err(|e| ParseError::Message {
                    message: format!("cannot create forward reference: {e}").into(),
                    loc: DiagLoc::span(loc),
                })?;
        let value = placeholder.as_value();
        let entry = ForwardRef { placeholder, loc };
        match reference {
            LocalRef::Named(name) => {
                self.forward_ref_named
                    .borrow_mut()
                    .insert(name.to_owned(), entry);
            }
            LocalRef::Numbered(id) => {
                self.forward_ref_numbered.borrow_mut().insert(id, entry);
            }
        }
        Ok(value)
    }

    /// Retire a forward reference now that its definition has been parsed.
    /// Mirrors the `Sentinel->replaceAllUsesWith(Inst)` step of
    /// `LLParser::PerFunctionState::setInstName`, including the type
    /// disagreement it reports first. Consuming `entry` is what makes
    /// "resolved exactly once" hold by construction.
    fn resolve_forward_ref(
        entry: ForwardRef<'ctx, B>,
        definition: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<()> {
        if entry.placeholder.ty() != definition.ty() {
            return Err(ParseError::InstructionForwardReferencedWithType {
                ty: entry.placeholder.ty().to_string(),
                loc: DiagLoc::span(loc),
            });
        }
        entry
            .placeholder
            .replace_all_uses_with(definition)
            .map_err(|e| ParseError::Message {
                message: format!("cannot resolve forward reference: {e}").into(),
                loc: DiagLoc::span(loc),
            })
    }

    /// Install an instruction's result name. Mirrors
    /// `LLParser::PerFunctionState::setInstName`.
    fn bind_local(
        &mut self,
        lhs: &LocalLhs,
        v: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<()> {
        if v.ty().is_void() {
            return reject_named_void(lhs, loc);
        }
        match lhs {
            LocalLhs::Named(n) => {
                let forward = self.forward_ref_named.borrow_mut().remove(n.as_str());
                if let Some(entry) = forward {
                    Self::resolve_forward_ref(entry, v, loc)?;
                }
                if self.local_named.insert(n.clone(), v).is_some() {
                    // `setInstName` sets the name and then notices the symbol
                    // table renamed it. The sentence is its own — not the
                    // `redefinition of <kind> '<sigil><name>'` shape — and
                    // upstream spells the name **without** a `%`.
                    return Err(ParseError::Message {
                        message: format!("multiple definition of local value named '{n}'").into(),
                        loc: DiagLoc::span(loc),
                    });
                }
            }
            LocalLhs::Numbered(_) | LocalLhs::None => {
                let id = match lhs {
                    LocalLhs::Numbered(id) => *id,
                    _ => self.next_unnamed_value_id,
                };
                check_value_id("instruction", "%", self.next_unnamed_value_id, id, loc)?;
                // A numbered slot already claimed by a forward-referenced
                // *block* is upstream's `ForwardRefValIDs` hit with a `label`
                // sentinel in it — one map there, so one diagnostic.
                if let Some(block) = self.numbered_blocks.borrow().get(&id).copied() {
                    return Err(ParseError::InstructionForwardReferencedWithType {
                        ty: block.ty().to_string(),
                        loc: DiagLoc::span(loc),
                    });
                }
                let forward = self.forward_ref_numbered.borrow_mut().remove(&id);
                if let Some(entry) = forward {
                    Self::resolve_forward_ref(entry, v, loc)?;
                }
                if self.local_numbered.insert(id, v).is_some() {
                    return Err(self.invalid_numbered_slot(id, loc));
                }
                self.next_unnamed_value_id = self.next_unnamed_value_id.max(id.saturating_add(1));
            }
        }
        Ok(())
    }

    /// Close the function: every forward reference must have been defined.
    /// Mirrors `LLParser::PerFunctionState::finishFunction`, called by
    /// `Parser::parse_define` before `}`.
    fn finish(self, module: &'ctx Module<B, Unverified>) -> ParseResult<()> {
        // Upstream holds blocks and values in the one `ForwardRefVals` map,
        // so an undefined label and an undefined value compete for the same
        // diagnostic — and both come out as `use of undefined value`, never
        // as a label. Merging llvmkit's two tables here reproduces both the
        // wording and `begin()`'s choice of which name to name.
        let mut undefined_named: BTreeMap<String, Span> = BTreeMap::new();
        for (name, loc) in self.block_refs.borrow().iter() {
            if !self.defined_blocks.contains(name) {
                undefined_named.insert(name.clone(), *loc);
            }
        }
        for (name, entry) in self.forward_ref_named.borrow().iter() {
            undefined_named.insert(name.clone(), entry.loc);
        }
        if let Some((name, loc)) = undefined_named.into_iter().next() {
            return Err(ParseError::UndefinedSymbol {
                kind: SYMBOL_KIND_LOCAL,
                id: SymbolId::Named(name),
                loc: DiagLoc::span(loc),
            });
        }
        let mut undefined_numbered: BTreeMap<u32, Span> = BTreeMap::new();
        for (id, loc) in self.numbered_block_refs.borrow().iter() {
            if !self.defined_numbered_blocks.contains(id) {
                undefined_numbered.insert(*id, *loc);
            }
        }
        for (id, entry) in self.forward_ref_numbered.borrow().iter() {
            undefined_numbered.insert(*id, entry.loc);
        }
        if let Some((id, loc)) = undefined_numbered.into_iter().next() {
            return Err(ParseError::UndefinedSymbol {
                kind: SYMBOL_KIND_LOCAL,
                id: SymbolId::Numbered(id),
                loc: DiagLoc::span(loc),
            });
        }
        // All blocks and edges now exist — every predecessor is known (the
        // parse-time analog of Cranelift's seal_block). Run the shared phi
        // coherence check here, anchored at the phi's source location,
        // instead of leaving an incomplete/incoherent phi to surface far
        // away from a later `Module::verify()`.
        if let Err(e) = llvmkit_ir::check_function_phi_coherence(module, self.func) {
            let loc = self
                .phi_locs
                .iter()
                .find(|(id, _)| *id == e.phi_id)
                .map(|(_, span)| DiagLoc::span(*span))
                .unwrap_or_else(|| DiagLoc::span(Span::default()));
            return Err(ParseError::Expected {
                expected: e.message.into(),
                loc,
            });
        }
        Ok(())
    }
}

enum BlockHeader {
    Named(String),
    Numbered(u32),
    Implicit,
}

/// Which of `defineBB`'s two `Name.empty()` branches a block definition took.
///
/// Upstream asks `Name.empty()` twice — once to pick the block, once to drop
/// it from the forward-ref sets — with the `F.splice` step between, and reads
/// `Name` and `NameID` (both `defineBB` parameters) on each side. llvmkit's
/// [`BlockHeader`] is consumed by the first `match`, so the answer is carried
/// across the splice in this rather than re-derived.
enum DefinedBlockName {
    /// `Name.empty()`: `ForwardRefValIDs.erase(NameID); NumberedVals.add(NameID, BB);`
    Numbered(u32),
    /// otherwise: `ForwardRefVals.erase(Name);`
    Named(String),
}

enum LocalLhs {
    Named(String),
    Numbered(u32),
    None,
}

impl LocalLhs {
    fn as_str(&self) -> &str {
        match self {
            LocalLhs::Named(n) => n.as_str(),
            // For numbered / unnamed LHS, pass an empty name; the
            // AsmWriter slot tracker will emit `%N` automatically.
            _ => "",
        }
    }
}

enum IntBinOp {
    Add,
    Sub,
    Mul,
    Udiv,
    Sdiv,
    Urem,
    Srem,
    Shl,
    Lshr,
    Ashr,
    And,
    Or,
    Xor,
}

impl IntBinOp {
    /// The IR opcode this keyword names.
    fn opcode(&self) -> llvmkit_ir::BinaryOpcode {
        use llvmkit_ir::BinaryOpcode as Op;
        match self {
            Self::Add => Op::Add,
            Self::Sub => Op::Sub,
            Self::Mul => Op::Mul,
            Self::Udiv => Op::Udiv,
            Self::Sdiv => Op::Sdiv,
            Self::Urem => Op::Urem,
            Self::Srem => Op::Srem,
            Self::Shl => Op::Shl,
            Self::Lshr => Op::Lshr,
            Self::Ashr => Op::Ashr,
            Self::And => Op::And,
            Self::Or => Op::Or,
            Self::Xor => Op::Xor,
        }
    }

    /// The `.ll` mnemonic, for builder-error reporting.
    fn mnemonic(&self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Udiv => "udiv",
            Self::Sdiv => "sdiv",
            Self::Urem => "urem",
            Self::Srem => "srem",
            Self::Shl => "shl",
            Self::Lshr => "lshr",
            Self::Ashr => "ashr",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
        }
    }
}

/// `true` for `<N x T>` and `<vscale x N x T>`.
///
/// The parser splits on this because llvmkit's typed integer handles carry a
/// scalar width; upstream's `LLParser` has one path for both shapes.
fn is_vector_type<B: llvmkit_ir::ModuleBrand>(ty: llvmkit_ir::Type<'_, B>) -> bool {
    matches!(
        ty.kind(),
        llvmkit_ir::TypeKind::FixedVector | llvmkit_ir::TypeKind::ScalableVector
    )
}

/// Collect the parsed flag keywords into the combined form the erased builder
/// takes. The opcode decides which of them survive.
fn int_binop_flags(nuw: bool, nsw: bool, exact: bool, disjoint: bool) -> llvmkit_ir::IntBinOpFlags {
    let mut flags = llvmkit_ir::IntBinOpFlags::new();
    if nuw {
        flags = flags.nuw();
    }
    if nsw {
        flags = flags.nsw();
    }
    if exact {
        flags = flags.exact();
    }
    if disjoint {
        flags = flags.disjoint();
    }
    flags
}

enum IntCast {
    Trunc,
    Zext,
    Sext,
}

impl IntCast {
    /// The IR opcode this spelling builds.
    fn cast_opcode(&self) -> llvmkit_ir::instr_types::CastOpcode {
        use llvmkit_ir::instr_types::CastOpcode;
        match self {
            Self::Trunc => CastOpcode::Trunc,
            Self::Zext => CastOpcode::Zext,
            Self::Sext => CastOpcode::Sext,
        }
    }

    /// The keyword, for a builder error that names the instruction.
    fn mnemonic(&self) -> &'static str {
        match self {
            Self::Trunc => "trunc",
            Self::Zext => "zext",
            Self::Sext => "sext",
        }
    }
}

enum FpBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

enum FpToInt {
    FpToSi,
    FpToUi,
}

enum IntToFp {
    SiToFp,
    UiToFp,
}

/// Alias for the dyn-positioned, dyn-return IrBuilder we drive while
/// emitting one block's instructions. The terminator-emitting calls
/// (`ret` / `br` / etc.) take this by value, so the parser
/// stores it inside an `Option<Self>` for the duration of the block.
type ParsedBlockBuilder<'m, 'ctx, B> = IrBuilder<'m, 'ctx, B, NoFolder, Positioned, Dyn>;

fn live_builder_error(loc: Span) -> ParseError {
    ParseError::Expected {
        expected: "live insertion builder before terminator".into(),
        loc: DiagLoc::span(loc),
    }
}

fn take_live_builder<'m, 'ctx, B: ModuleBrand + 'ctx>(
    builder: &mut Option<ParsedBlockBuilder<'ctx, 'ctx, B>>,
    loc: Span,
) -> ParseResult<ParsedBlockBuilder<'ctx, 'ctx, B>> {
    builder.take().ok_or_else(|| live_builder_error(loc))
}

fn borrow_live_builder<'b, 'm, 'ctx, B: ModuleBrand + 'ctx>(
    builder: &'b Option<ParsedBlockBuilder<'ctx, 'ctx, B>>,
    loc: Span,
) -> ParseResult<&'b ParsedBlockBuilder<'ctx, 'ctx, B>> {
    builder.as_ref().ok_or_else(|| live_builder_error(loc))
}

/// Local symbol kind label used in [`crate::parse_error::ParseError::UndefinedSymbol`].
const SYMBOL_KIND_LOCAL: SymbolKind = SymbolKind::Local;

#[derive(Clone, Debug, PartialEq, Eq)]
enum NameOrId {
    Name(String),
    Id(u32),
}

#[derive(Clone, Copy, Debug)]
enum PunctKind {
    Equal,
    Comma,
    /// Only reachable while the lexer is in `ignore_colon_in_idents` mode —
    /// otherwise a colon is absorbed into the preceding label token. That is
    /// the mode `memory(argmem: read)` is parsed in.
    Colon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LSquare,
    RSquare,
    Less,
    Greater,
}

impl PunctKind {
    fn matches(self, t: &Token<'_>) -> bool {
        matches!(
            (self, t),
            (PunctKind::Equal, Token::Equal)
                | (PunctKind::Comma, Token::Comma)
                | (PunctKind::Colon, Token::Colon)
                | (PunctKind::LParen, Token::LParen)
                | (PunctKind::RParen, Token::RParen)
                | (PunctKind::LBrace, Token::LBrace)
                | (PunctKind::RBrace, Token::RBrace)
                | (PunctKind::LSquare, Token::LSquare)
                | (PunctKind::RSquare, Token::RSquare)
                | (PunctKind::Less, Token::Less)
                | (PunctKind::Greater, Token::Greater)
        )
    }
}

// ── Helpers on Type that surface IR-level introspection ─────────────────────

/// Lift a [`Type<'ctx, B>`] to the matching [`AnyTypeEnum`] arm. Re-uses the
/// IR side's `try_into` impl so the parser does not duplicate the kind /
/// data-arm dispatch table.
trait IntoTypeEnum<'ctx, B: ModuleBrand> {
    fn into_type_enum(self) -> AnyTypeEnum<'ctx, B>;
}

impl<'ctx, B: ModuleBrand + 'ctx> IntoTypeEnum<'ctx, B> for Type<'ctx, B> {
    fn into_type_enum(self) -> AnyTypeEnum<'ctx, B> {
        AnyTypeEnum::from(self)
    }
}

/// The one legitimate runtime-variadic consumer: parsed IR discovers
/// `...` at run time, so this private choke point dispatches between
/// [`Module::function_type`] and [`Module::variadic_function_type`].
fn function_type_with_variadic<'ctx, B, I, R, T>(
    module: &'ctx Module<B, Unverified>,
    return_type: R,
    parameters: I,
    var_args: bool,
) -> llvmkit_ir::FunctionType<'ctx, B>
where
    B: ModuleBrand + 'ctx,
    I: IntoIterator<Item = T>,
    R: Into<llvmkit_ir::Type<'ctx, B>>,
    T: Into<llvmkit_ir::Type<'ctx, B>>,
{
    if var_args {
        module.variadic_function_type(return_type, parameters)
    } else {
        module.function_type(return_type, parameters)
    }
}

/// What a `name:` field wants, when nothing usable follows the colon.
///
/// Upstream has no shared "metadata field value" production: `PARSE_MD_FIELDS`
/// expands to one typed `LLParser::parseMDField` overload per field class, and
/// each opens with a token-kind check naming its own family. This is that
/// vocabulary, collected — the phrase is the argument to `tokError`, minus the
/// leading `expected `.
///
/// The families left at the syntactic phrase are the ones whose upstream
/// overload does not have a determinate one:
///
/// * `MDField`, `MDStringField`, `MDFieldList`, and the two `…OrMDField`
///   pairs delegate to `parseMetadata` / `parseStringConstant` /
///   `parseMDNodeVector`, whose own message depends on how far that routine
///   got.
/// * `ChecksumKindField` is upstream's odd one out: it reports
///   `invalid checksum kind '…'` interpolating `Lex.getStrVal()` **even when
///   the token kind is wrong**, so on an error token the quoted name is
///   whatever the previous token happened to leave in `StrVal`. llvmkit does
///   not carry a stale `StrVal`, so it cannot reproduce that string
///   (`docs/divergences.md` entry 34, its `ChecksumKindField` note).
fn expected_for_metadata_field_kind(kind: llvmkit_ir::metadata::MetadataFieldKind) -> &'static str {
    use llvmkit_ir::metadata::MetadataFieldKind;
    match kind {
        MetadataFieldKind::Unsigned { .. } => "unsigned integer",
        MetadataFieldKind::Signed { .. } => "signed integer",
        MetadataFieldKind::Bool => "'true' or 'false'",
        MetadataFieldKind::ApsInt => "integer",
        MetadataFieldKind::DwarfTag => "DWARF tag",
        MetadataFieldKind::DwarfAttEncoding => "DWARF type attribute encoding",
        MetadataFieldKind::DwarfVirtuality => "DWARF virtuality code",
        MetadataFieldKind::DwarfLang => "DWARF language",
        MetadataFieldKind::DwarfSourceLangName => "DWARF source language name",
        MetadataFieldKind::DwarfCc => "DWARF calling convention",
        MetadataFieldKind::DwarfMacinfoType => "DWARF macinfo type",
        MetadataFieldKind::DwarfEnumKind => "DWARF enum kind code",
        // Both flag overloads spell it the same way; only the *invalid*
        // message distinguishes them. Unreachable through this table since the
        // two overloads got their own routines — `parse_di_flag` and
        // `parse_disp_flag` raise the message themselves, at the token — but
        // kept so the mapping stays complete for the kind it names.
        MetadataFieldKind::DiFlags | MetadataFieldKind::DispFlags => "debug info flag",
        MetadataFieldKind::EmissionKind => "emission kind",
        MetadataFieldKind::NameTableKind => "nameTable kind",
        MetadataFieldKind::FixedPointKind => "fixed-point kind",
        MetadataFieldKind::Metadata { .. }
        | MetadataFieldKind::MetadataString { .. }
        | MetadataFieldKind::MetadataList
        | MetadataFieldKind::SignedOrMetadata
        | MetadataFieldKind::UnsignedOrMetadata { .. }
        | MetadataFieldKind::ChecksumKind => "metadata field value",
    }
}

/// `DICompileUnit::DebugEmissionKind` spellings
/// (`DebugInfoMetadata.h`; parsed by `LLParser`'s `EmissionKindField`).
fn emission_kind(spelling: &str) -> Option<u32> {
    match spelling {
        "NoDebug" => Some(0),
        "FullDebug" => Some(1),
        "LineTablesOnly" => Some(2),
        "DebugDirectivesOnly" => Some(3),
        _ => None,
    }
}

/// `DICompileUnit::DebugNameTableKind` spellings (`DebugInfoMetadata.h`).
fn name_table_kind(spelling: &str) -> Option<u32> {
    match spelling {
        "Default" => Some(0),
        "GNU" => Some(1),
        "Apple" => Some(2),
        "None" => Some(3),
        _ => None,
    }
}

/// `DIFile::ChecksumKind` spellings (`DebugInfoMetadata.h`). `CSK_None` is
/// deliberately absent: upstream reserves encoding 0 for bitcode compatibility
/// and models "no checksum" as an absent field.
fn checksum_kind(spelling: &str) -> Option<u32> {
    match spelling {
        "CSK_MD5" => Some(1),
        "CSK_SHA1" => Some(2),
        "CSK_SHA256" => Some(3),
        _ => None,
    }
}

/// `DIFixedPointType::FixedPointKind` spellings (`DebugInfoMetadata.h`:
/// `FixedPointBinary` / `FixedPointDecimal` / `FixedPointRational`, spelled
/// without the prefix in `.ll` — the same three words `LLLexer` accepts for
/// `lltok::FixedPointKind`).
fn fixed_point_kind(spelling: &str) -> Option<u32> {
    match spelling {
        "Binary" => Some(0),
        "Decimal" => Some(1),
        "Rational" => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llvmkit_ir::{Module, module_new};

    fn parse(src: &str) -> ParseResult<()> {
        let m = Module::dynamic("parse_test");
        let p = Parser::new(src.as_bytes(), &m)?;
        let _ = p.parse_module()?;
        Ok(())
    }

    /// Mirrors `test/Assembler/datalayout.ll` — the parser accepts the
    /// `target datalayout = "..."` directive and the module retains it.
    #[test]
    fn parses_target_datalayout() {
        let src = "target datalayout = \"e-m:e-i64:64\"\n";
        let m = module_new!("dl").expect("fresh module");
        Parser::new(src.as_bytes(), &m)
            .unwrap()
            .parse_module()
            .unwrap();
        let dl = m.data_layout();
        assert!(dl.is_little_endian());
    }

    /// Mirrors `test/Assembler/target-triple.ll` — `target triple = "..."`.
    #[test]
    fn parses_target_triple() {
        let src = "target triple = \"x86_64-pc-linux-gnu\"\n";
        let m = module_new!("triple").expect("fresh module");
        Parser::new(src.as_bytes(), &m)
            .unwrap()
            .parse_module()
            .unwrap();
        assert_eq!(m.target_triple().as_deref(), Some("x86_64-pc-linux-gnu"));
    }

    /// Mirrors the `module asm` arm of `test/Assembler/module-asm.ll`.
    #[test]
    fn parses_module_asm() {
        let src = "module asm \"hello\"\nmodule asm \"world\"\n";
        let m = module_new!("masm").expect("fresh module");
        Parser::new(src.as_bytes(), &m)
            .unwrap()
            .parse_module()
            .unwrap();
        let asm = m.module_asm();
        assert!(asm.contains("hello"));
        assert!(asm.contains("world"));
    }

    /// Mirrors `test/Assembler/named-types.ll` shape: a named struct
    /// definition followed by a forward reference is round-trip stable.
    #[test]
    fn parses_named_struct_definition() {
        parse("%foo = type { i32, i64 }\n").unwrap();
    }

    /// Mirrors `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
    /// SlotMappingTest)` shape: a forward-referenced numbered type closes
    /// cleanly.
    #[test]
    fn parses_numbered_struct_definition() {
        parse("%0 = type { i32, i32, i32, i32 }\n").unwrap();
    }

    /// Mirrors `test/Assembler/global-variable-attributes.ll` — simple
    /// integer global with explicit linkage.
    #[test]
    fn parses_simple_global_int() {
        parse("@x = global i32 42\n").unwrap();
    }

    /// Mirrors `test/Assembler/global-constant.ll`.
    #[test]
    fn parses_simple_global_constant() {
        parse("@y = constant i32 -7\n").unwrap();
    }

    /// Mirrors `test/Assembler/declare.ll` — simplest function declaration.
    #[test]
    fn parses_function_declaration() {
        parse("declare i32 @add(i32, i32)\n").unwrap();
    }

    /// Mirrors `test/Assembler/declare-variadic.ll` — variadic declaration.
    #[test]
    fn parses_variadic_declaration() {
        parse("declare i32 @printf(ptr, ...)\n").unwrap();
    }

    /// Mirrors `test/Assembler/source-filename.ll` — directive parses
    /// successfully even though the IR module does not yet model the slot.
    #[test]
    fn parses_source_filename_directive() {
        parse("source_filename = \"a.c\"\n").unwrap();
    }
}
