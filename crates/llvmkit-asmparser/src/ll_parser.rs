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

use core::marker::PhantomData;
use llvmkit_ir::attributes::{
    AttrIndex, AttrKind, Attribute, AttributeStorage, MemoryEffects, MemoryLocation, ModRefInfo,
};
use std::collections::HashMap;

use llvmkit_ir::{
    Align, AllocaFlags, AnyTypeEnum, ApFloat, ApFloatSemantics, ApInt, ApIntSignedness,
    AtomicLoadConfig, AtomicOrdering, AtomicRMWBinOp, AtomicStoreConfig, BasicBlockLabel, Brand,
    CallingConv, Constant, ConstantExprFlags, ConstantExprInRange, ConstantExprOpcode,
    ConstantExprOptions, DllStorageClass, Dyn, FastMathFlags, FloatDyn, FloatPredicate, FloatType,
    FloatValue, GepNoWrapFlags, IRBuilder, IntDyn, IntType, IntValue, IntrinsicNameResolution,
    IrError, IrResult, Linkage, MaybeAlign, Module, ModuleBrand, NoFolder, PointerValue,
    Positioned, RoundingMode, SelectionKind, StructType, SyncScope, ThreadLocalMode, Type,
    TypeKind, UIToFpFlags, UnnamedAddr, Unverified, UseListOrderBBRecord, UseListOrderRecord,
    Visibility, constant_fold_select_instruction, derived_types::PointerType,
    resolve_intrinsic_name, shufflevector_mask_from_constant,
};
use llvmkit_support::{Span, Spanned};

use super::ll_lexer::{LexError, Lexer};
use super::ll_token::{IntLit, Keyword, NumBase, PrimitiveTy, Sign, Token};
use super::numbered_values::NumberedValues;
use super::parse_error::{DiagLoc, ParseError, ParseResult};
use super::slot_mapping::{GlobalRef, SlotMapping};

type ParsedGlobalInitializer<'ctx, B> = (
    Option<Constant<'ctx, B>>,
    Option<DeferredConstantKind<'ctx, B>>,
);

type ParsedValueOrDeferredLocal<'ctx, B> = (
    llvmkit_ir::Value<'ctx, B>,
    Option<(DeferredLocalValueRef, Span)>,
);

// ── Identity of the next lookahead token ────────────────────────────────────

/// One-byte description of a `Token` kind for "expected ..." diagnostics.
///
/// Mirrors `lltok::describe`-style usage in `LLParser.cpp` -- upstream
/// embeds the description inline in `tokError("expected X")`; we keep the
/// description on the static side so structured errors stay matchable
/// without locale-sensitive string comparisons.
pub fn describe(t: &Token<'_>) -> String {
    match t {
        Token::Eof => "<eof>".into(),
        Token::DotDotDot => "'...'".into(),
        Token::Equal => "'='".into(),
        Token::Comma => "','".into(),
        Token::Star => "'*'".into(),
        Token::LSquare => "'['".into(),
        Token::RSquare => "']'".into(),
        Token::LBrace => "'{'".into(),
        Token::RBrace => "'}'".into(),
        Token::Less => "'<'".into(),
        Token::Greater => "'>'".into(),
        Token::LParen => "'('".into(),
        Token::RParen => "')'".into(),
        Token::Exclaim => "'!'".into(),
        Token::Bar => "'|'".into(),
        Token::Colon => "':'".into(),
        Token::Hash => "'#'".into(),
        Token::LabelStr(_) => "label".into(),
        Token::GlobalVar(_) | Token::GlobalId(_) => "global identifier".into(),
        Token::LocalVar(_) | Token::LocalVarId(_) => "local identifier".into(),
        Token::ComdatVar(_) => "comdat identifier".into(),
        Token::MetadataVar(_) => "metadata reference".into(),
        Token::StringConstant(_) => "string constant".into(),
        Token::AttrGrpId(_) => "attribute group id".into(),
        Token::SummaryId(_) => "summary id".into(),
        Token::IntegerLit(_) => "integer constant".into(),
        Token::FloatLit(_) => "floating-point constant".into(),
        Token::PrimitiveType(_) => "primitive type".into(),
        Token::Instruction(_) => "instruction opcode".into(),
        Token::Kw(k) => format!("keyword '{}'", keyword_text(*k)),
        // The DWARF-flavoured tokens are intentionally minimal — they only
        // appear inside metadata, which the parser does not yet cover.
        // Naming the family is sufficient for the diagnostics callers
        // currently see.
        Token::DwarfTag(_) => "DWARF tag".into(),
        Token::DwarfAttEncoding(_) => "DWARF attribute encoding".into(),
        Token::DwarfVirtuality(_) => "DWARF virtuality".into(),
        Token::DwarfLang(_) => "DWARF language".into(),
        Token::DwarfSourceLangName(_) => "DWARF source language name".into(),
        Token::DwarfCC(_) => "DWARF calling convention".into(),
        Token::DwarfOp(_) => "DWARF operation".into(),
        Token::DwarfMacinfo(_) => "DWARF macinfo".into(),
        Token::DwarfEnumKind(_) => "DWARF enum kind".into(),
        Token::DiFlag(_) => "DI flag".into(),
        Token::DiSpFlag(_) => "DI subprogram flag".into(),
        Token::ChecksumKind(_) => "checksum kind".into(),
        Token::EmissionKind(_) => "emission kind".into(),
        Token::NameTableKind(_) => "name-table kind".into(),
        Token::FixedPointKind(_) => "fixed-point kind".into(),
        Token::SpecializedMetadata(_) => "specialized metadata kind".into(),
        Token::DbgRecordType(_) => "dbg record type".into(),
    }
}

fn keyword_text(k: Keyword) -> &'static str {
    // Only the keywords the parser currently reaches; other arms fall back to
    // a generic label. Later revisions extend the table opportunistically.
    match k {
        Keyword::Target => "target",
        Keyword::Triple => "triple",
        Keyword::Datalayout => "datalayout",
        Keyword::SourceFilename => "source_filename",
        Keyword::Module => "module",
        Keyword::Asm => "asm",
        Keyword::Type => "type",
        Keyword::Declare => "declare",
        Keyword::Define => "define",
        Keyword::Global => "global",
        Keyword::Constant => "constant",
        Keyword::External => "external",
        Keyword::Internal => "internal",
        Keyword::Private => "private",
        Keyword::Common => "common",
        Keyword::Addrspace => "addrspace",
        Keyword::Opaque => "opaque",
        Keyword::Zeroinitializer => "zeroinitializer",
        Keyword::Null => "null",
        Keyword::None => "none",
        Keyword::Undef => "undef",
        Keyword::Poison => "poison",
        Keyword::True => "true",
        Keyword::False => "false",
        Keyword::X => "x",
        _ => "<keyword>",
    }
}

// ── Type pre-resolution table (mirrors LLParser::NamedTypes / NumberedTypes) ─

/// A type that has been parsed but may carry an unresolved forward
/// reference to an opaque-named struct. Mirrors the
/// `std::pair<Type *, LocTy>` entries in `LLParser`'s `NamedTypes` /
/// `NumberedTypes` maps: we keep the type handle plus the location of the
/// most recent forward reference so `validateEndOfModule` can
/// blame the right span if the definition never lands.
#[derive(Debug, Clone, Copy)]
struct TypeEntry<'ctx, B: ModuleBrand = Brand<'ctx>> {
    ty: Type<'ctx, B>,
}

struct MetadataSlotEntry {
    id: llvmkit_ir::metadata::MetadataId,
    defined: bool,
    first_ref: Span,
}

struct FunctionSuffix<'ctx, B: ModuleBrand = Brand<'ctx>> {
    attr_groups: Vec<u32>,
    section: Option<String>,
    partition: Option<String>,
    comdat: Option<Option<String>>,
    align: MaybeAlign,
    gc: Option<String>,
    prefix_data: Option<llvmkit_ir::Constant<'ctx, B>>,
    prologue_data: Option<llvmkit_ir::Constant<'ctx, B>>,
    personality_fn: Option<ParsedPersonalityFn<'ctx, B>>,
    metadata: Vec<(
        llvmkit_ir::metadata::MetadataAttachmentKind,
        llvmkit_ir::metadata::MetadataId,
    )>,
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

enum ParsedPersonalityFn<'ctx, B: ModuleBrand = Brand<'ctx>> {
    Resolved(llvmkit_ir::Constant<'ctx, B>),
    ForwardName { name: String, loc: Span },
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// Core parser state. Holds the lexer, a one-token cache, the IR module
/// being populated, and the slot tables that mirror upstream's
/// `LLParser::NumberedTypes` / `NamedTypes` / `NumberedVals` fields.
pub struct Parser<'src, 'm, 'ctx, B: ModuleBrand = Brand<'ctx>> {
    lex: Lexer<'src>,
    src: &'src [u8],
    /// Most recently produced token. The constructor primes this with the
    /// first token (mirrors `LLParser::Run`'s leading `Lex.Lex();`).
    current: Spanned<Token<'src>>,

    /// The module token being populated.
    module: &'m Module<'ctx, B, Unverified>,

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

    /// Maps a textual metadata slot (`!N`) to the `MetadataId` it names and
    /// whether a matching `!N = ...` definition was seen.
    metadata_slots: HashMap<u32, MetadataSlotEntry>,
    deferred_global_initializers: Vec<DeferredGlobalInitializer<'ctx, B>>,
    deferred_block_addresses: Vec<DeferredBlockAddress<'ctx, B>>,
    deferred_personality_fns: Vec<DeferredPersonalityFn<'ctx, B>>,
    deferred_intrinsic_attribute_checks: Vec<DeferredIntrinsicAttributeCheck>,
    forward_function_decls: HashMap<String, Span>,
    _brand: PhantomData<B>,
}

/// What the parser produces at end-of-module. Successful runs return the
/// module-level slot mapping so callers can re-use it for follow-on
/// `parse_constant_value` / `parse_type` calls (mirrors upstream's
/// `parseAssemblyString(..., SlotMapping *)` pattern).
#[derive(Debug, Default)]
pub struct ParsedModule<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub slot_mapping: SlotMapping<'ctx, B>,
    pub summary_index: Option<crate::module_summary::ModuleSummaryIndex>,
}

enum DeferredConstantKind<'ctx, B: ModuleBrand = Brand<'ctx>> {
    RawInitializer { ty: Type<'ctx, B>, span: Span },
}

struct DeferredGlobalInitializer<'ctx, B: ModuleBrand = Brand<'ctx>> {
    global: llvmkit_ir::GlobalVariable<'ctx, B>,
    value: DeferredConstantKind<'ctx, B>,
}

struct DeferredBlockAddress<'ctx, B: ModuleBrand = Brand<'ctx>> {
    placeholder: llvmkit_ir::BlockAddressPlaceholder<'ctx, B>,
    function: NameOrId,
    label: String,
    loc: Span,
}

struct DeferredPersonalityFn<'ctx, B: ModuleBrand = Brand<'ctx>> {
    function: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>,
    name: String,
    loc: Span,
}

struct DeferredIntrinsicAttributeCheck {
    attrs: AttributeStorage,
    attr_groups: Vec<u32>,
    expected_attrs: AttributeStorage,
    loc: Span,
}

enum ParsedBlockAddressFunction<'ctx, B: ModuleBrand = Brand<'ctx>> {
    Resolved(llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>),
    Forward { function: NameOrId, loc: Span },
}

enum ParsedDirectCallee<'ctx, B: ModuleBrand = Brand<'ctx>> {
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

struct ParsedInlineAsm {
    asm: String,
    constraints: String,
    has_side_effects: bool,
    is_align_stack: bool,
    dialect: llvmkit_ir::AsmDialect,
    can_unwind: bool,
}

enum ParsedCallee<'ctx, B: ModuleBrand = Brand<'ctx>> {
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
        signedness: ApIntSignedness,
        value: ApInt,
    },
}

#[derive(Debug, Clone, Copy)]
enum ExpectedIntWidth {
    Infer,
    Bits(u32),
}

#[derive(Debug)]
enum ValId<'ctx, B: ModuleBrand = Brand<'ctx>> {
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
            ApIntSignedness::Unsigned => value.zext_or_trunc(dest_width),
            ApIntSignedness::Signed => value.sext_or_trunc(dest_width),
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
            ApIntSignedness::Unsigned => i128::try_from(value.try_zext_u128()?).ok(),
            ApIntSignedness::Signed => value.try_sext_i128(),
        },
    }
}

fn is_supported_constant_expr_opcode(op: crate::ll_token::Opcode) -> bool {
    matches!(
        op,
        crate::ll_token::Opcode::GetElementPtr
            | crate::ll_token::Opcode::BitCast
            | crate::ll_token::Opcode::AddrSpaceCast
            | crate::ll_token::Opcode::IntToPtr
            | crate::ll_token::Opcode::PtrToInt
            | crate::ll_token::Opcode::PtrToAddr
            | crate::ll_token::Opcode::Trunc
            | crate::ll_token::Opcode::Add
            | crate::ll_token::Opcode::Sub
            | crate::ll_token::Opcode::Xor
            | crate::ll_token::Opcode::ExtractElement
            | crate::ll_token::Opcode::InsertElement
            | crate::ll_token::Opcode::ShuffleVector
    )
}

fn linkage_keyword(keyword: Keyword) -> Option<Linkage> {
    Some(match keyword {
        Keyword::External => Linkage::External,
        Keyword::AvailableExternally => Linkage::AvailableExternally,
        Keyword::Linkonce => Linkage::LinkOnceAny,
        Keyword::LinkonceOdr => Linkage::LinkOnceODR,
        Keyword::Weak => Linkage::WeakAny,
        Keyword::WeakOdr => Linkage::WeakODR,
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

fn is_int_or_int_vector_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> bool {
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Int(_) => true,
        AnyTypeEnum::Vector(v) => v.element().is_integer(),
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
fn type_contains_scalable_vector<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> bool {
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Vector(v) => v.is_scalable() || type_contains_scalable_vector(v.element()),
        AnyTypeEnum::Array(a) => type_contains_scalable_vector(a.element()),
        AnyTypeEnum::Struct(s) => (0..s.field_count()).any(|index| {
            s.field_type(index)
                .is_some_and(type_contains_scalable_vector)
        }),
        _ => false,
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

impl<'src, 'm, 'ctx, B: ModuleBrand + 'ctx> Parser<'src, 'm, 'ctx, B> {
    /// Construct a parser over `src`, populating `module`. Primes the lexer
    /// once (mirrors `LLParser::Run`'s leading `Lex.Lex()`).
    pub fn new(src: &'src [u8], module: &'m Module<'ctx, B, Unverified>) -> ParseResult<Self> {
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
            deferred_intrinsic_attribute_checks: Vec::new(),
            forward_function_decls: HashMap::new(),
            _brand: PhantomData,
        })
    }

    pub fn with_slot_mapping(
        src: &'src [u8],
        module: &'m Module<'ctx, B, Unverified>,
        slots: &SlotMapping<'ctx, B>,
    ) -> ParseResult<Self> {
        let mut parser = Self::new(src, module)?;
        parser.numbered_globals = slots.global_values.clone();
        parser.numbered_attr_groups = slots.attribute_groups.clone();
        parser.named_types = slots
            .named_types
            .iter()
            .map(|(name, ty)| (name.clone(), TypeEntry { ty: *ty }))
            .collect();
        parser.numbered_types = slots
            .numbered_types
            .iter()
            .map(|(id, ty)| (*id, TypeEntry { ty: *ty }))
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
        module: &'m Module<'ctx, B, Unverified>,
        _context: &'ctx mut crate::asm_parser_context::AsmParserContext<'ctx, B>,
    ) -> ParseResult<Self> {
        Self::new(src, module)
    }

    fn resolve_md_slot(&mut self, slot: u32, loc: Span) -> llvmkit_ir::metadata::MetadataId {
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
        content: llvmkit_ir::metadata::MetadataKind,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::metadata::MetadataId> {
        if let Some(entry) = self.metadata_slots.get_mut(&slot) {
            if entry.defined {
                return Err(ParseError::Redefinition {
                    kind: crate::parse_error::SymbolKind::Metadata,
                    id: crate::parse_error::SymbolId::Numbered(slot),
                    loc: DiagLoc::span(loc),
                });
            }
            self.module.metadata_set(entry.id, content);
            entry.defined = true;
            return Ok(entry.id);
        }

        let id = self.module.metadata_reserve();
        self.module.metadata_set(id, content);
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

        // Forward-referenced types that never received a definition stay
        // opaque. That matches upstream's behavior — `validateEndOfModule`
        // does not error on opaque structs that were referenced but never
        // bodied; only `LLParser`'s `error(forward_loc, ...)` paths flag
        // the truly malformed cases.

        for (slot, entry) in &self.metadata_slots {
            if !entry.defined {
                return Err(ParseError::UndefinedSymbol {
                    kind: crate::parse_error::SymbolKind::Metadata,
                    id: crate::parse_error::SymbolId::Numbered(*slot),
                    loc: DiagLoc::span(entry.first_ref),
                });
            }
        }

        self.resolve_deferred_global_initializers()?;
        self.resolve_deferred_block_addresses()?;
        self.resolve_deferred_personality_fns()?;
        self.validate_deferred_intrinsic_attribute_checks()?;
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
                    crate::parser::parse_constant_value(bytes, self.module, ty, Some(&slots))
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

    fn resolve_deferred_block_addresses(&mut self) -> ParseResult<()> {
        let deferred = std::mem::take(&mut self.deferred_block_addresses);
        for item in deferred {
            let function = match &item.function {
                NameOrId::Name(name) => self.module.function_by_name_dyn(name),
                NameOrId::Id(id) => self.numbered_globals.get(*id).and_then(|r| match r {
                    GlobalRef::Function(f) => Some(*f),
                    _ => None,
                }),
            }
            .ok_or_else(|| ParseError::Expected {
                expected: "function name in blockaddress".into(),
                loc: DiagLoc::span(item.loc),
            })?;
            if function.basic_blocks().len() == 0 {
                return Err(ParseError::Expected {
                    expected: "cannot take blockaddress inside a declaration".into(),
                    loc: DiagLoc::span(item.loc),
                });
            }
            let block = function
                .basic_blocks()
                .find(|bb| bb.name().as_deref() == Some(item.label.as_str()))
                .ok_or_else(|| ParseError::Expected {
                    expected: "referenced value is not a basic block".into(),
                    loc: DiagLoc::span(item.loc),
                })?;
            let resolved = self
                .module
                .block_address(function, &block)
                .map_err(|e| self.builder_err("blockaddress", e))?;
            item.placeholder
                .replace_all_uses_with(resolved)
                .map_err(|e| self.builder_err("forward blockaddress", e))?;
        }
        Ok(())
    }

    fn resolve_deferred_personality_fns(&mut self) -> ParseResult<()> {
        let deferred = std::mem::take(&mut self.deferred_personality_fns);
        for item in deferred {
            let personality = self
                .resolve_global_name_as_constant(item.name.clone())
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

    fn validate_forward_function_decls(&self) -> ParseResult<()> {
        if let Some((name, loc)) = self.forward_function_decls.iter().next() {
            return Err(ParseError::UndefinedSymbol {
                kind: crate::parse_error::SymbolKind::Global,
                id: crate::parse_error::SymbolId::Named(name.clone()),
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
        let mut attr_entries: Vec<_> = self.module.attribute_groups();
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
        let mut attr_entries: Vec<_> = self.module.attribute_groups();
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
    fn expect_punct(&mut self, t: PunctKind, expected: &str) -> ParseResult<Span> {
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

    fn expect_keyword(&mut self, k: Keyword, expected: &str) -> ParseResult<Span> {
        if matches!(self.peek(), Token::Kw(got) if *got == k) {
            self.bump()
        } else {
            Err(self.expected(expected))
        }
    }

    fn expect_primitive(
        &mut self,
        p: crate::ll_token::PrimitiveTy,
        expected: &str,
    ) -> ParseResult<Span> {
        if matches!(self.peek(), Token::PrimitiveType(got) if *got == p) {
            self.bump()
        } else {
            Err(self.expected(expected))
        }
    }

    fn token_error(&self, expected: &str) -> ParseError {
        self.expected(expected)
    }

    fn expected(&self, expected: &str) -> ParseError {
        ParseError::Expected {
            expected: expected.into(),
            loc: DiagLoc::span(self.loc()),
        }
    }

    /// Consume a `STRINGCONSTANT` token and decode it as UTF-8. Mirrors
    /// `LLParser::parseStringConstant`.
    fn parse_string_constant(&mut self, expected: &str) -> ParseResult<String> {
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
    fn parse_addr_space_paren(&mut self) -> ParseResult<u32> {
        self.expect_punct(PunctKind::LParen, "'(' in addrspace")?;
        let n = self.parse_uint32("address space (uint32)")?;
        self.expect_punct(PunctKind::RParen, "')' in addrspace")?;
        Ok(n)
    }

    fn parse_uint32(&mut self, expected: &str) -> ParseResult<u32> {
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

    fn parse_uint64(&mut self, expected: &str) -> ParseResult<u64> {
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
                    ApIntSignedness::Signed
                } else {
                    ApIntSignedness::Unsigned
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
            expected: format!("alignment must be non-zero power of two, got {n}"),
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
    fn parse_atomic_ordering(&mut self, expected: &str) -> ParseResult<AtomicOrdering> {
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
    /// Mirrors `LLParser::parseOptionalScope` (LLParser.cpp ~2826).
    fn parse_optional_syncscope(&mut self) -> ParseResult<SyncScope> {
        if !matches!(self.peek(), Token::Kw(Keyword::Syncscope)) {
            return Ok(SyncScope::System);
        }
        self.bump()?; // eat `syncscope`
        self.expect_punct(PunctKind::LParen, "'(' after syncscope")?;
        let name = self.parse_string_constant("sync scope name")?;
        self.expect_punct(PunctKind::RParen, "')' after sync scope")?;
        Ok(match name.as_str() {
            "system" => SyncScope::System,
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
                self.module.set_data_layout(s).map_err(|e| match e {
                    IrError::InvalidDataLayout { reason } => ParseError::Expected {
                        expected: format!("valid datalayout: {reason}"),
                        loc: DiagLoc::span(loc),
                    },
                    other => ParseError::Expected {
                        expected: format!("valid datalayout: {other}"),
                        loc: DiagLoc::span(loc),
                    },
                })?;
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

    fn parse_comdat_definition(&mut self) -> ParseResult<()> {
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
        let comdat = self.module.get_or_insert_comdat(&name);
        comdat.set_selection_kind(kind);
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
        UseListOrderRecord::new(value.id(), ty.id(), indexes).map_err(|e| match e {
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
                self.module.function_by_name_dyn(&name).ok_or_else(|| {
                    ParseError::UndefinedSymbol {
                        kind: crate::parse_error::SymbolKind::Global,
                        id: crate::parse_error::SymbolId::Named(name),
                        loc: DiagLoc::span(loc),
                    }
                })?
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
                        kind: crate::parse_error::SymbolKind::Block,
                        id: crate::parse_error::SymbolId::Named(name),
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
                    kind: crate::parse_error::SymbolKind::Block,
                    id: crate::parse_error::SymbolId::Numbered(id),
                    loc: DiagLoc::span(loc),
                })?
            }
            _ => return Err(self.expected("basic block in uselistorder_bb")),
        };
        self.expect_punct(PunctKind::Comma, "',' before uselistorder_bb indexes")?;
        let indexes = self.parse_use_list_order_indexes()?;
        let record =
            UseListOrderBBRecord::new(function.as_value().id(), block.as_value().id(), indexes)
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
        use llvmkit_ir::metadata::MetadataRef;

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
        let nmd_idx = self.module.get_or_insert_named_metadata(&name);

        // Parse comma-separated `!N` operands
        if !matches!(self.peek(), Token::RBrace) {
            loop {
                self.expect_exclaim("'!' before metadata operand")?;
                let loc = self.loc();
                let slot = self.parse_uint32("metadata operand number")?;
                let id = self.resolve_md_slot(slot, loc);
                self.module
                    .named_metadata_add_operand(nmd_idx, MetadataRef(id));
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
            let id = self.module.metadata_node(kind);
            return Ok(self.module.metadata_as_value(id));
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
                self.module.metadata_tuple(operands)
            }
            Token::SpecializedMetadata(_) | Token::MetadataVar(_) => {
                let kind = self.parse_md_node_after_bang(false)?;
                self.module.metadata_node(kind)
            }
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32("metadata slot number after '!'")?;
                self.resolve_md_slot(slot, loc)
            }
        };
        Ok(self.module.metadata_as_value(id))
    }

    fn parse_metadata_attachment_operand(
        &mut self,
    ) -> ParseResult<llvmkit_ir::metadata::MetadataId> {
        match self.peek() {
            Token::MetadataVar(_) => {
                let kind = self.parse_md_node_after_bang(false)?;
                Ok(self.module.metadata_node(kind))
            }
            Token::Exclaim => {
                self.bump()?;
                match self.peek() {
                    Token::LBrace | Token::SpecializedMetadata(_) | Token::MetadataVar(_) => {
                        let kind = self.parse_md_node_after_bang(false)?;
                        Ok(self.module.metadata_node(kind))
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
    ) -> ParseResult<(
        llvmkit_ir::metadata::MetadataAttachmentKind,
        llvmkit_ir::metadata::MetadataId,
    )> {
        let name = match self.peek() {
            Token::MetadataVar(bytes) => std::str::from_utf8(bytes.as_ref())
                .map_err(|_| self.expected("valid UTF-8 metadata attachment name"))?
                .to_owned(),
            _ => return Err(self.expected("metadata attachment")),
        };
        self.bump()?;
        let id = self.parse_metadata_attachment_operand()?;
        Ok((
            llvmkit_ir::metadata::MetadataAttachmentKind::from_name(&name),
            id,
        ))
    }

    /// Parse a single metadata tuple operand: an inline `!"string"`
    /// (interned and referenced) or a numbered `!N` reference. The
    /// inline-string form is what the AsmWriter emits for `MDString`
    /// tuple operands (`!{!"rsp"}`), so this keeps writer output
    /// round-trippable.
    fn parse_md_tuple_operand(&mut self) -> ParseResult<llvmkit_ir::metadata::MetadataRef> {
        use llvmkit_ir::metadata::MetadataRef;
        if matches!(self.peek(), Token::Kw(Keyword::Null)) {
            self.bump()?;
            let id = self
                .module
                .metadata_node(llvmkit_ir::metadata::MetadataKind::Null);
            return Ok(MetadataRef(id));
        }

        if matches!(self.peek(), Token::MetadataVar(_)) {
            let content = self.parse_md_node_after_bang(false)?;
            let id = self.module.metadata_node(content);
            return Ok(MetadataRef(id));
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
            let id = self.module.metadata_constant(constant);
            return Ok(MetadataRef(id));
        }

        self.expect_exclaim("'!' in metadata tuple operand")?;
        match self.peek() {
            Token::StringConstant(_) => {
                let s = self.parse_string_constant("metadata string operand")?;
                Ok(MetadataRef(self.module.metadata_string(s)))
            }
            Token::LBrace | Token::SpecializedMetadata(_) | Token::MetadataVar(_) => {
                let content = self.parse_md_node_after_bang(false)?;
                let id = self.module.metadata_node(content);
                Ok(MetadataRef(id))
            }
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32("metadata operand number")?;
                Ok(MetadataRef(self.resolve_md_slot(slot, loc)))
            }
        }
    }

    fn parse_md_node_after_bang(
        &mut self,
        distinct: bool,
    ) -> ParseResult<llvmkit_ir::metadata::MetadataKind> {
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
                self.bump()?;
                self.expect_punct(PunctKind::LParen, "'(' in specialized metadata")?;
                let mut fields = Vec::new();
                if !matches!(self.peek(), Token::RParen) {
                    loop {
                        let field_name = match self.peek() {
                            Token::LabelStr(bytes) => std::str::from_utf8(bytes.as_ref())
                                .map_err(|_| self.expected("valid UTF-8 metadata field name"))?
                                .to_owned(),
                            _ => return Err(self.expected("metadata field name")),
                        };
                        self.bump()?;
                        let value = self.parse_metadata_field_value()?;
                        fields.push(llvmkit_ir::metadata::MetadataField::new(field_name, value));
                        if !self.eat_punct(PunctKind::Comma)? {
                            break;
                        }
                    }
                }
                self.expect_punct(PunctKind::RParen, "')' closing specialized metadata")?;
                Ok(llvmkit_ir::metadata::MetadataKind::Specialized(
                    llvmkit_ir::metadata::SpecializedMetadataNode::new(kind)
                        .distinct(distinct)
                        .with_fields(fields),
                ))
            }
            Token::MetadataVar(bytes) => {
                let name = std::str::from_utf8(bytes.as_ref())
                    .map_err(|_| self.expected("specialized metadata kind"))?;
                let kind = llvmkit_ir::metadata::SpecializedMetadataKind::from_name(name)
                    .ok_or_else(|| self.expected("specialized metadata kind"))?;
                self.bump()?;
                self.expect_punct(PunctKind::LParen, "'(' in specialized metadata")?;
                let mut fields = Vec::new();
                if !matches!(self.peek(), Token::RParen) {
                    loop {
                        let field_name = match self.peek() {
                            Token::LabelStr(bytes) => std::str::from_utf8(bytes.as_ref())
                                .map_err(|_| self.expected("valid UTF-8 metadata field name"))?
                                .to_owned(),
                            _ => return Err(self.expected("metadata field name")),
                        };
                        self.bump()?;
                        let value = self.parse_metadata_field_value()?;
                        fields.push(llvmkit_ir::metadata::MetadataField::new(field_name, value));
                        if !self.eat_punct(PunctKind::Comma)? {
                            break;
                        }
                    }
                }
                self.expect_punct(PunctKind::RParen, "')' closing specialized metadata")?;
                Ok(llvmkit_ir::metadata::MetadataKind::Specialized(
                    llvmkit_ir::metadata::SpecializedMetadataNode::new(kind)
                        .distinct(distinct)
                        .with_fields(fields),
                ))
            }
            Token::StringConstant(_) => {
                let s = self.parse_string_constant("metadata string")?;
                Ok(llvmkit_ir::metadata::MetadataKind::String(s))
            }
            _ => Err(self.expected("metadata node")),
        }
    }

    fn parse_metadata_field_value(
        &mut self,
    ) -> ParseResult<llvmkit_ir::metadata::MetadataFieldValue> {
        use llvmkit_ir::metadata::{MetadataFieldValue, MetadataRef};
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
                Ok(MetadataFieldValue::Metadata(MetadataRef(
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
                        Ok(MetadataFieldValue::Metadata(MetadataRef(
                            self.module.metadata_string(s),
                        )))
                    }
                    Token::SpecializedMetadata(_) | Token::MetadataVar(_) => {
                        let content = self.parse_md_node_after_bang(false)?;
                        Ok(MetadataFieldValue::Metadata(MetadataRef(
                            self.module.metadata_node(content),
                        )))
                    }
                    _ => {
                        let loc = self.loc();
                        let slot = self.parse_uint32("metadata field metadata reference")?;
                        Ok(MetadataFieldValue::Metadata(MetadataRef(
                            self.resolve_md_slot(slot, loc),
                        )))
                    }
                }
            }
            Token::DwarfTag(s)
            | Token::DwarfAttEncoding(s)
            | Token::DwarfVirtuality(s)
            | Token::DwarfLang(s)
            | Token::DwarfSourceLangName(s)
            | Token::DwarfCC(s)
            | Token::DwarfOp(s)
            | Token::DwarfMacinfo(s)
            | Token::DwarfEnumKind(s)
            | Token::DiFlag(s)
            | Token::DiSpFlag(s)
            | Token::ChecksumKind(s)
            | Token::EmissionKind(s)
            | Token::NameTableKind(s)
            | Token::FixedPointKind(s) => {
                let value = (*s).to_owned();
                self.bump()?;
                Ok(MetadataFieldValue::Enum(value))
            }
            _ => Err(self.expected("metadata field value")),
        }
    }

    /// Consume a `!` token (Token::Exclaim). Helper for metadata parsing.
    fn expect_exclaim(&mut self, expected: &str) -> ParseResult<Span> {
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
    ) -> ParseResult<llvmkit_ir::metadata::DebugMetadataOperand> {
        if matches!(
            self.peek(),
            Token::Exclaim | Token::SpecializedMetadata(_) | Token::MetadataVar(_)
        ) {
            let id = self.parse_metadata_attachment_operand()?;
            return Ok(llvmkit_ir::metadata::DebugMetadataOperand::Metadata(
                llvmkit_ir::metadata::MetadataRef(id),
            ));
        }

        let ty = self.parse_type(false)?;
        let value = self.parse_value(state, ty)?;
        Ok(llvmkit_ir::metadata::DebugMetadataOperand::Value(
            value.id(),
        ))
    }

    fn parse_debug_record(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::metadata::DebugRecord> {
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
        pending_debug_records: &mut Vec<llvmkit_ir::metadata::DebugRecord>,
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
                inst.push_debug_record(record);
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
                inst.set_metadata(
                    llvmkit_ir::metadata::MetadataAttachmentKind::from_name(&name),
                    id,
                );
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
    fn parse_struct_definition(
        &mut self,
        name: Option<String>,
        slot: Option<u32>,
        decl_loc: Span,
    ) -> ParseResult<()> {
        // Resolve the *handle* the directive should populate.
        let handle: StructType<'ctx, llvmkit_ir::StructBodyDyn, B> = match (&name, slot) {
            (Some(n), None) => self.module.named_struct(n),
            (None, Some(id)) => {
                if id != self.next_unnamed_type_id {
                    return Err(ParseError::Expected {
                        expected: format!(
                            "monotonic numbered type id %{}",
                            self.next_unnamed_type_id
                        ),
                        loc: DiagLoc::span(decl_loc),
                    });
                }
                self.next_unnamed_type_id = self.next_unnamed_type_id.saturating_add(1);
                // Numbered types are anonymous in the IR; we still create a
                // fresh literal struct slot to represent the body.
                self.module
                    .struct_type(core::iter::empty::<Type<'ctx, B>>(), false)
            }
            _ => unreachable!("parse_struct_definition called without a name xor slot"),
        };

        // RHS: either `opaque` or a literal-struct body.
        match self.peek() {
            Token::Kw(Keyword::Opaque) => {
                self.bump()?;
                // Opaque named struct: nothing to set.
            }
            Token::LBrace | Token::Less => {
                let (elements, packed) = self.parse_struct_body()?;
                if name.is_some() {
                    self.module
                        .set_struct_body_dyn(handle, elements, packed)
                        .map_err(|e| ParseError::Expected {
                            expected: format!("valid struct body: {e}"),
                            loc: DiagLoc::span(decl_loc),
                        })?;
                } else {
                    // Numbered types record an anonymous literal struct;
                    // upstream never re-uses the name slot, so we keep the
                    // freshly built handle and the literal struct produced
                    // by `module.struct_type` as the table entry below.
                    let lit = self.module.struct_type(elements, packed);
                    self.numbered_types.insert(
                        match slot {
                            Some(slot) => slot,
                            None => {
                                return Err(ParseError::Expected {
                                    expected: "numbered type slot for anonymous literal body"
                                        .into(),
                                    loc: DiagLoc::span(decl_loc),
                                });
                            }
                        },
                        TypeEntry { ty: lit.as_type() },
                    );
                    return Ok(());
                }
            }
            _ => return Err(self.expected("'opaque' or '{' after 'type'")),
        }

        // Insert / update the named-type table.
        if let Some(n) = name {
            self.named_types.insert(
                n,
                TypeEntry {
                    ty: handle.as_type(),
                },
            );
        }
        Ok(())
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
                elems.push(self.parse_type(false)?);
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
        let mut result: Type<'ctx, B> =
            if let Some(ptr_ty) = self.parse_legacy_typed_pointer_type_syntax_only()? {
                ptr_ty
            } else {
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
                            ptr_ty.as_type()
                        } else {
                            ty
                        }
                    }
                    Token::LBrace => {
                        let (elems, packed) = self.parse_struct_body()?;
                        self.module.struct_type(elems, packed).as_type()
                    }
                    Token::Less => {
                        // `<` introduces vector or `<{ packed-struct }>`.
                        self.bump()?; // eat `<`
                        if matches!(self.peek(), Token::LBrace) {
                            let (elems, _was_packed_redundant) = self.parse_struct_body_braces()?;
                            self.expect_punct(PunctKind::Greater, "'>' at end of packed struct")?;
                            self.module.struct_type(elems, true).as_type()
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
                        if let Some(ptr_ty) = self.parse_legacy_typed_pointer_suffix()? {
                            ptr_ty
                        } else {
                            self.lookup_or_forward_named_type(&name, loc)
                        }
                    }
                    Token::LocalVarId(n) => {
                        let id = n;
                        let loc = self.loc();
                        self.bump()?;
                        if let Some(ptr_ty) = self.parse_legacy_typed_pointer_suffix()? {
                            ptr_ty
                        } else {
                            self.lookup_or_forward_numbered_type(id, loc)
                        }
                    }
                    _ => {
                        return Err(ParseError::Expected {
                            expected: "type".into(),
                            loc: DiagLoc::span(type_loc),
                        });
                    }
                }
            };

        // Type suffixes: `*` and `addrspace(N)*` are accepted as
        // parser-compatibility input for legacy typed-pointer `.ll`.
        // They lower immediately to opaque pointer types; the pointee
        // syntax is not represented in llvmkit-ir.
        loop {
            if let Some(ptr_ty) = self.parse_legacy_typed_pointer_suffix()? {
                result = ptr_ty;
            } else {
                match self.peek() {
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
                return Err(self.expected("target extension type"));
            } else {
                type_params.push(self.parse_type(false)?);
            }
        }
        self.expect_punct(PunctKind::RParen, "')' in target extension type")?;
        Ok(self
            .module
            .target_ext_type(name, type_params, int_params)
            .as_type())
    }

    fn parse_legacy_typed_pointer_suffix(&mut self) -> ParseResult<Option<Type<'ctx, B>>> {
        match self.peek() {
            Token::Kw(Keyword::Addrspace) => {
                self.bump()?;
                let address_space = self.parse_addr_space_paren()?;
                if !matches!(self.peek(), Token::Star) {
                    return Err(self.expected("legacy typed pointer addrspace suffix requires '*'"));
                }
                self.bump()?;
                Ok(Some(self.module.ptr_type(address_space).as_type()))
            }
            Token::Star => {
                self.bump()?;
                Ok(Some(self.module.ptr_type(0).as_type()))
            }
            _ => Ok(None),
        }
    }

    fn parse_legacy_typed_pointer_type_syntax_only(
        &mut self,
    ) -> ParseResult<Option<Type<'ctx, B>>> {
        let saved_lex = self.lex.clone();
        let saved_current = self.current.clone();

        if self
            .skip_type_before_legacy_pointer_suffix_syntax_only()
            .is_err()
        {
            self.lex = saved_lex;
            self.current = saved_current;
            return Ok(None);
        }

        let Some(mut ptr_ty) = self.parse_legacy_typed_pointer_suffix()? else {
            self.lex = saved_lex;
            self.current = saved_current;
            return Ok(None);
        };

        while let Some(next_ptr_ty) = self.parse_legacy_typed_pointer_suffix()? {
            ptr_ty = next_ptr_ty;
        }

        Ok(Some(ptr_ty))
    }

    fn skip_type_before_legacy_pointer_suffix_syntax_only(&mut self) -> ParseResult<()> {
        self.skip_type_atom_syntax_only()?;
        while matches!(self.peek(), Token::LParen) {
            self.skip_function_type_syntax_only()?;
        }
        Ok(())
    }

    fn skip_function_type_syntax_only(&mut self) -> ParseResult<()> {
        self.expect_punct(PunctKind::LParen, "'(' in function type")?;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    break;
                }
                self.skip_type_syntax_only()?;
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close function type")?;
        Ok(())
    }

    fn skip_type_syntax_only(&mut self) -> ParseResult<()> {
        self.skip_type_before_legacy_pointer_suffix_syntax_only()?;
        while self.parse_legacy_typed_pointer_suffix()?.is_some() {}
        Ok(())
    }

    fn skip_type_atom_syntax_only(&mut self) -> ParseResult<()> {
        match self.peek() {
            Token::PrimitiveType(_) => {
                let is_ptr = matches!(self.peek(), Token::PrimitiveType(PrimitiveTy::Ptr));
                self.bump()?;
                if is_ptr && matches!(self.peek(), Token::Kw(Keyword::Addrspace)) {
                    self.bump()?;
                    self.parse_addr_space_paren()?;
                }
            }
            Token::LocalVar(_) | Token::LocalVarId(_) => {
                self.bump()?;
            }
            Token::LBrace => {
                self.skip_struct_type_body_syntax_only()?;
            }
            Token::Less => {
                self.bump()?;
                if matches!(self.peek(), Token::LBrace) {
                    self.skip_struct_type_body_syntax_only()?;
                    self.expect_punct(PunctKind::Greater, "'>' at end of packed struct")?;
                } else {
                    self.skip_array_or_vector_type_syntax_only(true)?;
                }
            }
            Token::LSquare => {
                self.bump()?;
                self.skip_array_or_vector_type_syntax_only(false)?;
            }
            Token::Kw(Keyword::Target) => {
                self.skip_target_ext_type_syntax_only()?;
            }
            _ => return Err(self.expected("type")),
        }
        Ok(())
    }

    fn skip_struct_type_body_syntax_only(&mut self) -> ParseResult<()> {
        self.expect_punct(PunctKind::LBrace, "'{' to start struct body")?;
        if !matches!(self.peek(), Token::RBrace) {
            loop {
                self.skip_type_syntax_only()?;
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RBrace, "'}' to close struct body")?;
        Ok(())
    }

    fn skip_array_or_vector_type_syntax_only(&mut self, is_vector: bool) -> ParseResult<()> {
        let scalable = is_vector && self.eat_keyword(Keyword::Vscale)?;
        if scalable {
            self.expect_keyword(Keyword::X, "'x' after vscale")?;
        }
        self.parse_uint64("element count")?;
        self.expect_keyword(Keyword::X, "'x' between aggregate count and element type")?;
        self.skip_type_syntax_only()?;
        if is_vector {
            self.expect_punct(PunctKind::Greater, "'>' at end of vector type")?;
        } else {
            self.expect_punct(PunctKind::RSquare, "']' at end of array type")?;
        }
        Ok(())
    }

    fn skip_target_ext_type_syntax_only(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Target, "'target'")?;
        self.expect_punct(PunctKind::LParen, "'(' in target extension type")?;
        self.parse_string_constant("target extension type name")?;
        let mut seen_integer_param = false;
        while self.eat_punct(PunctKind::Comma)? {
            if matches!(self.peek(), Token::IntegerLit(_)) {
                seen_integer_param = true;
                self.parse_uint32("target extension integer parameter")?;
            } else if seen_integer_param {
                return Err(self.expected("target extension type"));
            } else {
                self.skip_type_syntax_only()?;
            }
        }
        self.expect_punct(PunctKind::RParen, "')' in target extension type")?;
        Ok(())
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
        let n = if is_vector {
            u64::from(self.parse_uint32("vector element count")?)
        } else {
            self.parse_uint64("array element count")?
        };
        self.expect_keyword(Keyword::X, "'x' between count and element type")?;
        let elem = self.parse_type(false)?;
        if is_vector {
            self.expect_punct(PunctKind::Greater, "'>' at end of vector type")?;
            let n32 = u32::try_from(n).map_err(|_| ParseError::Expected {
                expected: "vector element count fits in u32".into(),
                loc: DiagLoc::span(self.loc()),
            })?;
            let v = self.module.vector_type(elem, n32, scalable);
            Ok(v.as_type())
        } else {
            self.expect_punct(PunctKind::RSquare, "']' at end of array type")?;
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
                elems.push(self.parse_type(false)?);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RBrace, "'}' in packed struct body")?;
        Ok((elems, true))
    }

    /// `T (params...)` — mirrors `LLParser::parseFunctionType`. The opening
    /// `(` is the lookahead that triggered this arm.
    fn parse_function_type_after_return(
        &mut self,
        ret: Type<'ctx, B>,
    ) -> ParseResult<Type<'ctx, B>> {
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
                params.push(self.parse_type(false)?);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close function type")?;
        let fn_ty = self.module.fn_type(ret, params, var_args);
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
            PrimitiveTy::BFloat => Ok(m.bfloat_type().as_type()),
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
                    max: (1u32 << 24) - 1,
                    loc: DiagLoc::span(loc),
                }),
        }
    }

    fn lookup_or_forward_named_type(&mut self, name: &str, _loc: Span) -> Type<'ctx, B> {
        if let Some(entry) = self.named_types.get(name) {
            return entry.ty;
        }
        // Forward-reference: create an opaque-named struct now, record the
        // first-seen span, and let the matching definition fill in the body
        // later (or stay opaque if it never lands).
        let st = self.module.named_struct(name);
        self.named_types
            .insert(name.to_owned(), TypeEntry { ty: st.as_type() });
        st.as_type()
    }

    fn lookup_or_forward_numbered_type(&mut self, id: u32, _loc: Span) -> Type<'ctx, B> {
        if let Some(entry) = self.numbered_types.get(&id) {
            return entry.ty;
        }
        // Forward reference for a numbered type creates a fresh anonymous
        // literal struct slot whose body must be filled by a matching
        // `%N = type ...` directive. Upstream uses `StructType::create(ctx)`
        // for this; llvmkit-ir doesn't expose anonymous opaque structs, so
        // we use a literal empty struct and rely on the eventual
        // definition to update the table entry.
        let st = self
            .module
            .struct_type(core::iter::empty::<Type<'ctx, B>>(), false);
        self.numbered_types
            .insert(id, TypeEntry { ty: st.as_type() });
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
        let visibility = if self.eat_keyword(Keyword::Default)? {
            Visibility::Default
        } else if self.eat_keyword(Keyword::Hidden)? {
            Visibility::Hidden
        } else if self.eat_keyword(Keyword::Protected)? {
            Visibility::Protected
        } else {
            Visibility::Default
        };
        let dll_storage_class = if self.eat_keyword(Keyword::Dllimport)? {
            DllStorageClass::DllImport
        } else if self.eat_keyword(Keyword::Dllexport)? {
            DllStorageClass::DllExport
        } else {
            DllStorageClass::Default
        };
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
                    self.expect_punct(PunctKind::RParen, "')' after comdat")?;
                    name
                } else {
                    return Err(self.expected("explicit comdat($name)"));
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
            .visibility(visibility)
            .dll_storage_class(dll_storage_class)
            .thread_local_mode(thread_local_mode)
            .unnamed_addr(unnamed_addr)
            .address_space(address_space)
            .externally_initialized(externally_initialized)
            .align(align)
            .constant(is_constant);
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
            let comdat = self.module.get_or_insert_comdat(&name);
            builder = builder.comdat(comdat);
        }
        let g = builder.build().map_err(|e| ParseError::Expected {
            expected: format!("valid global definition: {e}"),
            loc: DiagLoc::span(decl_loc),
        })?;
        for (kind, id) in metadata {
            g.set_metadata(self.module, kind, id);
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
            return Err(ParseError::Expected {
                expected: "invalid linkage type for alias".into(),
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
            return Err(ParseError::Expected {
                expected: "symbol with local linkage must have default visibility".into(),
                loc: DiagLoc::span(decl_loc),
            });
        }
        if matches!(linkage, Linkage::Internal | Linkage::Private)
            && dll_storage_class != DllStorageClass::Default
        {
            return Err(ParseError::Expected {
                expected: "symbol with local linkage cannot have a DLL storage class".into(),
                loc: DiagLoc::span(decl_loc),
            });
        }

        let value_type = self.parse_type(false)?;
        self.expect_punct(
            PunctKind::Comma,
            "expected comma after alias or ifunc's type",
        )?;
        let target_ty = self.parse_type(false)?;
        match target_ty.into_type_enum() {
            AnyTypeEnum::Pointer(_) => {}
            _ => {
                return Err(ParseError::Expected {
                    expected: "An alias or ifunc must have pointer type".into(),
                    loc: DiagLoc::span(self.loc()),
                });
            }
        }
        let target = self
            .parse_constant(target_ty)?
            .ok_or_else(|| self.expected("alias or ifunc target constant"))?;

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
                .visibility(visibility)
                .dll_storage_class(dll_storage_class)
                .thread_local_mode(thread_local_mode)
                .unnamed_addr(unnamed_addr);
            if let Some(p) = partition {
                builder = builder.partition(p);
            }
            let a = builder.build().map_err(|e| ParseError::Expected {
                expected: format!("valid alias definition: {e}"),
                loc: DiagLoc::span(decl_loc),
            })?;
            if let NameOrId::Id(id) = name_id {
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
                .visibility(visibility);
            if let Some(p) = partition {
                builder = builder.partition(p);
            }
            let i = builder.build().map_err(|e| ParseError::Expected {
                expected: format!("valid ifunc definition: {e}"),
                loc: DiagLoc::span(decl_loc),
            })?;
            if let NameOrId::Id(id) = name_id {
                self.numbered_globals
                    .add(id, GlobalRef::IFunc(i))
                    .map_err(|source| ParseError::InvalidSlotId {
                        source,
                        loc: DiagLoc::span(decl_loc),
                    })?;
            }
        }
        Ok(())
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

    fn unsupported_constant_expr_at(&self, loc: Span, op: crate::ll_token::Opcode) -> ParseError {
        let opcode = match op {
            crate::ll_token::Opcode::ExtractValue => "extractvalue",
            crate::ll_token::Opcode::InsertValue => "insertvalue",
            crate::ll_token::Opcode::UDiv => "udiv",
            crate::ll_token::Opcode::SDiv => "sdiv",
            crate::ll_token::Opcode::URem => "urem",
            crate::ll_token::Opcode::SRem => "srem",
            crate::ll_token::Opcode::FAdd => "fadd",
            crate::ll_token::Opcode::FSub => "fsub",
            crate::ll_token::Opcode::FMul => "fmul",
            crate::ll_token::Opcode::FDiv => "fdiv",
            crate::ll_token::Opcode::FRem => "frem",
            crate::ll_token::Opcode::And => "and",
            crate::ll_token::Opcode::Or => "or",
            crate::ll_token::Opcode::LShr => "lshr",
            crate::ll_token::Opcode::AShr => "ashr",
            crate::ll_token::Opcode::Shl => "shl",
            crate::ll_token::Opcode::Mul => "mul",
            crate::ll_token::Opcode::FNeg => "fneg",
            crate::ll_token::Opcode::Select => "select",
            crate::ll_token::Opcode::ZExt => "zext",
            crate::ll_token::Opcode::SExt => "sext",
            crate::ll_token::Opcode::FPTrunc => "fptrunc",
            crate::ll_token::Opcode::FPExt => "fpext",
            crate::ll_token::Opcode::UIToFP => "uitofp",
            crate::ll_token::Opcode::SIToFP => "sitofp",
            crate::ll_token::Opcode::FPToUI => "fptoui",
            crate::ll_token::Opcode::FPToSI => "fptosi",
            crate::ll_token::Opcode::ICmp => "icmp",
            crate::ll_token::Opcode::FCmp => "fcmp",
            _ => return self.unsupported_constant_value_form_at(loc),
        };
        ParseError::Expected {
            expected: format!("{opcode} constexprs are no longer supported"),
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
                return Err(self.unsupported_constant_value_form_at(loc));
            }
            Token::Instruction(op) if !is_supported_constant_expr_opcode(*op) => {
                return Err(self.unsupported_constant_expr_at(loc, *op));
            }
            _ => {}
        }

        if let Some(ty) = expected_ty {
            match ty.into_type_enum() {
                AnyTypeEnum::Array(array_ty) if matches!(self.peek(), Token::LSquare) => {
                    self.expect_punct(PunctKind::LSquare, "'[' to open array constant")?;
                    let values = if matches!(self.peek(), Token::RSquare) {
                        Vec::new()
                    } else {
                        self.parse_global_value_vector()?
                    };
                    self.expect_punct(PunctKind::RSquare, "']' to close array constant")?;
                    let c = array_ty
                        .const_array(values)
                        .map_err(|e| ParseError::Expected {
                            expected: format!("valid array constant: {e}"),
                            loc: DiagLoc::span(self.loc()),
                        })?;
                    return Ok(ValId::Constant(c.as_constant()));
                }
                AnyTypeEnum::Vector(vec_ty) if matches!(self.peek(), Token::Less) => {
                    self.expect_punct(PunctKind::Less, "'<' to open vector constant")?;
                    let values = if matches!(self.peek(), Token::Greater) {
                        Vec::new()
                    } else {
                        self.parse_global_value_vector()?
                    };
                    self.expect_punct(PunctKind::Greater, "'>' to close vector constant")?;
                    let c = vec_ty
                        .const_vector(values)
                        .map_err(|e| ParseError::Expected {
                            expected: format!("valid vector constant: {e}"),
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
                    let c = struct_ty
                        .const_struct(values)
                        .map_err(|e| ParseError::Expected {
                            expected: format!("valid struct constant: {e}"),
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
                    _ => return Err(self.expected("float constant only valid for float type")),
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
                    _ => Err(self.expected("invalid type for none constant")),
                }
            }
            Token::Kw(Keyword::Blockaddress) => {
                let ty =
                    expected_ty.ok_or_else(|| self.expected("pointer type for blockaddress"))?;
                self.parse_blockaddress_constant(ty).map(ValId::Constant)
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
                    return Err(ParseError::Expected {
                        expected: "invalid type for null constant".into(),
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

    fn convert_val_id_to_value(
        &mut self,
        ty: Type<'ctx, B>,
        id: ValId<'ctx, B>,
        pfs: Option<&PerFunctionState<'ctx, B>>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        match id {
            ValId::LocalName(name) => pfs
                .ok_or_else(|| self.expected("local value in function context"))?
                .local_named
                .get(&name)
                .copied()
                .ok_or_else(|| ParseError::UndefinedSymbol {
                    kind: SYMBOL_KIND_LOCAL,
                    id: crate::parse_error::SymbolId::Named(name),
                    loc: DiagLoc::span(self.loc()),
                }),
            ValId::LocalId(id) => pfs
                .ok_or_else(|| self.expected("local value in function context"))?
                .local_numbered
                .get(&id)
                .copied()
                .ok_or_else(|| ParseError::UndefinedSymbol {
                    kind: SYMBOL_KIND_LOCAL,
                    id: crate::parse_error::SymbolId::Numbered(id),
                    loc: DiagLoc::span(self.loc()),
                }),
            ValId::GlobalName(name) => self.resolve_global_name_as_value(name),
            ValId::GlobalId(id) => self.resolve_global_id_as_value(id),
            ValId::ApsInt(parsed) => {
                let int_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Int(t) => t,
                    _ => return Err(self.expected("integer constant only valid for int type")),
                };
                let bits = lower_parsed_apsint(&parsed, int_ty.bit_width());
                let c = int_ty
                    .const_ap_int(&bits)
                    .map_err(|e| self.builder_err("integer constant", e))?;
                Ok(c.as_value())
            }
            ValId::ApFloat(value) => {
                let float_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Float(t) => t,
                    _ => return Err(self.expected("float constant only valid for float type")),
                };
                Ok(float_ty
                    .const_ap_float(&value)
                    .map_err(|e| self.builder_err("float constant", e))?
                    .as_value())
            }
            ValId::Null => {
                let pty = match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(t) => t,
                    _ => return Err(self.expected("'null' is only valid for pointer types")),
                };
                Ok(pty.const_null().as_value())
            }
            ValId::Zero => self.zero_initializer_constant(ty).map(|c| c.as_value()),
            ValId::Undef => Ok(ty.get_undef().as_value()),
            ValId::Poison => Ok(ty.get_poison().as_value()),
            ValId::Constant(c) => Ok(c.as_value()),
            ValId::ConstantSplat(c) => self.expand_splat_constant(ty, c).map(|c| c.as_value()),
            ValId::Value(v) => Ok(v),
        }
    }

    fn convert_val_id_to_constant(
        &mut self,
        ty: Type<'ctx, B>,
        id: ValId<'ctx, B>,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        match id {
            ValId::GlobalName(name) => {
                match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(_) => {}
                    _ => return Err(self.expected("global reference for pointer constant")),
                }
                self.resolve_global_name_as_constant(name)
            }
            ValId::GlobalId(id) => {
                match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(_) => {}
                    _ => return Err(self.expected("global reference for pointer constant")),
                }
                self.resolve_global_id_as_constant(id)
            }
            ValId::ApsInt(parsed) => {
                let int_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Int(t) => t,
                    _ => return Err(self.expected("integer constant for non-integer type")),
                };
                let bits = lower_parsed_apsint(&parsed, int_ty.bit_width());
                let c = int_ty
                    .const_ap_int(&bits)
                    .map_err(|e| ParseError::Expected {
                        expected: format!("valid integer constant: {e}"),
                        loc: DiagLoc::span(self.loc()),
                    })?;
                Ok(c.as_constant())
            }
            ValId::ApFloat(value) => {
                let float_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Float(t) => t,
                    _ => return Err(self.expected("float constant only valid for float type")),
                };
                Ok(float_ty
                    .const_ap_float(&value)
                    .map_err(|e| self.builder_err("float constant", e))?
                    .as_constant())
            }
            ValId::Null => {
                let ptr_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(t) => t,
                    _ => return Err(self.expected("'null' is only valid for pointer types")),
                };
                Ok(ptr_ty.const_null().as_constant())
            }
            ValId::Zero => self.zero_initializer_constant(ty),
            ValId::Undef => Ok(ty.get_undef().as_constant()),
            ValId::Poison => Ok(ty.get_poison().as_constant()),
            ValId::Constant(c) => Ok(c),
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

    fn parse_global_value_vector(&mut self) -> ParseResult<Vec<llvmkit_ir::Constant<'ctx, B>>> {
        let mut values = Vec::new();
        loop {
            values.push(self.parse_global_type_and_value()?);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }
        Ok(values)
    }

    fn resolve_global_name_as_value(
        &self,
        name: String,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        if !matches!(
            resolve_intrinsic_name(&name),
            IntrinsicNameResolution::NonIntrinsic
        ) {
            return Err(ParseError::Expected {
                expected: "intrinsic can only be used as callee".into(),
                loc: DiagLoc::span(self.loc()),
            });
        }
        if let Some(gv) = self.module.get_global(&name) {
            Ok(gv.as_value())
        } else if let Some(fv) = self.module.function_by_name_dyn(&name) {
            Ok(fv.as_value())
        } else if let Some(a) = self.module.get_alias(&name) {
            Ok(a.as_value())
        } else if let Some(i) = self.module.get_ifunc(&name) {
            Ok(i.as_value())
        } else {
            Err(ParseError::UndefinedSymbol {
                kind: crate::parse_error::SymbolKind::Global,
                id: crate::parse_error::SymbolId::Named(name),
                loc: DiagLoc::span(self.loc()),
            })
        }
    }

    fn resolve_global_id_as_value(&self, id: u32) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.numbered_globals
            .get(id)
            .copied()
            .map(|r| match r {
                GlobalRef::Function(f) => f.as_value(),
                GlobalRef::Variable(g) => g.as_value(),
                GlobalRef::Alias(a) => a.as_value(),
                GlobalRef::IFunc(i) => i.as_value(),
            })
            .ok_or_else(|| ParseError::UndefinedSymbol {
                kind: crate::parse_error::SymbolKind::Global,
                id: crate::parse_error::SymbolId::Numbered(id),
                loc: DiagLoc::span(self.loc()),
            })
    }

    fn resolve_global_name_as_constant(
        &self,
        name: String,
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        if !matches!(
            resolve_intrinsic_name(&name),
            IntrinsicNameResolution::NonIntrinsic
        ) {
            return Err(ParseError::Expected {
                expected: "intrinsic can only be used as callee".into(),
                loc: DiagLoc::span(self.loc()),
            });
        }
        if let Some(g) = self.module.get_global(&name) {
            Ok(g.as_global_constant_ptr())
        } else if let Some(f) = self.module.function_by_name_dyn(&name) {
            Ok(f.as_global_constant_ptr())
        } else if let Some(a) = self.module.get_alias(&name) {
            Ok(a.as_global_constant_ptr())
        } else if let Some(i) = self.module.get_ifunc(&name) {
            Ok(i.as_global_constant_ptr())
        } else {
            Err(ParseError::UndefinedSymbol {
                kind: crate::parse_error::SymbolKind::Global,
                id: crate::parse_error::SymbolId::Named(name),
                loc: DiagLoc::span(self.loc()),
            })
        }
    }

    fn resolve_global_id_as_constant(&self, id: u32) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        self.numbered_globals
            .get(id)
            .copied()
            .map(|r| self.global_ref_to_constant(r))
            .ok_or_else(|| ParseError::UndefinedSymbol {
                kind: crate::parse_error::SymbolKind::Global,
                id: crate::parse_error::SymbolId::Numbered(id),
                loc: DiagLoc::span(self.loc()),
            })
    }

    fn global_ref_to_constant(&self, r: GlobalRef<'ctx, B>) -> llvmkit_ir::Constant<'ctx, B> {
        match r {
            GlobalRef::Function(f) => f.as_global_constant_ptr(),
            GlobalRef::Variable(g) => g.as_global_constant_ptr(),
            GlobalRef::Alias(a) => a.as_global_constant_ptr(),
            GlobalRef::IFunc(i) => i.as_global_constant_ptr(),
        }
    }
    fn resolve_global_name_as_ref(&self, name: String) -> ParseResult<GlobalRef<'ctx, B>> {
        if let Some(gv) = self.module.get_global(&name) {
            Ok(GlobalRef::Variable(gv))
        } else if let Some(fv) = self.module.function_by_name_dyn(&name) {
            Ok(GlobalRef::Function(fv))
        } else if let Some(a) = self.module.get_alias(&name) {
            Ok(GlobalRef::Alias(a))
        } else if let Some(i) = self.module.get_ifunc(&name) {
            Ok(GlobalRef::IFunc(i))
        } else {
            Err(ParseError::UndefinedSymbol {
                kind: crate::parse_error::SymbolKind::Global,
                id: crate::parse_error::SymbolId::Named(name),
                loc: DiagLoc::span(self.loc()),
            })
        }
    }

    fn resolve_global_id_as_ref(&self, id: u32) -> ParseResult<GlobalRef<'ctx, B>> {
        self.numbered_globals
            .get(id)
            .copied()
            .ok_or_else(|| ParseError::UndefinedSymbol {
                kind: crate::parse_error::SymbolKind::Global,
                id: crate::parse_error::SymbolId::Numbered(id),
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
                kind: crate::parse_error::SymbolKind::Global,
                id: crate::parse_error::SymbolId::Numbered(id),
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
                if let Some(function) = self.module.function_by_name_dyn(&name) {
                    Ok(ParsedBlockAddressFunction::Resolved(function))
                } else if self.module.get_global(&name).is_some()
                    || self.module.get_alias(&name).is_some()
                    || self.module.get_ifunc(&name).is_some()
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
    ) -> ParseResult<llvmkit_ir::Constant<'ctx, B>> {
        if !matches!(expected_ty.into_type_enum(), AnyTypeEnum::Pointer(_)) {
            return Err(self.expected("pointer type for blockaddress"));
        }
        self.expect_keyword(Keyword::Blockaddress, "'blockaddress'")?;
        self.expect_punct(PunctKind::LParen, "'(' in blockaddress")?;
        let function = self.parse_function_ref_for_blockaddress("function name in blockaddress")?;
        self.expect_punct(PunctKind::Comma, "',' in blockaddress")?;
        let label = match self.peek() {
            Token::LocalVar(_) => self
                .current_str_payload()
                .ok_or_else(|| self.expected("basic block name in blockaddress"))?,
            Token::LocalVarId(id) => id.to_string(),
            _ => return Err(self.expected("basic block name in blockaddress")),
        };
        self.bump()?;
        self.expect_punct(PunctKind::RParen, "')' in blockaddress")?;
        match function {
            ParsedBlockAddressFunction::Resolved(function) => {
                if function.basic_blocks().len() == 0 {
                    return Err(self.expected("cannot take blockaddress inside a declaration"));
                }
                let block = function
                    .basic_blocks()
                    .find(|bb| bb.name().as_deref() == Some(label.as_str()))
                    .ok_or_else(|| self.expected("referenced value is not a basic block"))?;
                self.module
                    .block_address(function, &block)
                    .map_err(|e| self.builder_err("blockaddress", e))
            }
            ParsedBlockAddressFunction::Forward { function, loc } => {
                let placeholder = self
                    .module
                    .block_address_placeholder(expected_ty)
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
        }
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
        self.expect_punct(
            PunctKind::LParen,
            "expected '(' in constant ptrauth expression",
        )?;
        let pointer = self.parse_ptrauth_operand()?;
        self.expect_punct(
            PunctKind::Comma,
            "expected comma in constant ptrauth expression",
        )?;
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
        self.expect_punct(
            PunctKind::RParen,
            "expected ')' in constant ptrauth expression",
        )?;
        self.module
            .ptr_auth(
                pointer,
                key,
                discriminator,
                addr_discriminator,
                deactivation_symbol,
            )
            .map_err(|e| match e {
                IrError::InvalidOperation { message } => ParseError::Expected {
                    expected: message.into(),
                    loc: DiagLoc::span(self.loc()),
                },
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
            crate::ll_token::Opcode::Add => ConstantExprOpcode::Add,
            crate::ll_token::Opcode::Sub => ConstantExprOpcode::Sub,
            crate::ll_token::Opcode::Xor => ConstantExprOpcode::Xor,
            crate::ll_token::Opcode::GetElementPtr => ConstantExprOpcode::GetElementPtr,
            crate::ll_token::Opcode::ShuffleVector => ConstantExprOpcode::ShuffleVector,
            crate::ll_token::Opcode::InsertElement => ConstantExprOpcode::InsertElement,
            crate::ll_token::Opcode::ExtractElement => ConstantExprOpcode::ExtractElement,
            crate::ll_token::Opcode::Trunc => ConstantExprOpcode::Trunc,
            crate::ll_token::Opcode::PtrToAddr => ConstantExprOpcode::PtrToAddr,
            crate::ll_token::Opcode::PtrToInt => ConstantExprOpcode::PtrToInt,
            crate::ll_token::Opcode::IntToPtr => ConstantExprOpcode::IntToPtr,
            crate::ll_token::Opcode::BitCast => ConstantExprOpcode::BitCast,
            crate::ll_token::Opcode::AddrSpaceCast => ConstantExprOpcode::AddrSpaceCast,
            _ => return Err(self.unsupported_constant_value_form_at(self.loc())),
        };

        match opcode {
            ConstantExprOpcode::Add | ConstantExprOpcode::Sub | ConstantExprOpcode::Xor => {
                let flags = if matches!(opcode, ConstantExprOpcode::Add | ConstantExprOpcode::Sub) {
                    self.parse_overflowing_constant_expr_flags()?
                } else {
                    ConstantExprFlags::none()
                };
                self.expect_punct(PunctKind::LParen, "expected '(' in binary constantexpr")?;
                let lhs = self.parse_global_type_and_value()?;
                self.expect_punct(PunctKind::Comma, "expected comma in binary constantexpr")?;
                let rhs = self.parse_global_type_and_value()?;
                if lhs.ty() != rhs.ty() {
                    return Err(self.expected("operands of constexpr must have same type"));
                }
                if !is_int_or_int_vector_type(lhs.ty()) {
                    return Err(
                        self.expected("constexpr requires integer or integer vector operands")
                    );
                }
                self.expect_punct(PunctKind::RParen, "expected ')' in binary constantexpr")?;
                self.build_constant_expr(result_ty, None, opcode, vec![lhs, rhs], flags)
            }
            ConstantExprOpcode::Trunc
            | ConstantExprOpcode::IntToPtr
            | ConstantExprOpcode::PtrToAddr
            | ConstantExprOpcode::PtrToInt
            | ConstantExprOpcode::BitCast
            | ConstantExprOpcode::AddrSpaceCast => {
                self.expect_punct(PunctKind::LParen, "expected '(' after constantexpr cast")?;
                let operand = self.parse_global_type_and_value()?;
                self.expect_keyword(Keyword::To, "expected 'to' in constantexpr cast")?;
                let dst_ty = self.parse_type(false)?;
                if dst_ty != result_ty {
                    return Err(self.expected(
                        "constant expression destination type matches initializer type",
                    ));
                }
                self.expect_punct(
                    PunctKind::RParen,
                    "expected ')' at end of constantexpr cast",
                )?;
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
                self.expect_punct(PunctKind::LParen, "expected '(' in constantexpr")?;
                let source_ty = self.parse_type(false)?;
                if type_contains_scalable_vector(source_ty) {
                    return Err(self.expected("invalid base element for constant getelementptr"));
                }
                self.expect_punct(
                    PunctKind::Comma,
                    "expected comma after getelementptr's type",
                )?;
                let operands = self.parse_global_value_vector()?;
                self.expect_punct(PunctKind::RParen, "expected ')' in constantexpr")?;
                self.validate_parsed_gep_constant_expr(source_ty, &operands)?;
                let flags = self.finish_gep_constant_expr_flags(parsed_flags, &operands)?;
                self.build_constant_expr(result_ty, Some(source_ty), opcode, operands, flags)
            }
            ConstantExprOpcode::ShuffleVector
            | ConstantExprOpcode::InsertElement
            | ConstantExprOpcode::ExtractElement => {
                self.expect_punct(PunctKind::LParen, "expected '(' in constantexpr")?;
                let operands = self.parse_global_value_vector()?;
                self.expect_punct(PunctKind::RParen, "expected ')' in constantexpr")?;
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
            self.expect_punct(PunctKind::LParen, "expected '('")?;
            let start = self.parse_inrange_bound()?;
            self.expect_punct(PunctKind::Comma, "expected ','")?;
            let end = self.parse_inrange_bound()?;
            self.expect_punct(PunctKind::RParen, "expected ')'")?;
            Some((start, end))
        } else {
            None
        };

        Ok(ParsedGepConstantExprFlags { no_wrap, in_range })
    }

    fn finish_gep_constant_expr_flags(
        &self,
        parsed: ParsedGepConstantExprFlags,
        operands: &[Constant<'ctx, B>],
    ) -> ParseResult<ConstantExprFlags> {
        let Some((start, end)) = parsed.in_range else {
            return Ok(ConstantExprFlags::gep(parsed.no_wrap));
        };
        let Some(base) = operands.first() else {
            return Err(self.expected("base of getelementptr must be a pointer"));
        };
        let address_space = pointer_address_space_or_vector_element(base.ty())
            .ok_or_else(|| self.expected("base of getelementptr must be a pointer"))?;
        let bit_width = self.module.data_layout().index_size_in_bits(address_space);
        let start_words = inrange_bound_to_apint_words(&start, bit_width);
        let end_words = inrange_bound_to_apint_words(&end, bit_width);
        let in_range = ConstantExprInRange::new(start_words, end_words, bit_width);
        if !constant_expr_inrange_is_non_empty(&in_range) {
            return Err(self.expected("expected end to be larger than start"));
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
                let magnitude_words = decimal_digits_to_words(digits)
                    .ok_or_else(|| self.expected("expected integer"))?;
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
                let words =
                    hex_digits_to_words(digits).ok_or_else(|| self.expected("expected integer"))?;
                let bit_width = hex_apsint_bit_width(digits, &words)
                    .ok_or_else(|| self.expected("expected integer"))?;
                ParsedInRangeBound::HexApsInt {
                    signed: matches!(base, NumBase::HexSigned),
                    words,
                    bit_width,
                }
            }
            _ => return Err(self.expected("expected integer")),
        };
        self.bump()?;
        Ok(bound)
    }

    fn validate_parsed_gep_constant_expr(
        &self,
        source_ty: Type<'ctx, B>,
        operands: &[llvmkit_ir::Constant<'ctx, B>],
    ) -> ParseResult<()> {
        let Some((base, indices)) = operands.split_first() else {
            return Err(self.expected("base of getelementptr must be a pointer"));
        };
        if !is_ptr_or_ptr_vector_type(base.ty()) {
            return Err(self.expected("base of getelementptr must be a pointer"));
        }
        if !indices.is_empty() && !source_ty.is_sized() {
            return Err(self.expected("base element of getelementptr must be sized"));
        }
        if type_contains_scalable_vector(source_ty) {
            return Err(self.expected("invalid base element for constant getelementptr"));
        }
        let pointer_shape = vector_shape_type(base.ty());
        let mut gep_width = pointer_shape;
        for index in indices {
            if !is_int_or_int_vector_type(index.ty()) {
                return Err(self.expected("getelementptr index must be an integer"));
            }
            if let Some(index_shape) = vector_shape_type(index.ty()) {
                if let Some(pointer_shape) = gep_width
                    && index_shape != pointer_shape
                {
                    return Err(
                        self.expected("getelementptr vector index has a wrong number of elements")
                    );
                }
                gep_width = Some(index_shape);
            }
        }
        Ok(())
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
                    return Err(self.expected("expected three operands to shufflevector"));
                };
                if !is_valid_shufflevector(result_ty, lhs.ty(), rhs.ty(), mask.ty()) {
                    return Err(self.expected("invalid operands to shufflevector"));
                }
            }
            ConstantExprOpcode::ExtractElement => {
                let [vector, index] = operands else {
                    return Err(self.expected("expected two operands to extractelement"));
                };
                if !is_valid_extractelement(result_ty, vector.ty(), index.ty()) {
                    return Err(self.expected("invalid extractelement operands"));
                }
            }
            ConstantExprOpcode::InsertElement => {
                let [vector, value, index] = operands else {
                    return Err(self.expected("expected three operands to insertelement"));
                };
                if !is_valid_insertelement(result_ty, vector.ty(), value.ty(), index.ty()) {
                    return Err(self.expected("invalid insertelement operands"));
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
                operands.into_iter().map(|c| c.as_value()),
                [],
                [],
                options,
            )
            .map_err(|e| match e {
                IrError::InvalidOperation { message }
                    if matches!(opcode, ConstantExprOpcode::ShuffleVector)
                        && message == "invalid shufflevector constant expression" =>
                {
                    ParseError::Expected {
                        expected: "invalid operands to shufflevector".into(),
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
            Linkage::Appending | Linkage::Common => Err(ParseError::Expected {
                expected: "invalid function linkage type".into(),
                loc: DiagLoc::span(loc),
            }),
            Linkage::ExternalWeak if is_define => Err(ParseError::Expected {
                expected: "invalid linkage for function definition".into(),
                loc: DiagLoc::span(loc),
            }),
            Linkage::Private
            | Linkage::Internal
            | Linkage::AvailableExternally
            | Linkage::LinkOnceAny
            | Linkage::LinkOnceODR
            | Linkage::WeakAny
            | Linkage::WeakODR
                if !is_define =>
            {
                Err(ParseError::Expected {
                    expected: "invalid linkage for function declaration".into(),
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
            return Err(ParseError::Expected {
                expected: "attribute group has no attributes".into(),
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
            Keyword::Zeroext => AttrKind::ZExt,
            Keyword::Signext => AttrKind::SExt,
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
            Keyword::Strictfp => AttrKind::StrictFP,
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
                    out.add(index, Attribute::<B>::string_for_brand(key, value));
                }
                Token::Kw(Keyword::Align) if index == AttrIndex::Function && allow_group_refs => {
                    break;
                }
                Token::Kw(Keyword::Align) => {
                    self.bump()?;
                    let value = self.parse_uint64("align value")?;
                    let attr = Attribute::<B>::int_for_brand(AttrKind::Alignment, value)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Alignstack) => {
                    self.bump()?;
                    let value = self.parse_uint64("alignstack value")?;
                    let attr = Attribute::<B>::int_for_brand(AttrKind::StackAlignment, value)
                        .ok_or_else(|| self.expected("attribute"))?;
                    out.add(index, attr);
                }
                Token::Kw(Keyword::Memory) => {
                    let attr = self.parse_memory_attribute()?;
                    out.add(index, attr);
                }
                Token::Kw(keyword)
                    if index == AttrIndex::Function
                        && Self::legacy_memory_effects(*keyword).is_some() =>
                {
                    let effects = Self::legacy_memory_effects(*keyword)
                        .ok_or_else(|| self.expected("memory attribute"))?;
                    self.bump()?;
                    out.add(index, Attribute::<B>::memory_for_brand(effects));
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
                    let attr = Attribute::<B>::enum_attr_for_brand(kind)
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

    fn parse_memory_attribute(&mut self) -> ParseResult<Attribute<'ctx, B>> {
        self.expect_keyword(Keyword::Memory, "'memory'")?;
        self.expect_punct(PunctKind::LParen, "'(' in memory attribute")?;
        let mut effects = MemoryEffects::none();
        let mut parsed = false;
        let mut seen_location = false;
        loop {
            if self.eat_punct(PunctKind::RParen)? {
                if parsed {
                    return Ok(Attribute::<B>::memory_for_brand(effects));
                }
                return Err(self.expected("memory attribute access kind"));
            }
            if parsed {
                self.expect_punct(PunctKind::Comma, "',' in memory attribute")?;
            }
            let (next_effects, component_had_location) =
                self.parse_memory_effect_component(effects, seen_location)?;
            effects = next_effects;
            seen_location |= component_had_location;
            parsed = true;
        }
    }

    fn parse_memory_effect_component(
        &mut self,
        effects: MemoryEffects,
        seen_location: bool,
    ) -> ParseResult<(MemoryEffects, bool)> {
        if let Token::LabelStr(bytes) = self.peek() {
            let name = std::str::from_utf8(bytes.as_ref())
                .map_err(|_| self.expected("memory location"))?;
            if let Some(location) = Self::memory_location_for_name(name) {
                self.bump()?;
                let mod_ref = self.parse_memory_access_kind()?;
                return Ok((effects.with_mod_ref(location, mod_ref), true));
            }
        }
        if seen_location {
            return Err(self.expected("memory attribute access kind"));
        }
        let mod_ref = self.parse_memory_access_kind()?;
        Ok((Self::memory_effects_for_mod_ref(mod_ref), false))
    }

    fn parse_memory_access_kind(&mut self) -> ParseResult<ModRefInfo> {
        let mod_ref = match self.peek() {
            Token::Kw(Keyword::None) => ModRefInfo::NoModRef,
            Token::Kw(Keyword::Read) => ModRefInfo::Ref,
            Token::Kw(Keyword::Write) => ModRefInfo::Mod,
            Token::Kw(Keyword::Readwrite) => ModRefInfo::ModRef,
            _ => return Err(self.expected("memory attribute access kind")),
        };
        self.bump()?;
        Ok(mod_ref)
    }

    fn memory_location_for_name(name: &str) -> Option<MemoryLocation> {
        Some(match name {
            "argmem" => MemoryLocation::ArgMem,
            "inaccessiblemem" => MemoryLocation::InaccessibleMem,
            "errnomem" => MemoryLocation::ErrnoMem,
            "target_mem0" => MemoryLocation::TargetMem0,
            "target_mem1" => MemoryLocation::TargetMem1,
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
            "deactivation" => llvmkit_ir::instr_types::OperandBundleTag::DeactivationSymbol,
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
                        inputs.push(value.id());
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
        let linkage = self.parse_optional_function_linkage(false)?;
        let visibility = self.parse_optional_visibility()?;
        let dll_storage_class = self.parse_optional_dll_storage_class()?;
        let dso_locality = self.parse_optional_dso_locality()?;
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
        let address_space = if self.eat_keyword(Keyword::Addrspace)? {
            self.parse_addr_space_paren()?
        } else {
            0
        };
        let suffix = self.parse_optional_function_suffix(&mut attrs)?;

        let fn_ty = self.module.fn_type(ret_ty, params, var_args);
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
                for (slot, name) in param_names.into_iter().enumerate() {
                    if let Some(name) = name {
                        let slot = u32::try_from(slot).map_err(|_| ParseError::Expected {
                            expected: "parameter slot fits in u32".into(),
                            loc: DiagLoc::span(decl_loc),
                        })?;
                        let arg = f.param(slot).map_err(|e| ParseError::Expected {
                            expected: format!("function parameter slot {slot}: {e}"),
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
            .then(|| self.module.function_by_name_dyn(&name))
            .flatten();
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
                    expected: format!("valid function declaration: {e}"),
                    loc: DiagLoc::span(decl_loc),
                })?;
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
                        expected: format!("function parameter slot {slot}: {e}"),
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
            let name = comdat_name.unwrap_or_else(|| f.name().to_owned());
            let comdat = self.module.get_or_insert_comdat(&name);
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
            f.set_metadata(self.module, kind, id);
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
        let visibility = self.parse_optional_visibility()?;
        let dll_storage_class = self.parse_optional_dll_storage_class()?;
        let dso_locality = self.parse_optional_dso_locality()?;
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
        let address_space = if self.eat_keyword(Keyword::Addrspace)? {
            self.parse_addr_space_paren()?
        } else {
            0
        };
        let suffix = self.parse_optional_function_suffix(&mut attrs)?;

        let fn_ty = self.module.fn_type(ret_ty, param_types, var_args);
        let existing_by_id = match &name_id {
            NameOrId::Id(id) => self.numbered_globals.get(*id).and_then(|r| match r {
                GlobalRef::Function(f) => Some(*f),
                _ => None,
            }),
            NameOrId::Name(_) => None,
        };
        let existing_by_name = (!name.is_empty())
            .then(|| self.module.function_by_name_dyn(&name))
            .flatten();
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
                    expected: format!("valid function definition: {e}"),
                    loc: DiagLoc::span(decl_loc),
                })?;
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
                    expected: format!("function parameter slot {slot}: {e}"),
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
            let name = comdat_name.unwrap_or_else(|| f.name().to_owned());
            let comdat = self.module.get_or_insert_comdat(&name);
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
            f.set_metadata(self.module, kind, id);
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
                expected: format!("function parameter slot {slot}: {e}"),
                loc: DiagLoc::span(decl_loc),
            })?;
            let v = arg.as_value();
            match name {
                Some(ParamName::Named(n)) => {
                    state.local_named.insert(n, v);
                }
                Some(ParamName::Numbered(id)) => {
                    if state.local_numbered.contains_key(&id) || id != state.next_unnamed_value_id {
                        return Err(ParseError::InvalidSlotId {
                            source: crate::numbered_values::AddError::StaleId {
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
        let bb_value = bb.as_value();
        // Drive the typed builder for this block.
        let builder = IRBuilder::with_folder(self.module, NoFolder).position_at_end(bb);
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
                Token::Instruction(crate::ll_token::Opcode::Ret) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_ret(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Unreachable) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    let _ = b.build_unreachable();
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Br) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_br(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Store) => {
                    let b_ref = borrow_live_builder(&builder, self.loc())?;
                    self.bump()?;
                    self.parse_store(state, b_ref)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    seen_non_phi = true;
                    continue;
                }
                Token::Instruction(crate::ll_token::Opcode::Fence) => {
                    let b_ref = borrow_live_builder(&builder, self.loc())?;
                    self.bump()?;
                    self.parse_fence(b_ref)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    seen_non_phi = true;
                    continue;
                }
                Token::Instruction(crate::ll_token::Opcode::Switch) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_switch(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::IndirectBr) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.parse_indirectbr(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Invoke) => {
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
                Token::Instruction(crate::ll_token::Opcode::Resume) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_resume(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::CleanupRet) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_cleanupret(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::CatchRet) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    self.bump()?;
                    self.parse_catchret(state, b)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::CatchSwitch) => {
                    let b = take_live_builder(&mut builder, self.loc())?;
                    let result_loc = self.loc();
                    let result_name = self.parse_lhs_assignment()?;
                    let v = self.parse_catchswitch(state, b, &result_name)?;
                    self.finish_trailing_metadata(state, bb_value, &mut pending_debug_records)?;
                    state.bind_local(&result_name, v, result_loc)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::CallBr) => {
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
            if matches!(
                self.peek(),
                Token::Instruction(crate::ll_token::Opcode::Invoke)
            ) {
                let b = take_live_builder(&mut builder, self.loc())?;
                self.bump()?;
                let value = self.parse_invoke(state, b, &result_name)?;
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
            if matches!(opcode, crate::ll_token::Opcode::Phi) {
                if seen_non_phi {
                    return Err(self.expected("phi must be grouped at the top of its basic block"));
                }
            } else {
                seen_non_phi = true;
            }
            self.bump()?;
            let b_ref = borrow_live_builder(&builder, self.loc())?;
            let value = match opcode {
                crate::ll_token::Opcode::Add => {
                    self.parse_int_binop(state, b_ref, IntBinOp::Add, &result_name)?
                }
                crate::ll_token::Opcode::Sub => {
                    self.parse_int_binop(state, b_ref, IntBinOp::Sub, &result_name)?
                }
                crate::ll_token::Opcode::Mul => {
                    self.parse_int_binop(state, b_ref, IntBinOp::Mul, &result_name)?
                }
                crate::ll_token::Opcode::UDiv => {
                    self.parse_int_binop(state, b_ref, IntBinOp::UDiv, &result_name)?
                }
                crate::ll_token::Opcode::SDiv => {
                    self.parse_int_binop(state, b_ref, IntBinOp::SDiv, &result_name)?
                }
                crate::ll_token::Opcode::URem => {
                    self.parse_int_binop(state, b_ref, IntBinOp::URem, &result_name)?
                }
                crate::ll_token::Opcode::SRem => {
                    self.parse_int_binop(state, b_ref, IntBinOp::SRem, &result_name)?
                }
                crate::ll_token::Opcode::Shl => {
                    self.parse_int_binop(state, b_ref, IntBinOp::Shl, &result_name)?
                }
                crate::ll_token::Opcode::LShr => {
                    self.parse_int_binop(state, b_ref, IntBinOp::LShr, &result_name)?
                }
                crate::ll_token::Opcode::AShr => {
                    self.parse_int_binop(state, b_ref, IntBinOp::AShr, &result_name)?
                }
                crate::ll_token::Opcode::And => {
                    self.parse_int_binop(state, b_ref, IntBinOp::And, &result_name)?
                }
                crate::ll_token::Opcode::Or => {
                    self.parse_int_binop(state, b_ref, IntBinOp::Or, &result_name)?
                }
                crate::ll_token::Opcode::Xor => {
                    self.parse_int_binop(state, b_ref, IntBinOp::Xor, &result_name)?
                }
                crate::ll_token::Opcode::ICmp => self.parse_icmp(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::Trunc => {
                    self.parse_int_cast(state, b_ref, IntCast::Trunc, &result_name)?
                }
                crate::ll_token::Opcode::ZExt => {
                    self.parse_int_cast(state, b_ref, IntCast::ZExt, &result_name)?
                }
                crate::ll_token::Opcode::SExt => {
                    self.parse_int_cast(state, b_ref, IntCast::SExt, &result_name)?
                }
                crate::ll_token::Opcode::PtrToInt => {
                    self.parse_ptr_to_int(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::IntToPtr => {
                    self.parse_int_to_ptr(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::FNeg => self.parse_fneg(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::FAdd => {
                    self.parse_fp_binop(state, b_ref, FpBinOp::Add, &result_name)?
                }
                crate::ll_token::Opcode::FSub => {
                    self.parse_fp_binop(state, b_ref, FpBinOp::Sub, &result_name)?
                }
                crate::ll_token::Opcode::FMul => {
                    self.parse_fp_binop(state, b_ref, FpBinOp::Mul, &result_name)?
                }
                crate::ll_token::Opcode::FDiv => {
                    self.parse_fp_binop(state, b_ref, FpBinOp::Div, &result_name)?
                }
                crate::ll_token::Opcode::FRem => {
                    self.parse_fp_binop(state, b_ref, FpBinOp::Rem, &result_name)?
                }
                crate::ll_token::Opcode::FCmp => self.parse_fcmp(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::Alloca => self.parse_alloca(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::Load => self.parse_load(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::GetElementPtr => {
                    self.parse_gep(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::Select => self.parse_select(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::FPToUI => {
                    self.parse_fp_to_int(state, b_ref, FpToInt::FpToUI, &result_name)?
                }
                crate::ll_token::Opcode::FPToSI => {
                    self.parse_fp_to_int(state, b_ref, FpToInt::FpToSI, &result_name)?
                }
                crate::ll_token::Opcode::UIToFP => {
                    self.parse_int_to_fp(state, b_ref, IntToFp::UIToFp, &result_name)?
                }
                crate::ll_token::Opcode::SIToFP => {
                    self.parse_int_to_fp(state, b_ref, IntToFp::SIToFp, &result_name)?
                }
                crate::ll_token::Opcode::AddrSpaceCast => {
                    self.parse_addrspace_cast(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::BitCast => {
                    self.parse_bitcast(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::FPTrunc => {
                    self.parse_fptrunc(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::FPExt => self.parse_fpext(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::PtrToAddr => {
                    self.parse_ptrtoaddr(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::ExtractElement => {
                    self.parse_extractelement(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::InsertElement => {
                    self.parse_insertelement(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::ShuffleVector => {
                    self.parse_shufflevector(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::ExtractValue => {
                    self.parse_extractvalue(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::InsertValue => {
                    self.parse_insertvalue(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::Phi => self.parse_phi(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::Call => self.parse_call(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::VAArg => self.parse_vaarg(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::Freeze => self.parse_freeze(state, b_ref, &result_name)?,
                crate::ll_token::Opcode::AtomicCmpXchg => {
                    self.parse_cmpxchg(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::AtomicRMW => {
                    self.parse_atomicrmw(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::LandingPad => {
                    self.parse_landingpad(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::CleanupPad => {
                    self.parse_cleanuppad(state, b_ref, &result_name)?
                }
                crate::ll_token::Opcode::CatchPad => {
                    self.parse_catchpad(state, b_ref, &result_name)?
                }
                _ => {
                    return Err(ParseError::Expected {
                        expected: format!(
                            "instruction opcode supported by this parser (got {opcode:?})"
                        ),
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
        b: ParsedBlockBuilder<'m, 'ctx, B>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `ret`
        if let Token::PrimitiveType(crate::ll_token::PrimitiveTy::Void) = self.peek() {
            self.bump()?;
            let _ = b.build_ret_void().map_err(|e| ParseError::Expected {
                expected: format!("valid ret void: {e}"),
                loc: DiagLoc::span(self.loc()),
            })?;
            return Ok(());
        }
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let _ = b.build_ret(v).map_err(|e| ParseError::Expected {
            expected: format!("valid ret: {e}"),
            loc: DiagLoc::span(self.loc()),
        })?;
        Ok(())
    }

    /// `br label %t` or `br i1 %c, label %t, label %f`. Mirrors
    /// `LLParser::parseBr`.
    fn parse_br(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'m, 'ctx, B>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `br`
        if matches!(
            self.peek(),
            Token::PrimitiveType(crate::ll_token::PrimitiveTy::Label)
        ) {
            self.bump()?;
            let target = self.parse_block_ref(state)?;
            let _ = b.build_br(target).map_err(|e| ParseError::Expected {
                expected: format!("valid br: {e}"),
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
        self.expect_primitive(
            crate::ll_token::PrimitiveTy::Label,
            "'label' for then-target",
        )?;
        let then_bb = self.parse_block_ref(state)?;
        self.expect_punct(PunctKind::Comma, "',' between br targets")?;
        self.expect_primitive(
            crate::ll_token::PrimitiveTy::Label,
            "'label' for else-target",
        )?;
        let else_bb = self.parse_block_ref(state)?;
        let cond_iv: IntValue<'ctx, IntDyn, B> = cond_v
            .try_into()
            .map_err(|_| self.expected("i1 condition"))?;
        let cond_i1: IntValue<'ctx, bool, B> = cond_iv
            .try_into()
            .map_err(|_| self.expected("i1 condition"))?;
        let _ = b
            .build_cond_br(cond_i1, then_bb, else_bb)
            .map_err(|e| ParseError::Expected {
                expected: format!("valid cond_br: {e}"),
                loc: DiagLoc::span(self.loc()),
            })?;
        Ok(())
    }

    /// `OP [nuw] [nsw] TYPE LHS, RHS` or `OP [exact] TYPE LHS, RHS` or `OP [disjoint] TYPE LHS, RHS`.
    /// Mirrors `LLParser::parseArithmetic` / `parseLogical` (LLParser.cpp ~8132 / 8152).
    fn parse_int_binop(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        op: IntBinOp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        use llvmkit_ir::instr_types::{
            AShrFlags, AddFlags, LShrFlags, MulFlags, OrFlags, SDivFlags, ShlFlags, SubFlags,
            UDivFlags,
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
            IntBinOp::UDiv | IntBinOp::SDiv | IntBinOp::LShr | IntBinOp::AShr
        ) && self.eat_keyword(Keyword::Exact)?;
        let disjoint_or = matches!(op, IntBinOp::Or) && self.eat_keyword(Keyword::Disjoint)?;

        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between binop operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;
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
                b.build_int_add_with_flags::<IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("add", e))?
                    .as_value()
            }
            IntBinOp::Sub => {
                let mut flags = SubFlags::new();
                if nuw {
                    flags = flags.nuw();
                }
                if nsw {
                    flags = flags.nsw();
                }
                b.build_int_sub_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("sub", e))?
                    .as_value()
            }
            IntBinOp::Mul => {
                let mut flags = MulFlags::new();
                if nuw {
                    flags = flags.nuw();
                }
                if nsw {
                    flags = flags.nsw();
                }
                b.build_int_mul_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("mul", e))?
                    .as_value()
            }
            IntBinOp::Shl => {
                let mut flags = ShlFlags::new();
                if nuw {
                    flags = flags.nuw();
                }
                if nsw {
                    flags = flags.nsw();
                }
                b.build_int_shl_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("shl", e))?
                    .as_value()
            }
            IntBinOp::UDiv => {
                let mut flags = UDivFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.build_int_udiv_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("udiv", e))?
                    .as_value()
            }
            IntBinOp::SDiv => {
                let mut flags = SDivFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.build_int_sdiv_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("sdiv", e))?
                    .as_value()
            }
            IntBinOp::LShr => {
                let mut flags = LShrFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.build_int_lshr_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("lshr", e))?
                    .as_value()
            }
            IntBinOp::AShr => {
                let mut flags = AShrFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.build_int_ashr_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("ashr", e))?
                    .as_value()
            }
            IntBinOp::URem => b
                .build_int_urem::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("urem", e))?
                .as_value(),
            IntBinOp::SRem => b
                .build_int_srem::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("srem", e))?
                .as_value(),
            IntBinOp::And => b
                .build_int_and::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("and", e))?
                .as_value(),
            IntBinOp::Or => {
                let flags = if disjoint_or {
                    OrFlags::new().disjoint()
                } else {
                    OrFlags::new()
                };
                b.build_int_or_with_flags::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("or", e))?
                    .as_value()
            }
            IntBinOp::Xor => b
                .build_int_xor::<llvmkit_ir::IntDyn, _, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("xor", e))?
                .as_value(),
        };
        Ok(v)
    }

    /// `icmp [samesign] PRED TYPE LHS, RHS`. Mirrors `LLParser::parseCompare`.
    fn parse_icmp(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between icmp operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;
        let lhs: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = lhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed lhs"))?;
        let rhs: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = rhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed rhs"))?;
        let name = result_name.as_str();
        let flags = if samesign {
            llvmkit_ir::instr_types::ICmpFlags::new().samesign()
        } else {
            llvmkit_ir::instr_types::ICmpFlags::new()
        };
        let r = b
            .build_int_cmp_with_flags_dyn(pred, lhs, rhs, flags, name)
            .map_err(|e| self.builder_err("icmp", e))?;
        Ok(r.as_value())
    }

    /// `trunc [nuw] [nsw] TYPE VALUE to TYPE` / `zext [nneg] TYPE VALUE to TYPE` / `sext TYPE VALUE to TYPE`.
    /// Mirrors `LLParser::parseCast`'s integer-cast arm.
    fn parse_int_cast(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
        let zext_nneg = matches!(op, IntCast::ZExt) && self.eat_keyword(Keyword::Nneg)?;
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(
            Keyword::To,
            "'to' between cast operand and destination type",
        )?;
        let dst_ty = self.parse_type(false)?;
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
                    b.build_trunc_with_flags_dyn(src_int, dst_int, flags, name)
                } else {
                    b.build_trunc_dyn(src_int, dst_int, name)
                }
                .map_err(|e| self.builder_err("trunc", e))?
                .as_value()
            }
            IntCast::ZExt => if zext_nneg {
                b.build_zext_with_flags_dyn(
                    src_int,
                    dst_int,
                    llvmkit_ir::instr_types::ZExtFlags::new().nneg(),
                    name,
                )
            } else {
                b.build_zext_dyn(src_int, dst_int, name)
            }
            .map_err(|e| self.builder_err("zext", e))?
            .as_value(),
            IntCast::SExt => b
                .build_sext_dyn(src_int, dst_int, name)
                .map_err(|e| self.builder_err("sext", e))?
                .as_value(),
        };
        Ok(v)
    }

    /// `ptrtoint TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// `Instruction::PtrToInt` arm.
    fn parse_ptr_to_int(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            .build_ptr_to_int(src_ptr, dst_int, result_name.as_str())
            .map_err(|e| self.builder_err("ptrtoint", e))?;
        Ok(v.as_value())
    }

    /// `inttoptr TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// `Instruction::IntToPtr` arm.
    fn parse_int_to_ptr(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            .build_int_to_ptr(src_int, dst_ptr, result_name.as_str())
            .map_err(|e| self.builder_err("inttoptr", e))?;
        Ok(v.as_value())
    }

    /// `fneg [nnan ninf ...] TYPE VALUE`. Mirrors `LLParser::parseUnaryOp` for `Instruction::FNeg`.
    fn parse_fneg(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let f: FloatValue<'ctx, FloatDyn, B> = v
            .try_into()
            .map_err(|_| self.expected("float-typed fneg operand"))?;
        let r = if fmf.is_empty() {
            b.build_float_neg::<FloatDyn, _, _>(f, result_name.as_str())
        } else {
            b.build_float_neg_with_flags::<FloatDyn, _, _>(f, fmf, result_name.as_str())
        }
        .map_err(|e| self.builder_err("fneg", e))?;
        Ok(r.as_value())
    }

    /// `OP [nnan ninf ...] TYPE LHS, RHS` for fadd/fsub/fmul/fdiv/frem.
    /// Mirrors `LLParser::parseArithmetic` FP arm.
    fn parse_fp_binop(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        op: FpBinOp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between FP binop operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;
        let lhs: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> = lhs_v
            .try_into()
            .map_err(|_| self.expected("float-typed lhs"))?;
        let rhs: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> = rhs_v
            .try_into()
            .map_err(|_| self.expected("float-typed rhs"))?;
        let name = result_name.as_str();
        let v = match op {
            FpBinOp::Add => if fmf.is_empty() {
                b.build_fp_add::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_add_fmf::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("fadd", e))?
            .as_value(),
            FpBinOp::Sub => if fmf.is_empty() {
                b.build_fp_sub::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_sub_fmf::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("fsub", e))?
            .as_value(),
            FpBinOp::Mul => if fmf.is_empty() {
                b.build_fp_mul::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_mul_fmf::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("fmul", e))?
            .as_value(),
            FpBinOp::Div => if fmf.is_empty() {
                b.build_fp_div::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_div_fmf::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("fdiv", e))?
            .as_value(),
            FpBinOp::Rem => if fmf.is_empty() {
                b.build_fp_rem::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_rem_fmf::<llvmkit_ir::FloatDyn, _, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("frem", e))?
            .as_value(),
        };
        Ok(v)
    }

    /// `fcmp [nnan ninf ...] PRED TYPE LHS, RHS`. Mirrors `LLParser::parseCompare` FP arm.
    fn parse_fcmp(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between fcmp operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;
        let lhs: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> = lhs_v
            .try_into()
            .map_err(|_| self.expected("float-typed lhs"))?;
        let rhs: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> = rhs_v
            .try_into()
            .map_err(|_| self.expected("float-typed rhs"))?;
        let name = result_name.as_str();
        let r = if fmf.is_empty() {
            b.build_fp_cmp::<llvmkit_ir::FloatDyn, _, _, _>(pred, lhs, rhs, name)
                .map_err(|e| self.builder_err("fcmp", e))?
                .as_value()
        } else {
            b.build_fp_cmp_fmf::<llvmkit_ir::FloatDyn, _, _, _>(pred, lhs, rhs, fmf, name)
                .map_err(|e| self.builder_err("fcmp", e))?
                .as_value()
        };
        Ok(r)
    }

    /// `alloca TYPE [, TYPE COUNT] [, align N]`.
    /// Mirrors `LLParser::parseAlloc` (LLParser.cpp ~8540).
    fn parse_alloca(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        // `inalloca` / `swifterror` markers precede the type
        // (`LLParser::parseAlloc`).
        let mut flags = AllocaFlags::none();
        if self.eat_keyword(Keyword::Inalloca)? {
            flags = flags.with_inalloca();
        }
        if self.eat_keyword(Keyword::Swifterror)? {
            flags = flags.with_swifterror();
        }
        let ty = self.parse_type(false)?;
        // Upstream parses size, then alignment, then address space.
        let size = self.parse_optional_comma_array_size(state)?;
        let align = self
            .parse_optional_comma_align()?
            .map(MaybeAlign::new)
            .unwrap_or(MaybeAlign::NONE);
        let addr_space = self.parse_optional_comma_addrspace()?;
        let r = b
            .build_alloca_dyn(ty, size, align, addr_space, flags, result_name.as_str())
            .map_err(|e| self.builder_err("alloca", e))?;
        Ok(r.as_value())
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
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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

        if is_atomic {
            let sync_scope = self.parse_optional_syncscope()?;
            let ordering = self.parse_atomic_ordering("atomic ordering")?;
            self.expect_punct(PunctKind::Comma, "',' after atomic ordering")?;
            let align = self.parse_align_val()?;
            let config = AtomicLoadConfig::new(ordering, sync_scope, align);
            let config = if volatile { config.volatile() } else { config };
            let v = b
                .build_load_atomic(ty, ptr, config, result_name.as_str())
                .map_err(|e| self.builder_err("load", e))?;
            Ok(v)
        } else {
            let align = self.parse_optional_comma_align()?;
            let v = if volatile {
                match align {
                    Some(a) => b.build_load_volatile_with_align(ty, ptr, a, result_name.as_str()),
                    None => b.build_load_volatile(ty, ptr, result_name.as_str()),
                }
            } else {
                match align {
                    Some(a) => b.build_load_with_align(ty, ptr, a, result_name.as_str()),
                    None => b.build_load(ty, ptr, result_name.as_str()),
                }
            }
            .map_err(|e| self.builder_err("load", e))?;
            Ok(v)
        }
    }

    /// `store [volatile] TYPE VALUE, ptr PTR [, align N]` or
    /// `store atomic [volatile] TYPE VALUE, ptr PTR [syncscope("...")] ORDERING, align N`.
    /// Mirrors `LLParser::parseStore` (LLParser.cpp ~8658). Returns no value.
    fn parse_store(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
        if is_atomic {
            let sync_scope = self.parse_optional_syncscope()?;
            let ordering = self.parse_atomic_ordering("atomic ordering")?;
            self.expect_punct(PunctKind::Comma, "',' after atomic ordering")?;
            let align = self.parse_align_val()?;
            let config = AtomicStoreConfig::new(ordering, sync_scope, align);
            let config = if volatile { config.volatile() } else { config };
            b.build_store_atomic(val_v, ptr, config)
                .map_err(|e| self.builder_err("store", e))?;
        } else {
            let align = self.parse_optional_comma_align()?;
            match (volatile, align) {
                (true, Some(a)) => b.build_store_volatile_with_align(val_v, ptr, a),
                (true, None) => b.build_store_volatile(val_v, ptr),
                (false, Some(a)) => b.build_store_with_align(val_v, ptr, a),
                (false, None) => b.build_store(val_v, ptr),
            }
            .map_err(|e| self.builder_err("store", e))?;
        }
        Ok(())
    }

    /// `getelementptr FLAGS SOURCE_TY, ptr P, INDEX, INDEX, ...` where
    /// FLAGS is any-order `inbounds` / `nusw` / `nuw`.
    /// Mirrors `LLParser::parseGetElementPtr` (LLParser.cpp ~8900).
    fn parse_gep(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
        while self.eat_punct(PunctKind::Comma)? {
            let idx_ty = self.parse_type(false)?;
            let idx_v = self.parse_value(state, idx_ty)?;
            let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = idx_v
                .try_into()
                .map_err(|_| self.expected("integer GEP index"))?;
            indices.push(idx);
        }
        let name = result_name.as_str();
        let v = b
            .build_gep_with_flags(source_ty, ptr, indices, flags, name)
            .map_err(|e| self.builder_err("getelementptr", e))?;
        Ok(v.as_value())
    }

    /// `select i1 COND, TYPE TRUE, TYPE FALSE`. Dispatches to
    /// [`llvmkit_ir::IRBuilder::build_select`] on the appropriate
    /// [`llvmkit_ir::SelectArm`] depending on the arm category. Mirrors
    /// `LLParser::parseSelect`.
    fn parse_select(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
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
        let valid_arm_type = match true_ty.into_type_enum() {
            AnyTypeEnum::Int(_) | AnyTypeEnum::Float(_) | AnyTypeEnum::Pointer(_) => true,
            AnyTypeEnum::Vector(ty) => matches!(
                ty.element().into_type_enum(),
                AnyTypeEnum::Int(_) | AnyTypeEnum::Float(_) | AnyTypeEnum::Pointer(_)
            ),
            _ => false,
        };
        if !valid_arm_type {
            return Err(self.expected("select arm category supported by this parser (int/fp/ptr)"));
        }
        if let (Ok(condition), Ok(true_constant), Ok(false_constant)) = (
            Constant::try_from(cond_value),
            Constant::try_from(true_v),
            Constant::try_from(false_v),
        ) && let Some(folded) =
            constant_fold_select_instruction(condition, true_constant, false_constant)
                .map_err(|e| self.builder_err("select", e))?
        {
            return Ok(folded.as_value());
        }
        let cond_iv: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = cond_value
            .try_into()
            .map_err(|_| self.expected("integer-typed select condition"))?;
        let cond_i1: llvmkit_ir::IntValue<'ctx, bool, B> = cond_iv
            .try_into()
            .map_err(|_| self.expected("i1 select condition"))?;
        let name = result_name.as_str();
        let v = match true_ty.into_type_enum() {
            AnyTypeEnum::Int(_) => {
                let t: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = true_v
                    .try_into()
                    .map_err(|_| self.expected("int-typed select arm"))?;
                let f: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn, B> = false_v
                    .try_into()
                    .map_err(|_| self.expected("int-typed select arm"))?;
                b.build_select(cond_i1, t, f, name)
                    .map_err(|e| self.builder_err("select", e))?
                    .as_value()
            }
            AnyTypeEnum::Float(_) => {
                let t: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> = true_v
                    .try_into()
                    .map_err(|_| self.expected("float-typed select arm"))?;
                let f: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn, B> =
                    false_v
                        .try_into()
                        .map_err(|_| self.expected("float-typed select arm"))?;
                b.build_select(cond_i1, t, f, name)
                    .map_err(|e| self.builder_err("select", e))?
                    .as_value()
            }
            AnyTypeEnum::Pointer(_) => {
                let t: llvmkit_ir::PointerValue<'ctx, B> = true_v
                    .try_into()
                    .map_err(|_| self.expected("ptr-typed select arm"))?;
                let f: llvmkit_ir::PointerValue<'ctx, B> = false_v
                    .try_into()
                    .map_err(|_| self.expected("ptr-typed select arm"))?;
                b.build_select(cond_i1, t, f, name)
                    .map_err(|e| self.builder_err("select", e))?
                    .as_value()
            }
            _ => {
                return Err(
                    self.expected("select arm category supported by this parser (int/fp/ptr)")
                );
            }
        };
        Ok(v)
    }

    /// `fptosi`/`fptoui TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// for `Instruction::FPToSI` / `FPToUI`.
    fn parse_fp_to_int(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            FpToInt::FpToSI => b
                .build_fp_to_si(src_fp, dst_int, name)
                .map_err(|e| self.builder_err("fptosi", e))?
                .as_value(),
            FpToInt::FpToUI => b
                .build_fp_to_ui(src_fp, dst_int, name)
                .map_err(|e| self.builder_err("fptoui", e))?
                .as_value(),
        };
        Ok(v)
    }

    /// `sitofp`/`uitofp TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// for `Instruction::SIToFP` / `UIToFP`.
    fn parse_int_to_fp(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        op: IntToFp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let nneg = matches!(op, IntToFp::UIToFp) && self.eat_keyword(Keyword::Nneg)?;
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
            IntToFp::SIToFp => b
                .build_si_to_fp(src_int, dst_fp, name)
                .map_err(|e| self.builder_err("sitofp", e))?
                .as_value(),
            IntToFp::UIToFp => {
                if nneg {
                    b.build_ui_to_fp_with_flags_dyn(
                        src_int,
                        dst_fp,
                        UIToFpFlags::new().nneg(),
                        name,
                    )
                    .map_err(|e| self.builder_err("uitofp", e))?
                    .as_value()
                } else {
                    b.build_ui_to_fp(src_int, dst_fp, name)
                        .map_err(|e| self.builder_err("uitofp", e))?
                        .as_value()
                }
            }
        };
        Ok(v)
    }

    /// `addrspacecast ptr VALUE to ptr`. Mirrors `LLParser::parseCast`
    /// for `Instruction::AddrSpaceCast`.
    fn parse_addrspace_cast(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            .build_addrspace_cast(src_ptr, dst_ptr, result_name.as_str())
            .map_err(|e| self.builder_err("addrspacecast", e))?;
        Ok(v.as_value())
    }

    // ── S3.2: new opcode parsers ──────────────────────────────────────────

    /// `bitcast <src-ty> <src-val> to <dst-ty>`. Mirrors `LLParser::parseCast`
    /// `Instruction::BitCast` arm. Uses `build_bitcast_dyn` for the parser's
    /// runtime-typed path.
    ///
    /// Upstream: `test/Assembler/bitcast.ll`.
    fn parse_bitcast(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in bitcast")?;
        let dst_ty = self.parse_type(false)?;
        let name = result_name.as_str();
        let v = b
            .build_bitcast_dyn(src_v, dst_ty, name)
            .map_err(|e| self.builder_err("bitcast", e))?;
        Ok(v)
    }

    /// `fptrunc <fp-ty> <val> to <fp-ty>`. Mirrors `LLParser::parseCast`
    /// `Instruction::FPTrunc` arm. Uses `build_fp_trunc_dyn`.
    ///
    /// Upstream: `test/Assembler/fptrunc.ll`.
    fn parse_fptrunc(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
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
            .build_fp_trunc_dyn(sv, df, result_name.as_str())
            .map_err(|e| self.builder_err("fptrunc", e))?;
        Ok(v.as_value())
    }

    /// `fpext <fp-ty> <val> to <fp-ty>`. Mirrors `LLParser::parseCast`
    /// `Instruction::FPExt` arm. Uses `build_fp_ext_dyn`.
    ///
    /// Upstream: `test/Assembler/fpext.ll`.
    fn parse_fpext(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
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
            .build_fp_ext_dyn(sv, df, result_name.as_str())
            .map_err(|e| self.builder_err("fpext", e))?;
        Ok(v.as_value())
    }

    /// `ptrtoaddr <ptr-or-vector-ty> <val> to <int-or-vector-ty>`. Mirrors
    /// `LLParser::parseCast` for `Instruction::PtrToAddr`.
    ///
    /// Upstream: `test/Assembler/ptrtoaddr.ll`.
    fn parse_ptrtoaddr(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in ptrtoaddr")?;
        let dst_ty = self.parse_type(false)?;
        let v = b
            .build_ptr_to_addr_dyn(src_v, dst_ty, result_name.as_str())
            .map_err(|e| self.builder_err("ptrtoaddr", e))?;
        Ok(v)
    }

    /// `extractelement <vec-ty> <vec>, <idx-ty> <idx>`.
    /// Mirrors `LLParser::parseExtractElement`.
    ///
    /// Upstream: `test/Assembler/extractelement.ll`.
    fn parse_extractelement(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            .build_extract_element(vec_v, idx, result_name.as_str())
            .map_err(|e| self.builder_err("extractelement", e))?;
        Ok(v)
    }

    /// `insertelement <vec-ty> <vec>, <elt-ty> <elt>, <idx-ty> <idx>`.
    /// Mirrors `LLParser::parseInsertElement`.
    ///
    /// Upstream: `test/Assembler/insertelement.ll`.
    fn parse_insertelement(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            .build_insert_element(vec_v, elt_v, idx, result_name.as_str())
            .map_err(|e| self.builder_err("insertelement", e))?;
        Ok(v)
    }

    /// `shufflevector <vec-ty> <v1>, <vec-ty> <v2>, <mask>`.
    /// The mask is `< i32 N, i32 M, ... >` or `poison`. Mirrors
    /// `LLParser::parseShuffleVector`.
    ///
    /// Upstream: `test/Assembler/shufflevector.ll`.
    fn parse_shufflevector(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            .build_shuffle_vector(v1, v2, &mask, result_name.as_str())
            .map_err(|e| self.builder_err("shufflevector", e))?;
        Ok(v)
    }

    /// Parse a shufflevector mask typed constant operand and decode it with
    /// `ShuffleVectorInst::getShuffleMask` semantics.
    fn parse_shuffle_mask(&mut self, vector_ty: Type<'ctx, B>) -> ParseResult<Vec<i32>> {
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
            ParseError::Lex(LexError::UnknownToken { span }) => ParseError::Expected {
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
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let agg_ty = self.parse_type(false)?;
        let agg_v = self.parse_value(state, agg_ty)?;
        let mut indices = Vec::new();
        while self.eat_punct(PunctKind::Comma)? {
            let idx = self.parse_uint32("extractvalue index")?;
            indices.push(idx);
        }
        let v = b
            .build_extract_value_dyn(agg_v, &indices, result_name.as_str())
            .map_err(|e| self.builder_err("extractvalue", e))?;
        Ok(v)
    }

    /// `insertvalue <agg-ty> <agg>, <elt-ty> <elt>, <idx>, ...`. Mirrors
    /// `LLParser::parseInsertValue`.
    ///
    /// Upstream: `test/Assembler/insertvalue.ll`.
    fn parse_insertvalue(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let agg_ty = self.parse_type(false)?;
        let agg_v = self.parse_value(state, agg_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after agg in insertvalue")?;
        let elt_ty = self.parse_type(false)?;
        let elt_v = self.parse_value(state, elt_ty)?;
        let mut indices = Vec::new();
        while self.eat_punct(PunctKind::Comma)? {
            let idx = self.parse_uint32("insertvalue index")?;
            indices.push(idx);
        }
        let v = b
            .build_insert_value_dyn(agg_v, elt_v, &indices, result_name.as_str())
            .map_err(|e| self.builder_err("insertvalue", e))?;
        Ok(v)
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
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let _fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        let name = result_name.as_str();
        // Build the phi and extract its value ID for deferred edge resolution.
        let phi_val = match ty.into_type_enum() {
            AnyTypeEnum::Int(int_ty) => {
                let phi = b
                    .build_int_phi_dyn(int_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.as_value()
            }
            AnyTypeEnum::Float(fp_ty) => {
                let phi = b
                    .build_fp_phi_dyn(fp_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.as_value()
            }
            AnyTypeEnum::Pointer(ptr_ty) => {
                let phi = b
                    .build_pointer_phi_in_addrspace(ptr_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.as_value()
            }
            // The remaining first-class *data* types — vector, array, and
            // non-opaque struct — are legal phi result types. Route them through
            // the erased `build_phi_dyn`; the type-checked incoming-add path is
            // unchanged. The `is_first_class` guard is what excludes an *opaque*
            // struct (no body, hence unsized): it is `AnyTypeEnum::Struct` but
            // not a valid phi result, and `Module::verify()` rejects it, so
            // reject it here rather than parse IR that cannot verify.
            AnyTypeEnum::Vector(_) | AnyTypeEnum::Array(_) | AnyTypeEnum::Struct(_)
                if ty.is_first_class() =>
            {
                let phi = b
                    .build_phi_dyn(ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.as_value()
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
        state.phi_locs.push((phi_val.id(), self.loc()));
        // Parse incoming pairs: `[ val, label ], ...`
        // First pair has no leading comma; subsequent pairs have one.
        let mut first = true;
        loop {
            if !first {
                if !self.eat_punct(PunctKind::Comma)? {
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
            // Try to resolve the value from already-defined locals.
            let val_ref = match self.peek() {
                Token::LocalVar(_) => {
                    let n = self
                        .current_str_payload()
                        .ok_or_else(|| self.expected("local name in phi incoming value"))?;
                    self.bump()?;
                    // Try to resolve immediately; defer if not yet defined.
                    if let Some(v) = state.local_named.get(&n).copied() {
                        PhiValRef::Resolved(v)
                    } else {
                        PhiValRef::Named(n)
                    }
                }
                Token::LocalVarId(id) => {
                    let id = *id;
                    self.bump()?;
                    if let Some(v) = state.local_numbered.get(&id).copied() {
                        PhiValRef::Resolved(v)
                    } else {
                        PhiValRef::Numbered(id)
                    }
                }
                Token::IntegerLit(_) => {
                    let int_ty = match ty.into_type_enum() {
                        AnyTypeEnum::Int(t) => t,
                        _ => return Err(self.expected("integer literal only valid for int phi")),
                    };
                    let parsed =
                        self.parse_int_literal(ExpectedIntWidth::Bits(int_ty.bit_width()))?;
                    let bits = lower_parsed_apsint(&parsed, int_ty.bit_width());
                    let c = int_ty
                        .const_ap_int(&bits)
                        .map_err(|e| self.builder_err("phi constant", e))?;
                    PhiValRef::Resolved(c.as_value())
                }
                Token::Kw(Keyword::Zeroinitializer) => {
                    self.bump()?;
                    let v = match ty.into_type_enum() {
                        AnyTypeEnum::Int(t) => t.const_zero().as_value(),
                        AnyTypeEnum::Pointer(t) => t.const_null().as_value(),
                        _ => return Err(self.expected("zeroinitializer for int/ptr phi")),
                    };
                    PhiValRef::Resolved(v)
                }
                Token::Kw(Keyword::Null) => {
                    self.bump()?;
                    let v = match ty.into_type_enum() {
                        AnyTypeEnum::Pointer(t) => t.const_null().as_value(),
                        _ => return Err(self.expected("null only valid for pointer phi")),
                    };
                    PhiValRef::Resolved(v)
                }
                Token::Kw(Keyword::Undef) | Token::Kw(Keyword::Poison) => {
                    // undef/poison: treat as zeroinitializer for now (value is
                    // structurally needed to type-check the phi; semantics are
                    // handled by the verifier / optimizer).
                    self.bump()?;
                    let v = match ty.into_type_enum() {
                        AnyTypeEnum::Int(t) => t.const_zero().as_value(),
                        AnyTypeEnum::Pointer(t) => t.const_null().as_value(),
                        AnyTypeEnum::Float(_t) => {
                            self.expect_punct(PunctKind::Comma, "',' in phi incoming pair")?;
                            let bb_ref = self.parse_phi_label(state)?;
                            self.expect_punct(PunctKind::RSquare, "']' in phi incoming pair")?;
                            state.deferred_phi.push(DeferredPhiEdge {
                                phi_val,
                                val_ref: PhiValRef::Undef,
                                bb_ref,
                                loc: val_loc,
                            });
                            continue;
                        }
                        _ => return Err(self.expected("undef for int/float/ptr phi")),
                    };
                    PhiValRef::Resolved(v)
                }
                _ => return Err(self.expected("value in phi incoming pair")),
            };
            self.expect_punct(PunctKind::Comma, "',' in phi incoming pair")?;
            let bb_ref = self.parse_phi_label(state)?;
            self.expect_punct(PunctKind::RSquare, "']' to close phi incoming pair")?;
            // Either resolve immediately or defer.
            match val_ref {
                PhiValRef::Resolved(v) => {
                    let bb = state.resolve_block_ref(self.module, &bb_ref, val_loc)?;
                    let tmp_b = llvmkit_ir::IRBuilder::new(self.module);
                    tmp_b
                        .phi_add_incoming_from_value(phi_val, v, bb)
                        .map_err(|e| self.builder_err("phi.add_incoming", e))?;
                }
                other => {
                    state.deferred_phi.push(DeferredPhiEdge {
                        phi_val,
                        val_ref: other,
                        bb_ref,
                        loc: val_loc,
                    });
                }
            }
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
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
        if matches!(
            self.peek(),
            Token::Instruction(crate::ll_token::Opcode::Call)
        ) {
            self.bump()?;
        }
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
            return Err(self.expected(
                "expected '...' at end of argument list for musttail call in varargs function",
            ));
        }
        let (function_attrs, function_attr_groups) = self.parse_optional_fn_attrs()?;
        let operand_bundles = self.parse_optional_operand_bundles(state)?;
        let call_attrs = llvmkit_ir::instr_types::CallAttributeData::new(
            return_attrs,
            arg_attrs.into_boxed_slice(),
            function_attrs,
        )
        .function_attr_groups(function_attr_groups.into_boxed_slice())
        .operand_bundles(operand_bundles);
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => self.module.fn_type(callee_ty, arg_tys, var_args),
        };
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
                builder
                    .name(name)
                    .build()
                    .map_err(|e| self.builder_err("call", e))?
                    .as_value()
            }
            ParsedCallee::InlineAsm(asm) => {
                if asm.label_constraint_count() != 0 {
                    return Err(self.expected("inline asm call without label constraints"));
                }
                b.build_inline_asm_call::<llvmkit_ir::Dyn, _, _, _>(asm, args, name)
                    .map_err(|e| self.builder_err("call", e))?
                    .as_value()
            }
            ParsedCallee::Indirect(callee) => b
                .build_indirect_call_dyn::<llvmkit_ir::Dyn, _, _, _>(
                    parsed_fn_ty,
                    callee,
                    args,
                    name,
                )
                .map_err(|e| self.builder_err("indirect call", e))?
                .as_value(),
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
            Token::Kw(Keyword::Asm) => {
                self.bump()?;
                let has_side_effects = self.eat_keyword(Keyword::Sideeffect)?;
                let is_align_stack = self.eat_keyword(Keyword::Alignstack)?;
                let dialect = if self.eat_keyword(Keyword::Inteldialect)? {
                    llvmkit_ir::AsmDialect::Intel
                } else {
                    llvmkit_ir::AsmDialect::ATT
                };
                let can_unwind = self.eat_keyword(Keyword::Unwind)?;
                let asm = self.parse_string_constant("inline asm string")?;
                self.expect_punct(PunctKind::Comma, "',' after inline asm string")?;
                let constraints = self.parse_string_constant("inline asm constraint string")?;
                Ok(ParsedDirectCallee::InlineAsm(ParsedInlineAsm {
                    asm,
                    constraints,
                    has_side_effects,
                    is_align_stack,
                    dialect,
                    can_unwind,
                }))
            }
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
                if let Some(f) = self.module.function_by_name_dyn(&name) {
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
                        Ok(ParsedCallee::Function(f))
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
                                expected: format!("forward function declaration: {e}"),
                                loc: DiagLoc::span(loc),
                            })?;
                        self.forward_function_decls.entry(name).or_insert(loc);
                        Ok(ParsedCallee::Function(f))
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
                    kind: crate::parse_error::SymbolKind::Global,
                    id: crate::parse_error::SymbolId::Numbered(id),
                    loc: DiagLoc::span(loc),
                }),
            ParsedDirectCallee::InlineAsm(data) => Ok(ParsedCallee::InlineAsm(
                self.module.inline_asm(
                    parsed_fn_ty,
                    data.asm,
                    data.constraints,
                    llvmkit_ir::InlineAsmOptions::new()
                        .side_effects(data.has_side_effects)
                        .align_stack(data.is_align_stack)
                        .with_dialect(data.dialect)
                        .with_can_unwind(data.can_unwind),
                ),
            )),
            ParsedDirectCallee::Value { v, loc } => {
                // Mirrors `PerFunctionState::getVal`'s type check: whatever
                // value form the callee took, it must be pointer-typed.
                let callee =
                    llvmkit_ir::PointerValue::try_from(v).map_err(|e| ParseError::Expected {
                        expected: format!("pointer callee: {e}"),
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
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            .build_va_arg(list_ptr, result_ty, result_name.as_str())
            .map_err(|e| self.builder_err("va_arg", e))?;
        Ok(v.as_value())
    }

    /// `freeze <ty> <val>`. Mirrors `LLParser::parseFreeze`.
    ///
    /// Upstream: `test/Assembler/freeze.ll`.
    fn parse_freeze(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let r = b
            .build_freeze(v, result_name.as_str())
            .map_err(|e| self.builder_err("freeze", e))?;
        Ok(r.as_value())
    }

    /// `switch <ty> <val>, label %default [ <ty> N, label %case ... ]`.
    /// Mirrors `LLParser::parseSwitch` (LLParser.cpp ~7640).
    ///
    /// Upstream: `test/Assembler/switch.ll`.
    fn parse_switch(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'m, 'ctx, B>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `switch`
        let cond_ty = self.parse_type(false)?;
        let cond_v = self.parse_value(state, cond_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after switch condition")?;
        self.expect_primitive(
            crate::ll_token::PrimitiveTy::Label,
            "'label' for switch default",
        )?;
        let default_bb = self.parse_block_ref(state)?;
        let (_, mut sw) = b
            .build_switch_dyn(cond_v, default_bb, "")
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
            self.expect_primitive(
                crate::ll_token::PrimitiveTy::Label,
                "'label' for switch case destination",
            )?;
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
        b: ParsedBlockBuilder<'m, 'ctx, B>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `indirectbr`
        let addr_ty = self.parse_type(false)?;
        let addr_v = self.parse_value(state, addr_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after indirectbr address")?;
        let addr: PointerValue<'ctx, B> = addr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed indirectbr address"))?;
        let (_, mut ibr) = b
            .build_indirectbr(addr, "")
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
            self.expect_primitive(
                crate::ll_token::PrimitiveTy::Label,
                "'label' in indirectbr destination",
            )?;
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
    fn parse_fence(&mut self, b: &ParsedBlockBuilder<'m, 'ctx, B>) -> ParseResult<()> {
        let sync_scope = self.parse_optional_syncscope()?;
        let ordering = self.parse_atomic_ordering("fence ordering")?;
        let _ = b
            .build_fence(ordering, sync_scope, "")
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
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
            .build_atomic_cmpxchg(ptr, cmp_v, new_v, config, result_name.as_str())
            .map_err(|e| self.builder_err("cmpxchg", e))?;
        Ok(v.as_value())
    }

    /// `atomicrmw [volatile] <op> ptr <ptr>, <ty> <val>
    ///           [syncscope("...")] <ordering> [, align N]`.
    /// Returns the old value. Mirrors `LLParser::parseAtomicRMW`.
    ///
    /// Upstream: `test/Assembler/atomicrmw.ll`.
    fn parse_atomicrmw(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
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
        let (val_v, deferred_value) = self.parse_value_or_deferred_local(state, val_ty)?;
        let sync_scope = self.parse_optional_syncscope()?;
        let ordering = self.parse_atomic_ordering("atomicrmw ordering")?;
        let align = self.parse_optional_comma_align()?;
        let align = match align {
            Some(value) => llvmkit_ir::align::MaybeAlign::from(value),
            None => llvmkit_ir::align::MaybeAlign::NONE,
        };
        let mut config =
            llvmkit_ir::instr_types::AtomicRMWConfig::new(ordering, sync_scope).align(align);
        if volatile {
            config = config.volatile();
        }
        let v = b
            .build_atomicrmw(op, ptr, val_v, config, result_name.as_str())
            .map_err(|e| self.builder_err("atomicrmw", e))?;
        if let Some((val_ref, loc)) = deferred_value {
            state
                .deferred_atomicrmw_values
                .push(DeferredAtomicRmwValue {
                    inst: v,
                    val_ref,
                    loc,
                });
        }
        Ok(v.as_value())
    }

    /// Parse an `atomicrmw` operation keyword.
    fn parse_atomicrmw_op(&mut self) -> ParseResult<AtomicRMWBinOp> {
        use AtomicRMWBinOp as Op;
        let op = match self.peek() {
            Token::Kw(Keyword::Xchg) => Op::Xchg,
            Token::Instruction(crate::ll_token::Opcode::Add) => Op::Add,
            Token::Instruction(crate::ll_token::Opcode::Sub) => Op::Sub,
            Token::Instruction(crate::ll_token::Opcode::And) => Op::And,
            Token::Kw(Keyword::Nand) => Op::Nand,
            Token::Instruction(crate::ll_token::Opcode::Or) => Op::Or,
            Token::Instruction(crate::ll_token::Opcode::Xor) => Op::Xor,
            Token::Kw(Keyword::Max) => Op::Max,
            Token::Kw(Keyword::Min) => Op::Min,
            Token::Kw(Keyword::Umax) => Op::UMax,
            Token::Kw(Keyword::Umin) => Op::UMin,
            Token::Instruction(crate::ll_token::Opcode::FAdd) => Op::FAdd,
            Token::Instruction(crate::ll_token::Opcode::FSub) => Op::FSub,
            Token::Kw(Keyword::Fmax) => Op::FMax,
            Token::Kw(Keyword::Fmin) => Op::FMin,
            Token::Kw(Keyword::Fmaximum) => Op::FMaximum,
            Token::Kw(Keyword::Fminimum) => Op::FMinimum,
            Token::Kw(Keyword::UincWrap) => Op::UIncWrap,
            Token::Kw(Keyword::UdecWrap) => Op::UDecWrap,
            Token::Kw(Keyword::UsubCond) => Op::USubCond,
            Token::Kw(Keyword::UsubSat) => Op::USubSat,
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
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        let result_ty = self.parse_type(false)?;
        let cleanup = self.eat_keyword(Keyword::Cleanup)?;
        let mut lp = b
            .build_landingpad(result_ty, cleanup, result_name.as_str())
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
        Ok(lp.finish().as_value())
    }

    /// `cleanuppad within <token-or-none> [<args>]`. Non-terminator.
    /// Mirrors `LLParser::parseCleanupPad`.
    ///
    /// Upstream: `test/Assembler/cleanuppad.ll`.
    fn parse_cleanuppad(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.expect_keyword(Keyword::Within, "'within' in cleanuppad")?;
        let parent_pad = self.parse_optional_pad_token(state)?;
        let args = self.parse_bracket_value_list(state)?;
        let v = match parent_pad {
            Some(parent) => b.build_cleanup_pad(parent, args, result_name.as_str()),
            None => b.build_cleanup_pad_within_none(args, result_name.as_str()),
        }
        .map_err(|e| self.builder_err("cleanuppad", e))?;
        Ok(v.as_value())
    }

    /// `catchpad within <catchswitch> [<args>]`. Non-terminator.
    /// Mirrors `LLParser::parseCatchPad`.
    ///
    /// Upstream: `test/Assembler/catchpad.ll`.
    fn parse_catchpad(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: &ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.expect_keyword(Keyword::Within, "'within' in catchpad")?;
        let parent_ty = self.parse_type(false)?;
        let parent_v = self.parse_value(state, parent_ty)?;
        let args = self.parse_bracket_value_list(state)?;
        let v = b
            .build_catch_pad(parent_v, args, result_name.as_str())
            .map_err(|e| self.builder_err("catchpad", e))?;
        Ok(v.as_value())
    }

    /// `resume <ty> <val>`. Terminator.
    /// Mirrors `LLParser::parseResume` (LLParser.cpp ~7762).
    ///
    /// Upstream: `test/Assembler/resume.ll`.
    fn parse_resume(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'m, 'ctx, B>,
    ) -> ParseResult<()> {
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let _ = b
            .build_resume(v, "")
            .map_err(|e| self.builder_err("resume", e))?;
        Ok(())
    }

    /// `cleanupret from <val> [unwind (to caller | label %bb)]`.
    /// Terminator. Mirrors `LLParser::parseCleanupRet`.
    ///
    /// Upstream: `test/Assembler/cleanupret.ll`.
    fn parse_cleanupret(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'m, 'ctx, B>,
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
                    crate::ll_token::PrimitiveTy::Label,
                    "'label' in cleanupret unwind destination",
                )?;
                Some(self.parse_block_ref(state)?)
            }
        } else {
            None
        };
        let _ = match unwind_dest {
            Some(dest) => b.build_cleanup_ret(pad_v, dest, ""),
            None => b.build_cleanup_ret_to_caller(pad_v, ""),
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
        b: ParsedBlockBuilder<'m, 'ctx, B>,
    ) -> ParseResult<()> {
        self.expect_keyword(Keyword::From, "'from' in catchret")?;
        let pad_ty = self.parse_type(false)?;
        let pad_v = self.parse_value(state, pad_ty)?;
        self.expect_keyword(Keyword::To, "'to' in catchret")?;
        self.expect_primitive(
            crate::ll_token::PrimitiveTy::Label,
            "'label' in catchret destination",
        )?;
        let dest = self.parse_block_ref(state)?;
        let _ = b
            .build_catch_ret(pad_v, dest, "")
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
        b: ParsedBlockBuilder<'m, 'ctx, B>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx, B>> {
        self.bump()?; // eat `catchswitch`
        self.expect_keyword(Keyword::Within, "'within' in catchswitch")?;
        let parent_pad = self.parse_optional_pad_token(state)?;
        // `[handler1, handler2, ...]`
        self.expect_punct(PunctKind::LSquare, "'[' in catchswitch handlers")?;
        let mut handlers: Vec<BasicBlockLabel<'ctx, llvmkit_ir::Dyn, B>> = Vec::new();
        loop {
            if matches!(self.peek(), Token::RSquare) {
                self.bump()?;
                break;
            }
            self.expect_primitive(
                crate::ll_token::PrimitiveTy::Label,
                "'label' in catchswitch handler",
            )?;
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
                crate::ll_token::PrimitiveTy::Label,
                "'label' in catchswitch unwind destination",
            )?;
            Some(self.parse_block_ref(state)?)
        };
        let name = result_name.as_str();
        let (_, mut cs) = match (parent_pad, unwind_dest) {
            (Some(parent), Some(dest)) => b.build_catch_switch(parent, dest, name),
            (Some(parent), None) => b.build_catch_switch_to_caller(parent, name),
            (None, Some(dest)) => b.build_catch_switch_within_none(dest, name),
            (None, None) => b.build_catch_switch_within_none_to_caller(name),
        }
        .map_err(|e| self.builder_err("catchswitch", e))?;
        for h in handlers {
            cs = cs
                .add_handler(h)
                .map_err(|e| self.builder_err("catchswitch.add_handler", e))?;
        }
        Ok(cs.finish().as_value())
    }

    /// `invoke [cc] [ret-attrs] <ret-ty> @func(<args>) to label %normal
    ///        unwind label %unwind`. Terminator.
    /// Mirrors `LLParser::parseInvoke`.
    ///
    /// Upstream: `test/Assembler/invoke.ll`.
    fn parse_invoke(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
        b: ParsedBlockBuilder<'m, 'ctx, B>,
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
        self.expect_primitive(
            crate::ll_token::PrimitiveTy::Label,
            "'label' for invoke normal destination",
        )?;
        let normal_bb = self.parse_block_ref(state)?;
        self.expect_keyword(Keyword::Unwind, "'unwind' in invoke")?;
        self.expect_primitive(
            crate::ll_token::PrimitiveTy::Label,
            "'label' for invoke unwind destination",
        )?;
        let unwind_bb = self.parse_block_ref(state)?;
        // Upstream `resolveFunctionType`: an explicitly written function
        // type IS the call-site type; otherwise infer from the arguments.
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => self.module.fn_type(callee_ty, arg_tys, var_args),
        };
        let callee = self.resolve_direct_callee(parsed_callee, parsed_fn_ty)?;
        let name = result_name.as_str();
        let (_, inst) = match callee {
            ParsedCallee::Function(callee) => b
                .build_invoke_dyn_with_config(
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
            ParsedCallee::InlineAsm(asm) => {
                if asm.label_constraint_count() != 0 {
                    return Err(self.expected("inline asm call without label constraints"));
                }
                b.build_inline_asm_invoke_with_config::<llvmkit_ir::Dyn, _, _, _, _>(
                    asm,
                    args,
                    normal_bb,
                    unwind_bb,
                    llvmkit_ir::CallSiteConfig::new(name)
                        .calling_conv(calling_conv)
                        .attrs(call_attrs),
                )
                .map_err(|e| self.builder_err("invoke", e))?
            }
            ParsedCallee::Indirect(callee_ptr) => b
                .build_indirect_invoke_dyn_with_config::<llvmkit_ir::Dyn, _, _, _, _>(
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
            Ok(Some(inst.as_value()))
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
        b: ParsedBlockBuilder<'m, 'ctx, B>,
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
            crate::ll_token::PrimitiveTy::Label,
            "'label' for callbr fallthrough destination",
        )?;
        let fallthrough = self.parse_block_ref(state)?;
        // Optional `[ label %ind1, ... ]`
        let mut indirect: Vec<BasicBlockLabel<'ctx, llvmkit_ir::Dyn, B>> = Vec::new();
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
                self.expect_primitive(
                    crate::ll_token::PrimitiveTy::Label,
                    "'label' in callbr indirect target",
                )?;
                let bb = self.parse_block_ref(state)?;
                indirect.push(bb);
                let _ = self.eat_punct(PunctKind::Comma)?;
            }
        }
        // Upstream `resolveFunctionType`: an explicitly written function
        // type IS the call-site type; otherwise infer from the arguments.
        let parsed_fn_ty = match callee_ty.into_type_enum() {
            AnyTypeEnum::Function(fn_ty) => fn_ty,
            _ => self.module.fn_type(callee_ty, arg_tys, var_args),
        };
        let callee = self.resolve_direct_callee(parsed_callee, parsed_fn_ty)?;
        let name = result_name.as_str();
        let (_, inst) = match callee {
            ParsedCallee::Function(callee) => b
                .build_callbr_with_config(
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
            ParsedCallee::InlineAsm(asm) => {
                if asm.label_constraint_count() != indirect.len() {
                    return Err(self.expected(
                        "inline asm callbr label constraint count matches indirect labels",
                    ));
                }
                b.build_inline_asm_callbr_with_config::<llvmkit_ir::Dyn, _, _, _, _, _>(
                    asm,
                    args,
                    fallthrough,
                    indirect,
                    llvmkit_ir::CallSiteConfig::new(name)
                        .calling_conv(calling_conv)
                        .attrs(call_attrs),
                )
                .map_err(|e| self.builder_err("callbr", e))?
            }
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
            Ok(Some(inst.as_value()))
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
            expected: format!("valid {label}: {e}"),
            loc: DiagLoc::span(self.loc()),
        }
    }

    /// Parse a `label %name` / `label %N` operand. Forward references create
    /// an empty block, but existing references return label identity only so
    /// branches may target already-terminated blocks.
    fn parse_block_ref(
        &mut self,
        state: &mut PerFunctionState<'ctx, B>,
    ) -> ParseResult<BasicBlockLabel<'ctx, llvmkit_ir::Dyn, B>> {
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

    fn parse_value_or_deferred_local(
        &mut self,
        state: &PerFunctionState<'ctx, B>,
        ty: Type<'ctx, B>,
    ) -> ParseResult<ParsedValueOrDeferredLocal<'ctx, B>> {
        let loc = self.loc();
        let id = self.parse_val_id(Some(state), Some(ty))?;
        match id {
            ValId::LocalName(name) => match state.local_named.get(&name).copied() {
                Some(value) => Ok((value, None)),
                None => Ok((
                    ty.get_undef().as_value(),
                    Some((DeferredLocalValueRef::Named(name), loc)),
                )),
            },
            ValId::LocalId(id) => match state.local_numbered.get(&id).copied() {
                Some(value) => Ok((value, None)),
                None => Ok((
                    ty.get_undef().as_value(),
                    Some((DeferredLocalValueRef::Numbered(id), loc)),
                )),
            },
            other => self
                .convert_val_id_to_value(ty, other, Some(state))
                .map(|value| (value, None)),
        }
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
                FpLit::HexBFloat(s) => parse_hex_apfloat(ApFloatSemantics::BFloat, s)
                    .map_err(|_| self.expected("valid hex bfloat literal"))?,
                FpLit::HexX87(s) => parse_hex_apfloat(ApFloatSemantics::X87DoubleExtended, s)
                    .map_err(|_| self.expected("valid hex x87 literal"))?,
                FpLit::HexQuad(s) => parse_hex_apfloat(ApFloatSemantics::IeeeQuad, s)
                    .map_err(|_| self.expected("valid hex quad literal"))?,
                FpLit::HexPpc128(s) => parse_hex_apfloat(ApFloatSemantics::PpcDoubleDouble, s)
                    .map_err(|_| self.expected("valid hex ppc128 literal"))?,
            },
            _ => return Err(self.expected("floating-point literal")),
        };
        self.bump()?;
        Ok(value)
    }
}

fn parse_hex_apfloat(semantics: ApFloatSemantics, digits: &str) -> IrResult<ApFloat> {
    let bits = ApInt::from_string(semantics.bit_width(), digits, 16)?;
    ApFloat::from_bits(semantics, &bits)
}

// ── Helper enums ────────────────────────────────────────────────────────────

// ── Function-body helper types ──────────────────────────────────────────────

/// Outgoing reference to an incoming phi value that could not be resolved
/// immediately (forward reference). Resolved by `PerFunctionState::finish`.
#[derive(Clone, Debug)]
enum PhiValRef<'ctx, B: ModuleBrand = Brand<'ctx>> {
    /// Already resolved to a concrete value.
    Resolved(llvmkit_ir::Value<'ctx, B>),
    /// Named local (`%name`) not yet defined.
    Named(String),
    /// Numbered local (`%N`) not yet defined.
    Numbered(u32),
    /// `undef` / `poison` constant — resolved at finish time using zero.
    Undef,
}

#[derive(Clone, Debug)]
enum BlockRef {
    Named(String),
    Numbered(u32),
}

/// One deferred phi incoming edge. Resolved after all blocks are parsed.
struct DeferredPhiEdge<'ctx, B: ModuleBrand = Brand<'ctx>> {
    /// The phi instruction's Value handle. Used by `finish()` with
    /// `phi_add_incoming_from_value` to add the incoming edge.
    phi_val: llvmkit_ir::Value<'ctx, B>,
    /// The incoming value reference (may be a forward ref).
    val_ref: PhiValRef<'ctx, B>,
    /// Incoming basic block reference.
    bb_ref: BlockRef,
    /// Source location for error reporting.
    loc: llvmkit_support::Span,
}

#[derive(Clone, Debug)]
enum DeferredLocalValueRef {
    Named(String),
    Numbered(u32),
}

struct DeferredAtomicRmwValue<'ctx, B: ModuleBrand = Brand<'ctx>> {
    inst: llvmkit_ir::AtomicRMWInst<'ctx, B>,
    val_ref: DeferredLocalValueRef,
    loc: Span,
}

/// Per-function symbol tables. Mirrors `LLParser::PerFunctionState`'s
/// named/numbered value tables and the basic-block lookup map.
struct PerFunctionState<'ctx, B: ModuleBrand = Brand<'ctx>> {
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
    /// Deferred phi incoming edges for forward references. Resolved by
    /// `finish()` after all blocks in the function have been parsed.
    deferred_phi: Vec<DeferredPhiEdge<'ctx, B>>,
    /// Deferred `atomicrmw` value operands for non-PHI forward references.
    deferred_atomicrmw_values: Vec<DeferredAtomicRmwValue<'ctx, B>>,
    /// Source span of each parsed phi, keyed by its result name, so the
    /// end-of-function coherence check in `finish()` can point a diagnostic
    /// at the offending phi instead of at `Module::verify()`.
    phi_locs: Vec<(llvmkit_ir::value::ValueId, Span)>,
}

impl<'ctx, B: ModuleBrand + 'ctx> PerFunctionState<'ctx, B> {
    fn new(func: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn, B>) -> Self {
        let mut blocks = std::collections::HashMap::new();
        for bb in func.basic_blocks() {
            let name = bb.name().unwrap_or_default();
            blocks.insert(name, bb.as_value());
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
            deferred_phi: Vec::new(),
            deferred_atomicrmw_values: Vec::new(),
            phi_locs: Vec::new(),
        }
    }

    fn invalid_numbered_slot(&self, id: u32, loc: Span) -> ParseError {
        ParseError::InvalidSlotId {
            source: crate::numbered_values::AddError::StaleId {
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
        module: &Module<'ctx, B, Unverified>,
        name: &str,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        if let Some(value) = self.blocks.get(name).copied() {
            return self.value_as_block(module, value, loc);
        }
        let bb = self.func.append_basic_block(module, name);
        self.blocks.insert(name.to_owned(), bb.as_value());
        Ok(bb)
    }

    fn ensure_block_label(
        &mut self,
        module: &Module<'ctx, B, Unverified>,
        name: &str,
        loc: Span,
    ) -> ParseResult<BasicBlockLabel<'ctx, llvmkit_ir::Dyn, B>> {
        if let Some(value) = self.blocks.get(name).copied() {
            return self.value_as_block_label(value, loc);
        }
        let bb = self.func.append_basic_block(module, name);
        self.blocks.insert(name.to_owned(), bb.as_value());
        Ok(bb.label())
    }

    /// Define a textual basic block label.
    fn define_named_block(
        &mut self,
        module: &Module<'ctx, B, Unverified>,
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
        module: &Module<'ctx, B, Unverified>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        let id = self.next_unnamed_value_id;
        self.define_numbered_block(module, id, loc)
    }

    fn define_numbered_label(
        &mut self,
        module: &Module<'ctx, B, Unverified>,
        id: u32,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        if self.defined_numbered_blocks.contains(&id) {
            return Err(ParseError::Redefinition {
                kind: crate::parse_error::SymbolKind::Block,
                id: crate::parse_error::SymbolId::Numbered(id),
                loc: DiagLoc::span(loc),
            });
        }
        if id != self.next_unnamed_value_id {
            return Err(self.invalid_numbered_slot(id, loc));
        }
        self.define_numbered_block(module, id, loc)
    }

    fn define_numbered_block(
        &mut self,
        module: &Module<'ctx, B, Unverified>,
        id: u32,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unterminated, B>>
    {
        if self.defined_numbered_blocks.contains(&id) {
            return Err(ParseError::Redefinition {
                kind: crate::parse_error::SymbolKind::Block,
                id: crate::parse_error::SymbolId::Numbered(id),
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
            self.numbered_blocks.insert(id, bb.as_value());
            bb
        };
        let bb_value = bb.as_value();
        self.func
            .move_basic_block_to_end(module, bb)
            .map_err(|e| ParseError::Expected {
                expected: format!("numbered basic block definition: {e}"),
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
        module: &Module<'ctx, B, Unverified>,
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

    fn value_as_block_view(
        &self,
        value: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Terminated, B>> {
        self.func
            .basic_blocks()
            .find(|bb| bb.as_value() == value)
            .ok_or_else(|| ParseError::Expected {
                expected: "referenced value is not a basic block".into(),
                loc: DiagLoc::span(loc),
            })
    }

    fn value_as_block_label(
        &self,
        value: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<BasicBlockLabel<'ctx, llvmkit_ir::Dyn, B>> {
        Ok(self.value_as_block_view(value, loc)?.label())
    }

    fn get_or_create_numbered_block_label(
        &mut self,
        module: &Module<'ctx, B, Unverified>,
        id: u32,
        loc: Span,
    ) -> ParseResult<BasicBlockLabel<'ctx, llvmkit_ir::Dyn, B>> {
        if let Some(value) = self.local_numbered.get(&id).copied() {
            return self.value_as_block_label(value, loc);
        }
        if id < self.next_unnamed_value_id {
            return Err(self.invalid_numbered_slot(id, loc));
        }
        let label = if let Some(value) = self.numbered_blocks.get(&id).copied() {
            self.value_as_block_label(value, loc)?
        } else {
            let bb = self.func.append_basic_block(module, "");
            self.numbered_blocks.insert(id, bb.as_value());
            bb.label()
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
        module: &Module<'ctx, B, Unverified>,
        block_ref: &BlockRef,
        loc: Span,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Terminated, B>> {
        let label = match block_ref {
            BlockRef::Named(name) => self.ensure_block_label(module, name, loc)?,
            BlockRef::Numbered(id) => self.get_or_create_numbered_block_label(module, *id, loc)?,
        };
        self.value_as_block_view(label.as_value(), loc)
    }

    fn bind_local(
        &mut self,
        lhs: &LocalLhs,
        v: llvmkit_ir::Value<'ctx, B>,
        loc: Span,
    ) -> ParseResult<()> {
        if v.ty().is_void() {
            return match lhs {
                LocalLhs::None => Ok(()),
                LocalLhs::Named(_) | LocalLhs::Numbered(_) => Err(ParseError::Expected {
                    expected: "non-void instruction result for local binding".into(),
                    loc: DiagLoc::span(loc),
                }),
            };
        }
        match lhs {
            LocalLhs::Named(n) => {
                if self.local_named.insert(n.clone(), v).is_some() {
                    return Err(ParseError::Redefinition {
                        kind: SYMBOL_KIND_LOCAL,
                        id: crate::parse_error::SymbolId::Named(n.clone()),
                        loc: DiagLoc::span(loc),
                    });
                }
            }
            LocalLhs::Numbered(id) => {
                if self.local_numbered.contains_key(id) || *id != self.next_unnamed_value_id {
                    return Err(self.invalid_numbered_slot(*id, loc));
                }
                self.local_numbered.insert(*id, v);
                self.next_unnamed_value_id = id.saturating_add(1);
            }
            LocalLhs::None => {
                let id = self.next_unnamed_value_id;
                if self.local_numbered.contains_key(&id) {
                    return Err(self.invalid_numbered_slot(id, loc));
                }
                self.local_numbered.insert(id, v);
                self.next_unnamed_value_id = id.saturating_add(1);
            }
        }
        Ok(())
    }

    /// Resolve all deferred phi incoming edges after the function body has
    /// been fully parsed. Called by `Parser::parse_define` before `}`.
    fn finish(
        mut self,
        module: &Module<'ctx, B, Unverified>,
    ) -> crate::parse_error::ParseResult<()> {
        for (name, loc) in &self.block_refs {
            if !self.defined_blocks.contains(name) {
                return Err(ParseError::UndefinedSymbol {
                    kind: crate::parse_error::SymbolKind::Block,
                    id: crate::parse_error::SymbolId::Named(name.clone()),
                    loc: DiagLoc::span(*loc),
                });
            }
        }
        for (id, loc) in &self.numbered_block_refs {
            if !self.defined_numbered_blocks.contains(id) {
                return Err(ParseError::UndefinedSymbol {
                    kind: crate::parse_error::SymbolKind::Block,
                    id: crate::parse_error::SymbolId::Numbered(*id),
                    loc: DiagLoc::span(*loc),
                });
            }
        }
        let atomicrmw_values = std::mem::take(&mut self.deferred_atomicrmw_values);
        for deferred in atomicrmw_values {
            let val = match deferred.val_ref {
                DeferredLocalValueRef::Named(ref n) => {
                    self.local_named.get(n).copied().ok_or_else(|| {
                        crate::parse_error::ParseError::UndefinedSymbol {
                            kind: SYMBOL_KIND_LOCAL,
                            id: crate::parse_error::SymbolId::Named(n.clone()),
                            loc: DiagLoc::span(deferred.loc),
                        }
                    })?
                }
                DeferredLocalValueRef::Numbered(id) => {
                    self.local_numbered.get(&id).copied().ok_or_else(|| {
                        crate::parse_error::ParseError::UndefinedSymbol {
                            kind: SYMBOL_KIND_LOCAL,
                            id: crate::parse_error::SymbolId::Numbered(id),
                            loc: DiagLoc::span(deferred.loc),
                        }
                    })?
                }
            };
            deferred.inst.set_value_operand(module, val).map_err(|e| {
                crate::parse_error::ParseError::Expected {
                    expected: format!("valid atomicrmw forward value: {e}"),
                    loc: DiagLoc::span(deferred.loc),
                }
            })?;
        }
        let edges = std::mem::take(&mut self.deferred_phi);
        for edge in edges {
            let val = match edge.val_ref {
                PhiValRef::Resolved(v) => v,
                PhiValRef::Named(ref n) => self.local_named.get(n).copied().ok_or_else(|| {
                    crate::parse_error::ParseError::UndefinedSymbol {
                        kind: SYMBOL_KIND_LOCAL,
                        id: crate::parse_error::SymbolId::Named(n.clone()),
                        loc: DiagLoc::span(edge.loc),
                    }
                })?,
                PhiValRef::Numbered(id) => {
                    self.local_numbered.get(&id).copied().ok_or_else(|| {
                        crate::parse_error::ParseError::UndefinedSymbol {
                            kind: SYMBOL_KIND_LOCAL,
                            id: crate::parse_error::SymbolId::Numbered(id),
                            loc: DiagLoc::span(edge.loc),
                        }
                    })?
                }
                PhiValRef::Undef => {
                    // Use a zero constant of the appropriate type.
                    let ty = edge.phi_val.ty();
                    match llvmkit_ir::AnyTypeEnum::from(ty) {
                        llvmkit_ir::AnyTypeEnum::Int(t) => t.const_zero().as_value(),
                        llvmkit_ir::AnyTypeEnum::Float(_t) => continue,
                        llvmkit_ir::AnyTypeEnum::Pointer(t) => t.const_null().as_value(),
                        _ => continue,
                    }
                }
            };
            let bb = self.resolve_block_ref(module, &edge.bb_ref, edge.loc)?;
            let tmp_b = llvmkit_ir::IRBuilder::new(module);
            tmp_b
                .phi_add_incoming_from_value(edge.phi_val, val, bb)
                .map_err(|e| crate::parse_error::ParseError::Expected {
                    expected: format!("valid phi add_incoming: {e}"),
                    loc: DiagLoc::span(edge.loc),
                })?;
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
                expected: e.message,
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
    UDiv,
    SDiv,
    URem,
    SRem,
    Shl,
    LShr,
    AShr,
    And,
    Or,
    Xor,
}

enum IntCast {
    Trunc,
    ZExt,
    SExt,
}

enum FpBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

enum FpToInt {
    FpToSI,
    FpToUI,
}

enum IntToFp {
    SIToFp,
    UIToFp,
}

/// Alias for the dyn-positioned, dyn-return IRBuilder we drive while
/// emitting one block's instructions. The terminator-emitting calls
/// (`build_ret` / `build_br` / etc.) take this by value, so the parser
/// stores it inside an `Option<Self>` for the duration of the block.
type ParsedBlockBuilder<'m, 'ctx, B> = IRBuilder<'m, 'ctx, B, NoFolder, Positioned, Dyn>;

fn live_builder_error(loc: Span) -> ParseError {
    ParseError::Expected {
        expected: "live insertion builder before terminator".into(),
        loc: DiagLoc::span(loc),
    }
}

fn take_live_builder<'m, 'ctx, B: ModuleBrand + 'ctx>(
    builder: &mut Option<ParsedBlockBuilder<'m, 'ctx, B>>,
    loc: Span,
) -> ParseResult<ParsedBlockBuilder<'m, 'ctx, B>> {
    builder.take().ok_or_else(|| live_builder_error(loc))
}

fn borrow_live_builder<'b, 'm, 'ctx, B: ModuleBrand + 'ctx>(
    builder: &'b Option<ParsedBlockBuilder<'m, 'ctx, B>>,
    loc: Span,
) -> ParseResult<&'b ParsedBlockBuilder<'m, 'ctx, B>> {
    builder.as_ref().ok_or_else(|| live_builder_error(loc))
}

/// Local symbol kind label used in [`crate::parse_error::ParseError::UndefinedSymbol`].
const SYMBOL_KIND_LOCAL: crate::parse_error::SymbolKind = crate::parse_error::SymbolKind::Local;

#[derive(Clone, Debug)]
enum NameOrId {
    Name(String),
    Id(u32),
}

#[derive(Clone, Copy, Debug)]
enum PunctKind {
    Equal,
    Comma,
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
trait IntoTypeEnum<'ctx, B: ModuleBrand = Brand<'ctx>> {
    fn into_type_enum(self) -> AnyTypeEnum<'ctx, B>;
}

impl<'ctx, B: ModuleBrand + 'ctx> IntoTypeEnum<'ctx, B> for Type<'ctx, B> {
    fn into_type_enum(self) -> AnyTypeEnum<'ctx, B> {
        AnyTypeEnum::from(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llvmkit_ir::Module;

    fn parse(src: &str) -> ParseResult<()> {
        Module::with_new::<_, _, _>("parse_test", |m| {
            let p = Parser::new(src.as_bytes(), &m)?;
            let _ = p.parse_module()?;
            Ok(())
        })
    }

    /// Mirrors `test/Assembler/datalayout.ll` — the parser accepts the
    /// `target datalayout = "..."` directive and the module retains it.
    #[test]
    fn parses_target_datalayout() {
        let src = "target datalayout = \"e-m:e-i64:64\"\n";
        Module::with_new::<_, _, _>("dl", |m| {
            Parser::new(src.as_bytes(), &m)
                .unwrap()
                .parse_module()
                .unwrap();
            let dl = m.data_layout();
            assert!(dl.is_little_endian());
        });
    }

    /// Mirrors `test/Assembler/target-triple.ll` — `target triple = "..."`.
    #[test]
    fn parses_target_triple() {
        let src = "target triple = \"x86_64-pc-linux-gnu\"\n";
        Module::with_new::<_, _, _>("triple", |m| {
            Parser::new(src.as_bytes(), &m)
                .unwrap()
                .parse_module()
                .unwrap();
            assert_eq!(m.target_triple().as_deref(), Some("x86_64-pc-linux-gnu"));
        });
    }

    /// Mirrors the `module asm` arm of `test/Assembler/module-asm.ll`.
    #[test]
    fn parses_module_asm() {
        let src = "module asm \"hello\"\nmodule asm \"world\"\n";
        Module::with_new::<_, _, _>("masm", |m| {
            Parser::new(src.as_bytes(), &m)
                .unwrap()
                .parse_module()
                .unwrap();
            let asm = m.module_asm();
            assert!(asm.contains("hello"));
            assert!(asm.contains("world"));
        });
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
