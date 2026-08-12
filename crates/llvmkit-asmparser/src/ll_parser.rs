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
use llvmkit_ir::metadata::{
    DebugMetadataOperand, DebugRecord, MetadataAttachmentKind, MetadataFieldValue, MetadataId,
    MetadataKind,
};
use std::collections::BTreeMap;
use std::collections::HashMap;

use llvmkit_ir::{
    Align, AnyTypeEnum, ApFloat, ApFloatSemantics, ApInt, AtomicOrdering, AtomicRmwBinOp,
    CallingConv, Constant, ConstantExprFlags, ConstantExprInRange, ConstantExprOpcode,
    ConstantExprOptions, DllStorageClass, Dyn, FastMathFlags, FloatDyn, FloatPredicate, FloatType,
    FpClassTest, GepNoWrapFlags, IntCastFlags, IntDyn, IntType, IntValue, IntrinsicNameResolution,
    IrBuilder, IrError, IrResult, Linkage, MaybeAlign, Module, ModuleBrand, NoFolder, PointerValue,
    Positioned, RoundingMode, SelectionKind, ShuffleMaskElem, Signedness, StructType, SyncScope,
    ThreadLocalMode, Type, TypeKind, UiToFpFlags, UnnamedAddr, Unverified, UseListOrderBbRecord,
    UseListOrderRecord, Visibility, derived_types::PointerType, resolve_intrinsic_name,
    shufflevector_mask_from_constant,
};
use llvmkit_macros::Branded;
use llvmkit_support::{Span, Spanned};

use super::asm_parser_context::AsmParserContext;
use super::ll_lexer::{LexError, Lexer};
use super::ll_token::Opcode;
use super::ll_token::{IntLit, Keyword, NumBase, PrimitiveTy, Sign, Token};
use super::module_summary::ModuleSummaryIndex;
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

type ParsedGlobalInitializer<'ctx, B> = (
    Option<Constant<'ctx, B>>,
    Option<DeferredConstantKind<'ctx, B>>,
);

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
    section: Option<String>,
    partition: Option<String>,
    comdat: Option<Option<String>>,
    align: MaybeAlign,
    gc: Option<String>,
    prefix_data: Option<llvmkit_ir::Constant<'ctx, B>>,
    prologue_data: Option<llvmkit_ir::Constant<'ctx, B>>,
    personality_fn: Option<ParsedPersonalityFn<'ctx, B>>,
    metadata: Vec<(MetadataAttachmentKind, MetadataId<B>)>,
    _marker: core::marker::PhantomData<&'ctx ()>,
}

impl<'ctx, B: ModuleBrand + 'ctx> Default for FunctionSuffix<'ctx, B> {
    fn default() -> Self {
        Self {
            attr_groups: Vec::new(),
            section: None,
            partition: None,
            comdat: None,
            align: MaybeAlign::NONE,
            gc: None,
            prefix_data: None,
            prologue_data: None,
            personality_fn: None,
            metadata: Vec::new(),
            _marker: PhantomData,
        }
    }
}

enum ParsedPersonalityFn<'ctx, B: ModuleBrand> {
    Resolved(llvmkit_ir::Constant<'ctx, B>),
    ForwardName { name: String, loc: Span },
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
    deferred_global_initializers: Vec<DeferredGlobalInitializer<'ctx, B>>,
    deferred_block_addresses: Vec<DeferredBlockAddress<'ctx, B>>,
    deferred_personality_fns: Vec<DeferredPersonalityFn<'ctx, B>>,
    deferred_alias_targets: Vec<DeferredAliasTarget<'ctx, B>>,
    deferred_intrinsic_attribute_checks: Vec<DeferredIntrinsicAttributeCheck>,
    forward_function_decls: HashMap<String, Span>,
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
    _brand: PhantomData<B>,
}

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
#[derive(Branded)]
#[branded(Debug, Default)]
pub struct ParsedModule<'ctx, B: ModuleBrand> {
    pub slot_mapping: SlotMapping<'ctx, B>,
    pub summary_index: Option<ModuleSummaryIndex>,
}

enum DeferredConstantKind<'ctx, B: ModuleBrand> {
    RawInitializer { ty: Type<'ctx, B>, span: Span },
}

struct DeferredGlobalInitializer<'ctx, B: ModuleBrand> {
    global: llvmkit_ir::GlobalVariable<'ctx, B>,
    value: DeferredConstantKind<'ctx, B>,
}

struct DeferredBlockAddress<'ctx, B: ModuleBrand> {
    placeholder: llvmkit_ir::ForwardRefValue<'ctx, B>,
    function: DeferredBlockAddressFunction<'ctx, B>,
    label: BlockLabel,
    loc: Span,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedSign {
    Positive,
    Negative,
}

#[derive(Debug, Clone)]
enum ParsedApsInt {
    SignedMagnitude {
        sign: ParsedSign,
        magnitude: ApInt,
    },
    Hex {
        signedness: Signedness,
        value: ApInt,
    },
}

#[derive(Debug, Clone, Copy)]
enum ExpectedIntWidth {
    Infer,
    Bits(u32),
}

#[derive(Branded)]
#[branded(Debug)]
enum ValId<'ctx, B: ModuleBrand> {
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
}

fn inferred_decimal_bits(digits: &str) -> u32 {
    let digit_count = u32::try_from(digits.len()).unwrap_or(u32::MAX / 4);
    digit_count.saturating_mul(4).max(1)
}

fn inferred_hex_bits(digits: &str) -> u32 {
    let digit_count = u32::try_from(digits.len()).unwrap_or(u32::MAX / 4);
    digit_count.saturating_mul(4).max(1)
}

fn lower_parsed_apsint(parsed: &ParsedApsInt, dest_width: u32) -> ApInt {
    match parsed {
        ParsedApsInt::SignedMagnitude { sign, magnitude } => {
            let magnitude = magnitude.zext_or_trunc(dest_width);
            if matches!(sign, ParsedSign::Negative) {
                magnitude.negate()
            } else {
                magnitude
            }
        }
        ParsedApsInt::Hex { signedness, value } => match signedness {
            Signedness::Unsigned => value.zext_or_trunc(dest_width),
            Signedness::Signed => value.sext_or_trunc(dest_width),
        },
    }
}

fn parsed_apsint_to_i128(parsed: &ParsedApsInt) -> Option<i128> {
    match parsed {
        ParsedApsInt::SignedMagnitude { sign, magnitude } => {
            let value = magnitude.try_zext_u128()?;
            let signed = i128::try_from(value).ok()?;
            Some(if matches!(sign, ParsedSign::Negative) {
                -signed
            } else {
                signed
            })
        }
        ParsedApsInt::Hex { signedness, value } => match signedness {
            Signedness::Unsigned => i128::try_from(value.try_zext_u128()?).ok(),
            Signedness::Signed => value.try_sext_i128(),
        },
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

fn is_declaration_linkage(linkage: Linkage) -> bool {
    matches!(linkage, Linkage::External | Linkage::ExternalWeak)
}

fn keyword_starts_top_level_entity(keyword: Keyword) -> bool {
    matches!(
        keyword,
        Keyword::Target
            | Keyword::SourceFilename
            | Keyword::Module
            | Keyword::Uselistorder
            | Keyword::UselistorderBb
            | Keyword::Declare
            | Keyword::Define
            | Keyword::Attributes
    )
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
    in_range: Option<(ParsedInRangeBound, ParsedInRangeBound)>,
}

#[derive(Debug, Clone)]
enum ParsedInRangeBound {
    SignedMagnitude {
        negative: bool,
        magnitude_words: Box<[u64]>,
    },
    HexApsInt {
        signed: bool,
        words: Box<[u64]>,
        bit_width: u32,
    },
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

fn inrange_bound_to_apint_words(bound: &ParsedInRangeBound, bit_width: u32) -> Box<[u64]> {
    match bound {
        ParsedInRangeBound::SignedMagnitude {
            negative,
            magnitude_words,
        } => signed_magnitude_to_apint_words(*negative, magnitude_words, bit_width),
        ParsedInRangeBound::HexApsInt {
            signed,
            words,
            bit_width: source_bit_width,
        } => apsint_to_apint_words(*signed, words, *source_bit_width, bit_width),
    }
}

fn signed_magnitude_to_apint_words(
    negative: bool,
    magnitude_words: &[u64],
    bit_width: u32,
) -> Box<[u64]> {
    let word_count = usize::try_from(bit_width.div_ceil(64)).unwrap_or(0);
    let mut words = vec![0; word_count];
    let copy_count = words.len().min(magnitude_words.len());
    words[..copy_count].copy_from_slice(&magnitude_words[..copy_count]);
    mask_apint_top_word(&mut words, bit_width);
    if negative {
        negate_apint_words(&mut words, bit_width);
    }
    words.into_boxed_slice()
}

fn apsint_to_apint_words(
    signed: bool,
    source_words: &[u64],
    source_bit_width: u32,
    bit_width: u32,
) -> Box<[u64]> {
    let word_count = usize::try_from(bit_width.div_ceil(64)).unwrap_or(0);
    let negative = signed && apint_sign_bit(source_words, source_bit_width);
    let fill = if negative { u64::MAX } else { 0 };
    let mut words = vec![fill; word_count];
    let copy_count = words.len().min(source_words.len());
    words[..copy_count].copy_from_slice(&source_words[..copy_count]);
    if negative && source_bit_width < bit_width {
        sign_extend_apint_words(&mut words, source_bit_width);
    }
    mask_apint_top_word(&mut words, bit_width);
    words.into_boxed_slice()
}

fn sign_extend_apint_words(words: &mut [u64], source_bit_width: u32) {
    let start_word = usize::try_from(source_bit_width / 64).unwrap_or(usize::MAX);
    if start_word >= words.len() {
        return;
    }
    let start_bit = source_bit_width % 64;
    if start_bit == 0 {
        for word in &mut words[start_word..] {
            *word = u64::MAX;
        }
    } else {
        words[start_word] |= u64::MAX << start_bit;
        for word in &mut words[start_word + 1..] {
            *word = u64::MAX;
        }
    }
}

fn negate_apint_words(words: &mut [u64], bit_width: u32) {
    for word in words.iter_mut() {
        *word = !*word;
    }
    mask_apint_top_word(words, bit_width);
    let mut carry = true;
    for word in words.iter_mut() {
        if !carry {
            break;
        }
        let (next, overflowed) = word.overflowing_add(1);
        *word = next;
        carry = overflowed;
    }
    mask_apint_top_word(words, bit_width);
}

fn mask_apint_top_word(words: &mut [u64], bit_width: u32) {
    let top_bits = bit_width % 64;
    if top_bits != 0
        && let Some(top) = words.last_mut()
    {
        *top &= (1u64 << top_bits) - 1;
    }
}

fn constant_expr_inrange_is_non_empty(range: &ConstantExprInRange) -> bool {
    signed_apint_cmp(range.start(), range.end(), range.bit_width()).is_lt()
}

fn signed_apint_cmp(lhs: &[u64], rhs: &[u64], bit_width: u32) -> core::cmp::Ordering {
    let lhs_negative = apint_sign_bit(lhs, bit_width);
    let rhs_negative = apint_sign_bit(rhs, bit_width);
    match (lhs_negative, rhs_negative) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => unsigned_apint_cmp(lhs, rhs, bit_width),
    }
}

fn apint_sign_bit(words: &[u64], bit_width: u32) -> bool {
    if bit_width == 0 {
        return false;
    }
    let bit_index = bit_width - 1;
    let word_index = usize::try_from(bit_index / 64).unwrap_or(usize::MAX);
    let bit_in_word = bit_index % 64;
    words
        .get(word_index)
        .is_some_and(|word| ((word >> bit_in_word) & 1) != 0)
}

fn unsigned_apint_cmp(lhs: &[u64], rhs: &[u64], bit_width: u32) -> core::cmp::Ordering {
    let word_count = usize::try_from(bit_width.div_ceil(64)).unwrap_or(0);
    for idx in (0..word_count).rev() {
        let lhs_word = apint_word(lhs, idx, bit_width);
        let rhs_word = apint_word(rhs, idx, bit_width);
        match lhs_word.cmp(&rhs_word) {
            core::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    core::cmp::Ordering::Equal
}

fn decimal_digits_to_words(digits: &str) -> Option<Box<[u64]>> {
    let mut words = vec![0u64];
    for byte in digits.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        mul_add_words(&mut words, 10, u64::from(byte - b'0'));
    }
    while words.len() > 1 && words.last().copied() == Some(0) {
        words.pop();
    }
    Some(words.into_boxed_slice())
}

fn hex_digits_to_words(digits: &str) -> Option<Box<[u64]>> {
    let mut words = vec![0u64];
    for byte in digits.bytes() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        mul_add_words(&mut words, 16, u64::from(digit));
    }
    while words.len() > 1 && words.last().copied() == Some(0) {
        words.pop();
    }
    Some(words.into_boxed_slice())
}

fn hex_apsint_bit_width(digits: &str, words: &[u64]) -> Option<u32> {
    let syntactic_bits = u32::try_from(digits.len()).ok()?.checked_mul(4)?;
    let active_bits = apint_active_bits(words)?;
    if active_bits > 0 && active_bits < syntactic_bits {
        Some(active_bits)
    } else {
        Some(syntactic_bits)
    }
}

fn apint_active_bits(words: &[u64]) -> Option<u32> {
    for (idx, word) in words.iter().enumerate().rev() {
        if *word != 0 {
            let word_base = u32::try_from(idx).ok()?.checked_mul(64)?;
            return word_base.checked_add(64 - word.leading_zeros());
        }
    }
    Some(0)
}

fn mul_add_words(words: &mut Vec<u64>, multiplier: u64, addend: u64) {
    let mut carry = u128::from(addend);
    for word in words.iter_mut() {
        let value = u128::from(*word) * u128::from(multiplier) + carry;
        *word = low_u64(value);
        carry = value >> 64;
    }
    while carry != 0 {
        words.push(low_u64(carry));
        carry >>= 64;
    }
}

fn low_u64(value: u128) -> u64 {
    let bytes = value.to_le_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

fn apint_word(words: &[u64], idx: usize, bit_width: u32) -> u64 {
    let mut word = words.get(idx).copied().unwrap_or(0);
    let word_count = usize::try_from(bit_width.div_ceil(64)).unwrap_or(0);
    if word_count != 0 && idx + 1 == word_count {
        let top_bits = bit_width % 64;
        if top_bits != 0 {
            word &= (1u64 << top_bits) - 1;
        }
    }
    word
}

fn is_valid_extractelement<'ctx, B: ModuleBrand + 'ctx>(
    result_ty: Type<'ctx, B>,
    vector_ty: Type<'ctx, B>,
    index_ty: Type<'ctx, B>,
) -> bool {
    let AnyTypeEnum::Vector(vector_ty) = AnyTypeEnum::from(vector_ty) else {
        return false;
    };
    vector_ty.element() == result_ty && index_ty.is_integer()
}

fn is_valid_insertelement<'ctx, B: ModuleBrand + 'ctx>(
    result_ty: Type<'ctx, B>,
    vector_ty: Type<'ctx, B>,
    value_ty: Type<'ctx, B>,
    index_ty: Type<'ctx, B>,
) -> bool {
    let AnyTypeEnum::Vector(vector_ty) = AnyTypeEnum::from(vector_ty) else {
        return false;
    };
    vector_ty.as_type() == result_ty && vector_ty.element() == value_ty && index_ty.is_integer()
}

fn is_valid_shufflevector<'ctx, B: ModuleBrand + 'ctx>(
    result_ty: Type<'ctx, B>,
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
    let AnyTypeEnum::Vector(result_ty) = AnyTypeEnum::from(result_ty) else {
        return false;
    };
    lhs_ty.element() == rhs_ty.element()
        && lhs_ty.min_len() == rhs_ty.min_len()
        && lhs_ty.is_scalable() == rhs_ty.is_scalable()
        && matches!(mask_ty.element().kind(), TypeKind::Integer { bits: 32 })
        && result_ty.element() == lhs_ty.element()
        && result_ty.min_len() == mask_ty.min_len()
        && result_ty.is_scalable() == mask_ty.is_scalable()
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
            module,
            named_types: HashMap::new(),
            numbered_types: HashMap::new(),
            next_unnamed_type_id: 0,
            numbered_globals: NumberedValues::new(),
            numbered_attr_groups: NumberedValues::new(),
            deferred_block_addresses: Vec::new(),
            metadata_slots: HashMap::new(),
            deferred_global_initializers: Vec::new(),
            deferred_personality_fns: Vec::new(),
            deferred_alias_targets: Vec::new(),
            deferred_intrinsic_attribute_checks: Vec::new(),
            forward_function_decls: HashMap::new(),
            forward_ref_comdats: BTreeMap::new(),
            forward_ref_globals: BTreeMap::new(),
            forward_ref_global_ids: BTreeMap::new(),
            _brand: PhantomData,
        })
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

    pub fn with_context(
        src: &'src [u8],
        module: &'ctx Module<B, Unverified>,
        _context: &'ctx mut AsmParserContext<'ctx, B>,
    ) -> ParseResult<Self> {
        Self::new(src, module)
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
                return Err(ParseError::Redefinition {
                    kind: SymbolKind::Metadata,
                    id: SymbolId::Numbered(slot),
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

    /// Drive the parser to EOF. Mirrors `LLParser::Run` over the
    /// constructive subset modeled today.
    pub fn parse_module(mut self) -> ParseResult<ParsedModule<'ctx, B>> {
        // Upstream splits `parseTargetDefinitions` from `parseTopLevelEntities`
        // because LLVM 22 wants a chance to apply a default DataLayout
        // *before* anything that depends on it. We don't ship that callback
        // path yet; the dispatch loop below handles `target` keywords as a
        // top-level entity directly.
        loop {
            match self.current.value {
                Token::Eof => break,
                Token::Kw(Keyword::Target) => self.parse_target_definition()?,
                Token::Kw(Keyword::SourceFilename) => self.parse_source_filename()?,
                Token::Kw(Keyword::Module) => self.parse_module_asm()?,
                Token::Kw(Keyword::Uselistorder) => self.parse_module_use_list_order()?,
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
                _ => return Err(self.token_error("top-level entity")),
            }
        }

        self.validate_forward_ref_types()?;

        for (slot, entry) in &self.metadata_slots {
            if !entry.defined {
                return Err(ParseError::UndefinedSymbol {
                    kind: SymbolKind::Metadata,
                    id: SymbolId::Numbered(*slot),
                    loc: DiagLoc::span(entry.first_ref),
                });
            }
        }

        self.resolve_deferred_global_initializers()?;
        self.resolve_deferred_block_addresses()?;
        self.resolve_deferred_personality_fns()?;
        self.resolve_deferred_alias_targets()?;
        self.validate_deferred_intrinsic_attribute_checks()?;
        self.validate_forward_ref_comdats()?;
        self.resolve_forward_ref_globals()?;
        self.validate_forward_function_decls()?;

        Ok(ParsedModule {
            slot_mapping: self.into_slot_mapping(),
            summary_index: None,
        })
    }

    fn resolve_deferred_global_initializers(&mut self) -> ParseResult<()> {
        let deferred = std::mem::take(&mut self.deferred_global_initializers);
        let slots = self.slot_mapping_snapshot();
        for item in deferred {
            let constant = match item.value {
                DeferredConstantKind::RawInitializer { ty, span } => {
                    let start = usize::try_from(span.start)
                        .map_err(|_| self.expected("constant initializer"))?;
                    let end = usize::try_from(span.end)
                        .map_err(|_| self.expected("constant initializer"))?;
                    let bytes = self
                        .src
                        .get(start..end)
                        .ok_or_else(|| self.expected("constant initializer"))?;
                    crate::parser::parse_constant_value_with_slots(bytes, self.module, ty, &slots)
                        .map_err(|err| match err {
                        ParseError::Expected { expected, .. } => ParseError::Expected {
                            expected,
                            loc: DiagLoc::span(span),
                        },
                        other => other,
                    })?
                }
            };
            item.global
                .set_initializer(self.module, constant)
                .map_err(|e| self.builder_err("deferred global initializer", e))?;
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
                    loc: DiagLoc::span(item.loc),
                });
            };
            let block = state.value_as_block_view(block, item.loc)?;
            let address = self
                .module
                .block_address(state.func, &block)
                .map_err(|e| self.builder_err("blockaddress", e))?;
            // Upstream never defers this: `BlockAddressPFS->getBB` forward-
            // declares the block, so `BlockAddress::get` is built at once and
            // `convertValIDToValue` compares its type against the context's.
            // llvmkit resolves at function close instead, but the comparison
            // is the same one — the placeholder was minted at exactly the type
            // the context asked for — so the diagnostic is upstream's.
            let expected = item.placeholder.ty();
            let got = address.ty();
            if got != expected {
                return Err(ParseError::Message {
                    message: format!(
                        "constant expression type mismatch: got type '{got}' but expected '{expected}'"
                    )
                    .into(),
                    loc: DiagLoc::span(item.loc),
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
                loc: DiagLoc::span(item.loc),
            });
        }
        Ok(())
    }

    fn resolve_deferred_alias_targets(&mut self) -> ParseResult<()> {
        let deferred = std::mem::take(&mut self.deferred_alias_targets);
        // These resolve after the module is parsed; the referent, if it
        // exists, is a plain address-space-0 pointer.
        let ptr_ty = self.module.ptr_type(0).as_type();
        for item in deferred {
            let target = self
                .resolve_global_name_as_constant(item.name.clone(), ptr_ty)
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
        // These resolve after the module is parsed; the referent, if it
        // exists, is a plain address-space-0 pointer.
        let ptr_ty = self.module.ptr_type(0).as_type();
        for item in deferred {
            let personality = self
                .resolve_global_name_as_constant(item.name.clone(), ptr_ty)
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
    fn resolve_forward_ref_globals(&mut self) -> ParseResult<()> {
        let named = core::mem::take(&mut self.forward_ref_globals);
        for (name, entry) in named {
            let Ok(target) = self.resolve_global_name_as_ref(name.clone()) else {
                return Err(ParseError::UndefinedSymbol {
                    kind: SymbolKind::GlobalValue,
                    id: SymbolId::Named(name),
                    loc: DiagLoc::span(entry.loc),
                });
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
        if let Some(name) = name
            && let Some(entry) = self.forward_ref_globals.get(name)
        {
            return Ok(entry.placeholder.as_constant());
        }
        if let Some(id) = id
            && let Some(entry) = self.forward_ref_global_ids.get(&id)
        {
            return Ok(entry.placeholder.as_constant());
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

    fn validate_forward_function_decls(&self) -> ParseResult<()> {
        if let Some((name, loc)) = self.forward_function_decls.iter().next() {
            return Err(ParseError::UndefinedSymbol {
                kind: SymbolKind::Global,
                id: SymbolId::Named(name.clone()),
                loc: DiagLoc::span(*loc),
            });
        }
        Ok(())
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

    fn slot_mapping_snapshot(&self) -> SlotMapping<'ctx, B> {
        let mut named_types = HashMap::with_capacity(self.named_types.len());
        for (name, entry) in &self.named_types {
            named_types.insert(name.clone(), entry.ty);
        }
        let mut numbered_types = std::collections::BTreeMap::new();
        for (id, entry) in &self.numbered_types {
            numbered_types.insert(*id, entry.ty);
        }
        let mut metadata_nodes = NumberedValues::new();
        let mut metadata_entries: Vec<_> = self
            .metadata_slots
            .iter()
            .filter(|(_, entry)| entry.defined)
            .collect();
        metadata_entries.sort_by_key(|(slot, _)| *slot);
        for (slot, entry) in metadata_entries {
            let _ = metadata_nodes.add(*slot, entry.id);
        }
        let mut attribute_groups = NumberedValues::new();
        let mut attr_entries: Vec<_> = self.module.attribute_groups().collect();
        attr_entries.sort_by_key(|(slot, _)| *slot);
        for (slot, storage) in attr_entries {
            let _ = attribute_groups.add(slot, storage);
        }
        SlotMapping {
            global_values: self.numbered_globals.clone(),
            named_types,
            numbered_types,
            attribute_groups,
            metadata_nodes,
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
        self.current = self.lex.next_token().map_err(map_lex_error)?;
        Ok(prev)
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
        let scalar_start = matches!(
            self.peek(),
            Token::IntegerLit(_)
                | Token::FloatLit(_)
                | Token::GlobalVar(_)
                | Token::GlobalId(_)
                | Token::Kw(
                    Keyword::True
                        | Keyword::False
                        | Keyword::Null
                        | Keyword::Zeroinitializer
                        | Keyword::Undef
                        | Keyword::Poison
                )
        );
        let value = match self.parse_global_value(ty) {
            Ok(value) => value,
            Err(ParseError::Lex(_)) if scalar_start => {
                return Err(ParseError::Expected {
                    expected: "end of string".into(),
                    loc: DiagLoc::span(self.loc()),
                });
            }
            Err(err) => return Err(err),
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

    fn expect_primitive(&mut self, p: PrimitiveTy, expected: &'static str) -> ParseResult<Span> {
        if matches!(self.peek(), Token::PrimitiveType(got) if *got == p) {
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
    /// `addrspace ( <uint32> | "A" | "G" | "P" )`. Mirrors the inner
    /// `ParseAddrspaceValue` lambda of `LLParser::parseOptionalAddrSpace`.
    ///
    /// The three symbolic spellings resolve through the module's data layout,
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
                    _ => {
                        return Err(ParseError::Message {
                            message: format!("invalid symbolic addrspace '{name}'").into(),
                            loc: DiagLoc::span(self.loc()),
                        });
                    }
                };
                self.bump()?;
                resolved
            }
            Token::IntegerLit(_) => {
                let loc = self.loc();
                let n = self.parse_uint32("integer or string constant")?;
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

    fn parse_uint32(&mut self, expected: &'static str) -> ParseResult<u32> {
        let n = match self.peek() {
            Token::IntegerLit(IntLit {
                sign: Sign::Pos,
                base: NumBase::Dec,
                digits,
            }) => digits.parse::<u32>().ok(),
            _ => None,
        };
        match n {
            Some(n) => {
                self.bump()?;
                Ok(n)
            }
            None => Err(self.expected(expected)),
        }
    }

    fn parse_uint64(&mut self, expected: &'static str) -> ParseResult<u64> {
        let n = match self.peek() {
            Token::IntegerLit(IntLit {
                sign: Sign::Pos,
                base: NumBase::Dec,
                digits,
            }) => digits.parse::<u64>().ok(),
            _ => None,
        };
        match n {
            Some(n) => {
                self.bump()?;
                Ok(n)
            }
            None => Err(self.expected(expected)),
        }
    }

    fn parse_int_literal(&mut self, expected_width: ExpectedIntWidth) -> ParseResult<ParsedApsInt> {
        let lit = match self.peek() {
            Token::IntegerLit(lit) => *lit,
            _ => return Err(self.expected("integer literal")),
        };
        let parsed = match lit.base {
            NumBase::Dec => {
                let width = match expected_width {
                    ExpectedIntWidth::Bits(bits) => bits,
                    ExpectedIntWidth::Infer => inferred_decimal_bits(lit.digits),
                };
                let magnitude = ApInt::from_string(width, lit.digits, 10)
                    .map_err(|_| self.expected("valid integer literal"))?;
                let sign = if matches!(lit.sign, Sign::Neg) {
                    ParsedSign::Negative
                } else {
                    ParsedSign::Positive
                };
                ParsedApsInt::SignedMagnitude { sign, magnitude }
            }
            NumBase::HexSigned | NumBase::HexUnsigned => {
                let width = match expected_width {
                    ExpectedIntWidth::Bits(bits) => bits,
                    ExpectedIntWidth::Infer => inferred_hex_bits(lit.digits),
                };
                let value = ApInt::from_string(width, lit.digits, 16)
                    .map_err(|_| self.expected("valid hexadecimal integer literal"))?;
                let signedness = if matches!(lit.base, NumBase::HexSigned) {
                    Signedness::Signed
                } else {
                    Signedness::Unsigned
                };
                ParsedApsInt::Hex { signedness, value }
            }
        };
        self.bump()?;
        Ok(parsed)
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

    /// Parse `align N`. Returns the alignment value.
    /// Mirrors `LLParser::parseAlignment` (LLParser.cpp ~6539).
    fn parse_align_val(&mut self) -> ParseResult<Align> {
        self.expect_keyword(Keyword::Align, "'align'")?;
        let n = self.parse_uint64("alignment (bytes)")?;
        Align::new(n).map_err(|_| ParseError::Expected {
            expected: format!("alignment must be non-zero power of two, got {n}").into(),
            loc: DiagLoc::span(self.loc()),
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
        self.bump()?;
        if matches!(self.peek(), Token::Kw(Keyword::Align)) {
            Ok(Some(self.parse_align_val()?))
        } else {
            self.lex = saved_lex;
            self.current = saved_current;
            Ok(None)
        }
    }

    /// Parse an atomic ordering keyword.
    /// Mirrors `LLParser::parseOrdering` (LLParser.cpp ~2810).
    fn parse_atomic_ordering(&mut self, expected: &'static str) -> ParseResult<AtomicOrdering> {
        let ord = match self.peek() {
            Token::Kw(Keyword::Unordered) => AtomicOrdering::Unordered,
            Token::Kw(Keyword::Monotonic) => AtomicOrdering::Monotonic,
            Token::Kw(Keyword::Acquire) => AtomicOrdering::Acquire,
            Token::Kw(Keyword::Release) => AtomicOrdering::Release,
            Token::Kw(Keyword::AcqRel) => AtomicOrdering::AcquireRelease,
            Token::Kw(Keyword::SeqCst) => AtomicOrdering::SequentiallyConsistent,
            _ => return Err(self.expected(expected)),
        };
        self.bump()?;
        Ok(ord)
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
        self.expect_punct(PunctKind::LParen, "'(' after syncscope")?;
        let name = self.parse_string_constant("sync scope name")?;
        self.expect_punct(PunctKind::RParen, "')' after sync scope")?;
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

    /// `target datalayout = STRING` / `target triple = STRING`. Mirrors
    /// `LLParser::parseTargetDefinition`.
    fn parse_target_definition(&mut self) -> ParseResult<()> {
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
                let loc = self.loc();
                let s = self.parse_string_constant("target-datalayout string constant")?;
                let parsed = DataLayout::parse(&s).map_err(|e| match e {
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
            _ => Err(self.expected("'triple' or 'datalayout' after 'target'")),
        }
    }

    /// `source_filename = STRING`. Upstream sets `Module::SourceFileName`;
    /// llvmkit-ir does not yet model that slot, so the directive is parsed
    /// and discarded here. The parser still rejects malformed forms.
    fn parse_source_filename(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::SourceFilename, "'source_filename'")?;
        self.expect_punct(PunctKind::Equal, "'=' after source_filename")?;
        let source_filename = self.parse_string_constant("source-filename string constant")?;
        self.module.set_source_filename(source_filename);
        Ok(())
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
        self.expect_punct(PunctKind::Equal, "'=' after comdat name")?;
        self.expect_keyword(Keyword::Comdat, "'comdat'")?;
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
            return Err(self.expected("comdat selection kind"));
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

    fn parse_use_list_order_indexes(&mut self) -> ParseResult<Box<[u32]>> {
        self.expect_punct(PunctKind::LBrace, "'{' before uselistorder indexes")?;
        let mut indexes = Vec::new();
        if !matches!(self.peek(), Token::RBrace) {
            loop {
                indexes.push(self.parse_uint32("uselistorder index")?);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RBrace, "'}' after uselistorder indexes")?;
        Ok(indexes.into_boxed_slice())
    }

    fn parse_use_list_order_directive(
        &mut self,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<UseListOrderRecord> {
        let loc = self.loc();
        self.expect_keyword(Keyword::Uselistorder, "'uselistorder'")?;
        let ty = self.parse_type(false)?;
        let val_id = self.parse_val_id(pfs, Some(ty))?;
        let value = self.convert_val_id_to_value(ty, val_id, pfs)?;
        self.expect_punct(PunctKind::Comma, "',' before uselistorder indexes")?;
        let indexes = self.parse_use_list_order_indexes()?;
        UseListOrderRecord::new(value.slot(), ty.id(), indexes).map_err(|e| match e {
            IrError::InvalidOperation { message } => ParseError::Expected {
                expected: message.into(),
                loc: DiagLoc::span(loc),
            },
            other => self.builder_err("uselistorder", other),
        })
    }

    fn parse_module_use_list_order(&mut self) -> ParseResult<()> {
        let loc = self.loc();
        let record = self.parse_use_list_order_directive(None)?;
        self.module
            .append_use_list_order(record)
            .map_err(|e| match e {
                IrError::InvalidOperation { message } => ParseError::Expected {
                    expected: message.into(),
                    loc: DiagLoc::span(loc),
                },
                other => self.builder_err("uselistorder", other),
            })
    }

    fn parse_function_use_list_order(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<()> {
        let loc = self.loc();
        let record = self.parse_use_list_order_directive(Some(state))?;
        state
            .func
            .append_use_list_order(record)
            .map_err(|e| match e {
                IrError::InvalidOperation { message } => ParseError::Expected {
                    expected: message.into(),
                    loc: DiagLoc::span(loc),
                },
                other => self.builder_err("uselistorder", other),
            })
    }

    fn parse_use_list_order_bb(&mut self) -> ParseResult<()> {
        let loc = self.loc();
        self.expect_keyword(Keyword::UselistorderBb, "'uselistorder_bb'")?;
        let function = match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("function name in uselistorder_bb"))?;
                self.bump()?;
                let fn_id =
                    self.module
                        .function_dyn(&name)
                        .ok_or_else(|| ParseError::UndefinedSymbol {
                            kind: SymbolKind::Global,
                            id: SymbolId::Named(name),
                            loc: DiagLoc::span(loc),
                        })?;
                self.module.view(fn_id)
            }
            Token::GlobalId(id) => {
                let id = *id;
                self.bump()?;
                self.resolve_global_id_as_function(id)?
            }
            _ => return Err(self.expected("function name in uselistorder_bb")),
        };
        self.expect_punct(PunctKind::Comma, "',' after uselistorder_bb function")?;
        let block = match self.peek() {
            Token::LocalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("basic block in uselistorder_bb"))?;
                self.bump()?;
                function
                    .basic_blocks()
                    .find(|bb| bb.name().as_deref() == Some(name.as_str()))
                    .ok_or_else(|| ParseError::UndefinedSymbol {
                        kind: SymbolKind::Block,
                        id: SymbolId::Named(name),
                        loc: DiagLoc::span(loc),
                    })?
            }
            Token::LocalVarId(id) => {
                let id = *id;
                self.bump()?;
                let mut next = 0u32;
                for arg in function.params() {
                    if arg.name().is_none() {
                        next = next.saturating_add(1);
                    }
                }
                let mut found = None;
                'blocks: for bb in function.basic_blocks() {
                    if bb.name().is_none() {
                        if next == id {
                            found = Some(bb);
                            break 'blocks;
                        }
                        next = next.saturating_add(1);
                    }
                    for inst in bb.instructions() {
                        if !inst.ty().is_void() && inst.name().is_none() {
                            next = next.saturating_add(1);
                        }
                    }
                }
                found.ok_or_else(|| ParseError::UndefinedSymbol {
                    kind: SymbolKind::Block,
                    id: SymbolId::Numbered(id),
                    loc: DiagLoc::span(loc),
                })?
            }
            _ => return Err(self.expected("basic block in uselistorder_bb")),
        };
        self.expect_punct(PunctKind::Comma, "',' before uselistorder_bb indexes")?;
        let indexes = self.parse_use_list_order_indexes()?;
        let record = UseListOrderBbRecord::new(
            function.as_erased().slot(),
            block.to_erased().slot(),
            indexes,
        )
        .map_err(|e| match e {
            IrError::InvalidOperation { message } => ParseError::Expected {
                expected: message.into(),
                loc: DiagLoc::span(loc),
            },
            other => self.builder_err("uselistorder_bb", other),
        })?;
        self.module
            .append_use_list_order_bb(record)
            .map_err(|e| match e {
                IrError::InvalidOperation { message } => ParseError::Expected {
                    expected: message.into(),
                    loc: DiagLoc::span(loc),
                },
                other => self.builder_err("uselistorder_bb", other),
            })
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
        let slot = self.parse_uint32("metadata slot number after '!'")?;
        self.expect_punct(PunctKind::Equal, "'=' after metadata id")?;
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
                    _ => Err(self.expected("metadata string or tuple")),
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

        self.expect_punct(PunctKind::Equal, "'=' after metadata name")?;

        // `!{ !N, !N, ... }`
        self.expect_exclaim("'!' before '{' in named metadata")?;
        self.expect_punct(PunctKind::LBrace, "'{' in named metadata")?;
        let named_metadata_id = self.module.get_or_insert_named_metadata(name);

        // Parse comma-separated `!N` operands
        if !matches!(self.peek(), Token::RBrace) {
            loop {
                self.expect_exclaim("'!' before metadata operand")?;
                let loc = self.loc();
                let slot = self.parse_uint32("metadata operand number")?;
                let id = self.resolve_md_slot(slot, loc);
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

        self.expect_punct(PunctKind::RBrace, "'}' closing named metadata")?;
        Ok(())
    }

    /// Parse a metadata node body into its content: `!"string"` or
    /// Parse a `metadata`-typed value operand. Mirrors
    /// `LLParser::parseMetadataAsValue` delegating to `parseMetadata`: slot
    /// refs (`!N`), inline tuples (`!{...}`), and MDStrings (`!"..."`) are
    /// all legal metadata values.
    fn parse_metadata_value_operand(&mut self) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        if matches!(self.peek(), Token::MetadataVar(_)) {
            let kind = self.parse_md_node_after_bang(false)?;
            let id = own_metadata(self.module.metadata_node(kind));
            return Ok(own_metadata(self.module.metadata_as_value(id)));
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
                self.expect_punct(PunctKind::RBrace, "'}' closing metadata tuple")?;
                own_metadata(self.module.metadata_tuple(operands))
            }
            Token::SpecializedMetadata(_) | Token::MetadataVar(_) => {
                let kind = self.parse_md_node_after_bang(false)?;
                own_metadata(self.module.metadata_node(kind))
            }
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32("metadata slot number after '!'")?;
                self.resolve_md_slot(slot, loc)
            }
        };
        Ok(own_metadata(self.module.metadata_as_value(id)))
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
                    Token::LBrace | Token::SpecializedMetadata(_) | Token::MetadataVar(_) => {
                        let kind = self.parse_md_node_after_bang(false)?;
                        Ok(own_metadata(self.module.metadata_node(kind)))
                    }
                    _ => {
                        let loc = self.loc();
                        let slot = self.parse_uint32("metadata attachment operand number")?;
                        Ok(self.resolve_md_slot(slot, loc))
                    }
                }
            }
            _ => Err(self.expected("metadata attachment operand")),
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

        if matches!(
            self.peek(),
            Token::PrimitiveType(_)
                | Token::LBrace
                | Token::Less
                | Token::LSquare
                | Token::LocalVar(_)
                | Token::LocalVarId(_)
        ) {
            let ty = self.parse_type(false)?;
            let constant = self
                .parse_constant(ty)?
                .ok_or_else(|| self.expected("typed metadata constant"))?;
            return Ok(own_metadata(self.module.metadata_constant(constant)));
        }

        self.expect_exclaim("'!' in metadata tuple operand")?;
        match self.peek() {
            Token::StringConstant(_) => {
                let s = self.parse_string_constant("metadata string operand")?;
                Ok(self.module.metadata_string(s))
            }
            Token::LBrace | Token::SpecializedMetadata(_) | Token::MetadataVar(_) => {
                let content = self.parse_md_node_after_bang(false)?;
                Ok(own_metadata(self.module.metadata_node(content)))
            }
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32("metadata operand number")?;
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
                self.expect_punct(PunctKind::RBrace, "'}' closing metadata tuple")?;
                Ok(llvmkit_ir::metadata::MetadataKind::Tuple { distinct, operands })
            }
            Token::SpecializedMetadata(name) => {
                let kind = llvmkit_ir::metadata::SpecializedMetadataKind::from_name(name)
                    .ok_or_else(|| self.expected("specialized metadata kind"))?;
                self.parse_specialized_metadata_body(kind, distinct)
            }
            Token::MetadataVar(bytes) => {
                let name = std::str::from_utf8(bytes.as_ref())
                    .map_err(|_| self.expected("specialized metadata kind"))?;
                let kind = llvmkit_ir::metadata::SpecializedMetadataKind::from_name(name)
                    .ok_or_else(|| self.expected("specialized metadata kind"))?;
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
            return Err(self.expected("'distinct', required for !DIAssignID()"));
        }
        self.bump()?;
        if kind == llvmkit_ir::metadata::SpecializedMetadataKind::DiExpression {
            return self.parse_di_expression_body(distinct);
        }
        self.expect_punct(PunctKind::LParen, "'(' in specialized metadata")?;
        let mut fields: Vec<llvmkit_ir::metadata::MetadataField<B>> = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                let field_loc = DiagLoc::span(self.loc());
                let field_name = match self.peek() {
                    Token::LabelStr(bytes) => std::str::from_utf8(bytes.as_ref())
                        .map_err(|_| self.expected("valid UTF-8 metadata field name"))?
                        .to_owned(),
                    _ => return Err(self.expected("metadata field name")),
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
                let value_loc = DiagLoc::span(self.loc());
                let value = self.parse_metadata_field_value()?;
                self.check_metadata_field_value(declared, &value, value_loc)?;
                fields.push(llvmkit_ir::metadata::MetadataField::new(field_name, value));
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        let closing_loc = DiagLoc::span(self.loc());
        self.expect_punct(PunctKind::RParen, "')' closing specialized metadata")?;
        for required in kind.required_fields() {
            if !fields.iter().any(|f| f.name() == required.name()) {
                return Err(ParseError::MissingRequiredMetadataField {
                    kind: kind.name(),
                    field: required.name(),
                    loc: closing_loc,
                });
            }
        }
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

        self.expect_punct(PunctKind::LParen, "'(' in specialized metadata")?;
        let mut operands: Vec<DwarfExpressionOperand> = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                let operand = match self.peek() {
                    // `DW_OP_*` and `DW_ATE_*` are the two keyword families
                    // upstream accepts here.
                    Token::DwarfOp(s) | Token::DwarfAttEncoding(s) => {
                        let name = (*s).to_owned();
                        self.bump()?;
                        DwarfExpressionOperand::Operation(name)
                    }
                    // Anything else must be an unsigned literal: upstream
                    // rejects a signed element with "expected unsigned
                    // integer", and one above `UINT64_MAX` with "element too
                    // large".
                    _ => DwarfExpressionOperand::Literal(
                        self.parse_uint64("unsigned integer in DIExpression")?,
                    ),
                };
                operands.push(operand);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' closing specialized metadata")?;
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
    fn check_metadata_field_value(
        &self,
        field: llvmkit_ir::metadata::SpecializedMetadataField,
        value: &MetadataFieldValue<B>,
        loc: DiagLoc,
    ) -> ParseResult<()> {
        use llvmkit_ir::dwarf;
        use llvmkit_ir::metadata::{MetadataFieldKind, MetadataFieldValue};

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

        // `DIFlag*` / `DISPFlag*` accept a `|`-joined disjunction; every term
        // must resolve, which is what upstream's per-term loop enforces.
        let flags = |what: &'static str, lookup: fn(&str) -> Option<u32>| -> ParseResult<()> {
            let MetadataFieldValue::Enum(spelling) = value else {
                return Ok(());
            };
            for term in spelling.split('|') {
                let term = term.trim();
                if !term.is_empty() && lookup(term).is_none() {
                    return Err(ParseError::InvalidMetadataFieldValue {
                        what,
                        value: term.to_owned(),
                        loc,
                    });
                }
            }
            Ok(())
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
                    return Err(self.expected("unsigned integer"));
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
                    return Ok(());
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
                    Err(self.expected("'true' or 'false'"))
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
            MetadataFieldKind::DiFlags => flags("debug info flag", dwarf::di_flag),
            MetadataFieldKind::DispFlags => flags("subprogram debug info flag", dwarf::disp_flag),
            MetadataFieldKind::EmissionKind => keyword("emission kind", emission_kind),
            MetadataFieldKind::NameTableKind => keyword("nameTable kind", name_table_kind),
            MetadataFieldKind::ChecksumKind => keyword("checksum kind", checksum_kind),
            MetadataFieldKind::FixedPointKind => keyword("fixed-point kind", fixed_point_kind),
            // `DwarfEnumKindField`'s table lives in `DICompositeType`'s Apple
            // enum-kind set rather than `Dwarf.def`; unmodelled, so unchecked.
            MetadataFieldKind::DwarfEnumKind
            | MetadataFieldKind::ApsInt
            | MetadataFieldKind::MetadataList
            | MetadataFieldKind::SignedOrMetadata => Ok(()),
        }
    }

    fn parse_metadata_field_value(&mut self) -> ParseResult<MetadataFieldValue<B>> {
        use llvmkit_ir::metadata::MetadataFieldValue;
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
                let parsed = self.parse_int_literal(ExpectedIntWidth::Infer)?;
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
                    Token::SpecializedMetadata(_) | Token::MetadataVar(_) => {
                        let content = self.parse_md_node_after_bang(false)?;
                        Ok(MetadataFieldValue::Metadata(own_metadata(
                            self.module.metadata_node(content),
                        )))
                    }
                    _ => {
                        let loc = self.loc();
                        let slot = self.parse_uint32("metadata field metadata reference")?;
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
            // `flags:` and `spFlags:` take a `|`-joined disjunction, which
            // upstream reads with a repeated `lltok::bar` loop in
            // `LLParser::parseMDField` for `MDFieldImpl<DIFlags>` /
            // `<DISPFlags>`. Kept here as the joined source text rather than a
            // bitmask: modelling `DINode::DIFlags` / `DISubprogram::DISPFlags`
            // as bitflags is deferred (see `docs/future-work.md`), and the
            // joined form is byte-for-byte what `AsmWriter.cpp`'s
            // `printDIFlags` emits, whose separator is `ListSeparator(" | ")`.
            Token::DiFlag(s) | Token::DiSpFlag(s) => {
                let mut value = (*s).to_owned();
                self.bump()?;
                while matches!(self.peek(), Token::Bar) {
                    self.bump()?;
                    let next = match self.peek() {
                        Token::DiFlag(s) | Token::DiSpFlag(s) => (*s).to_owned(),
                        _ => return Err(self.expected("debug info flag after '|'")),
                    };
                    self.bump()?;
                    value.push_str(" | ");
                    value.push_str(&next);
                }
                Ok(MetadataFieldValue::Enum(value))
            }
            _ => Err(self.expected("metadata field value")),
        }
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
    fn parse_debug_metadata_operand(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<DebugMetadataOperand<B>> {
        if matches!(
            self.peek(),
            Token::Exclaim | Token::SpecializedMetadata(_) | Token::MetadataVar(_)
        ) {
            let id = self.parse_metadata_attachment_operand()?;
            return Ok(DebugMetadataOperand::Metadata(id));
        }

        let ty = self.parse_type(false)?;
        let value = self.parse_value(state, ty)?;
        Ok(DebugMetadataOperand::Value(value.id()))
    }

    fn parse_debug_record(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<DebugRecord<B>> {
        use llvmkit_ir::metadata::{DebugRecord, DebugVariableRecord, DebugVariableRecordKind};

        let record_type = match self.peek() {
            Token::DbgRecordType(name) => *name,
            _ => return Err(self.expected("debug record type")),
        };
        self.bump()?;
        self.expect_punct(PunctKind::LParen, "'(' in debug record")?;

        if record_type == "label" {
            let label = self.parse_metadata_attachment_operand()?;
            self.expect_punct(PunctKind::Comma, "',' after debug label")?;
            let debug_loc = self.parse_metadata_attachment_operand()?;
            self.expect_punct(PunctKind::RParen, "')' closing debug record")?;
            return Ok(DebugRecord::Label { label, debug_loc });
        }

        let kind = match record_type {
            "declare" => DebugVariableRecordKind::Declare,
            "value" => DebugVariableRecordKind::Value,
            "assign" => DebugVariableRecordKind::Assign,
            "declare_value" => DebugVariableRecordKind::DeclareValue,
            _ => return Err(self.expected("known debug record type")),
        };

        let location = self.parse_debug_metadata_operand(state)?;
        self.expect_punct(PunctKind::Comma, "',' after debug location operand")?;
        let variable = self.parse_metadata_attachment_operand()?;
        self.expect_punct(PunctKind::Comma, "',' after debug variable")?;
        let expression = self.parse_metadata_attachment_operand()?;
        self.expect_punct(PunctKind::Comma, "',' after debug expression")?;

        let (assign_id, address_location, address_expression) =
            if kind == DebugVariableRecordKind::Assign {
                let assign_id = self.parse_metadata_attachment_operand()?;
                self.expect_punct(PunctKind::Comma, "',' after DIAssignID")?;
                let address_location = self.parse_debug_metadata_operand(state)?;
                self.expect_punct(PunctKind::Comma, "',' after debug address location")?;
                let address_expression = self.parse_metadata_attachment_operand()?;
                self.expect_punct(PunctKind::Comma, "',' after debug address expression")?;
                (
                    Some(assign_id),
                    Some(address_location),
                    Some(address_expression),
                )
            } else {
                (None, None, None)
            };

        let debug_loc = self.parse_metadata_attachment_operand()?;
        self.expect_punct(PunctKind::RParen, "')' closing debug record")?;
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

    fn finish_trailing_metadata(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        bb_value: llvmkit_ir::Value<'ctx, B>,
        pending_debug_records: &mut Vec<DebugRecord<B>>,
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
                _ => return Err(self.expected("metadata attachment")),
            };
            self.bump()?;
            let id = self.parse_metadata_attachment_operand()?;
            if let Some(inst) = bb.instructions().last() {
                own_metadata(inst.set_metadata(
                    self.module,
                    MetadataAttachmentKind::from_name(&name),
                    id,
                ));
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
        self.expect_punct(PunctKind::Equal, "'=' after type name")?;
        self.expect_keyword(Keyword::Type, "'type' after '='")?;
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
        self.expect_punct(PunctKind::Equal, "'=' after type id")?;
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
                        return Err(ParseError::Expected {
                            expected: "non-void type (void only allowed at function results)"
                                .into(),
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
                int_params.push(self.parse_uint32("target extension integer parameter")?);
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
        let n = self.parse_uint64("array or vector element count")?;
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

    /// `T (params...)` — mirrors `LLParser::parseFunctionType`. The opening
    /// `(` is the lookahead that triggered this arm.
    fn parse_function_type_after_return(
        &mut self,
        ret: Type<'ctx, B>,
    ) -> ParseResult<Type<'ctx, B>> {
        if !ret.is_valid_function_return() {
            return Err(self.message("invalid function return type"));
        }
        self.expect_punct(PunctKind::LParen, "'(' in function type")?;
        let mut params = Vec::new();
        let mut var_args = false;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    var_args = true;
                    break;
                }
                let arg_loc = self.loc();
                let param = self.parse_type(false)?;
                if !param.is_valid_function_argument() {
                    return Err(self.message_at(arg_loc, "invalid type for function argument"));
                }
                // Upstream shares `parseArgumentList` between a function
                // *type* and a function *header*, so attributes and a name
                // parse here and are rejected afterwards — which is why these
                // two messages exist at all. Reading them keeps the diagnostic
                // instead of a generic "expected ')'".
                let attrs = self.parse_optional_param_attrs()?;
                if !attrs.is_empty() {
                    return Err(
                        self.message_at(arg_loc, "argument attributes invalid in function type")
                    );
                }
                if matches!(self.peek(), Token::LocalVar(_) | Token::LocalVarId(_)) {
                    return Err(self.message_at(arg_loc, "argument name invalid in function type"));
                }
                params.push(param);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close function type")?;
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
                    return Err(self.expected("thread-local model"));
                };
                self.expect_punct(PunctKind::RParen, "')' after thread-local model")?;
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

        let is_constant = if self.eat_keyword(Keyword::Global)? {
            false
        } else if self.eat_keyword(Keyword::Constant)? {
            true
        } else {
            return Err(self.expected("'global' or 'constant' after linkage"));
        };

        let ty = self.parse_type(false)?;
        let (initializer, deferred_initializer) = if has_linkage && is_declaration_linkage(linkage)
        {
            // Upstream parses the initializer and then rejects it, so the
            // diagnostic names the actual problem. Detect the same case here:
            // anything that is not a property comma, a new top-level entity,
            // or end of input can only be an initializer.
            if self.starts_global_initializer() {
                // `External` is the default linkage, so its keyword is the
                // empty string; name it explicitly rather than printing ''.
                let spelled = match linkage.keyword() {
                    "" => "external",
                    other => other,
                };
                return Err(ParseError::Expected {
                    expected: format!(
                        "no initializer: a global with '{spelled}' linkage is a declaration"
                    )
                    .into(),
                    loc: DiagLoc::span(self.loc()),
                });
            }
            (None, None)
        } else {
            self.parse_global_initializer(ty)?
        };
        let mut section = None;
        let mut partition = None;
        let mut align = MaybeAlign::NONE;
        let mut comdat_name = None;
        let mut metadata = Vec::new();
        while self.eat_punct(PunctKind::Comma)? {
            if self.eat_keyword(Keyword::Section)? {
                section = Some(self.parse_string_constant("section name")?);
            } else if self.eat_keyword(Keyword::Partition)? {
                partition = Some(self.parse_string_constant("partition name")?);
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
                return Err(self.expected("global attribute"));
            }
        }

        let name_string = match &name_id {
            NameOrId::Name(n) => n.clone(),
            NameOrId::Id(_) => String::new(),
        };
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
        for (kind, id) in metadata {
            own_metadata(g.set_metadata(self.module, kind, id));
        }
        if let Some(value) = deferred_initializer {
            self.deferred_global_initializers
                .push(DeferredGlobalInitializer { global: g, value });
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
        if !is_alias && !llvmkit_ir::global_ifunc::is_valid_ifunc_linkage(linkage) {
            return Err(ParseError::Expected {
                expected: "invalid linkage type for ifunc".into(),
                loc: DiagLoc::span(decl_loc),
            });
        }
        if matches!(linkage, Linkage::Internal | Linkage::Private)
            && visibility != Visibility::Default
        {
            return Err(ParseError::Message {
                message: "symbol with local linkage must have default visibility".into(),
                loc: DiagLoc::span(decl_loc),
            });
        }
        if matches!(linkage, Linkage::Internal | Linkage::Private)
            && dll_storage_class != DllStorageClass::Default
        {
            return Err(ParseError::Message {
                message: "symbol with local linkage cannot have a DLL storage class".into(),
                loc: DiagLoc::span(decl_loc),
            });
        }

        let value_type = self.parse_type(false)?;
        self.expect_punct(PunctKind::Comma, "',' after the alias or ifunc type")?;
        let target_ty = self.parse_type(false)?;
        let target_loc = self.loc();
        match target_ty.into_type_enum() {
            AnyTypeEnum::Pointer(_) => {}
            _ => {
                return Err(ParseError::Expected {
                    expected: "pointer type for the alias or ifunc target".into(),
                    loc: DiagLoc::span(self.loc()),
                });
            }
        }
        // A forward-referenced target becomes a null placeholder patched at
        // end of module, exactly as `personality` already handles the same
        // ordering problem.
        let (target, forward_target) = match self.parse_alias_target(target_ty) {
            Ok(c) => (c, None),
            Err(ParseError::UndefinedSymbol {
                id: SymbolId::Named(name),
                ..
            }) => {
                let AnyTypeEnum::Pointer(pty) = target_ty.into_type_enum() else {
                    return Err(self.expected("pointer type for alias or ifunc target"));
                };
                (pty.const_null().as_constant(), Some(name))
            }
            Err(other) => return Err(other),
        };

        let mut partition = None;
        while self.eat_punct(PunctKind::Comma)? {
            if self.eat_keyword(Keyword::Partition)? {
                partition = Some(self.parse_string_constant("partition name")?);
            } else {
                return Err(self.expected("unknown alias or ifunc property"));
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
                .visibility(visibility);
            if let Some(p) = partition {
                builder = builder.partition(p);
            }
            let i = builder.build().map_err(|e| ParseError::Expected {
                expected: format!("valid ifunc definition: {e}").into(),
                loc: DiagLoc::span(decl_loc),
            })?;
            let i_view = self.module.view(i);
            if let Some(name) = forward_target {
                self.deferred_alias_targets.push(DeferredAliasTarget {
                    object: DeferredAliasObject::Ifunc(i_view),
                    name,
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

    /// Whether the token after a global's type begins an initializer. The only
    /// other legal continuations are a property list (`, section ...`), the
    /// next top-level entity, or end of input.
    fn starts_global_initializer(&self) -> bool {
        match self.peek() {
            Token::Eof | Token::Comma => false,
            Token::Kw(keyword) => !keyword_starts_top_level_entity(*keyword),
            Token::GlobalVar(_) | Token::GlobalId(_) => false,
            Token::ComdatVar(_) | Token::LocalVar(_) | Token::LocalVarId(_) => false,
            Token::Exclaim | Token::MetadataVar(_) => false,
            _ => true,
        }
    }

    fn parse_global_initializer(
        &mut self,
        ty: Type<'ctx, B>,
    ) -> ParseResult<ParsedGlobalInitializer<'ctx, B>> {
        if let Some(deferred) = self.defer_initializer_if_contains_special_constant(ty)? {
            return Ok((None, Some(deferred)));
        }
        self.parse_constant(ty).map(|c| (c, None))
    }

    fn defer_initializer_if_contains_special_constant(
        &mut self,
        ty: Type<'ctx, B>,
    ) -> ParseResult<Option<DeferredConstantKind<'ctx, B>>> {
        let Some((span, contains_special)) = self.scan_initializer_span()? else {
            return Ok(None);
        };
        if !contains_special {
            return Ok(None);
        }
        self.skip_initializer_span(span.end)?;
        Ok(Some(DeferredConstantKind::RawInitializer { ty, span }))
    }

    fn scan_initializer_span(&self) -> ParseResult<Option<(Span, bool)>> {
        if matches!(self.peek(), Token::Comma | Token::Eof) {
            return Ok(None);
        }
        let mut lex = self.lex.clone();
        let mut current = self.current.clone();
        let start = current.span.start;
        let mut end = current.span.end;
        let mut depth = 0u32;
        let mut contains_special = false;
        let mut consumed_any = false;
        loop {
            if consumed_any
                && depth == 0
                && self.scan_token_ends_global_initializer(&current.value, &lex)?
            {
                break;
            }
            match current.value {
                Token::Kw(Keyword::Blockaddress)
                | Token::Kw(Keyword::DsoLocalEquivalent)
                | Token::Kw(Keyword::NoCfi) => contains_special = true,
                Token::LParen | Token::LSquare | Token::LBrace | Token::Less => {
                    depth = depth.saturating_add(1);
                }
                Token::RParen | Token::RSquare | Token::RBrace | Token::Greater => {
                    depth = depth.saturating_sub(1);
                }
                Token::Eof => break,
                _ => {}
            }
            end = current.span.end;
            consumed_any = true;
            current = lex.next_token().map_err(map_lex_error)?;
        }
        Ok(Some((Span::new(start, end), contains_special)))
    }

    fn scan_token_ends_global_initializer(
        &self,
        token: &Token<'src>,
        lex_after_token: &Lexer<'src>,
    ) -> ParseResult<bool> {
        match token {
            Token::Eof | Token::Comma => Ok(true),
            Token::Kw(keyword) => Ok(keyword_starts_top_level_entity(*keyword)),
            Token::GlobalVar(_)
            | Token::GlobalId(_)
            | Token::LocalVar(_)
            | Token::LocalVarId(_)
            | Token::ComdatVar(_)
            | Token::MetadataVar(_) => self.scan_next_token_is_equal(lex_after_token),
            Token::Exclaim => self.scan_numbered_metadata_definition(lex_after_token),
            _ => Ok(false),
        }
    }

    fn scan_next_token_is_equal(&self, lex_after_token: &Lexer<'src>) -> ParseResult<bool> {
        let mut lookahead = lex_after_token.clone();
        let next = lookahead.next_token().map_err(map_lex_error)?;
        Ok(matches!(next.value, Token::Equal))
    }

    fn scan_numbered_metadata_definition(
        &self,
        lex_after_token: &Lexer<'src>,
    ) -> ParseResult<bool> {
        let mut lookahead = lex_after_token.clone();
        let slot = lookahead.next_token().map_err(map_lex_error)?;
        if !matches!(slot.value, Token::IntegerLit(_)) {
            return Ok(false);
        }
        let equal = lookahead.next_token().map_err(map_lex_error)?;
        Ok(matches!(equal.value, Token::Equal))
    }

    fn skip_initializer_span(&mut self, end: u32) -> ParseResult<()> {
        while self.current.span.start < end && !matches!(self.peek(), Token::Eof) {
            self.bump()?;
        }
        Ok(())
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

    fn parse_val_id(
        &mut self,
        pfs: Option<&PerFunctionState<'ctx, B>>,
        expected_ty: Option<Type<'ctx, B>>,
    ) -> ParseResult<ValId<'ctx, B>> {
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

        if let Some(ty) = expected_ty {
            match ty.into_type_enum() {
                AnyTypeEnum::Array(array_ty) if matches!(self.peek(), Token::Kw(Keyword::C)) => {
                    self.bump()?;
                    let bytes: Vec<u8> = match self.peek() {
                        Token::StringConstant(b) => b.as_ref().to_vec(),
                        _ => return Err(self.expected("string constant after 'c'")),
                    };
                    self.bump()?;
                    let AnyTypeEnum::Int(elem_ty) = array_ty.element().into_type_enum() else {
                        return Err(self.expected("i8 array type for c\"...\" constant"));
                    };
                    let mut values = Vec::with_capacity(bytes.len());
                    for b in &bytes {
                        let c = elem_ty.const_int_checked(u64::from(*b)).map_err(|e| {
                            ParseError::Expected {
                                expected: format!("i8 array element for c\"...\" constant: {e}")
                                    .into(),
                                loc: DiagLoc::span(self.loc()),
                            }
                        })?;
                        values.push(c.as_constant());
                    }
                    let c = array_ty
                        .const_array(values)
                        .map_err(|e| ParseError::Expected {
                            expected: format!("valid c\"...\" constant: {e}").into(),
                            loc: DiagLoc::span(self.loc()),
                        })?;
                    return Ok(ValId::Constant(c.as_constant()));
                }
                AnyTypeEnum::Array(array_ty) if matches!(self.peek(), Token::LSquare) => {
                    self.expect_punct(PunctKind::LSquare, "'[' to open array constant")?;
                    let first_elt_loc = self.loc();
                    let values = if matches!(self.peek(), Token::RSquare) {
                        Vec::new()
                    } else {
                        self.parse_global_value_vector()?
                    };
                    self.expect_punct(PunctKind::RSquare, "']' to close array constant")?;
                    if values.is_empty() {
                        // `[]` is upstream's `t_EmptyArray`, legal only at a
                        // zero-length array type — it defers the check to
                        // `convertValIDToValue` because with no elements there
                        // is nothing to derive an element type from.
                        if !array_ty.is_empty() {
                            return Err(self.message("invalid empty array initializer"));
                        }
                    } else {
                        self.check_aggregate_elements(&values, "array", first_elt_loc)?;
                    }
                    let c = array_ty
                        .const_array(values)
                        .map_err(|e| ParseError::Expected {
                            expected: format!("valid array constant: {e}").into(),
                            loc: DiagLoc::span(self.loc()),
                        })?;
                    return Ok(ValId::Constant(c.as_constant()));
                }
                AnyTypeEnum::Vector(vec_ty) if matches!(self.peek(), Token::Less) => {
                    self.expect_punct(PunctKind::Less, "'<' to open vector constant")?;
                    let first_elt_loc = self.loc();
                    let values = if matches!(self.peek(), Token::Greater) {
                        Vec::new()
                    } else {
                        self.parse_global_value_vector()?
                    };
                    self.expect_punct(PunctKind::Greater, "'>' to close vector constant")?;
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
                    let c = vec_ty
                        .const_vector(values)
                        .map_err(|e| ParseError::Expected {
                            expected: format!("valid vector constant: {e}").into(),
                            loc: DiagLoc::span(self.loc()),
                        })?;
                    return Ok(ValId::Constant(c.as_constant()));
                }
                AnyTypeEnum::Struct(struct_ty)
                    if matches!(self.peek(), Token::LBrace)
                        || (struct_ty.is_packed() && matches!(self.peek(), Token::Less)) =>
                {
                    if struct_ty.is_opaque() {
                        return Err(self.expected("non-opaque struct type for struct constant"));
                    }
                    if struct_ty.is_packed() {
                        self.expect_punct(PunctKind::Less, "'<' to open packed struct constant")?;
                    }
                    self.expect_punct(PunctKind::LBrace, "'{' to open struct constant")?;
                    let values = if matches!(self.peek(), Token::RBrace) {
                        Vec::new()
                    } else {
                        self.parse_global_value_vector()?
                    };
                    self.expect_punct(PunctKind::RBrace, "'}' to close struct constant")?;
                    if struct_ty.is_packed() {
                        self.expect_punct(
                            PunctKind::Greater,
                            "'>' to close packed struct constant",
                        )?;
                    }
                    if struct_ty.field_count() != values.len() {
                        return Err(
                            self.message("initializer with struct type has wrong # elements")
                        );
                    }
                    for (index, value) in values.iter().enumerate() {
                        let field_ty = struct_ty.field_type(index).ok_or_else(|| {
                            self.message("initializer with struct type has wrong # elements")
                        })?;
                        if value.ty() != field_ty {
                            return Err(ParseError::Message {
                                message: format!(
                                    "element {index} of struct initializer doesn't match struct element type"
                                )
                                .into(),
                                loc: DiagLoc::span(self.loc()),
                            });
                        }
                    }
                    let c = struct_ty
                        .const_struct(values)
                        .map_err(|e| ParseError::Expected {
                            expected: format!("valid struct constant: {e}").into(),
                            loc: DiagLoc::span(self.loc()),
                        })?;
                    return Ok(ValId::Constant(c.as_constant()));
                }
                _ => {}
            }
        }

        match self.peek() {
            Token::LocalVar(_) => {
                if pfs.is_none() {
                    return Err(self.expected("global constant value"));
                }
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("local SSA name"))?;
                self.bump()?;
                Ok(ValId::LocalName(name))
            }
            Token::LocalVarId(id) => {
                if pfs.is_none() {
                    return Err(self.expected("global constant value"));
                }
                let id = *id;
                self.bump()?;
                Ok(ValId::LocalId(id))
            }
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("global variable name"))?;
                self.bump()?;
                Ok(ValId::GlobalName(name))
            }
            Token::GlobalId(id) => {
                let id = *id;
                self.bump()?;
                Ok(ValId::GlobalId(id))
            }
            Token::IntegerLit(_) => {
                let expected_width = match expected_ty.map(Type::into_type_enum) {
                    Some(AnyTypeEnum::Int(t)) => ExpectedIntWidth::Bits(t.bit_width()),
                    _ => ExpectedIntWidth::Infer,
                };
                self.parse_int_literal(expected_width).map(ValId::ApsInt)
            }
            Token::FloatLit(_) => {
                let float_ty = match expected_ty.map(Type::into_type_enum) {
                    Some(AnyTypeEnum::Float(t)) => t,
                    _ => return Err(self.message("floating point constant invalid for type")),
                };
                let bits = self.parse_fp_literal(&float_ty)?;
                Ok(ValId::ApFloat(bits))
            }
            Token::Kw(Keyword::True) => {
                let ty = expected_ty.ok_or_else(|| self.expected("i1 type for boolean literal"))?;
                if ty != self.module.i1_type().as_type() {
                    return Err(self.expected("i1 type for boolean literal"));
                }
                self.bump()?;
                Ok(ValId::ApsInt(ParsedApsInt::SignedMagnitude {
                    sign: ParsedSign::Positive,
                    magnitude: ApInt::from_words(1, &[1]),
                }))
            }
            Token::Kw(Keyword::False) => {
                let ty = expected_ty.ok_or_else(|| self.expected("i1 type for boolean literal"))?;
                if ty != self.module.i1_type().as_type() {
                    return Err(self.expected("i1 type for boolean literal"));
                }
                self.bump()?;
                Ok(ValId::ApsInt(ParsedApsInt::SignedMagnitude {
                    sign: ParsedSign::Positive,
                    magnitude: ApInt::zero(1),
                }))
            }
            Token::Kw(Keyword::Null) => {
                self.bump()?;
                Ok(ValId::Null)
            }
            Token::Kw(Keyword::Zeroinitializer) => {
                self.bump()?;
                Ok(ValId::Zero)
            }
            Token::Kw(Keyword::Undef) => {
                self.bump()?;
                Ok(ValId::Undef)
            }
            Token::Kw(Keyword::Poison) => {
                self.bump()?;
                Ok(ValId::Poison)
            }
            Token::Kw(Keyword::None) => {
                let ty = expected_ty.ok_or_else(|| self.expected("type for none constant"))?;
                self.bump()?;
                match ty.into_type_enum() {
                    AnyTypeEnum::Token(_) => Ok(ValId::Constant(self.module.token_none())),
                    _ => Err(self.message("invalid type for none constant")),
                }
            }
            Token::Kw(Keyword::Blockaddress) => {
                let ty =
                    expected_ty.ok_or_else(|| self.expected("pointer type for blockaddress"))?;
                self.parse_blockaddress_constant(ty, pfs)
                    .map(ValId::Constant)
            }
            Token::Kw(Keyword::DsoLocalEquivalent) => self
                .parse_dso_local_equivalent_constant()
                .map(ValId::Constant),
            Token::Kw(Keyword::NoCfi) => self.parse_no_cfi_constant().map(ValId::Constant),
            Token::MetadataVar(_) => {
                let ty = expected_ty.ok_or_else(|| self.expected("metadata operand type"))?;
                if !ty.is_metadata() {
                    return Err(self.expected("`metadata` type for a metadata operand"));
                }
                Ok(ValId::Value(self.parse_metadata_value_operand()?))
            }
            Token::Exclaim => {
                let ty = expected_ty.ok_or_else(|| self.expected("metadata operand type"))?;
                if !ty.is_metadata() {
                    return Err(self.expected("`metadata` type for a metadata operand"));
                }
                Ok(ValId::Value(self.parse_metadata_value_operand()?))
            }
            Token::Kw(Keyword::Ptrauth) => self.parse_ptrauth_constant().map(ValId::Constant),
            Token::Kw(Keyword::Splat) => {
                self.expect_keyword(Keyword::Splat, "'splat'")?;
                self.expect_punct(PunctKind::LParen, "'(' in splat constant")?;
                let scalar = self.parse_global_type_and_value()?;
                self.expect_punct(PunctKind::RParen, "')' in splat constant")?;
                Ok(ValId::ConstantSplat(scalar))
            }
            Token::Instruction(op) if is_supported_constant_expr_opcode(*op) => {
                let ty = expected_ty.ok_or_else(|| self.unsupported_constant_value_form_at(loc))?;
                self.parse_constant_expr(ty).map(ValId::Constant)
            }
            _ => Err(self.expected("constant initializer")),
        }
    }

    fn expand_splat_constant(
        &self,
        ty: Type<'ctx, B>,
        scalar: llvmkit_ir::Constant<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        let AnyTypeEnum::Vector(vec_ty) = ty.into_type_enum() else {
            return Err(self.expected("vector type for splat constant"));
        };
        if scalar.ty() != vec_ty.element() {
            return Err(self.expected("vector type for splat constant"));
        }
        let len = usize::try_from(vec_ty.min_len()).map_err(|_| ParseError::Expected {
            expected: "vector type for splat constant".into(),
            loc: DiagLoc::span(self.loc()),
        })?;
        let elements = vec![scalar; len];
        vec_ty
            .const_vector(elements)
            .map(|c| c.as_constant())
            .map_err(|e| self.builder_err("splat constant", e))
    }

    fn zero_initializer_constant(
        &self,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        match ty.into_type_enum() {
            AnyTypeEnum::Int(t) => Ok(t.const_zero().as_constant()),
            AnyTypeEnum::Pointer(t) => Ok(t.const_null().as_constant()),
            AnyTypeEnum::Float(t) => Ok(t.const_from_bits(0).as_constant()),
            AnyTypeEnum::Array(t) => {
                let len = usize::try_from(t.len()).map_err(|_| ParseError::Expected {
                    expected: "array zeroinitializer length fits in usize".into(),
                    loc: DiagLoc::span(self.loc()),
                })?;
                let element = t.element();
                let mut elements = Vec::with_capacity(len);
                for _ in 0..len {
                    elements.push(self.zero_initializer_constant(element)?);
                }
                t.const_array(elements)
                    .map(|c| c.as_constant())
                    .map_err(|e| self.builder_err("array zeroinitializer", e))
            }
            AnyTypeEnum::Vector(t) => {
                let len = usize::try_from(t.min_len()).map_err(|_| ParseError::Expected {
                    expected: "vector zeroinitializer length fits in usize".into(),
                    loc: DiagLoc::span(self.loc()),
                })?;
                let element = t.element();
                let mut elements = Vec::with_capacity(len);
                for _ in 0..len {
                    elements.push(self.zero_initializer_constant(element)?);
                }
                t.const_vector(elements)
                    .map(|c| c.as_constant())
                    .map_err(|e| self.builder_err("vector zeroinitializer", e))
            }
            AnyTypeEnum::Struct(t) => {
                if t.is_opaque() {
                    return Err(ParseError::Message {
                        message: "invalid type for null constant".into(),
                        loc: DiagLoc::span(self.loc()),
                    });
                }
                let mut elements = Vec::with_capacity(t.field_count());
                for idx in 0..t.field_count() {
                    let field_ty = t
                        .field_type(idx)
                        .ok_or_else(|| self.expected("struct field type for zeroinitializer"))?;
                    elements.push(self.zero_initializer_constant(field_ty)?);
                }
                t.const_struct(elements)
                    .map(|c| c.as_constant())
                    .map_err(|e| self.builder_err("struct zeroinitializer", e))
            }
            AnyTypeEnum::TargetExt(_) => self.module.target_ext_none(ty).map_err(|e| match e {
                IrError::InvalidOperation { message } => ParseError::Expected {
                    expected: message.into(),
                    loc: DiagLoc::span(self.loc()),
                },
                other => self.builder_err("target extension none", other),
            }),
            _ => Err(self.expected("zeroinitializer for a zeroable type")),
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
    fn reject_function_typed_value(&self, ty: Type<'ctx, B>) -> ParseResult<()> {
        if matches!(ty.kind(), llvmkit_ir::TypeKind::Function) {
            return Err(self.message("functions are not values, refer to them as pointers"));
        }
        Ok(())
    }

    /// The first-class-and-not-label guard the `t_Undef`, `t_Poison` and
    /// `t_Zero` arms share. Upstream carries a `FIXME` about `LabelTy` being
    /// first-class at all, which is why the label test is separate.
    fn check_undef_like_type(&self, ty: Type<'ctx, B>, what: &'static str) -> ParseResult<()> {
        if !ty.is_first_class() || ty.is_label() {
            return Err(ParseError::Message {
                message: format!("invalid type for {what} constant").into(),
                loc: DiagLoc::span(self.loc()),
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
                loc: DiagLoc::span(self.loc()),
            });
        }
        Ok(constant)
    }

    fn convert_val_id_to_value(
        &mut self,
        ty: Type<'ctx, B>,
        id: ValId<'ctx, B>,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.reject_function_typed_value(ty)?;
        match id {
            ValId::LocalName(name) => pfs
                .ok_or_else(|| self.message("invalid use of function-local name"))?
                .get_val(self.module, LocalRef::Named(&name), ty, self.loc()),
            ValId::LocalId(id) => pfs
                .ok_or_else(|| self.message("invalid use of function-local name"))?
                .get_val(self.module, LocalRef::Numbered(id), ty, self.loc()),
            ValId::GlobalName(name) => self.resolve_global_name_as_value(name, ty),
            ValId::GlobalId(id) => self.resolve_global_id_as_value(id, ty),
            ValId::ApsInt(parsed) => {
                let int_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Int(t) => t,
                    _ => return Err(self.message("integer constant must have integer type")),
                };
                let bits = lower_parsed_apsint(&parsed, int_ty.bit_width());
                let c = int_ty
                    .const_ap_int(&bits)
                    .map_err(|e| self.builder_err("integer constant", e))?;
                Ok(c.as_erased())
            }
            ValId::ApFloat(value) => {
                let float_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Float(t) => t,
                    _ => return Err(self.message("floating point constant invalid for type")),
                };
                Ok(float_ty
                    .const_ap_float(&value)
                    .map_err(|e| self.builder_err("float constant", e))?
                    .as_erased())
            }
            ValId::Null => {
                let pty = match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(t) => t,
                    _ => return Err(self.message("null must be a pointer type")),
                };
                Ok(pty.const_null().as_erased())
            }
            ValId::Zero => {
                self.check_undef_like_type(ty, "null")?;
                self.zero_initializer_constant(ty).map(|c| c.as_erased())
            }
            ValId::Undef => {
                self.check_undef_like_type(ty, "undef")?;
                Ok(ty.undef().as_erased())
            }
            ValId::Poison => {
                self.check_undef_like_type(ty, "poison")?;
                Ok(ty.poison().as_erased())
            }
            ValId::Constant(c) => self.checked_constant_type(ty, c).map(|c| c.as_erased()),
            ValId::ConstantSplat(c) => self.expand_splat_constant(ty, c).map(|c| c.as_erased()),
            ValId::Value(v) => Ok(v),
        }
    }

    fn convert_val_id_to_constant(
        &mut self,
        ty: Type<'ctx, B>,
        id: ValId<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.reject_function_typed_value(ty)?;
        match id {
            ValId::GlobalName(name) => {
                match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(_) => {}
                    _ => return Err(self.expected("global reference for pointer constant")),
                }
                self.resolve_global_name_as_constant(name, ty)
            }
            ValId::GlobalId(id) => {
                match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(_) => {}
                    _ => return Err(self.expected("global reference for pointer constant")),
                }
                self.resolve_global_id_as_constant(id, ty)
            }
            ValId::ApsInt(parsed) => {
                let int_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Int(t) => t,
                    _ => return Err(self.message("integer constant must have integer type")),
                };
                let bits = lower_parsed_apsint(&parsed, int_ty.bit_width());
                let c = int_ty
                    .const_ap_int(&bits)
                    .map_err(|e| ParseError::Expected {
                        expected: format!("valid integer constant: {e}").into(),
                        loc: DiagLoc::span(self.loc()),
                    })?;
                Ok(c.as_constant())
            }
            ValId::ApFloat(value) => {
                let float_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Float(t) => t,
                    _ => return Err(self.message("floating point constant invalid for type")),
                };
                Ok(float_ty
                    .const_ap_float(&value)
                    .map_err(|e| self.builder_err("float constant", e))?
                    .as_constant())
            }
            ValId::Null => {
                let ptr_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(t) => t,
                    _ => return Err(self.message("null must be a pointer type")),
                };
                Ok(ptr_ty.const_null().as_constant())
            }
            ValId::Zero => self.zero_initializer_constant(ty),
            ValId::Undef => {
                self.check_undef_like_type(ty, "undef")?;
                Ok(ty.undef().as_constant())
            }
            ValId::Poison => {
                self.check_undef_like_type(ty, "poison")?;
                Ok(ty.poison().as_constant())
            }
            ValId::Constant(c) => self.checked_constant_type(ty, c),
            ValId::ConstantSplat(c) => self.expand_splat_constant(ty, c),
            ValId::LocalId(_) | ValId::LocalName(_) | ValId::Value(_) => {
                Err(self.expected("constant value"))
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
        let value_loc = self.loc();
        let id = self.parse_val_id(None, Some(ty))?;
        if let ValId::GlobalName(name) = id {
            match self.convert_val_id_to_constant(ty, ValId::GlobalName(name.clone())) {
                Ok(constant) => Ok(ParsedPersonalityFn::Resolved(constant)),
                Err(ParseError::UndefinedSymbol { .. }) if ty.is_pointer() => {
                    Ok(ParsedPersonalityFn::ForwardName {
                        name,
                        loc: value_loc,
                    })
                }
                Err(err) => Err(err),
            }
        } else {
            self.convert_val_id_to_constant(ty, id)
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

    fn resolve_global_name_as_value(
        &mut self,
        name: String,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        if !matches!(
            resolve_intrinsic_name(&name),
            IntrinsicNameResolution::NonIntrinsic
        ) {
            return Err(ParseError::Message {
                message: "intrinsic can only be used as callee".into(),
                loc: DiagLoc::span(self.loc()),
            });
        }
        if let Some(id) = self.module.global(&name) {
            Ok(self.module.view(id).as_erased())
        } else if let Some(id) = self.module.function_dyn(&name) {
            Ok(self.module.view(id).as_erased())
        } else if let Some(id) = self.module.alias(&name) {
            Ok(self.module.view(id).as_erased())
        } else if let Some(id) = self.module.ifunc(&name) {
            Ok(self.module.view(id).as_erased())
        } else {
            let loc = self.loc();
            self.global_forward_ref(Some(&name), None, ty, loc)
                .map(|c| c.as_erased())
        }
    }

    fn resolve_global_id_as_value(
        &mut self,
        id: u32,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.numbered_globals
            .get(id)
            .copied()
            .map(|r| match r {
                GlobalRef::Function(f) => f.as_erased(),
                GlobalRef::Variable(g) => g.as_erased(),
                GlobalRef::Alias(a) => a.as_erased(),
                GlobalRef::Ifunc(i) => i.as_erased(),
            })
            .map(Ok)
            .unwrap_or_else(|| {
                let loc = self.loc();
                self.global_forward_ref(None, Some(id), ty, loc)
                    .map(|c| c.as_erased())
            })
    }

    fn resolve_global_name_as_constant(
        &mut self,
        name: String,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        if !matches!(
            resolve_intrinsic_name(&name),
            IntrinsicNameResolution::NonIntrinsic
        ) {
            return Err(ParseError::Message {
                message: "intrinsic can only be used as callee".into(),
                loc: DiagLoc::span(self.loc()),
            });
        }
        if let Some(id) = self.module.global(&name) {
            Ok(self.module.view(id).as_global_constant_ptr())
        } else if let Some(id) = self.module.function_dyn(&name) {
            Ok(self.module.view(id).as_global_constant_ptr())
        } else if let Some(id) = self.module.alias(&name) {
            Ok(self.module.view(id).as_global_constant_ptr())
        } else if let Some(id) = self.module.ifunc(&name) {
            Ok(self.module.view(id).as_global_constant_ptr())
        } else {
            let loc = self.loc();
            self.global_forward_ref(Some(&name), None, ty, loc)
        }
    }

    fn resolve_global_id_as_constant(
        &mut self,
        id: u32,
        ty: Type<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.numbered_globals
            .get(id)
            .copied()
            .map(|r| Ok(self.global_ref_to_constant(r)))
            .unwrap_or_else(|| {
                let loc = self.loc();
                self.global_forward_ref(None, Some(id), ty, loc)
            })
    }

    fn global_ref_to_constant(&self, r: GlobalRef<'ctx, B>) -> llvmkit_ir::Constant<'ctx, B> {
        match r {
            GlobalRef::Function(f) => f.as_global_constant_ptr(),
            GlobalRef::Variable(g) => g.as_global_constant_ptr(),
            GlobalRef::Alias(a) => a.as_global_constant_ptr(),
            GlobalRef::Ifunc(i) => i.as_global_constant_ptr(),
        }
    }
    fn resolve_global_name_as_ref(&self, name: String) -> ParseResult<GlobalRef<'ctx, B>> {
        if let Some(id) = self.module.global(&name) {
            Ok(GlobalRef::Variable(self.module.view(id)))
        } else if let Some(id) = self.module.function_dyn(&name) {
            Ok(GlobalRef::Function(self.module.view(id)))
        } else if let Some(id) = self.module.alias(&name) {
            Ok(GlobalRef::Alias(self.module.view(id)))
        } else if let Some(id) = self.module.ifunc(&name) {
            Ok(GlobalRef::Ifunc(self.module.view(id)))
        } else {
            Err(ParseError::UndefinedSymbol {
                kind: SymbolKind::Global,
                id: SymbolId::Named(name),
                loc: DiagLoc::span(self.loc()),
            })
        }
    }

    fn resolve_global_id_as_ref(&self, id: u32) -> ParseResult<GlobalRef<'ctx, B>> {
        self.numbered_globals
            .get(id)
            .copied()
            .ok_or_else(|| ParseError::UndefinedSymbol {
                kind: SymbolKind::Global,
                id: SymbolId::Numbered(id),
                loc: DiagLoc::span(self.loc()),
            })
    }

    fn resolve_global_id_as_function(
        &self,
        id: u32,
    ) -> ParseResult<llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>> {
        self.numbered_globals
            .get(id)
            .and_then(|r| match r {
                GlobalRef::Function(f) => Some(*f),
                _ => None,
            })
            .ok_or_else(|| ParseError::UndefinedSymbol {
                kind: SymbolKind::Global,
                id: SymbolId::Numbered(id),
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

    fn parse_blockaddress_constant(
        &mut self,
        expected_ty: Type<'ctx, B>,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        if !matches!(expected_ty.into_type_enum(), AnyTypeEnum::Pointer(_)) {
            return Err(self.expected("pointer type for blockaddress"));
        }
        self.expect_keyword(Keyword::Blockaddress, "'blockaddress'")?;
        self.expect_punct(PunctKind::LParen, "'(' in blockaddress")?;
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
                        label_loc,
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
            ParsedBlockAddressFunction::Forward { function, loc } => self.defer_block_address(
                expected_ty,
                DeferredBlockAddressFunction::Forward(function),
                label,
                loc,
            ),
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
        loc: Span,
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
            loc,
        });
        Ok(constant)
    }

    fn parse_dso_local_equivalent_constant(
        &mut self,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.expect_keyword(Keyword::DsoLocalEquivalent, "'dso_local_equivalent'")?;
        let global = match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("global value name in dso_local_equivalent"))?;
                self.bump()?;
                self.resolve_global_name_as_ref(name)
            }
            Token::GlobalId(id) => {
                let id = *id;
                self.bump()?;
                self.resolve_global_id_as_ref(id)
            }
            _ => Err(self.expected("global value name in dso_local_equivalent")),
        }?;
        self.module
            .dso_local_equivalent_global(self.global_ref_to_constant(global))
            .map_err(|e| self.builder_err("dso_local_equivalent", e))
    }

    fn parse_no_cfi_constant(&mut self) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.expect_keyword(Keyword::NoCfi, "'no_cfi'")?;
        let global = match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("global value name in no_cfi"))?;
                self.bump()?;
                self.resolve_global_name_as_ref(name)
            }
            Token::GlobalId(id) => {
                let id = *id;
                self.bump()?;
                self.resolve_global_id_as_ref(id)
            }
            _ => Err(self.expected("global value name in no_cfi")),
        }?;
        self.module
            .no_cfi_global(self.global_ref_to_constant(global))
            .map_err(|e| self.builder_err("no_cfi", e))
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

    fn parse_constant_expr(&mut self, result_ty: Type<'ctx, B>) -> ParseResult<Constant<'ctx, B>> {
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
                if lhs.ty() != rhs.ty() {
                    return Err(self.message("operands of constexpr must have same type"));
                }
                if !is_int_or_int_vector_type(lhs.ty()) {
                    return Err(
                        self.message("constexpr requires integer or integer vector operands")
                    );
                }
                self.expect_punct(PunctKind::RParen, "')' in binary constantexpr")?;
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
                        loc: DiagLoc::span(self.loc()),
                    });
                }
                self.build_constant_expr(
                    result_ty,
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
                let flags =
                    self.validate_parsed_gep_constant_expr(source_ty, &operands, parsed_flags)?;
                self.build_constant_expr(result_ty, Some(source_ty), opcode, operands, flags)
            }
            ConstantExprOpcode::ShuffleVector
            | ConstantExprOpcode::InsertElement
            | ConstantExprOpcode::ExtractElement => {
                self.expect_punct(PunctKind::LParen, "'(' in constantexpr")?;
                let operands = self.parse_global_value_vector()?;
                self.expect_punct(PunctKind::RParen, "')' in constantexpr")?;
                self.validate_parsed_vector_constant_expr(opcode, result_ty, &operands)?;
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
    fn gep_constant_expr_flags(
        &self,
        parsed: ParsedGepConstantExprFlags,
        address_space: u32,
    ) -> ParseResult<ConstantExprFlags> {
        let Some((start, end)) = parsed.in_range else {
            return Ok(ConstantExprFlags::gep(parsed.no_wrap));
        };
        let bit_width = self.module.data_layout().index_size_in_bits(address_space);
        let start_words = inrange_bound_to_apint_words(&start, bit_width);
        let end_words = inrange_bound_to_apint_words(&end, bit_width);
        let in_range = ConstantExprInRange::new(start_words, end_words, bit_width);
        if !constant_expr_inrange_is_non_empty(&in_range) {
            return Err(self.expected("end to be larger than start"));
        }
        Ok(ConstantExprFlags::gep_with_in_range(
            parsed.no_wrap,
            in_range,
        ))
    }

    fn parse_inrange_bound(&mut self) -> ParseResult<ParsedInRangeBound> {
        let bound = match self.peek() {
            Token::IntegerLit(IntLit {
                sign,
                base: NumBase::Dec,
                digits,
            }) => {
                let magnitude_words =
                    decimal_digits_to_words(digits).ok_or_else(|| self.expected("integer"))?;
                ParsedInRangeBound::SignedMagnitude {
                    negative: matches!(sign, Sign::Neg),
                    magnitude_words,
                }
            }
            Token::IntegerLit(IntLit {
                base: base @ (NumBase::HexSigned | NumBase::HexUnsigned),
                digits,
                ..
            }) => {
                let words = hex_digits_to_words(digits).ok_or_else(|| self.expected("integer"))?;
                let bit_width =
                    hex_apsint_bit_width(digits, &words).ok_or_else(|| self.expected("integer"))?;
                ParsedInRangeBound::HexApsInt {
                    signed: matches!(base, NumBase::HexSigned),
                    words,
                    bit_width,
                }
            }
            _ => return Err(self.expected("integer")),
        };
        self.bump()?;
        Ok(bound)
    }

    /// Everything `LLParser::parseValID`'s `getelementptr` arm does after the
    /// closing paren, in upstream's order: base type, `inrange` bounds, index
    /// agreement, sizedness, constant-expression support, index walk.
    ///
    /// The order *is* the behaviour — several of these overlap, so which one
    /// fires decides the message. `getelementptr({<vscale x 2 x i32>, i32},
    /// ptr @g, i32 0)` is both unsized and unsupported, and upstream reports
    /// it unsized.
    fn validate_parsed_gep_constant_expr(
        &self,
        source_ty: Type<'ctx, B>,
        operands: &[llvmkit_ir::Constant<'ctx, B>],
        parsed_flags: ParsedGepConstantExprFlags,
    ) -> ParseResult<ConstantExprFlags> {
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
        Ok(flags)
    }

    fn validate_parsed_vector_constant_expr(
        &self,
        opcode: ConstantExprOpcode,
        result_ty: Type<'ctx, B>,
        operands: &[Constant<'ctx, B>],
    ) -> ParseResult<()> {
        match opcode {
            ConstantExprOpcode::ShuffleVector => {
                let [lhs, rhs, mask] = operands else {
                    return Err(self.expected("three operands to shufflevector"));
                };
                if !is_valid_shufflevector(result_ty, lhs.ty(), rhs.ty(), mask.ty()) {
                    return Err(self.message("invalid operands to shufflevector"));
                }
            }
            ConstantExprOpcode::ExtractElement => {
                let [vector, index] = operands else {
                    return Err(self.expected("two operands to extractelement"));
                };
                if !is_valid_extractelement(result_ty, vector.ty(), index.ty()) {
                    return Err(self.message("invalid extractelement operands"));
                }
            }
            ConstantExprOpcode::InsertElement => {
                let [vector, value, index] = operands else {
                    return Err(self.expected("three operands to insertelement"));
                };
                if !is_valid_insertelement(result_ty, vector.ty(), value.ty(), index.ty()) {
                    return Err(self.message("invalid insertelement operands"));
                }
            }
            _ => {}
        }
        Ok(())
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

    fn parse_optional_function_linkage(&mut self, is_define: bool) -> ParseResult<Linkage> {
        let loc = self.loc();
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
            _ => Ok(linkage),
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

    fn parse_unnamed_attr_group(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Attributes, "'attributes'")?;
        let loc = self.loc();
        let id = match self.peek() {
            Token::AttrGrpId(id) => {
                let id = *id;
                self.bump()?;
                id
            }
            _ => return Err(self.expected("attribute group id")),
        };
        self.expect_punct(PunctKind::Equal, "'=' after attribute group id")?;
        self.expect_punct(PunctKind::LBrace, "'{' in attribute group")?;
        let mut storage = AttributeStorage::new();
        let groups =
            self.parse_fn_attribute_value_pairs(&mut storage, AttrIndex::Function, false)?;
        if !groups.is_empty() {
            return Err(ParseError::Expected {
                expected: "attribute".into(),
                loc: DiagLoc::span(loc),
            });
        }
        self.expect_punct(PunctKind::RBrace, "'}' closing attribute group")?;
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
            Keyword::Nocapture => AttrKind::NoCapture,
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
            _ => return None,
        })
    }
    fn is_attr_start(&self) -> bool {
        match self.peek() {
            Token::AttrGrpId(_) | Token::StringConstant(_) => true,
            Token::Kw(Keyword::Align | Keyword::Alignstack | Keyword::Memory) => true,
            Token::Kw(keyword) if Self::legacy_memory_effects(*keyword).is_some() => true,
            Token::Kw(keyword) => Self::attr_kind_for_keyword(*keyword).is_some(),
            _ => false,
        }
    }

    fn is_function_header_attr_start(&self) -> bool {
        // `align N` in the function header grammar is a function alignment
        // suffix, not an `AttributeList` entry. Leave it for
        // `parse_optional_function_suffix` so intrinsic declarations reject it
        // through the noncanonical modifier path rather than treating it as an
        // extra generated-attribute mismatch.
        self.is_attr_start() && !matches!(self.peek(), Token::Kw(Keyword::Align))
    }

    fn parse_optional_function_header_attrs(
        &mut self,
        attrs: &mut AttributeStorage,
    ) -> ParseResult<Vec<u32>> {
        let mut groups = Vec::new();
        while self.is_function_header_attr_start() {
            groups.extend(self.parse_fn_attribute_value_pairs(attrs, AttrIndex::Function, true)?);
        }
        Ok(groups)
    }

    fn parse_optional_function_suffix(
        &mut self,
        attrs: &mut AttributeStorage,
    ) -> ParseResult<FunctionSuffix<'ctx, B>> {
        let mut suffix = FunctionSuffix {
            attr_groups: self.parse_optional_function_header_attrs(attrs)?,
            ..FunctionSuffix::default()
        };
        loop {
            match self.peek() {
                Token::Kw(Keyword::Section) => {
                    self.bump()?;
                    suffix.section = Some(self.parse_string_constant("section name")?);
                }
                Token::Kw(Keyword::Partition) => {
                    self.bump()?;
                    suffix.partition = Some(self.parse_string_constant("partition name")?);
                }
                Token::Kw(Keyword::Comdat) => {
                    self.bump()?;
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
                Token::Kw(Keyword::Align) => {
                    self.bump()?;
                    let bytes = self.parse_uint64("function alignment")?;
                    suffix.align = MaybeAlign::new(
                        Align::new(bytes).map_err(|e| self.builder_err("function align", e))?,
                    );
                }
                Token::Kw(Keyword::Gc) => {
                    self.bump()?;
                    suffix.gc = Some(self.parse_string_constant("gc name")?);
                }
                Token::Kw(Keyword::Prefix) => {
                    self.bump()?;
                    suffix.prefix_data = Some(self.parse_global_type_and_value()?);
                }
                Token::Kw(Keyword::Prologue) => {
                    self.bump()?;
                    suffix.prologue_data = Some(self.parse_global_type_and_value()?);
                }
                Token::Kw(Keyword::Personality) => {
                    self.bump()?;
                    suffix.personality_fn = Some(self.parse_personality_fn()?);
                }
                Token::MetadataVar(_) => {
                    suffix
                        .metadata
                        .push(self.parse_named_metadata_attachment()?);
                }
                Token::Comma => {
                    self.bump()?;
                    if matches!(self.peek(), Token::MetadataVar(_)) {
                        suffix
                            .metadata
                            .push(self.parse_named_metadata_attachment()?);
                    } else {
                        return Err(self.expected("metadata attachment"));
                    }
                }
                _ if self.is_attr_start() => {
                    suffix
                        .attr_groups
                        .extend(self.parse_optional_function_header_attrs(attrs)?);
                }
                _ => break,
            }
        }
        Ok(suffix)
    }

    fn parse_fn_attribute_value_pairs(
        &mut self,
        out: &mut AttributeStorage,
        index: AttrIndex,
        allow_group_refs: bool,
    ) -> ParseResult<Vec<u32>> {
        let mut groups = Vec::new();
        loop {
            match self.peek() {
                Token::RBrace | Token::LBrace | Token::Comma | Token::Eof => break,
                Token::AttrGrpId(id) if allow_group_refs => {
                    let id = *id;
                    self.bump()?;
                    groups.push(id);
                }
                Token::AttrGrpId(_) => return Err(self.expected("attribute")),
                Token::StringConstant(_) => {
                    let key = self.parse_string_constant("attribute string key")?;
                    let value = if self.eat_punct(PunctKind::Equal)? {
                        self.parse_string_constant("attribute string value")?
                    } else {
                        String::new()
                    };
                    out.add(index, Attribute::<B>::string(key, value));
                }
                Token::Kw(Keyword::Align) if index == AttrIndex::Function && allow_group_refs => {
                    break;
                }
                Token::Kw(Keyword::Align) => {
                    self.bump()?;
                    let value = self.parse_uint64("align value")?;
                    let attr = Attribute::<B>::int(AttrKind::Alignment, value)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Alignstack) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::LParen, "'(' in alignstack attribute")?;
                    let value = self.parse_uint64("alignstack value")?;
                    self.expect_punct(PunctKind::RParen, "')' after alignstack value")?;
                    let attr = Attribute::<B>::int(AttrKind::StackAlignment, value)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Memory) => {
                    let attr = self.parse_memory_attribute()?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Nofpclass) => {
                    let attr = self.parse_nofpclass_attribute()?;
                    out.add(index, attr);
                }
                Token::Kw(keyword)
                    if index == AttrIndex::Function
                        && Self::legacy_memory_effects(*keyword).is_some() =>
                {
                    let effects = Self::legacy_memory_effects(*keyword)
                        .ok_or_else(|| self.expected("memory attribute"))?;
                    self.bump()?;
                    out.add(index, Attribute::<B>::memory(effects));
                }
                Token::Kw(Keyword::Uwtable) => {
                    self.bump()?;
                    let kind = if self.eat_punct(PunctKind::LParen)? {
                        let kind = if self.eat_keyword(Keyword::Sync)? {
                            1
                        } else if self.eat_keyword(Keyword::Async)? {
                            2
                        } else {
                            return Err(self.expected("'sync' or 'async' in uwtable"));
                        };
                        self.expect_punct(PunctKind::RParen, "')' after uwtable kind")?;
                        kind
                    } else {
                        2
                    };
                    let attr = Attribute::<B>::int(AttrKind::UwTable, kind)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                Token::Kw(kw @ (Keyword::Dereferenceable | Keyword::DereferenceableOrNull)) => {
                    let kind = if *kw == Keyword::Dereferenceable {
                        AttrKind::Dereferenceable
                    } else {
                        AttrKind::DereferenceableOrNull
                    };
                    self.bump()?;
                    self.expect_punct(PunctKind::LParen, "'(' in dereferenceable attribute")?;
                    let bytes = self.parse_uint64("dereferenceable byte count")?;
                    self.expect_punct(PunctKind::RParen, "')' after dereferenceable byte count")?;
                    let attr = Attribute::<B>::int(kind, bytes)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                Token::Kw(
                    kw @ (Keyword::Byval
                    | Keyword::Byref
                    | Keyword::Inalloca
                    | Keyword::Sret
                    | Keyword::Elementtype),
                ) => {
                    let kind = match kw {
                        Keyword::Byval => AttrKind::ByVal,
                        Keyword::Byref => AttrKind::ByRef,
                        Keyword::Inalloca => AttrKind::InAlloca,
                        Keyword::Sret => AttrKind::StructRet,
                        _ => AttrKind::ElementType,
                    };
                    self.bump()?;
                    self.expect_punct(PunctKind::LParen, "'(' in type attribute")?;
                    let ty = self.parse_type(false)?;
                    self.expect_punct(PunctKind::RParen, "')' after type attribute")?;
                    let attr = Attribute::<B>::type_attr(kind, ty)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Captures) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::LParen, "'(' in captures attribute")?;
                    if !self.eat_keyword(Keyword::None)? {
                        return Err(self.expected(
                            "captures components other than `none` are not supported yet",
                        ));
                    }
                    self.expect_punct(PunctKind::RParen, "')' after captures(none)")?;
                    let attr = Attribute::<B>::enum_attr(AttrKind::NoCapture)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Range) => {
                    let attr = self.parse_range_attribute()?;
                    out.add(index, attr);
                }
                Token::Kw(keyword) => {
                    let Some(kind) = Self::attr_kind_for_keyword(*keyword) else {
                        break;
                    };
                    self.bump()?;
                    let attr = Attribute::<B>::enum_attr(kind)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                _ => break,
            }
        }
        Ok(groups)
    }

    fn parse_range_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Range, "'range'")?;
        self.expect_punct(PunctKind::LParen, "'(' in range attribute")?;
        let ty = self.parse_type(false)?;
        let TypeKind::Integer { bits } = ty.kind() else {
            return Err(self.expected("range attribute integer type"));
        };
        let lower = self.parse_int_literal(ExpectedIntWidth::Bits(bits))?;
        self.expect_punct(PunctKind::Comma, "',' in range attribute")?;
        let upper = self.parse_int_literal(ExpectedIntWidth::Bits(bits))?;
        self.expect_punct(PunctKind::RParen, "')' in range attribute")?;
        Attribute::<B>::range(
            ty,
            lower_parsed_apsint(&lower, bits),
            lower_parsed_apsint(&upper, bits),
        )
        .ok_or_else(|| self.expected("valid range attribute"))
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
        self.expect_punct(PunctKind::LParen, "'(' in nofpclass attribute")?;

        let mut mask = FpClassTest::NONE;
        loop {
            if let Some(component) = self.nofpclass_component() {
                mask |= component;
                self.bump()?;
            } else if mask.is_none() {
                // The integer spelling, which replaces the whole list.
                let value = self.parse_uint64("nofpclass test mask")?;
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
                self.expect_punct(PunctKind::RParen, "')' in nofpclass attribute")?;
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
                    return Err(self.message("expected ':' after location"));
                }
            }
            let Some(mod_ref) = Self::memory_access_kind_for_token(self.peek()) else {
                return Err(self.message(if location.is_none() {
                    "expected memory location (argmem, inaccessiblemem, errnomem) or access kind (none, read, write, readwrite)"
                } else {
                    "expected access kind (none, read, write, readwrite)"
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
        let groups =
            self.parse_fn_attribute_value_pairs(&mut storage, AttrIndex::Param(0), false)?;
        if !groups.is_empty() {
            return Err(self.expected("attribute"));
        }
        Ok(storage)
    }

    fn parse_optional_return_attrs(&mut self) -> ParseResult<AttributeStorage> {
        let mut storage = AttributeStorage::new();
        let groups = self.parse_fn_attribute_value_pairs(&mut storage, AttrIndex::Return, false)?;
        if !groups.is_empty() {
            return Err(self.expected("attribute"));
        }
        Ok(storage)
    }

    fn parse_optional_fn_attrs(&mut self) -> ParseResult<(AttributeStorage, Vec<u32>)> {
        let mut storage = AttributeStorage::new();
        let groups =
            self.parse_fn_attribute_value_pairs(&mut storage, AttrIndex::Function, true)?;
        Ok((storage, groups))
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
                        let value = self.parse_value(state, ty)?;
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
        let linkage = self.parse_optional_function_linkage(false)?;
        let (dso_locality, visibility, dll_storage_class) =
            self.parse_optional_preemption_visibility_dll()?;
        let calling_conv = self.parse_optional_calling_conv()?;
        let mut attrs = AttributeStorage::new();
        self.parse_fn_attribute_value_pairs(&mut attrs, AttrIndex::Return, false)?;
        let ret_ty = self.parse_type(true)?;
        let (name_id, name) = match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("function name"))?;
                (NameOrId::Name(name.clone()), name)
            }
            Token::GlobalId(n) => (NameOrId::Id(*n), String::new()),
            _ => return Err(self.expected("function name after return type")),
        };
        let decl_loc = self.loc();
        self.bump()?;
        self.expect_punct(PunctKind::LParen, "'(' in function declaration")?;
        let mut params = Vec::new();
        let mut param_names: Vec<Option<String>> = Vec::new();
        let mut var_args = false;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    var_args = true;
                    break;
                }
                let p_ty = self.parse_type(false)?;
                let slot = u32::try_from(params.len()).map_err(|_| ParseError::Expected {
                    expected: "parameter slot fits in u32".into(),
                    loc: DiagLoc::span(decl_loc),
                })?;
                self.parse_fn_attribute_value_pairs(&mut attrs, AttrIndex::Param(slot), false)?;
                let p_name = if matches!(self.peek(), Token::LocalVar(_)) {
                    let n = self
                        .current_str_payload()
                        .ok_or_else(|| self.expected("parameter name"))?;
                    self.bump()?;
                    Some(n)
                } else if matches!(self.peek(), Token::LocalVarId(_)) {
                    self.bump()?;
                    None
                } else {
                    None
                };
                param_names.push(p_name);
                params.push(p_ty);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close function declaration")?;
        let unnamed_addr = self.parse_optional_function_unnamed_addr()?;
        // A function with no explicit `addrspace` lives in the *program*
        // address space, not 0. Mirrors `parseOptionalProgramAddrSpace`, which
        // is `parseOptionalAddrSpace` with `DefaultAS =
        // getProgramAddressSpace()` — the `DefaultAS` parameter llvmkit had no
        // equivalent of.
        let address_space = self.parse_optional_program_addr_space()?;
        let suffix = self.parse_optional_function_suffix(&mut attrs)?;

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
                    || !suffix.metadata.is_empty()
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
        let existing_by_id = match &name_id {
            NameOrId::Id(id) => self.numbered_globals.get(*id).and_then(|r| match r {
                GlobalRef::Function(f) => Some(*f),
                _ => None,
            }),
            NameOrId::Name(_) => None,
        };
        let existing_by_name = (!name.is_empty())
            .then(|| self.module.function_dyn(&name))
            .flatten()
            .map(|id| self.module.view(id));
        let f = if let Some(existing) = existing_by_id.or(existing_by_name) {
            if existing.signature() != fn_ty || existing.basic_blocks().len() != 0 {
                return Err(ParseError::Expected {
                    expected: "forward function declaration with matching signature".into(),
                    loc: DiagLoc::span(decl_loc),
                });
            }
            existing.set_linkage(self.module, linkage);
            existing.set_visibility(self.module, visibility);
            existing.set_dll_storage_class(self.module, dll_storage_class);
            existing.set_dso_locality(self.module, dso_locality);
            existing.set_calling_conv(self.module, calling_conv);
            existing.set_unnamed_addr(self.module, unnamed_addr);
            existing.set_address_space(self.module, address_space);
            if !name.is_empty() {
                self.forward_function_decls.remove(&name);
            }
            existing.set_attributes(self.module, attrs);
            existing
        } else {
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
            f
        };
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
                ParsedPersonalityFn::ForwardName { name, loc } => {
                    self.deferred_personality_fns.push(DeferredPersonalityFn {
                        function: f,
                        name,
                        loc,
                    });
                }
            }
        }
        // Upstream applies the pre-header attachments first, in the order
        // they were written, then anything the suffix carried.
        for (kind, id) in leading_metadata.into_iter().chain(suffix.metadata) {
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
        Ok(())
    }

    // ── define ──────────────────────────────────────────────────────────

    /// `define RET @name(PARAMS) { ... }` — full function definition with
    /// a body. Mirrors `LLParser::parseDefine` for the constructive
    /// instruction subset currently shipped (ret / unreachable / br /
    /// cond_br / icmp / add / sub / mul). Function linkage and
    /// unnamed-address markers are preserved when present.
    fn parse_define(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Define, "'define'")?;
        let linkage = self.parse_optional_function_linkage(true)?;
        let (dso_locality, visibility, dll_storage_class) =
            self.parse_optional_preemption_visibility_dll()?;
        let calling_conv = self.parse_optional_calling_conv()?;
        let mut attrs = AttributeStorage::new();
        self.parse_fn_attribute_value_pairs(&mut attrs, AttrIndex::Return, false)?;
        let ret_ty = self.parse_type(true)?;
        let (name_id, name) = match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("function name"))?;
                (NameOrId::Name(name.clone()), name)
            }
            Token::GlobalId(n) => (NameOrId::Id(*n), String::new()),
            _ => return Err(self.expected("function name after return type")),
        };
        let decl_loc = self.loc();
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
        self.expect_punct(PunctKind::LParen, "'(' in function header")?;

        let mut param_types = Vec::new();
        let mut param_names: Vec<Option<ParamName>> = Vec::new();
        let mut var_args = false;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    var_args = true;
                    break;
                }
                let p_ty = self.parse_type(false)?;
                let slot = u32::try_from(param_types.len()).map_err(|_| ParseError::Expected {
                    expected: "parameter slot fits in u32".into(),
                    loc: DiagLoc::span(decl_loc),
                })?;
                self.parse_fn_attribute_value_pairs(&mut attrs, AttrIndex::Param(slot), false)?;
                let p_name = match self.peek() {
                    Token::LocalVar(_) => {
                        let s = self
                            .current_str_payload()
                            .ok_or_else(|| self.expected("local identifier payload"))?;
                        self.bump()?;
                        Some(ParamName::Named(s))
                    }
                    Token::LocalVarId(id) => {
                        let id = *id;
                        self.bump()?;
                        Some(ParamName::Numbered(id))
                    }
                    _ => None,
                };
                param_types.push(p_ty);
                param_names.push(p_name);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close function header")?;
        let unnamed_addr = self.parse_optional_function_unnamed_addr()?;
        // A function with no explicit `addrspace` lives in the *program*
        // address space, not 0. Mirrors `parseOptionalProgramAddrSpace`, which
        // is `parseOptionalAddrSpace` with `DefaultAS =
        // getProgramAddressSpace()` — the `DefaultAS` parameter llvmkit had no
        // equivalent of.
        let address_space = self.parse_optional_program_addr_space()?;
        let suffix = self.parse_optional_function_suffix(&mut attrs)?;

        let fn_ty = function_type_with_variadic(self.module, ret_ty, param_types, var_args);
        let existing_by_id = match &name_id {
            NameOrId::Id(id) => self.numbered_globals.get(*id).and_then(|r| match r {
                GlobalRef::Function(f) => Some(*f),
                _ => None,
            }),
            NameOrId::Name(_) => None,
        };
        let existing_by_name = (!name.is_empty())
            .then(|| self.module.function_dyn(&name))
            .flatten()
            .map(|id| self.module.view(id));
        let f = if let Some(existing) = existing_by_id.or(existing_by_name) {
            if existing.signature() != fn_ty || existing.basic_blocks().any(|bb| !bb.is_empty()) {
                return Err(ParseError::Expected {
                    expected: "forward function definition with matching signature".into(),
                    loc: DiagLoc::span(decl_loc),
                });
            }
            existing.set_linkage(self.module, linkage);
            existing.set_visibility(self.module, visibility);
            existing.set_dll_storage_class(self.module, dll_storage_class);
            existing.set_dso_locality(self.module, dso_locality);
            existing.set_calling_conv(self.module, calling_conv);
            existing.set_unnamed_addr(self.module, unnamed_addr);
            existing.set_address_space(self.module, address_space);
            existing.set_attributes(self.module, attrs);
            if !name.is_empty() {
                self.forward_function_decls.remove(&name);
            }
            existing
        } else {
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
            f
        };
        for (slot, p) in param_names.iter().enumerate() {
            if let Some(ParamName::Named(n)) = p {
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
                ParsedPersonalityFn::ForwardName { name, loc } => {
                    self.deferred_personality_fns.push(DeferredPersonalityFn {
                        function: f,
                        name,
                        loc,
                    });
                }
            }
        }
        for (kind, id) in suffix.metadata {
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

        self.expect_punct(PunctKind::LBrace, "'{' to open function body")?;

        let mut state = PerFunctionState::new(f);
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
                Some(ParamName::Named(n)) => {
                    state.local_named.insert(n, v);
                }
                Some(ParamName::Numbered(id)) => {
                    check_value_id("argument", "%", state.next_unnamed_value_id, id, decl_loc)?;
                    if state.local_numbered.contains_key(&id) {
                        return Err(ParseError::InvalidSlotId {
                            source: AddError::StaleId {
                                id,
                                next: state.next_unnamed_value_id,
                            },
                            loc: DiagLoc::span(decl_loc),
                        });
                    }
                    state.local_numbered.insert(id, v);
                    state.next_unnamed_value_id = id.saturating_add(1);
                }
                None => {
                    let id = state.next_unnamed_value_id;
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
        state.finish(self.module)?;
        self.expect_punct(PunctKind::RBrace, "'}' to close function body")?;
        Ok(())
    }

    // ── Function body driver ─────────────────────────────────────────────

    fn parse_function_body(&mut self, state: &mut PerFunctionState<'ctx, B>) -> ParseResult<()> {
        // Mirrors `LLParser::parseBasicBlock`: a body must contain at least
        // one block, and an unlabeled block is assigned the next shared
        // function-local numbered value slot.
        loop {
            match self.peek() {
                Token::RBrace => break,
                Token::Kw(Keyword::Uselistorder) => {
                    self.parse_function_use_list_order(state)?;
                }
                Token::LabelStr(_) => {
                    let label_loc = self.loc();
                    let label = self
                        .current_label_str()
                        .ok_or_else(|| self.expected("basic-block label"))?;
                    self.bump()?;
                    let header = if !self.label_span_is_quoted(label_loc) {
                        numbered_label_id(&label)
                            .map(BlockHeader::Numbered)
                            .unwrap_or(BlockHeader::Named(label))
                    } else {
                        BlockHeader::Named(label)
                    };
                    self.parse_basic_block(state, header, label_loc)?;
                }
                _ => {
                    // LLVM defines an unlabeled block with the next shared
                    // function-local numbered value slot.
                    let loc = self.loc();
                    self.parse_basic_block(state, BlockHeader::Implicit, loc)?;
                }
            }
        }
        Ok(())
    }

    fn current_label_str(&self) -> Option<String> {
        match self.peek() {
            Token::LabelStr(bytes) => std::str::from_utf8(bytes.as_ref()).ok().map(str::to_owned),
            _ => None,
        }
    }

    fn label_span_is_quoted(&self, loc: Span) -> bool {
        usize::try_from(loc.start)
            .ok()
            .and_then(|idx| self.src.get(idx))
            .is_some_and(|byte| *byte == b'"')
    }

    fn parse_basic_block(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        header: BlockHeader,
        header_loc: Span,
    ) -> ParseResult<()> {
        let bb = match header {
            BlockHeader::Named(n) => state.define_named_block(self.module, n, header_loc)?,
            BlockHeader::Numbered(id) => {
                state.define_numbered_label(self.module, id, header_loc)?
            }
            BlockHeader::Implicit => state.define_implicit_block(self.module, header_loc)?,
        };
        let bb_value = bb.to_erased();
        // Drive the typed builder for this block.
        let builder = IrBuilder::with_folder(self.module, NoFolder).position_at_end(bb);
        // Emit instructions until a terminator consumes `builder`.
        let mut builder = Some(builder);
        let mut pending_debug_records = Vec::new();
        // Track whether any non-phi instruction has been emitted in this block.
        // A `phi` appearing after one is ill-formed `.ll`: the auto-hoisting phi
        // builders would silently reorder it into valid position, so reject it
        // at parse time instead of laundering bad input into valid IR.
        let mut seen_non_phi = false;
        loop {
            while matches!(self.peek(), Token::Hash) {
                self.bump()?;
                pending_debug_records.push(self.parse_debug_record(state)?);
            }

            // Terminator — these consume the builder.
            match self.peek() {
                Token::Instruction(Opcode::Ret) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_ret(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::Unreachable) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    let _ = b.unreachable();
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::Br) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_br(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::Store) => {
                    let b_ref = borrow_live_builder(&builder, self.loc())?;
                    self.bump()?;
                    self.parse_store(state, b_ref)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    seen_non_phi = true;
                    continue;
                }
                Token::Instruction(Opcode::Fence) => {
                    let b_ref = borrow_live_builder(&builder, self.loc())?;
                    self.bump()?;
                    self.parse_fence(b_ref)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    seen_non_phi = true;
                    continue;
                }
                Token::Instruction(Opcode::Switch) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_switch(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::IndirectBr) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_indirectbr(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::Invoke) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    let result_loc = self.loc();
                    let result_name = self.parse_lhs_before_invoke()?;
                    let v = self.parse_invoke(state, b, &result_name)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    if let Some(val) = v {
                        state.bind_local(&result_name, val, result_loc)?;
                    }
                    return Ok(());
                }
                Token::Instruction(Opcode::Resume) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_resume(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::CleanupRet) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_cleanupret(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::CatchRet) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_catchret(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::CatchSwitch) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    let result_loc = self.loc();
                    let result_name = self.parse_lhs_assignment()?;
                    let v = self.parse_catchswitch(state, b, &result_name)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    state.bind_local(&result_name, v, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(Opcode::CallBr) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    let result_loc = self.loc();
                    let result_name = self.parse_lhs_assignment()?;
                    let v = self.parse_callbr(state, b, &result_name)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    if let Some(v) = v {
                        state.bind_local(&result_name, v, result_loc)?;
                    }
                    return Ok(());
                }
                _ => {}
            }
            // Non-terminator: an `%lhs = OP ...` or a void-result
            // instruction. Only result-producing arms are shipped so far.
            let result_name = self.parse_lhs_assignment()?;
            let result_loc = self.loc();
            if matches!(
                self.peek(),
                Token::Kw(Keyword::Tail | Keyword::Musttail | Keyword::Notail)
            ) {
                let b_ref = borrow_live_builder(&builder, self.loc())?;
                let value = self.parse_call(state, b_ref, &result_name)?;
                self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                state.bind_local(&result_name, value, result_loc)?;
                seen_non_phi = true;
                continue;
            }
            if matches!(self.peek(), Token::Instruction(Opcode::Invoke)) {
                let b = take_live_builder(&mut builder, self.loc())?;
                self.bump()?;
                let value = self.parse_invoke(state, b, &result_name)?;
                self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                if let Some(value) = value {
                    state.bind_local(&result_name, value, result_loc)?;
                }
                return Ok(());
            }
            // `callbr` is a terminator that may still bind a result, so it
            // arrives here rather than at the terminator dispatch above
            // whenever the line opens with `%res =` — `%res = callbr i32 asm
            // ...` is `test/Assembler/inline-asm-constraint-error.ll`'s
            // `output-after-label` split, and every `callbr` returning a value.
            if matches!(self.peek(), Token::Instruction(Opcode::CallBr)) {
                let b = take_live_builder(&mut builder, self.loc())?;
                let value = self.parse_callbr(state, b, &result_name)?;
                self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                if let Some(value) = value {
                    state.bind_local(&result_name, value, result_loc)?;
                }
                return Ok(());
            }
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
                Opcode::Select => self.parse_select(state, b_ref, &result_name)?,
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
                Opcode::Phi => self.parse_phi(state, b_ref, &result_name)?,
                Opcode::Call => self.parse_call(state, b_ref, &result_name)?,
                Opcode::VaArg => self.parse_vaarg(state, b_ref, &result_name)?,
                Opcode::Freeze => self.parse_freeze(state, b_ref, &result_name)?,
                Opcode::AtomicCmpXchg => self.parse_cmpxchg(state, b_ref, &result_name)?,
                Opcode::AtomicRmw => self.parse_atomicrmw(state, b_ref, &result_name)?,
                Opcode::LandingPad => self.parse_landingpad(state, b_ref, &result_name)?,
                Opcode::CleanupPad => self.parse_cleanuppad(state, b_ref, &result_name)?,
                Opcode::CatchPad => self.parse_catchpad(state, b_ref, &result_name)?,
                _ => {
                    return Err(ParseError::Expected {
                        expected: format!(
                            "instruction opcode supported by this parser (got {opcode:?})"
                        )
                        .into(),
                        loc: DiagLoc::span(result_loc),
                    });
                }
            };
            self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
            state.bind_local(&result_name, value, result_loc)?;
        }
    }

    /// Parse an optional `%name = ` / `%N = ` LHS introduction. When the
    /// next instruction has no LHS (terminator-only), this returns
    /// [`LocalLhs::None`]; otherwise it consumes the local var and `=`.
    fn parse_lhs_assignment(&mut self) -> ParseResult<LocalLhs> {
        match self.peek() {
            Token::LocalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("local SSA name"))?;
                self.bump()?;
                self.expect_punct(PunctKind::Equal, "'=' after local SSA name")?;
                Ok(LocalLhs::Named(name))
            }
            Token::LocalVarId(id) => {
                let id = *id;
                self.bump()?;
                self.expect_punct(PunctKind::Equal, "'=' after local SSA id")?;
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
        if let Token::PrimitiveType(PrimitiveTy::Void) = self.peek() {
            self.bump()?;
            let _ = b.ret_void().map_err(|e| ParseError::Expected {
                expected: format!("valid ret void: {e}").into(),
                loc: DiagLoc::span(self.loc()),
            })?;
            return Ok(());
        }
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
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
        if matches!(self.peek(), Token::PrimitiveType(PrimitiveTy::Label)) {
            self.bump()?;
            let target = self.parse_block_ref(state)?;
            let _ = b.br(target).map_err(|e| ParseError::Expected {
                expected: format!("valid br: {e}").into(),
                loc: DiagLoc::span(self.loc()),
            })?;
            return Ok(());
        }
        // Conditional: `i1 %cond, label %t, label %f`.
        let cond_ty = self.parse_type(false)?;
        if !matches!(
            cond_ty.into_type_enum(),
            AnyTypeEnum::Int(t) if t.bit_width() == 1
        ) {
            return Err(self.expected("'i1' condition for cond-br"));
        }
        let cond_v = self.parse_value(state, cond_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after br condition")?;
        self.expect_primitive(PrimitiveTy::Label, "'label' for then-target")?;
        let then_bb = self.parse_block_ref(state)?;
        self.expect_punct(PunctKind::Comma, "',' between br targets")?;
        self.expect_primitive(PrimitiveTy::Label, "'label' for else-target")?;
        let else_bb = self.parse_block_ref(state)?;
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

        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between binop operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;

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
            _ => return Err(self.expected("integer compare predicate")),
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
            _ => return Err(self.expected("floating-point compare predicate")),
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
        let ty = self.parse_type(false)?;
        // Upstream parses size, then alignment, then address space.
        let size = self.parse_optional_comma_array_size(state)?;
        let align = self.parse_optional_comma_align()?;
        let addr_space = self.parse_optional_comma_addrspace()?;
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
            return Ok(None);
        }
        let size_ty = self.parse_type(false)?;
        let size_v = self.parse_value(state, size_ty)?;
        let n: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = size_v
            .try_into()
            .map_err(|_| self.expected("integer alloca array size"))?;
        Ok(Some(n))
    }

    /// Optional `, addrspace(N)` clause for `alloca` (after any align),
    /// mirroring `LLParser::parseAlloc`. Uses the same save/restore peek so a
    /// trailing `, !dbg` metadata comma is left intact.
    fn parse_optional_comma_addrspace(&mut self) -> ParseResult<Option<u32>> {
        if !matches!(self.peek(), Token::Comma) {
            return Ok(None);
        }
        let saved_lex = self.lex.clone();
        let saved_current = self.current.clone();
        self.bump()?;
        if !matches!(self.peek(), Token::Kw(Keyword::Addrspace)) {
            self.lex = saved_lex;
            self.current = saved_current;
            return Ok(None);
        }
        self.bump()?;
        Ok(Some(self.parse_addr_space_paren()?))
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
        let ty = self.parse_type(false)?;
        self.expect_punct(PunctKind::Comma, "',' between load type and pointer")?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed load operand"))?;

        // Runtime-clause dispatch: `volatile` / `atomic` / the align and
        // syncscope clauses are all decided by the source text, so the
        // builder chain is assembled with explicit ifs (the same shape
        // [`Self::function_type_with_variadic`] uses for its runtime split).
        let mut load = b.load_from(ptr);
        if volatile {
            load = load.volatile();
        }
        if is_atomic {
            let sync_scope = self.parse_optional_syncscope()?;
            let ordering = self.parse_atomic_ordering("atomic ordering")?;
            self.expect_punct(PunctKind::Comma, "',' after atomic ordering")?;
            // Upstream requires the align clause on an atomic load
            // ("atomic load must have explicit non-zero alignment",
            // `LLParser::parseLoad`), so this is not optional here.
            let align = self.parse_align_val()?;
            load = load.atomic(ordering).sync_scope(sync_scope).align(align);
        } else if let Some(align) = self.parse_optional_comma_align()? {
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
        let val_ty = self.parse_type(false)?;
        let val_v = self.parse_value(state, val_ty)?;
        self.expect_punct(PunctKind::Comma, "',' between store value and pointer")?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed store target"))?;
        // Runtime-clause dispatch, mirroring [`Self::parse_load`].
        let mut store = b.store_to(val_v, ptr);
        if volatile {
            store = store.volatile();
        }
        if is_atomic {
            let sync_scope = self.parse_optional_syncscope()?;
            let ordering = self.parse_atomic_ordering("atomic ordering")?;
            self.expect_punct(PunctKind::Comma, "',' after atomic ordering")?;
            // Upstream requires the align clause on an atomic store
            // ("atomic store must have explicit non-zero alignment",
            // `LLParser::parseStore`), so this is not optional here.
            let align = self.parse_align_val()?;
            store = store.atomic(ordering).sync_scope(sync_scope).align(align);
        } else if let Some(align) = self.parse_optional_comma_align()? {
            store = store.align(align);
        }
        store.build().map_err(|e| self.builder_err("store", e))?;
        Ok(())
    }

    /// `getelementptr FLAGS SOURCE_TY, ptr P, INDEX, INDEX, ...` where
    /// FLAGS is any-order `inbounds` / `nusw` / `nuw`.
    /// Mirrors `LLParser::parseGetElementPtr` (LLParser.cpp ~8900).
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
        self.expect_punct(PunctKind::Comma, "',' after GEP source type")?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed GEP base"))?;
        let mut indices: Vec<llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B>> = Vec::new();
        while matches!(self.peek(), Token::Comma) {
            let saved_lex = self.lex.clone();
            let saved_current = self.current.clone();
            self.bump()?;
            // A trailing `, !dbg !N` attachment is not an index. Upstream
            // breaks out of the index loop on `MetadataVar` and reports the
            // comma as already eaten (`InstExtraComma`); llvmkit restores it
            // so `skip_trailing_metadata` sees the comma it expects, the same
            // backtrack `parse_optional_comma_array_size` uses for alloca.
            if matches!(self.peek(), Token::MetadataVar(_)) {
                self.lex = saved_lex;
                self.current = saved_current;
                break;
            }
            let idx_ty = self.parse_type(false)?;
            let idx_v = self.parse_value(state, idx_ty)?;
            let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = idx_v
                .try_into()
                .map_err(|_| self.expected("integer GEP index"))?;
            indices.push(idx);
        }
        let name = result_name.as_str();
        let v = b
            .gep_with_flags(source_ty, ptr, indices, flags, name)
            .map_err(|e| self.builder_err("getelementptr", e))?;
        Ok(b.view(v).as_erased())
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
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `LLParser::parseInstruction`'s `kw_select` arm eats fast-math flags
        // before calling `parseSelect`, then applies them to the result --
        // rejecting them outright when the result is not floating-point.
        let fmf_loc = self.loc();
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
        if !fmf.is_empty() && !is_fp_or_fp_vector_type(true_ty) {
            return Err(self.message_at(
                fmf_loc,
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
        let vec_ty = self.parse_type(false)?;
        let vec_v = self.parse_value(state, vec_ty)?;
        self.expect_punct(PunctKind::Comma, "',' in extractelement")?;
        let idx_ty = self.parse_type(false)?;
        let idx_v = self.parse_value(state, idx_ty)?;
        let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = idx_v
            .try_into()
            .map_err(|_| self.expected("integer index for extractelement"))?;
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
        let vec_ty = self.parse_type(false)?;
        let vec_v = self.parse_value(state, vec_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after vector in insertelement")?;
        let elt_ty = self.parse_type(false)?;
        let elt_v = self.parse_value(state, elt_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after element in insertelement")?;
        let idx_ty = self.parse_type(false)?;
        let idx_v = self.parse_value(state, idx_ty)?;
        let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = idx_v
            .try_into()
            .map_err(|_| self.expected("integer index for insertelement"))?;
        let v = b
            .insert_element(vec_v, elt_v, idx, result_name.as_str())
            .map_err(|e| self.builder_err("insertelement", e))?;
        Ok(b.view(v))
    }

    /// `shufflevector <vec-ty> <v1>, <vec-ty> <v2>, <mask>`.
    /// The mask is `< i32 N, i32 M, ... >` or `poison`. Mirrors
    /// `LLParser::parseShuffleVector`.
    ///
    /// Upstream: `test/Assembler/shufflevector.ll`.
    fn parse_shufflevector(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let v1_ty = self.parse_type(false)?;
        let v1 = self.parse_value(state, v1_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after v1 in shufflevector")?;
        let v2_ty = self.parse_type(false)?;
        let v2 = self.parse_value(state, v2_ty)?;
        self.expect_punct(PunctKind::Comma, "',' before mask in shufflevector")?;
        // Parse mask as the upstream typed constant operand.
        let mask = self.parse_shuffle_mask(v1_ty)?;
        let v = b
            .shuffle_vector(v1, v2, &mask, result_name.as_str())
            .map_err(|e| self.builder_err("shufflevector", e))?;
        Ok(b.view(v))
    }

    /// Parse a shufflevector mask typed constant operand and decode it with
    /// `ShuffleVectorInst::getShuffleMask` semantics.
    fn parse_shuffle_mask(
        &mut self,
        vector_ty: Type<'ctx, B>,
    ) -> ParseResult<Vec<ShuffleMaskElem>> {
        let mask_ty = self.parse_type(false)?;
        let loc = self.loc();
        let valid_mask_ty = match (AnyTypeEnum::from(vector_ty), AnyTypeEnum::from(mask_ty)) {
            (AnyTypeEnum::Vector(vector_ty), AnyTypeEnum::Vector(mask_ty)) => {
                matches!(mask_ty.element().kind(), TypeKind::Integer { bits: 32 })
                    && mask_ty.is_scalable() == vector_ty.is_scalable()
            }
            _ => false,
        };
        if !valid_mask_ty {
            return Err(ParseError::Expected {
                expected: "valid shufflevector mask".into(),
                loc: DiagLoc::span(loc),
            });
        }
        let mask = self.parse_global_value(mask_ty).map_err(|err| match err {
            ParseError::Lex(LexError::UnknownToken { span, .. }) => ParseError::Expected {
                expected: "valid shufflevector mask element".into(),
                loc: DiagLoc::span(span),
            },
            ParseError::Expected { .. } => ParseError::Expected {
                expected: "valid shufflevector mask".into(),
                loc: DiagLoc::span(loc),
            },
            other => other,
        })?;
        shufflevector_mask_from_constant(mask).ok_or_else(|| ParseError::Expected {
            expected: "valid shufflevector mask".into(),
            loc: DiagLoc::span(loc),
        })
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
        let agg_ty = self.parse_type(false)?;
        let agg_v = self.parse_value(state, agg_ty)?;
        let mut indices = Vec::new();
        while matches!(self.peek(), Token::Comma) {
            let saved_lex = self.lex.clone();
            let saved_current = self.current.clone();
            self.bump()?;
            // A trailing `, !dbg !N` attachment is not an index. Upstream
            // breaks out of the index loop on `MetadataVar` and reports the
            // comma as already eaten (`InstExtraComma`); llvmkit restores it
            // so `skip_trailing_metadata` sees the comma it expects, the same
            // backtrack `parse_optional_comma_array_size` uses for alloca.
            if matches!(self.peek(), Token::MetadataVar(_)) {
                self.lex = saved_lex;
                self.current = saved_current;
                break;
            }
            let idx = self.parse_uint32("extractvalue index")?;
            indices.push(idx);
        }
        let v = b
            .extract_value_dyn(agg_v, &indices, result_name.as_str())
            .map_err(|e| self.builder_err("extractvalue", e))?;
        Ok(b.view(v))
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
        let agg_ty = self.parse_type(false)?;
        let agg_v = self.parse_value(state, agg_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after agg in insertvalue")?;
        let elt_ty = self.parse_type(false)?;
        let elt_v = self.parse_value(state, elt_ty)?;
        let mut indices = Vec::new();
        while matches!(self.peek(), Token::Comma) {
            let saved_lex = self.lex.clone();
            let saved_current = self.current.clone();
            self.bump()?;
            // A trailing `, !dbg !N` attachment is not an index. Upstream
            // breaks out of the index loop on `MetadataVar` and reports the
            // comma as already eaten (`InstExtraComma`); llvmkit restores it
            // so `skip_trailing_metadata` sees the comma it expects, the same
            // backtrack `parse_optional_comma_array_size` uses for alloca.
            if matches!(self.peek(), Token::MetadataVar(_)) {
                self.lex = saved_lex;
                self.current = saved_current;
                break;
            }
            let idx = self.parse_uint32("insertvalue index")?;
            indices.push(idx);
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
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `LLParser::parseInstruction`'s `kw_phi` arm eats fast-math flags
        // before calling `parsePHI`, then applies them -- rejecting them when
        // the phi's result type is not floating-point. They used to be parsed
        // and dropped here, so `phi fast float ...` round-tripped without its
        // flags.
        let fmf_loc = self.loc();
        let fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        if !fmf.is_empty() && !ty.is_float_or_float_vector() {
            return Err(self.message_at(
                fmf_loc,
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
                if !state.defined_blocks.contains(&n) {
                    state.block_refs.entry(n.clone()).or_insert(loc);
                }
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
        if matches!(self.peek(), Token::Instruction(Opcode::Call)) {
            self.bump()?;
        }
        // `LLParser::parseCall` eats the flags here, before the calling
        // convention, and rejects them below when the return type is not
        // floating-point.
        let fmf = self.parse_optional_fmf()?;
        let calling_conv = self.parse_optional_calling_conv()?;
        let return_attrs = self.parse_optional_return_attrs()?;
        let callee_ty = self.parse_type(true)?;
        let parsed_callee = self.parse_direct_callee_ref(state)?;
        self.expect_punct(PunctKind::LParen, "'(' in call argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx, B>> = Vec::new();
        let mut arg_tys: Vec<Type<'ctx, B>> = Vec::new();
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
                let arg_ty = self.parse_type(false)?;
                let one_arg_attrs = self.parse_optional_param_attrs()?;
                let arg_v = self.parse_value(state, arg_ty)?;
                arg_tys.push(arg_ty);
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
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => function_type_with_variadic(self.module, callee_ty, arg_tys, var_args),
        };
        // `LLParser::parseCall`'s FMF guard, with its own wording.
        if !fmf.is_empty() && !is_fp_or_fp_vector_type(parsed_fn_ty.return_type()) {
            return Err(self.message(
                "fast-math-flags specified for call without floating-point scalar or vector return type",
            ));
        }
        let callee = self.resolve_direct_callee(parsed_callee, parsed_fn_ty)?;
        let name = result_name.as_str();
        let v = match callee {
            ParsedCallee::Function(callee) => {
                let mut builder = b
                    .call_builder(callee)
                    .call_site_type(parsed_fn_ty)
                    .calling_conv(calling_conv)
                    .call_attributes(call_attrs);
                builder = match tail_kind {
                    llvmkit_ir::instr_types::TailCallKind::None => builder,
                    llvmkit_ir::instr_types::TailCallKind::Tail => builder.tail(),
                    llvmkit_ir::instr_types::TailCallKind::MustTail => builder.must_tail(),
                    llvmkit_ir::instr_types::TailCallKind::NoTail => builder.no_tail(),
                };
                for arg in args {
                    builder = builder.arg(arg);
                }
                let call = builder
                    .name(name)
                    .build()
                    .map_err(|e| self.builder_err("call", e))?;
                b.view(call).to_erased()
            }
            ParsedCallee::InlineAsm(asm) => {
                let call = b
                    .inline_asm_call::<llvmkit_ir::Dyn, _, _, _>(asm, args, name)
                    .map_err(|e| self.builder_err("call", e))?;
                b.view(call).to_erased()
            }
            ParsedCallee::Indirect(callee) => {
                let call = b
                    .indirect_call_dyn::<llvmkit_ir::Dyn, _, _, _, _>(
                        parsed_fn_ty,
                        callee,
                        args,
                        name,
                    )
                    .map_err(|e| self.builder_err("indirect call", e))?;
                b.view(call).to_erased()
            }
        };
        Ok(v)
    }

    /// Optionally skip a calling convention keyword. Returns the CC token if
    /// consumed, but the calling convention is not yet plumbed through to
    /// the IR (deferred).
    fn parse_optional_calling_conv(&mut self) -> ParseResult<CallingConv> {
        let cc = match self.peek() {
            Token::Kw(Keyword::Ccc) => Some(CallingConv::C),
            Token::Kw(Keyword::Fastcc) => Some(CallingConv::FAST),
            Token::Kw(Keyword::Coldcc) => Some(CallingConv::COLD),
            Token::Kw(Keyword::Anyregcc) => Some(CallingConv::ANY_REG),
            Token::Kw(Keyword::PreserveMostcc) => Some(CallingConv::PRESERVE_MOST),
            Token::Kw(Keyword::PreserveAllcc) => Some(CallingConv::PRESERVE_ALL),
            Token::Kw(Keyword::Ghccc) => Some(CallingConv::GHC),
            Token::Kw(Keyword::Swiftcc) => Some(CallingConv::SWIFT),
            Token::Kw(Keyword::Swifttailcc) => Some(CallingConv::SWIFT_TAIL),
            Token::Kw(Keyword::X86Stdcallcc) => Some(CallingConv::X86_STD_CALL),
            Token::Kw(Keyword::X86Fastcallcc) => Some(CallingConv::X86_FAST_CALL),
            Token::Kw(Keyword::X86Thiscallcc) => Some(CallingConv::X86_THIS_CALL),
            Token::Kw(Keyword::X86Vectorcallcc) => Some(CallingConv::X86_VECTOR_CALL),
            Token::Kw(Keyword::X86Regcallcc) => Some(CallingConv::X86_REG_CALL),
            Token::Kw(Keyword::IntelOclBicc) => Some(CallingConv::INTEL_OCL_BI),
            Token::Kw(Keyword::Win64cc) => Some(CallingConv::WIN64),
            Token::Kw(Keyword::X86_64Sysvcc) => Some(CallingConv::X86_64_SYS_V),
            Token::Kw(Keyword::Hhvmcc) => Some(CallingConv::DUMMY_HHVM),
            Token::Kw(Keyword::HhvmCcc) => Some(CallingConv::DUMMY_HHVM_C),
            Token::Kw(Keyword::AmdgpuVs) => Some(CallingConv::AMDGPU_VS),
            Token::Kw(Keyword::AmdgpuLs) => Some(CallingConv::AMDGPU_LS),
            Token::Kw(Keyword::AmdgpuHs) => Some(CallingConv::AMDGPU_HS),
            Token::Kw(Keyword::AmdgpuEs) => Some(CallingConv::AMDGPU_ES),
            Token::Kw(Keyword::AmdgpuGs) => Some(CallingConv::AMDGPU_GS),
            Token::Kw(Keyword::AmdgpuPs) => Some(CallingConv::AMDGPU_PS),
            Token::Kw(Keyword::AmdgpuCs) => Some(CallingConv::AMDGPU_CS),
            Token::Kw(Keyword::AmdgpuKernel) => Some(CallingConv::AMDGPU_KERNEL),
            Token::Kw(Keyword::Tailcc) => Some(CallingConv::TAIL),
            Token::Kw(Keyword::CfguardCheckcc) => Some(CallingConv::CF_GUARD_CHECK),
            Token::Kw(Keyword::M68kRtdcc) => Some(CallingConv::M68K_RTD),
            Token::Kw(Keyword::Cc) => {
                self.bump()?;
                let raw = self.parse_uint32("calling convention number")?;
                return CallingConv::from_raw(raw)
                    .ok_or_else(|| self.expected("valid calling convention number"));
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
    /// `parseValID` + `convertValIDToValue(PointerType)` callee handling.
    fn parse_direct_callee_ref(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
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
                let ptr_ty = self.module.ptr_type(0).as_type();
                let v = self.parse_value(state, ptr_ty)?;
                Ok(ParsedDirectCallee::Value { v, loc })
            }
        }
    }

    fn resolve_direct_callee(
        &mut self,
        parsed: ParsedDirectCallee<'ctx, B>,
        parsed_fn_ty: llvmkit_ir::FunctionType<'ctx, B>,
    ) -> ParseResult<ParsedCallee<'ctx, B>> {
        match parsed {
            ParsedDirectCallee::Name { name, loc } => {
                if let Some(f) = self
                    .module
                    .function_dyn(&name)
                    .map(|id| self.module.view(id))
                {
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
                        Ok(ParsedCallee::Function(self.module.view(f)))
                    }
                    IntrinsicNameResolution::UnknownIntrinsic => Err(ParseError::Expected {
                        expected: "unknown intrinsic".into(),
                        loc: DiagLoc::span(loc),
                    }),
                    IntrinsicNameResolution::NonIntrinsic => {
                        let f = self
                            .module
                            .add_function_dyn(&name, parsed_fn_ty, Linkage::External)
                            .map_err(|e| ParseError::Expected {
                                expected: format!("forward function declaration: {e}").into(),
                                loc: DiagLoc::span(loc),
                            })?;
                        self.forward_function_decls.entry(name).or_insert(loc);
                        Ok(ParsedCallee::Function(self.module.view(f)))
                    }
                }
            }
            ParsedDirectCallee::Id { id, loc } => self
                .numbered_globals
                .get(id)
                .and_then(|r| match r {
                    GlobalRef::Function(f) => Some(*f),
                    _ => None,
                })
                .map(ParsedCallee::Function)
                .ok_or_else(|| ParseError::UndefinedSymbol {
                    kind: SymbolKind::Global,
                    id: SymbolId::Numbered(id),
                    loc: DiagLoc::span(loc),
                }),
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

    /// Parse an LHS assignment that may precede an `invoke` terminator.
    /// Invoke may or may not have an LHS result binding. Mirrors
    /// `LLParser::parseInstruction`'s handling of `invoke`.
    fn parse_lhs_before_invoke(&mut self) -> ParseResult<LocalLhs> {
        // Consume the `invoke` keyword (already peeked; dispatch already
        // established this is Opcode::Invoke).
        self.bump()?; // eat `invoke`
        // An invoke with a result has already had its LHS consumed before
        // the opcode. But for invoke, the structure is:
        //   [%name =] invoke ...
        // The dispatch for Invoke is reached BEFORE parse_lhs_assignment.
        // So we need to do it here.
        self.parse_lhs_assignment()
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
        self.expect_punct(PunctKind::Comma, "',' in va_arg")?;
        let result_ty = self.parse_type(false)?;
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
        let cond_ty = self.parse_type(false)?;
        let cond_v = self.parse_value(state, cond_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after switch condition")?;
        self.expect_primitive(PrimitiveTy::Label, "'label' for switch default")?;
        let default_bb = self.parse_block_ref(state)?;
        let (_, mut sw) = b
            .switch_dyn(cond_v, default_bb, "")
            .map_err(|e| self.builder_err("switch", e))?;
        // Case list: `[ ty N, label %bb, ... ]`
        self.expect_punct(PunctKind::LSquare, "'[' to open switch case list")?;
        loop {
            if matches!(self.peek(), Token::RSquare) {
                self.bump()?;
                break;
            }
            let case_ty = self.parse_type(false)?;
            let case_v = self.parse_value(state, case_ty)?;
            let case_int: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = case_v
                .try_into()
                .map_err(|_| self.expected("integer switch case value"))?;
            self.expect_punct(PunctKind::Comma, "',' between case value and label")?;
            self.expect_primitive(PrimitiveTy::Label, "'label' for switch case destination")?;
            let case_bb = self.parse_block_ref(state)?;
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
        let addr_ty = self.parse_type(false)?;
        let addr_v = self.parse_value(state, addr_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after indirectbr address")?;
        let addr: PointerValue<'ctx, B> = addr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed indirectbr address"))?;
        let (_, mut ibr) = b
            .indirectbr(addr, "")
            .map_err(|e| self.builder_err("indirectbr", e))?;
        // Destination list: `[ label %dest, ... ]`
        self.expect_punct(
            PunctKind::LSquare,
            "'[' to open indirectbr destination list",
        )?;
        loop {
            if matches!(self.peek(), Token::RSquare) {
                self.bump()?;
                break;
            }
            self.expect_primitive(PrimitiveTy::Label, "'label' in indirectbr destination")?;
            let dest_bb = self.parse_block_ref(state)?;
            ibr = ibr
                .add_destination(dest_bb)
                .map_err(|e| self.builder_err("indirectbr.add_destination", e))?;
            let _ = self.eat_punct(PunctKind::Comma)?;
        }
        let _ = ibr.finish();
        Ok(())
    }

    /// `fence [syncscope("...")] <ordering>`. Void instruction.
    /// Mirrors `LLParser::parseFence` (LLParser.cpp ~8476).
    ///
    /// Upstream: `test/Assembler/fence.ll`.
    fn parse_fence(&mut self, b: &ParsedBlockBuilder<'ctx, 'ctx, B>) -> ParseResult<()> {
        let sync_scope = self.parse_optional_syncscope()?;
        let ordering = self.parse_atomic_ordering("fence ordering")?;
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
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr operand for cmpxchg"))?;
        self.expect_punct(PunctKind::Comma, "',' in cmpxchg")?;
        let cmp_ty = self.parse_type(false)?;
        let cmp_v = self.parse_value(state, cmp_ty)?;
        self.expect_punct(PunctKind::Comma, "',' in cmpxchg after cmp")?;
        let new_ty = self.parse_type(false)?;
        let new_v = self.parse_value(state, new_ty)?;
        let sync_scope = self.parse_optional_syncscope()?;
        let success_ord = self.parse_atomic_ordering("cmpxchg success ordering")?;
        let failure_ord = self.parse_atomic_ordering("cmpxchg failure ordering")?;
        let align = self.parse_optional_comma_align()?;
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
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx, B> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr operand for atomicrmw"))?;
        self.expect_punct(PunctKind::Comma, "',' in atomicrmw")?;
        let val_ty = self.parse_type(false)?;
        let val_v = self.parse_value(state, val_ty)?;
        let sync_scope = self.parse_optional_syncscope()?;
        let ordering = self.parse_atomic_ordering("atomicrmw ordering")?;
        let align = self.parse_optional_comma_align()?;
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
            _ => return Err(self.expected("atomicrmw operation keyword")),
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
                    let clause_ty = self.parse_type(false)?;
                    let clause_v = self.parse_value(state, clause_ty)?;
                    lp = lp
                        .add_catch_clause(clause_v)
                        .map_err(|e| self.builder_err("landingpad.catch", e))?;
                }
                Token::Kw(Keyword::Filter) => {
                    self.bump()?;
                    let filter_ty = self.parse_type(false)?;
                    let filter_v = self.parse_value(state, filter_ty)?;
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
    /// Upstream: `test/Assembler/cleanuppad.ll`.
    fn parse_cleanuppad(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.expect_keyword(Keyword::Within, "'within' in cleanuppad")?;
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
    /// Upstream: `test/Assembler/catchpad.ll`.
    fn parse_catchpad(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.expect_keyword(Keyword::Within, "'within' in catchpad")?;
        let parent_ty = self.parse_type(false)?;
        let parent_v = self.parse_value(state, parent_ty)?;
        let args = self.parse_bracket_value_list(state)?;
        let v = b
            .catch_pad(parent_v, args, result_name.as_str())
            .map_err(|e| self.builder_err("catchpad", e))?;
        Ok(v.to_erased())
    }

    /// `resume <ty> <val>`. Terminator.
    /// Mirrors `LLParser::parseResume` (LLParser.cpp ~7762).
    ///
    /// Upstream: `test/Assembler/resume.ll`.
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

    /// `cleanupret from <val> [unwind (to caller | label %bb)]`.
    /// Terminator. Mirrors `LLParser::parseCleanupRet`.
    ///
    /// Upstream: `test/Assembler/cleanupret.ll`.
    fn parse_cleanupret(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        self.expect_keyword(Keyword::From, "'from' in cleanupret")?;
        let pad_ty = self.parse_type(false)?;
        let pad_v = self.parse_value(state, pad_ty)?;
        let unwind_dest = if self.eat_keyword(Keyword::Unwind)? {
            if self.eat_keyword(Keyword::To)? {
                self.expect_keyword(Keyword::Caller, "'caller' in cleanupret unwind")?;
                None
            } else {
                self.expect_primitive(
                    PrimitiveTy::Label,
                    "'label' in cleanupret unwind destination",
                )?;
                Some(self.parse_block_ref(state)?)
            }
        } else {
            None
        };
        let _ = match unwind_dest {
            Some(dest) => b.cleanup_ret(pad_v, dest, ""),
            None => b.cleanup_ret_to_caller(pad_v, ""),
        }
        .map_err(|e| self.builder_err("cleanupret", e))?;
        Ok(())
    }

    /// `catchret from <val> to label %bb`. Terminator.
    /// Mirrors `LLParser::parseCatchRet`.
    ///
    /// Upstream: `test/Assembler/catchret.ll`.
    fn parse_catchret(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
    ) -> ParseResult<()> {
        self.expect_keyword(Keyword::From, "'from' in catchret")?;
        let pad_ty = self.parse_type(false)?;
        let pad_v = self.parse_value(state, pad_ty)?;
        self.expect_keyword(Keyword::To, "'to' in catchret")?;
        self.expect_primitive(PrimitiveTy::Label, "'label' in catchret destination")?;
        let dest = self.parse_block_ref(state)?;
        let _ = b
            .catch_ret(pad_v, dest, "")
            .map_err(|e| self.builder_err("catchret", e))?;
        Ok(())
    }

    /// `catchswitch within <token> [<handlers>] unwind (to caller | label %bb)`.
    /// Terminator. Returns the catchswitch value.
    /// Mirrors `LLParser::parseCatchSwitch`.
    ///
    /// Upstream: `test/Assembler/catchswitch.ll`.
    fn parse_catchswitch(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.bump()?; // eat `catchswitch`
        self.expect_keyword(Keyword::Within, "'within' in catchswitch")?;
        let parent_pad = self.parse_optional_pad_token(state)?;
        // `[handler1, handler2, ...]`
        self.expect_punct(PunctKind::LSquare, "'[' in catchswitch handlers")?;
        let mut handlers: Vec<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> = Vec::new();
        loop {
            if matches!(self.peek(), Token::RSquare) {
                self.bump()?;
                break;
            }
            self.expect_primitive(PrimitiveTy::Label, "'label' in catchswitch handler")?;
            let bb = self.parse_block_ref(state)?;
            handlers.push(bb);
            let _ = self.eat_punct(PunctKind::Comma)?;
        }
        // `unwind (to caller | label %bb)`
        self.expect_keyword(Keyword::Unwind, "'unwind' in catchswitch")?;
        let unwind_dest = if self.eat_keyword(Keyword::To)? {
            self.expect_keyword(Keyword::Caller, "'caller' after 'to' in catchswitch")?;
            None
        } else {
            self.expect_primitive(
                PrimitiveTy::Label,
                "'label' in catchswitch unwind destination",
            )?;
            Some(self.parse_block_ref(state)?)
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
    /// Upstream: `test/Assembler/invoke.ll`.
    fn parse_invoke(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'ctx, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<Option<llvmkit_ir::Value<'ctx, B>>> {
        // parse_lhs_before_invoke already consumed `invoke` and optionally LHS.
        let calling_conv = self.parse_optional_calling_conv()?;
        let return_attrs = self.parse_optional_return_attrs()?;
        let callee_ty = self.parse_type(true)?;
        let parsed_callee = self.parse_direct_callee_ref(state)?;
        self.expect_punct(PunctKind::LParen, "'(' in invoke argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx, B>> = Vec::new();
        let mut arg_tys: Vec<Type<'ctx, B>> = Vec::new();
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
                let arg_ty = self.parse_type(false)?;
                let one_arg_attrs = self.parse_optional_param_attrs()?;
                let arg_v = self.parse_value(state, arg_ty)?;
                arg_attrs.push(one_arg_attrs);
                arg_tys.push(arg_ty);
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
        self.expect_primitive(PrimitiveTy::Label, "'label' for invoke normal destination")?;
        let normal_bb = self.parse_block_ref(state)?;
        self.expect_keyword(Keyword::Unwind, "'unwind' in invoke")?;
        self.expect_primitive(PrimitiveTy::Label, "'label' for invoke unwind destination")?;
        let unwind_bb = self.parse_block_ref(state)?;
        // Upstream `resolveFunctionType`: an explicitly written function
        // type IS the call-site type; otherwise infer from the arguments.
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => function_type_with_variadic(self.module, callee_ty, arg_tys, var_args),
        };
        let callee = self.resolve_direct_callee(parsed_callee, parsed_fn_ty)?;
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
        let calling_conv = self.parse_optional_calling_conv()?;
        let return_attrs = self.parse_optional_return_attrs()?;
        let callee_ty = self.parse_type(true)?;
        let parsed_callee = self.parse_direct_callee_ref(state)?;
        self.expect_punct(PunctKind::LParen, "'(' in callbr argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx, B>> = Vec::new();
        let mut arg_tys: Vec<Type<'ctx, B>> = Vec::new();
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
                let arg_ty = self.parse_type(false)?;
                let one_arg_attrs = self.parse_optional_param_attrs()?;
                let arg_v = self.parse_value(state, arg_ty)?;
                arg_attrs.push(one_arg_attrs);
                arg_tys.push(arg_ty);
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
        self.expect_primitive(
            PrimitiveTy::Label,
            "'label' for callbr fallthrough destination",
        )?;
        let fallthrough = self.parse_block_ref(state)?;
        // Optional `[ label %ind1, ... ]`
        let mut indirect: Vec<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> = Vec::new();
        if matches!(self.peek(), Token::Comma) || matches!(self.peek(), Token::LSquare) {
            if matches!(self.peek(), Token::Comma) {
                self.bump()?;
            }
            self.expect_punct(PunctKind::LSquare, "'[' in callbr indirect targets")?;
            loop {
                if matches!(self.peek(), Token::RSquare) {
                    self.bump()?;
                    break;
                }
                self.expect_primitive(PrimitiveTy::Label, "'label' in callbr indirect target")?;
                let bb = self.parse_block_ref(state)?;
                indirect.push(bb);
                let _ = self.eat_punct(PunctKind::Comma)?;
            }
        }
        // Upstream `resolveFunctionType`: an explicitly written function
        // type IS the call-site type; otherwise infer from the arguments.
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => function_type_with_variadic(self.module, callee_ty, arg_tys, var_args),
        };
        let callee = self.resolve_direct_callee(parsed_callee, parsed_fn_ty)?;
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
            ParsedCallee::Indirect(_) => {
                // A non-inline-asm callbr with an indirect callee is invalid
                // IR upstream too (`Verifier::visitCallBrInst` requires a
                // direct callee — "Callbr: indirect function / invalid
                // signature"), so rejecting it at parse reaches the same
                // verdict.
                return Err(self.expected("direct function callee for callbr"));
            }
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

    /// Parse `none` or a local token as a parent-pad value for EH pads.
    fn parse_optional_pad_token(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<Option<llvmkit_ir::Value<'ctx, B>>> {
        if matches!(self.peek(), Token::Kw(Keyword::None)) {
            self.bump()?;
            return Ok(None);
        }
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
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
                let v = self.parse_value(state, ty)?;
                args.push(v);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RSquare, "']' to close pad argument list")?;
        Ok(args)
    }

    fn builder_err(&self, label: &str, e: IrError) -> ParseError {
        ParseError::Expected {
            expected: format!("valid {label}: {e}").into(),
            loc: DiagLoc::span(self.loc()),
        }
    }

    /// Parse a `label %name` / `label %N` operand. Forward references create
    /// an empty block, but existing references return label identity only so
    /// branches may target already-terminated blocks.
    fn parse_block_ref(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> {
        let loc = self.loc();
        match self.peek() {
            Token::LocalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("block label name"))?;
                self.bump()?;
                if !state.defined_blocks.contains(&name) {
                    state.block_refs.entry(name.clone()).or_insert(loc);
                }
                state.ensure_block_label(self.module, &name, loc)
            }
            Token::LocalVarId(id) => {
                let id = *id;
                self.bump()?;
                state.get_or_create_numbered_block_label(self.module, id, loc)
            }
            _ => Err(self.expected("block label after 'label'")),
        }
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
    fn parse_fp_literal(
        &mut self,
        float_ty: &FloatType<'ctx, FloatDyn, B>,
    ) -> ParseResult<ApFloat> {
        use super::ll_token::FpLit;
        let value = match self.peek() {
            Token::FloatLit(fp) => match *fp {
                FpLit::Decimal(s) => {
                    ApFloat::from_string(float_ty.semantics(), s, RoundingMode::NearestTiesToEven)
                        .map(|(value, _status)| value)
                        .map_err(|_| self.expected("valid decimal float literal"))?
                }
                FpLit::HexDouble(s) => {
                    let value = parse_hex_apfloat(ApFloatSemantics::IeeeDouble, s)
                        .map_err(|_| self.expected("valid hex double literal"))?;
                    if float_ty.semantics() == ApFloatSemantics::IeeeDouble {
                        value
                    } else {
                        value
                            .convert(float_ty.semantics(), RoundingMode::NearestTiesToEven)
                            .0
                    }
                }
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
fn check_valid_variable_type<'ctx, B: ModuleBrand + 'ctx>(
    loc: Span,
    reference: LocalRef<'_>,
    ty: Type<'ctx, B>,
    value: llvmkit_ir::Value<'ctx, B>,
) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
    let value_ty = value.ty();
    if value_ty == ty {
        return Ok(value);
    }
    if ty.is_label() {
        return Err(ParseError::NotABasicBlock {
            name: reference.display(),
            loc: DiagLoc::span(loc),
        });
    }
    Err(ParseError::DefinedWithWrongType {
        name: reference.display(),
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
    blocks: std::collections::HashMap<String, llvmkit_ir::Value<'ctx, B>>,
    block_refs: std::collections::HashMap<String, Span>,
    defined_blocks: std::collections::HashSet<String>,
    /// `%N` block placeholder identities and definitions, keyed by the shared
    /// local numbered-value slot.
    numbered_blocks: std::collections::HashMap<u32, llvmkit_ir::Value<'ctx, B>>,
    numbered_block_refs: std::collections::HashMap<u32, Span>,
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
            blocks,
            block_refs: std::collections::HashMap::new(),
            defined_blocks: std::collections::HashSet::new(),
            numbered_blocks: std::collections::HashMap::new(),
            numbered_block_refs: std::collections::HashMap::new(),
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

    /// Look up or lazily create the named basic block. Mirrors
    /// `PerFunctionState::getBB(StringRef)`: named forward references create
    /// the block in advance and the label definition later marks it defined.
    fn ensure_block(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        name: &str,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        if let Some(value) = self.blocks.get(name).copied() {
            return self.value_as_block(module, value, loc);
        }
        let bb = self.func.append_basic_block(module, name);
        self.blocks.insert(name.to_owned(), bb.to_erased());
        Ok(bb)
    }

    fn ensure_block_label(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        name: &str,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BlockId<llvmkit_ir::Dyn, B>> {
        if let Some(value) = self.blocks.get(name).copied() {
            return self.value_as_block_label(value, loc);
        }
        let bb = self.func.append_basic_block(module, name);
        self.blocks.insert(name.to_owned(), bb.to_erased());
        Ok(bb.id())
    }

    /// Define a textual basic block label.
    fn define_named_block(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        name: String,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        self.defined_blocks.insert(name.clone());
        self.ensure_block(module, &name, loc)
    }

    /// Define an unlabeled block at `NumberedVals.getNext()`, matching
    /// `PerFunctionState::defineBB(Name.empty())`.
    fn define_implicit_block(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        let id = self.next_unnamed_value_id;
        self.define_numbered_block(module, id, loc)
    }

    fn define_numbered_label(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        id: u32,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        if self.defined_numbered_blocks.contains(&id) {
            return Err(ParseError::Redefinition {
                kind: SymbolKind::Block,
                id: SymbolId::Numbered(id),
                loc: DiagLoc::span(loc),
            });
        }
        check_value_id("label", "", self.next_unnamed_value_id, id, loc)?;
        self.define_numbered_block(module, id, loc)
    }

    fn define_numbered_block(
        &mut self,
        module: &'ctx Module<B, Unverified>,
        id: u32,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        if self.defined_numbered_blocks.contains(&id) {
            return Err(ParseError::Redefinition {
                kind: SymbolKind::Block,
                id: SymbolId::Numbered(id),
                loc: DiagLoc::span(loc),
            });
        }
        if self.local_numbered.contains_key(&id) {
            return Err(self.invalid_numbered_slot(id, loc));
        }
        let bb = if let Some(value) = self.numbered_blocks.get(&id).copied() {
            self.value_as_block(module, value, loc)?
        } else {
            let bb = self.func.append_basic_block(module, "");
            self.numbered_blocks.insert(id, bb.to_erased());
            bb
        };
        let bb_value = bb.to_erased();
        self.func
            .move_basic_block_to_end(module, bb)
            .map_err(|e| ParseError::Expected {
                expected: format!("numbered basic block definition: {e}").into(),
                loc: DiagLoc::span(loc),
            })?;
        self.local_numbered.insert(id, bb_value);
        self.defined_numbered_blocks.insert(id);
        self.numbered_block_refs.remove(&id);
        self.next_unnamed_value_id = self.next_unnamed_value_id.max(id.saturating_add(1));
        self.value_as_block(module, bb_value, loc)
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
                .then(|| self.blocks.get(name).copied())
                .flatten(),
            BlockLabel::Numbered(id) => self
                .defined_numbered_blocks
                .contains(id)
                .then(|| self.numbered_blocks.get(id).copied())
                .flatten(),
        }
    }

    fn value_as_block_view(
        &self,
        value: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Terminated, B>> {
        self.func
            .basic_blocks()
            .find(|bb| bb.to_erased() == value)
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
        if let Some(value) = self.local_numbered.get(&id).copied() {
            return self.value_as_block_label(value, loc);
        }
        // Upstream reaches this rejection one step later: `getBB(ID)` creates
        // a forward-reference block unconditionally and the *definition* runs
        // `checkValueID`. llvmkit has no block forward-reference placeholder
        // yet, so the backward slot is caught here at the reference instead.
        // Same verdict and same wording; the location differs by one token.
        // Folds into the definition site when forward references land.
        check_value_id("label", "", self.next_unnamed_value_id, id, loc)?;
        let label = if let Some(value) = self.numbered_blocks.get(&id).copied() {
            self.value_as_block_label(value, loc)?
        } else {
            let bb = self.func.append_basic_block(module, "");
            self.numbered_blocks.insert(id, bb.to_erased());
            bb.id()
        };
        self.numbered_block_refs.entry(id).or_insert(loc);
        Ok(label)
    }

    /// Resolve a phi-incoming predecessor block reference for an edge-add.
    ///
    /// Unlike block *construction*, a phi predecessor is a label reference and
    /// is usually already terminated (the common merge-block / diamond-tail
    /// case), so this resolves through the state-agnostic label path and
    /// returns a view rather than an [`Unterminated`] construction handle. The
    /// block was ensured to exist when the phi incoming pair was parsed
    /// (`parse_phi_label`). Only phi resolution uses this; branch/switch
    /// targets go through `parse_block_ref`.
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

    /// Look up a function-local value, minting a forward-reference
    /// placeholder when the name has not been defined yet. Mirrors
    /// `LLParser::PerFunctionState::getVal`: symbol table, then the
    /// forward-reference map, then a fresh sentinel of the demanded type.
    ///
    /// Blocks are consulted alongside values because upstream keeps both in
    /// the function's one value symbol table, which is what makes
    /// `'%x' is not a basic block` and `'%x' defined with type 'label'`
    /// reachable.
    fn get_val(
        &self,
        module: &'ctx Module<B, Unverified>,
        reference: LocalRef<'_>,
        ty: Type<'ctx, B>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let existing = match reference {
            LocalRef::Named(name) => self
                .local_named
                .get(name)
                .copied()
                .or_else(|| self.blocks.get(name).copied())
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
                .or_else(|| self.numbered_blocks.get(&id).copied())
                .or_else(|| {
                    self.forward_ref_numbered
                        .borrow()
                        .get(&id)
                        .map(|entry| entry.placeholder.as_value())
                }),
        };
        if let Some(value) = existing {
            return check_valid_variable_type(loc, reference, ty, value);
        }
        // "Don't make placeholders with invalid type" — upstream refuses a
        // sentinel it could not give a type to.
        if !ty.is_first_class() {
            return Err(ParseError::Message {
                message: "invalid use of a non-first-class type".into(),
                loc: DiagLoc::span(loc),
            });
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
            return match lhs {
                LocalLhs::None => Ok(()),
                LocalLhs::Named(_) | LocalLhs::Numbered(_) => Err(ParseError::Message {
                    message: "instructions returning void cannot have a name".into(),
                    loc: DiagLoc::span(loc),
                }),
            };
        }
        match lhs {
            LocalLhs::Named(n) => {
                let forward = self.forward_ref_named.borrow_mut().remove(n.as_str());
                if let Some(entry) = forward {
                    Self::resolve_forward_ref(entry, v, loc)?;
                }
                if self.local_named.insert(n.clone(), v).is_some() {
                    return Err(ParseError::Redefinition {
                        kind: SYMBOL_KIND_LOCAL,
                        id: SymbolId::Named(n.clone()),
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
                if let Some(block) = self.numbered_blocks.get(&id).copied() {
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
        for (name, loc) in &self.block_refs {
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
        for (id, loc) in &self.numbered_block_refs {
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

fn numbered_label_id(name: &str) -> Option<u32> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    name.parse().ok()
}

enum ParamName {
    Named(String),
    Numbered(u32),
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
