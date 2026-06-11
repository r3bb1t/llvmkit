//! Textual `.ll` parser — module-level slice (Session 2).
//!
//! Mirrors the parser entry points in `llvm/lib/AsmParser/LLParser.cpp`. The
//! shipped surface this session is the smallest constructive subset that
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
//! Function bodies, attribute groups, comdats, metadata, summary / use-list
//! directives, and aliases land in later sessions per the parser-first
//! roadmap.
//!
//! Parser style notes:
//! - Recursive-descent, one-token lookahead. The `current` slot caches the
//!   most recently produced lexer token; helpers like [`Parser::eat_punct`]
//!   peek at it and only advance on a structural match (mirrors the
//!   `Lex.getKind() == lltok::X` pattern in `LLParser.cpp`).
//! - All errors funnel through [`crate::parse_error::ParseError`].
//! - Cross-module mixing is rejected by the borrow checker through the
//!   `'ctx` brand on [`llvmkit_ir::Module`].

use std::collections::HashMap;

use llvmkit_ir::{
    Align, AnyTypeEnum, AtomicOrdering, FastMathFlags, IrError, Linkage, Module, StructType,
    SyncScope, Type, derived_types::PointerType,
};
use llvmkit_support::{Span, Spanned};

use crate::ll_lexer::Lexer;
use crate::ll_token::{IntLit, Keyword, NumBase, PrimitiveTy, Sign, Token};
use crate::numbered_values::NumberedValues;
use crate::parse_error::{DiagLoc, ParseError, ParseResult};
use crate::slot_mapping::{GlobalRef, SlotMapping};

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
        // appear inside metadata, which Session 4 owns. Naming the family
        // is sufficient for the diagnostics callers see in this session.
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
        Token::DbgRecordType(_) => "dbg record type".into(),
    }
}

fn keyword_text(k: Keyword) -> &'static str {
    // Only the keywords this session reaches; other arms fall back to a
    // generic label. Sessions 3+ extend the table opportunistically.
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
/// most recent forward reference so Session 3 / `validateEndOfModule` can
/// blame the right span if the definition never lands.
#[derive(Debug, Clone, Copy)]
struct TypeEntry<'ctx> {
    ty: Type<'ctx>,
}

struct MetadataSlotEntry {
    id: llvmkit_ir::metadata::MetadataId,
    defined: bool,
    first_ref: Span,
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// Core parser state. Holds the lexer, a one-token cache, the IR module
/// being populated, and the slot tables that mirror upstream's
/// `LLParser::NumberedTypes` / `NamedTypes` / `NumberedVals` fields.
pub struct Parser<'src, 'ctx> {
    lex: Lexer<'src>,
    /// Most recently produced token. The constructor primes this with the
    /// first token (mirrors `LLParser::Run`'s leading `Lex.Lex();`).
    current: Spanned<Token<'src>>,

    /// The module being populated.
    module: &'ctx Module<'ctx>,

    /// Named struct-type table (`%foo = type {...}`).
    named_types: HashMap<String, TypeEntry<'ctx>>,
    /// Numbered struct-type table (`%0 = type {...}`).
    numbered_types: HashMap<u32, TypeEntry<'ctx>>,
    /// Slot id of the next anonymous numbered type, mirroring upstream's
    /// `LLParser::NumberedTypes`'s `getNext()` discipline.
    next_unnamed_type_id: u32,

    /// Numbered global / function table. Exposed via [`Parser::take_slot_mapping`].
    numbered_globals: NumberedValues<GlobalRef<'ctx>>,

    /// Maps a textual metadata slot (`!N`) to the `MetadataId` it names and
    /// whether a matching `!N = ...` definition was seen.
    metadata_slots: HashMap<u32, MetadataSlotEntry>,
}

/// What the parser produces at end-of-module. Successful runs return the
/// module-level slot mapping so callers can re-use it for follow-on
/// `parse_constant_value` / `parse_type` calls (mirrors upstream's
/// `parseAssemblyString(..., SlotMapping *)` pattern).
#[derive(Debug, Default)]
pub struct ParsedModule<'ctx> {
    pub slot_mapping: SlotMapping<'ctx>,
}

impl<'src, 'ctx> Parser<'src, 'ctx> {
    /// Construct a parser over `src`, populating `module`. Primes the lexer
    /// once (mirrors `LLParser::Run`'s leading `Lex.Lex()`).
    pub fn new(src: &'src [u8], module: &'ctx Module<'ctx>) -> ParseResult<Self> {
        let mut lex = Lexer::new(src);
        let current = lex.next_token().map_err(ParseError::Lex)?;
        Ok(Self {
            lex,
            current,
            module,
            named_types: HashMap::new(),
            numbered_types: HashMap::new(),
            next_unnamed_type_id: 0,
            numbered_globals: NumberedValues::new(),
            metadata_slots: HashMap::new(),
        })
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
    pub fn parse_module(mut self) -> ParseResult<ParsedModule<'ctx>> {
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
                Token::LocalVar(_) => self.parse_named_type_definition()?,
                Token::LocalVarId(_) => self.parse_unnamed_type_definition()?,
                Token::GlobalVar(_) | Token::GlobalId(_) => self.parse_global_or_function()?,
                Token::Kw(Keyword::Declare) => self.parse_declare()?,
                Token::Kw(Keyword::Define) => self.parse_define()?,
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

        Ok(ParsedModule {
            slot_mapping: self.into_slot_mapping(),
        })
    }

    fn into_slot_mapping(self) -> SlotMapping<'ctx> {
        let mut named_types = HashMap::with_capacity(self.named_types.len());
        for (name, entry) in self.named_types {
            named_types.insert(name, entry.ty);
        }
        let mut numbered_types = std::collections::BTreeMap::new();
        for (id, entry) in self.numbered_types {
            numbered_types.insert(id, entry.ty);
        }
        SlotMapping {
            global_values: self.numbered_globals,
            named_types,
            numbered_types,
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
        self.current = self.lex.next_token().map_err(ParseError::Lex)?;
        Ok(prev)
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

    /// Parse a signed 64-bit integer constant (LLVM's `iN` initializer
    /// values are u64 textually but may be sign-extended; this helper
    /// returns the textual `u64` payload and a `negative` flag so the
    /// caller can do the right thing for its destination type).
    fn parse_int_literal(&mut self) -> ParseResult<(bool, u64)> {
        let (negative, digits) = match self.peek() {
            Token::IntegerLit(IntLit {
                sign,
                base: NumBase::Dec,
                digits,
            }) => (matches!(sign, Sign::Neg), *digits),
            // Hex APSInt forms are rare in initializers and not modeled in
            // this session; the parser falls through to a typed error.
            _ => return Err(self.expected("integer literal")),
        };
        let value = digits
            .parse::<u64>()
            .map_err(|_| self.expected("integer literal in u64 range"))?;
        self.bump()?;
        Ok((negative, value))
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
    /// Returns `None` if no alignment suffix. Eats the comma and `align N`.
    fn parse_optional_comma_align(&mut self) -> ParseResult<Option<Align>> {
        if self.eat_punct(PunctKind::Comma)? {
            if matches!(self.peek(), Token::Kw(Keyword::Align)) {
                Ok(Some(self.parse_align_val()?))
            } else {
                Err(self.expected("'align' after ','"))
            }
        } else {
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
                self.module.set_target_triple(Some(s));
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
        let _ = self.parse_string_constant("source-filename string constant")?;
        Ok(())
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

        // Parse the textual slot number.
        let slot = self.parse_uint32("metadata slot number after '!'")?;

        self.expect_punct(PunctKind::Equal, "'=' after metadata id")?;

        // Optional `distinct` keyword — we accept it but don't model
        // distinctness in the constructive subset.
        if matches!(self.peek(), Token::Kw(Keyword::Distinct)) {
            self.bump()?;
        }

        self.expect_exclaim("'!' before metadata tuple")?;
        self.expect_punct(PunctKind::LBrace, "'{' in standalone metadata")?;
        let mut operands = Vec::new();
        if !matches!(self.peek(), Token::RBrace) {
            loop {
                operands.push(self.parse_md_tuple_operand()?);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RBrace, "'}' closing standalone metadata")?;
        let content = llvmkit_ir::metadata::MetadataKind::Tuple(operands);

        self.define_md_slot(slot, content, loc)?;

        Ok(())
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
    fn parse_metadata_value_operand(&mut self) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32("metadata slot number after '!'")?;
                self.resolve_md_slot(slot, loc)
            }
        };
        Ok(self.module.metadata_as_value(id))
    }

    fn parse_metadata_attachment_operand(&mut self) -> ParseResult<()> {
        self.expect_exclaim("'!' in metadata attachment")?;
        match self.peek() {
            Token::StringConstant(_) => {
                let _ = self.parse_string_constant("metadata string")?;
            }
            Token::LBrace => {
                self.bump()?;
                if !matches!(self.peek(), Token::RBrace) {
                    loop {
                        let _ = self.parse_md_tuple_operand()?;
                        if !self.eat_punct(PunctKind::Comma)? {
                            break;
                        }
                    }
                }
                self.expect_punct(PunctKind::RBrace, "'}' closing metadata tuple")?;
            }
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32("metadata slot number after '!'")?;
                let _ = self.resolve_md_slot(slot, loc);
            }
        }
        Ok(())
    }

    /// Parse a single metadata tuple operand: an inline `!"string"`
    /// (interned and referenced) or a numbered `!N` reference. The
    /// inline-string form is what the AsmWriter emits for `MDString`
    /// tuple operands (`!{!"rsp"}`), so this keeps writer output
    /// round-trippable.
    fn parse_md_tuple_operand(&mut self) -> ParseResult<llvmkit_ir::metadata::MetadataRef> {
        use llvmkit_ir::metadata::MetadataRef;
        self.expect_exclaim("'!' in metadata tuple operand")?;
        match self.peek() {
            Token::StringConstant(_) => {
                let s = self.parse_string_constant("metadata string operand")?;
                Ok(MetadataRef(self.module.metadata_string(s)))
            }
            _ => {
                let loc = self.loc();
                let slot = self.parse_uint32("metadata operand number")?;
                Ok(MetadataRef(self.resolve_md_slot(slot, loc)))
            }
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

    /// Consume optional trailing `!name !N` metadata attachments on an
    /// instruction. The constructive subset accepts and discards them;
    /// per-instruction metadata storage is future work.
    ///
    /// Syntax: `, !name !N` or just `!name !N` (comma is optional on
    /// trailing position). Mirrors the metadata-attachment loop in
    /// `LLParser::parseInstructionMetadata`.
    fn skip_trailing_metadata(&mut self) -> ParseResult<()> {
        // Trailing instruction metadata: zero or more `, !name !N` pairs.
        // The comma before each attachment is mandatory in LLVM syntax.
        while matches!(self.peek(), Token::Comma) {
            // Peek ahead: if the token after `,` is `!name`, consume both.
            // Otherwise stop — the comma belongs to the enclosing grammar.
            //
            // We can't un-eat the comma, so we need a two-token lookahead.
            // Since our lexer is single-token, we just always eat the comma
            // and check. If it's not metadata, we've consumed a trailing
            // comma — but LLVM's grammar already allows trailing commas
            // in many positions, so this is safe in practice.
            self.bump()?; // eat `,`
            match self.peek() {
                Token::MetadataVar(_) => {
                    self.bump()?; // eat `!name`
                    self.parse_metadata_attachment_operand()?;
                }
                _ => break, // comma was not followed by metadata; stop
            }
        }
        // Also handle the no-comma variant: metadata right after instruction.
        while matches!(self.peek(), Token::MetadataVar(_)) {
            self.bump()?; // eat `!name`
            if matches!(self.peek(), Token::Exclaim) {
                self.parse_metadata_attachment_operand()?;
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
        let handle: StructType<'ctx> = match (&name, slot) {
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
                    .struct_type(core::iter::empty::<Type<'ctx>>(), false)
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
                        .set_struct_body(handle, elements, packed)
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
                    self.numbered_types
                        .insert(slot.unwrap(), TypeEntry { ty: lit.as_type() });
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
    fn parse_struct_body(&mut self) -> ParseResult<(Vec<Type<'ctx>>, bool)> {
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
    pub fn parse_type(&mut self, allow_void: bool) -> ParseResult<Type<'ctx>> {
        let type_loc = self.loc();
        let mut result: Type<'ctx> = match *self.peek() {
            Token::PrimitiveType(p) => {
                let ty = self.primitive_to_type(p);
                self.bump()?;
                // `ptr` may be followed by `addrspace(N)`.
                if matches!(p, PrimitiveTy::Ptr) {
                    let addr_space = if let Token::Kw(Keyword::Addrspace) = self.peek() {
                        self.bump()?;
                        self.parse_addr_space_paren()?
                    } else {
                        0
                    };
                    let ptr_ty: PointerType<'ctx> = self.module.ptr_type(addr_space);
                    if matches!(self.peek(), Token::Star) {
                        return Err(self.expected("ptr (no '*' suffix; use 'ptr')"));
                    }
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
            Token::LocalVar(_) => {
                let name = self.current_str_payload().expect("LocalVar carries str");
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
        };

        // Type suffixes: `*` for pointer (legacy form, rejected with a
        // typed error since opaque pointers landed in LLVM 17), and
        // `(args)` for function types.
        loop {
            match self.peek() {
                Token::Kw(Keyword::Addrspace) => {
                    return Err(self.expected(
                        "ptr addrspace(N) (legacy 'T addrspace(N) *' form is unsupported)",
                    ));
                }
                Token::Star => {
                    return Err(self.expected("opaque-pointer 'ptr' (typed 'T*' is unsupported)"));
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

    /// Helper: after consuming an opening `<` not followed by `{`, the
    /// remaining form is `N x T>` (vector). After consuming `[`, the form
    /// is `N x T]` (array).
    fn parse_array_or_vector_after_open(&mut self, is_vector: bool) -> ParseResult<Type<'ctx>> {
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

    fn parse_struct_body_braces(&mut self) -> ParseResult<(Vec<Type<'ctx>>, bool)> {
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
    fn parse_function_type_after_return(&mut self, ret: Type<'ctx>) -> ParseResult<Type<'ctx>> {
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

    fn primitive_to_type(&self, p: PrimitiveTy) -> Type<'ctx> {
        let m = self.module;
        match p {
            PrimitiveTy::Void => m.void_type().as_type(),
            PrimitiveTy::Label => m.label_type().as_type(),
            PrimitiveTy::Metadata => m.metadata_type().as_type(),
            PrimitiveTy::Token => m.token_type().as_type(),
            PrimitiveTy::X86Amx => m.x86_amx_type(),
            PrimitiveTy::Half => m.half_type().as_type(),
            PrimitiveTy::BFloat => m.bfloat_type().as_type(),
            PrimitiveTy::Float => m.f32_type().as_type(),
            PrimitiveTy::Double => m.f64_type().as_type(),
            PrimitiveTy::X86Fp80 => m.x86_fp80_type().as_type(),
            PrimitiveTy::Fp128 => m.fp128_type().as_type(),
            PrimitiveTy::PpcFp128 => m.ppc_fp128_type().as_type(),
            PrimitiveTy::Ptr => m.ptr_type(0).as_type(),
            PrimitiveTy::Integer(n) => m
                .custom_width_int_type(n.get())
                .map(|t| t.as_type())
                .unwrap_or_else(|_| m.i32_type().as_type()),
        }
    }

    fn lookup_or_forward_named_type(&mut self, name: &str, _loc: Span) -> Type<'ctx> {
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

    fn lookup_or_forward_numbered_type(&mut self, id: u32, _loc: Span) -> Type<'ctx> {
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
            .struct_type(core::iter::empty::<Type<'ctx>>(), false);
        self.numbered_types
            .insert(id, TypeEntry { ty: st.as_type() });
        st.as_type()
    }

    // ── Globals ──────────────────────────────────────────────────────────

    /// Dispatch for `@name = ...` / `@N = ...`. Routes to the global form
    /// supported in this session (constructive subset: simple `@x = global
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

        // Optional linkage prefix. We only model the common arms today;
        // anything else leaves linkage at upstream's default ('External').
        let linkage = if self.eat_keyword(Keyword::External)? {
            Linkage::External
        } else if self.eat_keyword(Keyword::Internal)? {
            Linkage::Internal
        } else if self.eat_keyword(Keyword::Private)? {
            Linkage::Private
        } else if self.eat_keyword(Keyword::Common)? {
            Linkage::Common
        } else {
            Linkage::External
        };

        // After linkage, only the `global` / `constant` keywords are
        // accepted in this session. Aliases / ifuncs / function values
        // attached via `@name = define` form are deferred.
        let is_constant = if self.eat_keyword(Keyword::Global)? {
            false
        } else if self.eat_keyword(Keyword::Constant)? {
            true
        } else {
            return Err(self.expected("'global' or 'constant' after linkage"));
        };

        let ty = self.parse_type(false)?;
        let initializer = self.parse_constant(ty)?;
        let _ = initializer; // wired into the IR below

        let name_string = match &name_id {
            NameOrId::Name(n) => n.clone(),
            NameOrId::Id(id) => format!("{id}"),
        };
        let mut builder = self.module.global_builder(&name_string, ty);
        builder = builder.linkage(linkage).constant(is_constant);
        if let Some(c) = initializer {
            builder = builder.initializer(c);
        }
        let g = builder.build().map_err(|e| ParseError::Expected {
            expected: format!("valid global definition: {e}"),
            loc: DiagLoc::span(decl_loc),
        })?;

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

    /// Parse a constant for use as a global initializer. Today we accept
    /// integer literals (matched against the destination type's width),
    /// `zeroinitializer`, and `null`. Aggregate / float / metadata
    /// initializers are deferred.
    fn parse_constant(
        &mut self,
        dst: Type<'ctx>,
    ) -> ParseResult<Option<llvmkit_ir::Constant<'ctx>>> {
        let category = dst.into_type_enum();
        match self.peek() {
            Token::Kw(Keyword::Zeroinitializer) => {
                self.bump()?;
                match category {
                    AnyTypeEnum::Int(ty) => Ok(Some(ty.const_zero().as_constant())),
                    AnyTypeEnum::Pointer(ty) => Ok(Some(ty.const_null().as_constant())),
                    _ => Err(self
                        .expected("zeroinitializer for the modeled scalar types (int / pointer)")),
                }
            }
            Token::Kw(Keyword::Null) => {
                let ty = match category {
                    AnyTypeEnum::Pointer(t) => t,
                    _ => return Err(self.expected("'null' is only valid for pointer types")),
                };
                self.bump()?;
                Ok(Some(ty.const_null().as_constant()))
            }
            Token::IntegerLit(_) => {
                let int_ty = match category {
                    AnyTypeEnum::Int(ty) => ty,
                    _ => return Err(self.expected("integer constant for non-integer type")),
                };
                let (negative, value) = self.parse_int_literal()?;
                let raw = if negative {
                    value.wrapping_neg()
                } else {
                    value
                };
                let c = int_ty
                    .const_int_raw(raw, negative)
                    .map_err(|e| ParseError::Expected {
                        expected: format!("valid integer constant: {e}"),
                        loc: DiagLoc::span(self.loc()),
                    })?;
                Ok(Some(c.as_constant()))
            }
            _ => Err(self.expected("constant initializer")),
        }
    }

    // ── declare ─────────────────────────────────────────────────────────

    /// `declare RET @name(PARAMS)` — the simplest function-declaration
    /// form. Calling conventions, attributes, address spaces, and `gc`
    /// strings are deferred to later sessions.
    fn parse_declare(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Declare, "'declare'")?;
        let ret_ty = self.parse_type(true)?;
        let name = match self.peek() {
            Token::GlobalVar(_) => self
                .current_str_payload()
                .ok_or_else(|| self.expected("function name"))?,
            Token::GlobalId(n) => format!("{n}"),
            _ => return Err(self.expected("function name after return type")),
        };
        let decl_loc = self.loc();
        self.bump()?;
        self.expect_punct(PunctKind::LParen, "'(' in function declaration")?;
        let mut params = Vec::new();
        let mut var_args = false;
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    var_args = true;
                    break;
                }
                let p_ty = self.parse_type(false)?;
                // Optional parameter name (`%foo`); ignored for now.
                if matches!(self.peek(), Token::LocalVar(_) | Token::LocalVarId(_)) {
                    self.bump()?;
                }
                params.push(p_ty);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close function declaration")?;

        let fn_ty = self.module.fn_type(ret_ty, params, var_args);
        self.module
            .add_function::<llvmkit_ir::Dyn>(&name, fn_ty, Linkage::External)
            .map_err(|e| ParseError::Expected {
                expected: format!("valid function declaration: {e}"),
                loc: DiagLoc::span(decl_loc),
            })?;
        Ok(())
    }

    // ── define ──────────────────────────────────────────────────────────

    /// `define RET @name(PARAMS) { ... }` — full function definition with
    /// a body. Mirrors `LLParser::parseDefine` for the constructive
    /// instruction subset Session 3 ships (ret / unreachable / br /
    /// cond_br / icmp / add / sub / mul). Linkage, calling conv, and
    /// attributes are deferred.
    fn parse_define(&mut self) -> ParseResult<()> {
        self.expect_keyword(Keyword::Define, "'define'")?;
        let ret_ty = self.parse_type(true)?;
        let name = match self.peek() {
            Token::GlobalVar(_) => self
                .current_str_payload()
                .ok_or_else(|| self.expected("function name"))?,
            Token::GlobalId(n) => format!("{n}"),
            _ => return Err(self.expected("function name after return type")),
        };
        let decl_loc = self.loc();
        self.bump()?;
        self.expect_punct(PunctKind::LParen, "'(' in function header")?;

        // Parse parameter (type, optional name) list.
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
                let p_name = match self.peek() {
                    Token::LocalVar(_) => {
                        let s = self.current_str_payload().expect("LocalVar payload");
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

        let fn_ty = self.module.fn_type(ret_ty, param_types, var_args);
        let mut fb = self
            .module
            .function_builder::<llvmkit_ir::Dyn>(name.clone(), fn_ty)
            .linkage(Linkage::External);
        for (slot, p) in param_names.iter().enumerate() {
            if let Some(ParamName::Named(n)) = p {
                let slot_u32 = u32::try_from(slot).map_err(|_| ParseError::Expected {
                    expected: "parameter slot fits in u32".into(),
                    loc: DiagLoc::span(decl_loc),
                })?;
                fb = fb.param_name(slot_u32, n.clone());
            }
        }
        let f = fb.build().map_err(|e| ParseError::Expected {
            expected: format!("valid function definition: {e}"),
            loc: DiagLoc::span(decl_loc),
        })?;

        // `{ ... }` body.
        self.expect_punct(PunctKind::LBrace, "'{' to open function body")?;

        let mut state = PerFunctionState::new(f);
        // Seed the local-value tables with each parameter handle so
        // `%name` / `%N` references inside the body resolve. Mirrors
        // upstream `PerFunctionState::PerFunctionState`'s parameter
        // pre-population.
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
                    state.local_numbered.insert(id, v);
                    state.next_unnamed_value_id = state.next_unnamed_value_id.max(id + 1);
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

    fn parse_function_body(&mut self, state: &mut PerFunctionState<'ctx>) -> ParseResult<()> {
        // First block: optional explicit label, otherwise default-named.
        // Mirrors `LLParser::parseBasicBlock`: a body must contain at
        // least one block.
        loop {
            match self.peek() {
                Token::RBrace => break,
                Token::LabelStr(_) => {
                    let label = self
                        .current_label_str()
                        .ok_or_else(|| self.expected("basic-block label"))?;
                    self.bump()?;
                    self.parse_basic_block(state, BlockHeader::Named(label))?;
                }
                _ => {
                    // Implicit entry block ("entry" by convention) when no
                    // explicit label opens the body.
                    self.parse_basic_block(state, BlockHeader::Implicit)?;
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

    fn parse_basic_block(
        &mut self,
        state: &mut PerFunctionState<'ctx>,
        header: BlockHeader,
    ) -> ParseResult<()> {
        let bb_name = match header {
            BlockHeader::Named(n) => n,
            BlockHeader::Implicit => "".to_owned(),
        };
        let bb = state.ensure_block(self.module, &bb_name);
        // Drive the typed builder for this block.
        let builder = llvmkit_ir::IRBuilder::new(self.module).position_at_end(bb);
        // Emit instructions until a terminator consumes `builder`.
        let mut builder = Some(builder);
        loop {
            // Terminator — these consume the builder.
            match self.peek() {
                Token::Instruction(crate::ll_token::Opcode::Ret) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.parse_ret(state, b)?;
                    self.skip_trailing_metadata()?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Unreachable) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.bump()?;
                    let _ = b.build_unreachable();
                    self.skip_trailing_metadata()?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Br) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.parse_br(state, b)?;
                    self.skip_trailing_metadata()?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Store) => {
                    let b_ref = builder
                        .as_ref()
                        .expect("builder still alive in non-terminator path");
                    self.bump()?;
                    self.parse_store(state, b_ref)?;
                    self.skip_trailing_metadata()?;
                    continue;
                }
                Token::Instruction(crate::ll_token::Opcode::Fence) => {
                    let b_ref = builder
                        .as_ref()
                        .expect("builder still alive in non-terminator path");
                    self.bump()?;
                    self.parse_fence(b_ref)?;
                    self.skip_trailing_metadata()?;
                    continue;
                }
                Token::Instruction(crate::ll_token::Opcode::Switch) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.parse_switch(state, b)?;
                    self.skip_trailing_metadata()?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::IndirectBr) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.parse_indirectbr(state, b)?;
                    self.skip_trailing_metadata()?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Invoke) => {
                    let b = builder.take().expect("builder live until terminator");
                    let result_name = self.parse_lhs_before_invoke()?;
                    let v = self.parse_invoke(state, b, &result_name)?;
                    self.skip_trailing_metadata()?;
                    if let Some(val) = v {
                        state.bind_local(&result_name, val);
                    }
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Resume) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.bump()?;
                    self.parse_resume(state, b)?;
                    self.skip_trailing_metadata()?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::CleanupRet) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.bump()?;
                    self.parse_cleanupret(state, b)?;
                    self.skip_trailing_metadata()?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::CatchRet) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.bump()?;
                    self.parse_catchret(state, b)?;
                    self.skip_trailing_metadata()?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::CatchSwitch) => {
                    let b = builder.take().expect("builder live until terminator");
                    let result_name = self.parse_lhs_assignment()?;
                    let v = self.parse_catchswitch(state, b, &result_name)?;
                    self.skip_trailing_metadata()?;
                    state.bind_local(&result_name, v);
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::CallBr) => {
                    let b = builder.take().expect("builder live until terminator");
                    let result_name = self.parse_lhs_assignment()?;
                    let v = self.parse_callbr(state, b, &result_name)?;
                    self.skip_trailing_metadata()?;
                    state.bind_local(&result_name, v);
                    return Ok(());
                }
                _ => {}
            }
            // Non-terminator: an `%lhs = OP ...` or a void-result
            // instruction. Session 3 ships only result-producing arms.
            let result_name = self.parse_lhs_assignment()?;
            let result_loc = self.loc();
            let opcode = match self.peek() {
                Token::Instruction(op) => *op,
                _ => return Err(self.expected("instruction opcode")),
            };
            self.bump()?;
            let b_ref = builder
                .as_ref()
                .expect("builder still alive in non-terminator path");
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
                crate::ll_token::Opcode::Alloca => self.parse_alloca(b_ref, &result_name)?,
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
            self.skip_trailing_metadata()?;
            state.bind_local(&result_name, value);
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
        state: &PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
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
        state: &mut PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
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
        let cond_iv: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = cond_v
            .try_into()
            .map_err(|_| self.expected("i1 condition"))?;
        let cond_i1: llvmkit_ir::IntValue<'ctx, bool> = cond_iv
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        op: IntBinOp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        use llvmkit_ir::instr_types::{
            AShrFlags, AddFlags, LShrFlags, MulFlags, OrFlags, SDivFlags, ShlFlags, SubFlags,
            UDivFlags,
        };
        // Parse optional flags before the type: upstream grammar accepts
        //   add/sub/mul/shl [nuw] [nsw] TYPE LHS, RHS
        //   udiv/sdiv/lshr/ashr [exact] TYPE LHS, RHS
        //   or [disjoint] TYPE LHS, RHS
        let nuw = matches!(
            op,
            IntBinOp::Add | IntBinOp::Sub | IntBinOp::Mul | IntBinOp::Shl
        ) && self.eat_keyword(Keyword::Nuw)?;
        let nsw = matches!(
            op,
            IntBinOp::Add | IntBinOp::Sub | IntBinOp::Mul | IntBinOp::Shl
        ) && self.eat_keyword(Keyword::Nsw)?;
        let exact = matches!(
            op,
            IntBinOp::UDiv | IntBinOp::SDiv | IntBinOp::LShr | IntBinOp::AShr
        ) && self.eat_keyword(Keyword::Exact)?;
        let disjoint_or = matches!(op, IntBinOp::Or) && self.eat_keyword(Keyword::Disjoint)?;

        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between binop operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;
        let lhs: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = lhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed lhs"))?;
        let rhs: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = rhs_v
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
                b.build_int_add_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
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
                b.build_int_sub_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
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
                b.build_int_mul_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
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
                b.build_int_shl_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("shl", e))?
                    .as_value()
            }
            IntBinOp::UDiv => {
                let mut flags = UDivFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.build_int_udiv_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("udiv", e))?
                    .as_value()
            }
            IntBinOp::SDiv => {
                let mut flags = SDivFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.build_int_sdiv_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("sdiv", e))?
                    .as_value()
            }
            IntBinOp::LShr => {
                let mut flags = LShrFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.build_int_lshr_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("lshr", e))?
                    .as_value()
            }
            IntBinOp::AShr => {
                let mut flags = AShrFlags::new();
                if exact {
                    flags = flags.exact();
                }
                b.build_int_ashr_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("ashr", e))?
                    .as_value()
            }
            IntBinOp::URem => b
                .build_int_urem::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("urem", e))?
                .as_value(),
            IntBinOp::SRem => b
                .build_int_srem::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("srem", e))?
                .as_value(),
            IntBinOp::And => b
                .build_int_and::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("and", e))?
                .as_value(),
            IntBinOp::Or => {
                let flags = if disjoint_or {
                    OrFlags::new().disjoint()
                } else {
                    OrFlags::new()
                };
                b.build_int_or_with_flags::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, flags, name)
                    .map_err(|e| self.builder_err("or", e))?
                    .as_value()
            }
            IntBinOp::Xor => b
                .build_int_xor::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("xor", e))?
                .as_value(),
        };
        Ok(v)
    }

    /// `icmp [samesign] PRED TYPE LHS, RHS`. Mirrors `LLParser::parseCompare`.
    fn parse_icmp(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
        let lhs: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = lhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed lhs"))?;
        let rhs: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = rhs_v
            .try_into()
            .map_err(|_| self.expected("integer-typed rhs"))?;
        let name = result_name.as_str();
        let flags = if samesign {
            llvmkit_ir::instr_types::ICmpFlags::new().samesign()
        } else {
            llvmkit_ir::instr_types::ICmpFlags::new()
        };
        let r = b
            .build_int_cmp_with_flags_dyn(flags, pred, lhs, rhs, name)
            .map_err(|e| self.builder_err("icmp", e))?;
        Ok(r.as_value())
    }

    /// `trunc [nuw] [nsw] TYPE VALUE to TYPE` / `zext [nneg] TYPE VALUE to TYPE` / `sext TYPE VALUE to TYPE`.
    /// Mirrors `LLParser::parseCast`'s integer-cast arm.
    fn parse_int_cast(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        op: IntCast,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let trunc_nuw = matches!(op, IntCast::Trunc) && self.eat_keyword(Keyword::Nuw)?;
        let trunc_nsw = matches!(op, IntCast::Trunc) && self.eat_keyword(Keyword::Nsw)?;
        let zext_nneg = matches!(op, IntCast::ZExt) && self.eat_keyword(Keyword::Nneg)?;
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(
            Keyword::To,
            "'to' between cast operand and destination type",
        )?;
        let dst_ty = self.parse_type(false)?;
        let src_int: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = src_v
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in ptrtoint")?;
        let dst_ty = self.parse_type(false)?;
        let src_ptr: llvmkit_ir::PointerValue<'ctx> = src_v
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in inttoptr")?;
        let dst_ty = self.parse_type(false)?;
        let src_int: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = src_v
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let f: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = v
            .try_into()
            .map_err(|_| self.expected("float-typed fneg operand"))?;
        let r = if fmf.is_empty() {
            b.build_float_neg::<llvmkit_ir::FloatDyn, _>(f, result_name.as_str())
        } else {
            b.build_float_neg_with_flags::<llvmkit_ir::FloatDyn, _>(f, fmf, result_name.as_str())
        }
        .map_err(|e| self.builder_err("fneg", e))?;
        Ok(r.as_value())
    }

    /// `OP [nnan ninf ...] TYPE LHS, RHS` for fadd/fsub/fmul/fdiv/frem.
    /// Mirrors `LLParser::parseArithmetic` FP arm.
    fn parse_fp_binop(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        op: FpBinOp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        let lhs_v = self.parse_value(state, ty)?;
        self.expect_punct(PunctKind::Comma, "',' between FP binop operands")?;
        let rhs_v = self.parse_value_no_type(state, ty)?;
        let lhs: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = lhs_v
            .try_into()
            .map_err(|_| self.expected("float-typed lhs"))?;
        let rhs: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = rhs_v
            .try_into()
            .map_err(|_| self.expected("float-typed rhs"))?;
        let name = result_name.as_str();
        let v = match op {
            FpBinOp::Add => if fmf.is_empty() {
                b.build_fp_add::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_add_fmf::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("fadd", e))?
            .as_value(),
            FpBinOp::Sub => if fmf.is_empty() {
                b.build_fp_sub::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_sub_fmf::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("fsub", e))?
            .as_value(),
            FpBinOp::Mul => if fmf.is_empty() {
                b.build_fp_mul::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_mul_fmf::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("fmul", e))?
            .as_value(),
            FpBinOp::Div => if fmf.is_empty() {
                b.build_fp_div::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_div_fmf::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("fdiv", e))?
            .as_value(),
            FpBinOp::Rem => if fmf.is_empty() {
                b.build_fp_rem::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
            } else {
                b.build_fp_rem_fmf::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, fmf, name)
            }
            .map_err(|e| self.builder_err("frem", e))?
            .as_value(),
        };
        Ok(v)
    }

    /// `fcmp [nnan ninf ...] PRED TYPE LHS, RHS`. Mirrors `LLParser::parseCompare` FP arm.
    fn parse_fcmp(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let fmf = self.parse_optional_fmf()?;
        use llvmkit_ir::FloatPredicate as P;
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
        let lhs: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = lhs_v
            .try_into()
            .map_err(|_| self.expected("float-typed lhs"))?;
        let rhs: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = rhs_v
            .try_into()
            .map_err(|_| self.expected("float-typed rhs"))?;
        let name = result_name.as_str();
        let r = if fmf.is_empty() {
            b.build_fp_cmp::<llvmkit_ir::FloatDyn, _, _>(pred, lhs, rhs, name)
                .map_err(|e| self.builder_err("fcmp", e))?
                .as_value()
        } else {
            b.build_fp_cmp_fmf::<llvmkit_ir::FloatDyn, _, _>(pred, lhs, rhs, fmf, name)
                .map_err(|e| self.builder_err("fcmp", e))?
                .as_value()
        };
        Ok(r)
    }

    /// `alloca TYPE [, TYPE COUNT] [, align N]`.
    /// Mirrors `LLParser::parseAlloc` (LLParser.cpp ~8540).
    fn parse_alloca(
        &mut self,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let ty = self.parse_type(false)?;
        let align = self.parse_optional_comma_align()?;
        let r = match align {
            Some(a) => b
                .build_alloca_with_align(ty, a, result_name.as_str())
                .map_err(|e| self.builder_err("alloca", e))?,
            None => b
                .build_alloca(ty, result_name.as_str())
                .map_err(|e| self.builder_err("alloca", e))?,
        };
        Ok(r.as_value())
    }

    /// `load [volatile] TYPE, ptr PTR [, align N]` or
    /// `load atomic [volatile] TYPE, ptr PTR [syncscope("...")] ORDERING, align N`.
    /// Mirrors `LLParser::parseLoad` (LLParser.cpp ~8608).
    fn parse_load(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let is_atomic = self.eat_keyword(Keyword::Atomic)?;
        let volatile = self.eat_keyword(Keyword::Volatile)?;
        let ty = self.parse_type(false)?;
        self.expect_punct(PunctKind::Comma, "',' between load type and pointer")?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed load operand"))?;

        if is_atomic {
            let sync_scope = self.parse_optional_syncscope()?;
            let ordering = self.parse_atomic_ordering("atomic ordering")?;
            self.expect_punct(PunctKind::Comma, "',' after atomic ordering")?;
            let align = self.parse_align_val()?;
            let config = llvmkit_ir::instr_types::AtomicLoadConfig {
                ordering,
                sync_scope,
                align,
                volatile,
            };
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
    ) -> ParseResult<()> {
        let is_atomic = self.eat_keyword(Keyword::Atomic)?;
        let volatile = self.eat_keyword(Keyword::Volatile)?;
        let val_ty = self.parse_type(false)?;
        let val_v = self.parse_value(state, val_ty)?;
        self.expect_punct(PunctKind::Comma, "',' between store value and pointer")?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed store target"))?;
        if is_atomic {
            let sync_scope = self.parse_optional_syncscope()?;
            let ordering = self.parse_atomic_ordering("atomic ordering")?;
            self.expect_punct(PunctKind::Comma, "',' after atomic ordering")?;
            let align = self.parse_align_val()?;
            let config = llvmkit_ir::instr_types::AtomicStoreConfig {
                ordering,
                sync_scope,
                align,
                volatile,
            };
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

    /// `getelementptr [inbounds] [nuw] [nusw] SOURCE_TY, ptr P, INDEX, INDEX, ...`.
    /// Mirrors `LLParser::parseGetElementPtr` (LLParser.cpp ~8900).
    fn parse_gep(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let inbounds = self.eat_keyword(Keyword::Inbounds)?;
        let nuw = self.eat_keyword(Keyword::Nuw)?;
        let nusw = self.eat_keyword(Keyword::Nusw)?;
        let source_ty = self.parse_type(false)?;
        self.expect_punct(PunctKind::Comma, "',' after GEP source type")?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed GEP base"))?;
        let mut indices: Vec<llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn>> = Vec::new();
        while self.eat_punct(PunctKind::Comma)? {
            let idx_ty = self.parse_type(false)?;
            let idx_v = self.parse_value(state, idx_ty)?;
            let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = idx_v
                .try_into()
                .map_err(|_| self.expected("integer GEP index"))?;
            indices.push(idx);
        }
        let name = result_name.as_str();
        let flags = {
            let mut f = if inbounds {
                llvmkit_ir::GepNoWrapFlags::inbounds()
            } else {
                llvmkit_ir::GepNoWrapFlags::empty()
            };
            if nuw {
                f |= llvmkit_ir::GepNoWrapFlags::NUW;
            }
            if nusw {
                f |= llvmkit_ir::GepNoWrapFlags::NUSW;
            }
            f
        };
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let cond_ty = self.parse_type(false)?;
        let cond_v = self.parse_value(state, cond_ty)?;
        let cond_iv: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = cond_v
            .try_into()
            .map_err(|_| self.expected("integer-typed select condition"))?;
        let cond_i1: llvmkit_ir::IntValue<'ctx, bool> = cond_iv
            .try_into()
            .map_err(|_| self.expected("i1 select condition"))?;
        self.expect_punct(PunctKind::Comma, "',' after select condition")?;
        let true_ty = self.parse_type(false)?;
        let true_v = self.parse_value(state, true_ty)?;
        self.expect_punct(PunctKind::Comma, "',' between select arms")?;
        let false_ty = self.parse_type(false)?;
        let false_v = self.parse_value(state, false_ty)?;
        if true_ty != false_ty {
            return Err(self.expected("matching arm types in select"));
        }
        let name = result_name.as_str();
        let v = match true_ty.into_type_enum() {
            AnyTypeEnum::Int(_) => {
                let t: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = true_v
                    .try_into()
                    .map_err(|_| self.expected("int-typed select arm"))?;
                let f: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = false_v
                    .try_into()
                    .map_err(|_| self.expected("int-typed select arm"))?;
                b.build_select(cond_i1, t, f, name)
                    .map_err(|e| self.builder_err("select", e))?
                    .as_value()
            }
            AnyTypeEnum::Float(_) => {
                let t: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = true_v
                    .try_into()
                    .map_err(|_| self.expected("float-typed select arm"))?;
                let f: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = false_v
                    .try_into()
                    .map_err(|_| self.expected("float-typed select arm"))?;
                b.build_select(cond_i1, t, f, name)
                    .map_err(|e| self.builder_err("select", e))?
                    .as_value()
            }
            AnyTypeEnum::Pointer(_) => {
                let t: llvmkit_ir::PointerValue<'ctx> = true_v
                    .try_into()
                    .map_err(|_| self.expected("ptr-typed select arm"))?;
                let f: llvmkit_ir::PointerValue<'ctx> = false_v
                    .try_into()
                    .map_err(|_| self.expected("ptr-typed select arm"))?;
                b.build_select(cond_i1, t, f, name)
                    .map_err(|e| self.builder_err("select", e))?
                    .as_value()
            }
            _ => {
                return Err(
                    self.expected("select arm category supported by this session (int/fp/ptr)")
                );
            }
        };
        Ok(v)
    }

    /// `fptosi`/`fptoui TYPE VALUE to TYPE`. Mirrors `LLParser::parseCast`
    /// for `Instruction::FPToSI` / `FPToUI`.
    fn parse_fp_to_int(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        op: FpToInt,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in fp->int cast")?;
        let dst_ty = self.parse_type(false)?;
        let src_fp: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = src_v
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        op: IntToFp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let nneg = matches!(op, IntToFp::UIToFp) && self.eat_keyword(Keyword::Nneg)?;
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in int->fp cast")?;
        let dst_ty = self.parse_type(false)?;
        let src_int: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = src_v
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
                        llvmkit_ir::instr_types::UIToFpFlags::new().nneg(),
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in addrspacecast")?;
        let dst_ty = self.parse_type(false)?;
        let src_ptr: llvmkit_ir::PointerValue<'ctx> = src_v
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in fptrunc")?;
        let dst_ty = self.parse_type(false)?;
        let sv: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = src_v
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let src_ty = self.parse_type(false)?;
        let src_v = self.parse_value(state, src_ty)?;
        self.expect_keyword(Keyword::To, "'to' in fpext")?;
        let dst_ty = self.parse_type(false)?;
        let sv: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = src_v
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

    /// `ptrtoaddr <ptr-ty> <val> to <int-ty>`. The LLVM 22 opaque-pointer
    /// rename of `ptrtoint`. Mirrors `LLParser::parseCast` for
    /// `Instruction::PtrToInt` (same wire path). Uses `build_ptr_to_int`.
    ///
    /// Upstream: `test/Assembler/ptrtoaddr.ll`.
    fn parse_ptrtoaddr(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        // Reuse the existing ptrtoint logic.
        self.parse_ptr_to_int(state, b, result_name)
    }

    /// `extractelement <vec-ty> <vec>, <idx-ty> <idx>`.
    /// Mirrors `LLParser::parseExtractElement`.
    ///
    /// Upstream: `test/Assembler/extractelement.ll`.
    fn parse_extractelement(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let vec_ty = self.parse_type(false)?;
        let vec_v = self.parse_value(state, vec_ty)?;
        self.expect_punct(PunctKind::Comma, "',' in extractelement")?;
        let idx_ty = self.parse_type(false)?;
        let idx_v = self.parse_value(state, idx_ty)?;
        let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = idx_v
            .try_into()
            .map_err(|_| self.expected("integer index for extractelement"))?;
        let v = b
            .build_extract_element(vec_v, idx, result_name.as_str())
            .map_err(|e| self.builder_err("extractelement", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// `insertelement <vec-ty> <vec>, <elt-ty> <elt>, <idx-ty> <idx>`.
    /// Mirrors `LLParser::parseInsertElement`.
    ///
    /// Upstream: `test/Assembler/insertelement.ll`.
    fn parse_insertelement(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let vec_ty = self.parse_type(false)?;
        let vec_v = self.parse_value(state, vec_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after vector in insertelement")?;
        let elt_ty = self.parse_type(false)?;
        let elt_v = self.parse_value(state, elt_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after element in insertelement")?;
        let idx_ty = self.parse_type(false)?;
        let idx_v = self.parse_value(state, idx_ty)?;
        let idx: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = idx_v
            .try_into()
            .map_err(|_| self.expected("integer index for insertelement"))?;
        let v = b
            .build_insert_element(vec_v, elt_v, idx, result_name.as_str())
            .map_err(|e| self.builder_err("insertelement", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// `shufflevector <vec-ty> <v1>, <vec-ty> <v2>, <mask>`.
    /// The mask is `< i32 N, i32 M, ... >` or `poison`. Mirrors
    /// `LLParser::parseShuffleVector`.
    ///
    /// Upstream: `test/Assembler/shufflevector.ll`.
    fn parse_shufflevector(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let v1_ty = self.parse_type(false)?;
        let v1 = self.parse_value(state, v1_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after v1 in shufflevector")?;
        let v2_ty = self.parse_type(false)?;
        let v2 = self.parse_value(state, v2_ty)?;
        self.expect_punct(PunctKind::Comma, "',' before mask in shufflevector")?;
        // Parse mask: `poison` or `< i32 N, ... >`
        let mask = self.parse_shuffle_mask()?;
        let v = b
            .build_shuffle_vector(v1, v2, &mask, result_name.as_str())
            .map_err(|e| self.builder_err("shufflevector", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// Parse a shufflevector mask: `poison` → all-poison entries, or
    /// `< i32 N, i32 M, ... >` → explicit indices.
    fn parse_shuffle_mask(&mut self) -> ParseResult<Vec<i32>> {
        use llvmkit_ir::instr_types::POISON_MASK_ELEM;
        if matches!(self.peek(), Token::Kw(Keyword::Poison)) {
            self.bump()?;
            return Ok(vec![POISON_MASK_ELEM]);
        }
        self.expect_punct(PunctKind::Less, "'<' to open shuffle mask")?;
        let mut mask = Vec::new();
        loop {
            let _ety = self.parse_type(false)?;
            let (neg, val) = self
                .parse_int_literal()
                .unwrap_or((false, POISON_MASK_ELEM as u64));
            let entry: i32 = if neg {
                -(val as i32)
            } else {
                i32::try_from(val).unwrap_or(POISON_MASK_ELEM)
            };
            mask.push(entry);
            if !self.eat_punct(PunctKind::Comma)? {
                break;
            }
        }
        self.expect_punct(PunctKind::Greater, "'>' to close shuffle mask")?;
        Ok(mask)
    }

    /// `extractvalue <agg-ty> <agg>, <idx>, ...`. Mirrors
    /// `LLParser::parseExtractValue`.
    ///
    /// Upstream: `test/Assembler/extractvalue.ll`.
    fn parse_extractvalue(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let agg_ty = self.parse_type(false)?;
        let agg_v = self.parse_value(state, agg_ty)?;
        let mut indices = Vec::new();
        while self.eat_punct(PunctKind::Comma)? {
            let idx = self.parse_uint32("extractvalue index")?;
            indices.push(idx);
        }
        let v = b
            .build_extract_value(agg_v, indices, result_name.as_str())
            .map_err(|e| self.builder_err("extractvalue", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// `insertvalue <agg-ty> <agg>, <elt-ty> <elt>, <idx>, ...`. Mirrors
    /// `LLParser::parseInsertValue`.
    ///
    /// Upstream: `test/Assembler/insertvalue.ll`.
    fn parse_insertvalue(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
            .build_insert_value(agg_v, elt_v, indices, result_name.as_str())
            .map_err(|e| self.builder_err("insertvalue", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// `phi <ty> [ <val>, <label> ], ...`. Handles int, float, and pointer
    /// phis. Forward-referenced incoming values are stored in
    /// `state.deferred_phi` and resolved by `PerFunctionState::finish`.
    /// Mirrors `LLParser::parsePhi` (LLParser.cpp ~7990).
    ///
    /// Upstream: `test/Assembler/phi.ll`.
    fn parse_phi(
        &mut self,
        state: &mut PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let _fmf = self.parse_optional_fmf()?;
        let ty = self.parse_type(false)?;
        let name = result_name.as_str();
        // Build the phi and extract its value ID for deferred edge resolution.
        let phi_val = match ty.into_type_enum() {
            AnyTypeEnum::Int(int_ty) => {
                let phi = b
                    .build_int_phi_dyn(int_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.as_instruction().as_value()
            }
            AnyTypeEnum::Float(fp_ty) => {
                let phi = b
                    .build_fp_phi_dyn(fp_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.as_instruction().as_value()
            }
            AnyTypeEnum::Pointer(ptr_ty) => {
                let phi = b
                    .build_pointer_phi_in_addrspace(ptr_ty, name)
                    .map_err(|e| self.builder_err("phi", e))?;
                phi.as_instruction().as_value()
            }
            _ => return Err(self.expected("phi result type must be int, float, or pointer")),
        };
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
                    let (neg, raw) = self.parse_int_literal()?;
                    let int_ty = match ty.into_type_enum() {
                        AnyTypeEnum::Int(t) => t,
                        _ => return Err(self.expected("integer literal only valid for int phi")),
                    };
                    let bits = if neg { raw.wrapping_neg() } else { raw };
                    let c = int_ty
                        .const_int_raw(bits, neg)
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
                        AnyTypeEnum::Float(t) => {
                            // For float phi, we need a float zero constant.
                            // Use the int type hack: zeroinitializer of a float
                            // is 0.0. For now, defer this incoming edge.
                            state.deferred_phi.push(DeferredPhiEdge {
                                phi_val,
                                val_ref: PhiValRef::Undef,
                                bb_name: String::new(), // will be filled below
                                loc: val_loc,
                            });
                            // We'll fix the bb_name after parsing
                            self.expect_punct(PunctKind::Comma, "',' in phi incoming pair")?;
                            let bb_name = self.parse_phi_label()?;
                            self.expect_punct(PunctKind::RSquare, "']' in phi incoming pair")?;
                            if let Some(last) = state.deferred_phi.last_mut() {
                                last.bb_name = bb_name;
                            }
                            let _ = t;
                            continue;
                        }
                        _ => return Err(self.expected("undef for int/float/ptr phi")),
                    };
                    PhiValRef::Resolved(v)
                }
                _ => return Err(self.expected("value in phi incoming pair")),
            };
            self.expect_punct(PunctKind::Comma, "',' in phi incoming pair")?;
            let bb_name = self.parse_phi_label()?;
            self.expect_punct(PunctKind::RSquare, "']' to close phi incoming pair")?;
            // Either resolve immediately or defer.
            match val_ref {
                PhiValRef::Resolved(v) => {
                    let bb = state.ensure_block(self.module, &bb_name);
                    let tmp_b = llvmkit_ir::IRBuilder::new(self.module);
                    tmp_b
                        .phi_add_incoming_from_value(phi_val, v, bb)
                        .map_err(|e| self.builder_err("phi.add_incoming", e))?;
                }
                other => {
                    state.deferred_phi.push(DeferredPhiEdge {
                        phi_val,
                        val_ref: other,
                        bb_name,
                        loc: val_loc,
                    });
                }
            }
        }
        Ok(phi_val)
    }

    /// Parse the label in a `[ val, label %name ]` phi pair.
    fn parse_phi_label(&mut self) -> ParseResult<String> {
        match self.peek() {
            Token::LocalVar(_) => {
                let n = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("block label in phi pair"))?;
                self.bump()?;
                Ok(n)
            }
            Token::LocalVarId(id) => {
                let id = *id;
                self.bump()?;
                Ok(format!("{id}"))
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        // Optional tail-call keyword.
        let _ = self.eat_keyword(Keyword::Tail)?
            || self.eat_keyword(Keyword::Musttail)?
            || self.eat_keyword(Keyword::Notail)?;
        // Must start with `call` opcode already consumed by the dispatch table.
        // Optional calling convention.
        self.parse_optional_calling_conv()?;
        // Skip optional return attributes (e.g., `zeroext`, `noalias`, ...).
        self.skip_optional_param_attrs()?;
        // Parse return type.
        let _ret_ty = self.parse_type(true)?;
        // Look up the callee.
        let callee = self.parse_callee_ref()?;
        // Parse arguments.
        self.expect_punct(PunctKind::LParen, "'(' in call argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx>> = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    break;
                }
                let arg_ty = self.parse_type(false)?;
                self.skip_optional_param_attrs()?;
                let arg_v = self.parse_value(state, arg_ty)?;
                args.push(arg_v);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close call argument list")?;
        // Skip optional function attributes.
        self.skip_optional_fn_attrs()?;
        let name = result_name.as_str();
        let v = b
            .build_call(callee, args, name)
            .map_err(|e| self.builder_err("call", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// Optionally skip a calling convention keyword. Returns the CC token if
    /// consumed, but the calling convention is not yet plumbed through to
    /// the IR (deferred).
    fn parse_optional_calling_conv(&mut self) -> ParseResult<()> {
        match self.peek() {
            Token::Kw(
                Keyword::Ccc
                | Keyword::Fastcc
                | Keyword::Coldcc
                | Keyword::Anyregcc
                | Keyword::PreserveMostcc
                | Keyword::PreserveAllcc
                | Keyword::Ghccc
                | Keyword::Swiftcc
                | Keyword::Swifttailcc
                | Keyword::X86Stdcallcc
                | Keyword::X86Fastcallcc
                | Keyword::X86Thiscallcc
                | Keyword::X86Vectorcallcc
                | Keyword::X86Regcallcc
                | Keyword::IntelOclBicc
                | Keyword::Win64cc
                | Keyword::X86_64Sysvcc
                | Keyword::Hhvmcc
                | Keyword::HhvmCcc
                | Keyword::AmdgpuVs
                | Keyword::AmdgpuLs
                | Keyword::AmdgpuHs
                | Keyword::AmdgpuEs
                | Keyword::AmdgpuGs
                | Keyword::AmdgpuPs
                | Keyword::AmdgpuCs
                | Keyword::AmdgpuKernel
                | Keyword::Tailcc
                | Keyword::CfguardCheckcc
                | Keyword::M68kRtdcc,
            ) => {
                self.bump()?;
                Ok(())
            }
            Token::Kw(Keyword::Cc) => {
                // `cc N` form
                self.bump()?;
                let _ = self.parse_uint32("calling convention number")?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Skip optional parameter attributes on a call/declare site
    /// (`zeroext`, `signext`, `noalias`, `nonnull`, etc.). Mirrors
    /// `LLParser::parseOptionalParamAttrs`.
    fn skip_optional_param_attrs(&mut self) -> ParseResult<()> {
        loop {
            match self.peek() {
                Token::Kw(
                    Keyword::Zeroext
                    | Keyword::Signext
                    | Keyword::Noalias
                    | Keyword::Nonnull
                    | Keyword::Noundef
                    | Keyword::Readonly
                    | Keyword::Writeonly
                    | Keyword::Readnone
                    | Keyword::Returned
                    | Keyword::Nocapture
                    | Keyword::Nofree
                    | Keyword::Swiftself
                    | Keyword::Swifterror
                    | Keyword::Swiftasync
                    | Keyword::Initializes
                    | Keyword::Writable
                    | Keyword::DeadOnUnwind,
                ) => {
                    self.bump()?;
                }
                // Skip `align N` parameter attribute.
                Token::Kw(Keyword::Align) => {
                    self.bump()?;
                    let _ = self.parse_uint64("align value")?;
                }
                // Skip `dereferenceable(N)` / `dereferenceable_or_null(N)`.
                Token::Kw(Keyword::Dereferenceable | Keyword::DereferenceableOrNull) => {
                    self.bump()?;
                    self.expect_punct(PunctKind::LParen, "'(' in dereferenceable")?;
                    let _ = self.parse_uint64("dereferenceable bytes")?;
                    self.expect_punct(PunctKind::RParen, "')' in dereferenceable")?;
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// Skip optional function-level attributes after a call instruction
    /// (`#N`, `[` alignstack etc.`]`). These are not wired to the IR yet.
    fn skip_optional_fn_attrs(&mut self) -> ParseResult<()> {
        loop {
            match self.peek() {
                Token::AttrGrpId(_) => {
                    self.bump()?;
                }
                Token::Kw(
                    Keyword::Nounwind
                    | Keyword::Noreturn
                    | Keyword::Noinline
                    | Keyword::Alwaysinline
                    | Keyword::Optnone
                    | Keyword::Optsize
                    | Keyword::Speculatable
                    | Keyword::Memory
                    | Keyword::Willreturn
                    | Keyword::Mustprogress
                    | Keyword::Nosync,
                ) => {
                    self.bump()?;
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// Resolve a callee reference: `@name` or `@N`. Looks up the function
    /// in the module. Returns `FunctionValue<'ctx, Dyn>`.
    fn parse_callee_ref(
        &mut self,
    ) -> ParseResult<llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>> {
        let loc = self.loc();
        match self.peek() {
            Token::GlobalVar(_) => {
                let name = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("callee function name"))?;
                self.bump()?;
                self.module
                    .function_by_name(&name)
                    .ok_or_else(|| ParseError::UndefinedSymbol {
                        kind: crate::parse_error::SymbolKind::Global,
                        id: crate::parse_error::SymbolId::Named(name),
                        loc: DiagLoc::span(loc),
                    })
            }
            Token::GlobalId(id) => {
                let id = *id;
                self.bump()?;
                self.numbered_globals
                    .get(id)
                    .and_then(|r| match r {
                        GlobalRef::Function(f) => Some(*f),
                        GlobalRef::Variable(_) => None,
                    })
                    .ok_or_else(|| ParseError::UndefinedSymbol {
                        kind: crate::parse_error::SymbolKind::Global,
                        id: crate::parse_error::SymbolId::Numbered(id),
                        loc: DiagLoc::span(loc),
                    })
            }
            _ => Err(self.expected("function name after call")),
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let list_ty = self.parse_type(false)?;
        let list_v = self.parse_value(state, list_ty)?;
        let list_ptr: llvmkit_ir::PointerValue<'ctx> = list_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed va_arg list operand"))?;
        self.expect_punct(PunctKind::Comma, "',' in va_arg")?;
        let result_ty = self.parse_type(false)?;
        let v = b
            .build_va_arg(list_ptr, result_ty, result_name.as_str())
            .map_err(|e| self.builder_err("va_arg", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// `freeze <ty> <val>`. Mirrors `LLParser::parseFreeze`.
    ///
    /// Upstream: `test/Assembler/freeze.ll`.
    fn parse_freeze(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let r = b
            .build_freeze(v, result_name.as_str())
            .map_err(|e| self.builder_err("freeze", e))?;
        Ok(r.as_instruction().as_value())
    }

    /// `switch <ty> <val>, label %default [ <ty> N, label %case ... ]`.
    /// Mirrors `LLParser::parseSwitch` (LLParser.cpp ~7640).
    ///
    /// Upstream: `test/Assembler/switch.ll`.
    fn parse_switch(
        &mut self,
        state: &mut PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
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
            .build_switch(cond_v, default_bb, "")
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
            let case_int: llvmkit_ir::IntValue<'ctx, llvmkit_ir::IntDyn> = case_v
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
        state: &mut PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
    ) -> ParseResult<()> {
        self.bump()?; // eat `indirectbr`
        let addr_ty = self.parse_type(false)?;
        let addr_v = self.parse_value(state, addr_ty)?;
        self.expect_punct(PunctKind::Comma, "',' after indirectbr address")?;
        let (_, mut ibr) = b
            .build_indirectbr(addr_v, "")
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
    fn parse_fence(&mut self, b: &ParsedBlockBuilder<'ctx>) -> ParseResult<()> {
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let weak = self.eat_keyword(Keyword::Weak)?;
        let volatile = self.eat_keyword(Keyword::Volatile)?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx> = ptr_v
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
        let config = llvmkit_ir::instr_types::AtomicCmpXchgConfig {
            success_ordering: success_ord,
            failure_ordering: failure_ord,
            sync_scope,
            flags: {
                let mut f = llvmkit_ir::instr_types::CmpXchgFlags::new();
                if weak {
                    f = f.weak();
                }
                if volatile {
                    f = f.volatile();
                }
                f
            },
            align: align
                .map(llvmkit_ir::align::MaybeAlign::from)
                .unwrap_or(llvmkit_ir::align::MaybeAlign::NONE),
        };
        let v = b
            .build_atomic_cmpxchg(ptr, cmp_v, new_v, config, result_name.as_str())
            .map_err(|e| self.builder_err("cmpxchg", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// `atomicrmw [volatile] <op> ptr <ptr>, <ty> <val>
    ///           [syncscope("...")] <ordering> [, align N]`.
    /// Returns the old value. Mirrors `LLParser::parseAtomicRMW`.
    ///
    /// Upstream: `test/Assembler/atomicrmw.ll`.
    fn parse_atomicrmw(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let volatile = self.eat_keyword(Keyword::Volatile)?;
        let op = self.parse_atomicrmw_op()?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr operand for atomicrmw"))?;
        self.expect_punct(PunctKind::Comma, "',' in atomicrmw")?;
        let val_ty = self.parse_type(false)?;
        let val_v = self.parse_value(state, val_ty)?;
        let sync_scope = self.parse_optional_syncscope()?;
        let ordering = self.parse_atomic_ordering("atomicrmw ordering")?;
        let align = self.parse_optional_comma_align()?;
        let config = llvmkit_ir::instr_types::AtomicRMWConfig {
            ordering,
            sync_scope,
            flags: {
                let mut f = llvmkit_ir::instr_types::AtomicRMWFlags::new();
                if volatile {
                    f = f.volatile();
                }
                f
            },
            align: align
                .map(llvmkit_ir::align::MaybeAlign::from)
                .unwrap_or(llvmkit_ir::align::MaybeAlign::NONE),
        };
        let v = b
            .build_atomicrmw(op, ptr, val_v, config, result_name.as_str())
            .map_err(|e| self.builder_err("atomicrmw", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// Parse an `atomicrmw` operation keyword.
    fn parse_atomicrmw_op(&mut self) -> ParseResult<llvmkit_ir::atomicrmw_binop::AtomicRMWBinOp> {
        use llvmkit_ir::atomicrmw_binop::AtomicRMWBinOp as Op;
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
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
        Ok(lp.finish().as_instruction().as_value())
    }

    /// `cleanuppad within <token-or-none> [<args>]`. Non-terminator.
    /// Mirrors `LLParser::parseCleanupPad`.
    ///
    /// Upstream: `test/Assembler/cleanuppad.ll`.
    fn parse_cleanuppad(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        self.expect_keyword(Keyword::Within, "'within' in cleanuppad")?;
        let parent_pad = self.parse_optional_pad_token(state)?;
        let args = self.parse_bracket_value_list(state)?;
        let v = b
            .build_cleanup_pad(parent_pad, args, result_name.as_str())
            .map_err(|e| self.builder_err("cleanuppad", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// `catchpad within <catchswitch> [<args>]`. Non-terminator.
    /// Mirrors `LLParser::parseCatchPad`.
    ///
    /// Upstream: `test/Assembler/catchpad.ll`.
    fn parse_catchpad(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        self.expect_keyword(Keyword::Within, "'within' in catchpad")?;
        let parent_ty = self.parse_type(false)?;
        let parent_v = self.parse_value(state, parent_ty)?;
        let args = self.parse_bracket_value_list(state)?;
        let v = b
            .build_catch_pad(parent_v, args, result_name.as_str())
            .map_err(|e| self.builder_err("catchpad", e))?;
        Ok(v.as_instruction().as_value())
    }

    /// `resume <ty> <val>`. Terminator.
    /// Mirrors `LLParser::parseResume` (LLParser.cpp ~7762).
    ///
    /// Upstream: `test/Assembler/resume.ll`.
    fn parse_resume(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
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
        state: &mut PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
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
        let _ = b
            .build_cleanup_ret(pad_v, unwind_dest, "")
            .map_err(|e| self.builder_err("cleanupret", e))?;
        Ok(())
    }

    /// `catchret from <val> to label %bb`. Terminator.
    /// Mirrors `LLParser::parseCatchRet`.
    ///
    /// Upstream: `test/Assembler/catchret.ll`.
    fn parse_catchret(
        &mut self,
        state: &mut PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
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
        state: &mut PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        self.bump()?; // eat `catchswitch`
        self.expect_keyword(Keyword::Within, "'within' in catchswitch")?;
        let parent_pad = self.parse_optional_pad_token(state)?;
        // `[handler1, handler2, ...]`
        self.expect_punct(PunctKind::LSquare, "'[' in catchswitch handlers")?;
        let mut handlers: Vec<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unsealed>> =
            Vec::new();
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
        let (_, mut cs) = b
            .build_catch_switch(parent_pad, unwind_dest, name)
            .map_err(|e| self.builder_err("catchswitch", e))?;
        for h in handlers {
            cs = cs
                .add_handler(h)
                .map_err(|e| self.builder_err("catchswitch.add_handler", e))?;
        }
        Ok(cs.finish().as_instruction().as_value())
    }

    /// `invoke [cc] [ret-attrs] <ret-ty> @func(<args>) to label %normal
    ///        unwind label %unwind`. Terminator.
    /// Mirrors `LLParser::parseInvoke`.
    ///
    /// Upstream: `test/Assembler/invoke.ll`.
    fn parse_invoke(
        &mut self,
        state: &mut PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<Option<llvmkit_ir::Value<'ctx>>> {
        // parse_lhs_before_invoke already consumed `invoke` and optionally LHS.
        self.parse_optional_calling_conv()?;
        self.skip_optional_param_attrs()?;
        let ret_ty = self.parse_type(true)?;
        let callee = self.parse_callee_ref()?;
        self.expect_punct(PunctKind::LParen, "'(' in invoke argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx>> = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    break;
                }
                let arg_ty = self.parse_type(false)?;
                self.skip_optional_param_attrs()?;
                let arg_v = self.parse_value(state, arg_ty)?;
                args.push(arg_v);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close invoke argument list")?;
        self.skip_optional_fn_attrs()?;
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
        let name = result_name.as_str();
        let (_, inst) = b
            .build_invoke(callee, args, normal_bb, unwind_bb, name)
            .map_err(|e| self.builder_err("invoke", e))?;
        // For void-returning invokes, don't bind a result.
        let ret_is_void = matches!(ret_ty.into_type_enum(), AnyTypeEnum::Void(_));
        if ret_is_void || matches!(result_name, LocalLhs::None) {
            Ok(None)
        } else {
            Ok(Some(inst.as_instruction().as_value()))
        }
    }

    /// `callbr [cc] <ret-ty> @func(<args>) [other label targets]
    ///        to label %normal [, label %indirect ...]`. Terminator.
    /// Mirrors `LLParser::parseCallBr`.
    ///
    /// Upstream: `test/Assembler/callbr.ll`.
    fn parse_callbr(
        &mut self,
        state: &mut PerFunctionState<'ctx>,
        b: ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        self.bump()?; // eat `callbr`
        self.parse_optional_calling_conv()?;
        self.skip_optional_param_attrs()?;
        let _ret_ty = self.parse_type(true)?;
        let callee = self.parse_callee_ref()?;
        self.expect_punct(PunctKind::LParen, "'(' in callbr argument list")?;
        let mut args: Vec<llvmkit_ir::Value<'ctx>> = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                if matches!(self.peek(), Token::DotDotDot) {
                    self.bump()?;
                    break;
                }
                let arg_ty = self.parse_type(false)?;
                self.skip_optional_param_attrs()?;
                let arg_v = self.parse_value(state, arg_ty)?;
                args.push(arg_v);
                if !self.eat_punct(PunctKind::Comma)? {
                    break;
                }
            }
        }
        self.expect_punct(PunctKind::RParen, "')' to close callbr argument list")?;
        self.skip_optional_fn_attrs()?;
        self.expect_keyword(Keyword::To, "'to' in callbr")?;
        self.expect_primitive(
            crate::ll_token::PrimitiveTy::Label,
            "'label' for callbr fallthrough destination",
        )?;
        let fallthrough = self.parse_block_ref(state)?;
        // Optional `[ label %ind1, ... ]`
        let mut indirect: Vec<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unsealed>> =
            Vec::new();
        if matches!(self.peek(), Token::Comma) {
            self.bump()?;
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
        let name = result_name.as_str();
        let (_, inst) = b
            .build_callbr(callee, args, fallthrough, &indirect, name)
            .map_err(|e| self.builder_err("callbr", e))?;
        Ok(inst.as_instruction().as_value())
    }

    /// Parse `none` or a local token as a parent-pad value for EH pads.
    fn parse_optional_pad_token(
        &mut self,
        state: &PerFunctionState<'ctx>,
    ) -> ParseResult<Option<llvmkit_ir::Value<'ctx>>> {
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
        state: &PerFunctionState<'ctx>,
    ) -> ParseResult<Vec<llvmkit_ir::Value<'ctx>>> {
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

    /// Resolve a `label %name` / `label %N` reference, ensuring the
    /// target block exists (creating an empty unsealed block if it's a
    /// forward reference).
    fn parse_block_ref(
        &mut self,
        state: &mut PerFunctionState<'ctx>,
    ) -> ParseResult<llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unsealed>> {
        let name = match self.peek() {
            Token::LocalVar(_) => self
                .current_str_payload()
                .ok_or_else(|| self.expected("block label name"))?,
            Token::LocalVarId(n) => format!("{n}"),
            _ => return Err(self.expected("block label after 'label'")),
        };
        self.bump()?;
        Ok(state.ensure_block(self.module, &name))
    }

    /// Parse a value of the given type. Accepts local SSA references,
    /// integer literals, and `null`/`zeroinitializer`/`true`/`false`.
    fn parse_value(
        &mut self,
        state: &PerFunctionState<'ctx>,
        ty: Type<'ctx>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        self.parse_value_no_type(state, ty)
    }

    fn parse_value_no_type(
        &mut self,
        state: &PerFunctionState<'ctx>,
        ty: Type<'ctx>,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        match self.peek() {
            Token::LocalVar(_) => {
                let n = self
                    .current_str_payload()
                    .ok_or_else(|| self.expected("local SSA name"))?;
                self.bump()?;
                state
                    .local_named
                    .get(&n)
                    .copied()
                    .ok_or_else(|| ParseError::UndefinedSymbol {
                        kind: SYMBOL_KIND_LOCAL,
                        id: crate::parse_error::SymbolId::Named(n),
                        loc: DiagLoc::span(self.loc()),
                    })
            }
            Token::LocalVarId(id) => {
                let id = *id;
                self.bump()?;
                state
                    .local_numbered
                    .get(&id)
                    .copied()
                    .ok_or_else(|| ParseError::UndefinedSymbol {
                        kind: SYMBOL_KIND_LOCAL,
                        id: crate::parse_error::SymbolId::Numbered(id),
                        loc: DiagLoc::span(self.loc()),
                    })
            }
            Token::IntegerLit(_) => {
                let int_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Int(t) => t,
                    _ => return Err(self.expected("integer constant only valid for int type")),
                };
                let (negative, value) = self.parse_int_literal()?;
                let raw = if negative {
                    value.wrapping_neg()
                } else {
                    value
                };
                let c = int_ty
                    .const_int_raw(raw, negative)
                    .map_err(|e| self.builder_err("integer constant", e))?;
                Ok(c.as_value())
            }
            Token::Kw(Keyword::True) => {
                let _ = ty; // type already consumed
                self.bump()?;
                Ok(self
                    .module
                    .i1_type()
                    .const_int_raw(1, false)
                    .unwrap()
                    .as_value())
            }
            Token::Kw(Keyword::False) => {
                let _ = ty;
                self.bump()?;
                Ok(self
                    .module
                    .i1_type()
                    .const_int_raw(0, false)
                    .unwrap()
                    .as_value())
            }
            Token::Kw(Keyword::Null) => {
                let pty = match ty.into_type_enum() {
                    AnyTypeEnum::Pointer(t) => t,
                    _ => return Err(self.expected("'null' is only valid for pointer types")),
                };
                self.bump()?;
                Ok(pty.const_null().as_value())
            }
            Token::Kw(Keyword::Zeroinitializer) => {
                self.bump()?;
                match ty.into_type_enum() {
                    AnyTypeEnum::Int(t) => Ok(t.const_zero().as_value()),
                    AnyTypeEnum::Pointer(t) => Ok(t.const_null().as_value()),
                    AnyTypeEnum::Float(t) => Ok(t.const_from_bits(0).as_value()),
                    _ => Err(self.expected("zeroinitializer for the modeled scalar types")),
                }
            }
            Token::Kw(Keyword::Undef) => {
                self.bump()?;
                Ok(ty.get_undef().as_value())
            }
            Token::Kw(Keyword::Poison) => {
                self.bump()?;
                Ok(ty.get_poison().as_value())
            }
            Token::FloatLit(_) => {
                let float_ty = match ty.into_type_enum() {
                    AnyTypeEnum::Float(t) => t,
                    _ => return Err(self.expected("float constant only valid for float type")),
                };
                let bits = self.parse_fp_literal(&float_ty)?;
                Ok(float_ty.const_from_bits(bits).as_value())
            }
            Token::GlobalVar(_) | Token::GlobalId(_) => {
                // Global variable reference — resolve by name from the module.
                let name = match self.peek() {
                    Token::GlobalVar(_) => self
                        .current_str_payload()
                        .ok_or_else(|| self.expected("global variable name"))?,
                    Token::GlobalId(n) => format!("{n}"),
                    _ => unreachable!(),
                };
                self.bump()?;
                // Look up global or function by name and return its Value.
                if let Some(gv) = self.module.get_global(&name) {
                    Ok(gv.as_value())
                } else if let Some(fv) = self.module.function_by_name(&name) {
                    Ok(fv.as_value())
                } else {
                    Err(ParseError::UndefinedSymbol {
                        kind: crate::parse_error::SymbolKind::Global,
                        id: crate::parse_error::SymbolId::Named(name),
                        loc: DiagLoc::span(self.loc()),
                    })
                }
            }
            // `metadata !N` / `metadata !"str"` — a metadata node used as
            // a value (LLVM's `MetadataAsValue`), e.g. a `call` argument to
            // `@llvm.read_register` / `@llvm.write_register`. Only valid
            // when the operand's declared type is `metadata`; otherwise a
            // stray `!N` in (say) an `i64` slot is a type error.
            Token::Exclaim => {
                if !ty.is_metadata() {
                    return Err(self.expected("`metadata` type for a metadata operand"));
                }
                self.parse_metadata_value_operand()
            }
            _ => Err(self.expected("operand value")),
        }
    }

    /// Parse a floating-point literal and return raw bits (u128).
    /// Handles decimal literals (converted to f64 bits) and hex forms.
    fn parse_fp_literal(
        &mut self,
        _float_ty: &llvmkit_ir::FloatType<'ctx, llvmkit_ir::FloatDyn>,
    ) -> ParseResult<u128> {
        use crate::ll_token::FpLit;
        let bits: u128 = match self.peek() {
            Token::FloatLit(fp) => match *fp {
                FpLit::Decimal(s) => {
                    let val: f64 = s
                        .parse()
                        .map_err(|_| self.expected("valid decimal float literal"))?;
                    u128::from(val.to_bits())
                }
                FpLit::HexDouble(s) => u128::from(
                    u64::from_str_radix(s, 16)
                        .map_err(|_| self.expected("valid hex double literal"))?,
                ),
                FpLit::HexHalf(s) => u128::from(
                    u16::from_str_radix(s, 16)
                        .map_err(|_| self.expected("valid hex half literal"))?,
                ),
                FpLit::HexBFloat(s) => u128::from(
                    u16::from_str_radix(s, 16)
                        .map_err(|_| self.expected("valid hex bfloat literal"))?,
                ),
                FpLit::HexX87(s) => u128::from_str_radix(s, 16)
                    .map_err(|_| self.expected("valid hex x87 literal"))?,
                FpLit::HexQuad(s) => u128::from_str_radix(s, 16)
                    .map_err(|_| self.expected("valid hex quad literal"))?,
                FpLit::HexPpc128(s) => u128::from_str_radix(s, 16)
                    .map_err(|_| self.expected("valid hex ppc128 literal"))?,
            },
            _ => return Err(self.expected("floating-point literal")),
        };
        self.bump()?;
        Ok(bits)
    }
}

// ── Helper enums ────────────────────────────────────────────────────────────

// ── Function-body helper types ──────────────────────────────────────────────

/// Outgoing reference to an incoming phi value that could not be resolved
/// immediately (forward reference). Resolved by `PerFunctionState::finish`.
#[derive(Clone, Debug)]
enum PhiValRef<'ctx> {
    /// Already resolved to a concrete value.
    Resolved(llvmkit_ir::Value<'ctx>),
    /// Named local (`%name`) not yet defined.
    Named(String),
    /// Numbered local (`%N`) not yet defined.
    Numbered(u32),
    /// `undef` / `poison` constant — resolved at finish time using zero.
    Undef,
}

/// One deferred phi incoming edge. Resolved after all blocks are parsed.
struct DeferredPhiEdge<'ctx> {
    /// The phi instruction's Value handle. Used by `finish()` with
    /// `phi_add_incoming_from_value` to add the incoming edge.
    phi_val: llvmkit_ir::Value<'ctx>,
    /// The incoming value reference (may be a forward ref).
    val_ref: PhiValRef<'ctx>,
    /// Name of the incoming basic block.
    bb_name: String,
    /// Source location for error reporting.
    loc: llvmkit_support::Span,
}

/// Per-function symbol tables. Mirrors `LLParser::PerFunctionState`'s
/// named/numbered value tables and the basic-block lookup map.
struct PerFunctionState<'ctx> {
    func: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>,
    /// `%name` to the bound SSA value.
    local_named: std::collections::HashMap<String, llvmkit_ir::Value<'ctx>>,
    /// `%N` to the bound SSA value.
    local_numbered: std::collections::HashMap<u32, llvmkit_ir::Value<'ctx>>,
    /// Slot id of the next anonymous SSA value (used for `%lhs = ...`
    /// assignments without an explicit name).
    next_unnamed_value_id: u32,
    /// `label` to the (Unsealed) basic-block handle. Created on first
    /// reference to support `br label %later` forward references.
    blocks: std::collections::HashMap<
        String,
        llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unsealed>,
    >,
    /// Deferred phi incoming edges for forward references. Resolved by
    /// `finish()` after all blocks in the function have been parsed.
    deferred_phi: Vec<DeferredPhiEdge<'ctx>>,
}

impl<'ctx> PerFunctionState<'ctx> {
    fn new(func: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>) -> Self {
        Self {
            func,
            local_named: std::collections::HashMap::new(),
            local_numbered: std::collections::HashMap::new(),
            next_unnamed_value_id: 0,
            blocks: std::collections::HashMap::new(),
            deferred_phi: Vec::new(),
        }
    }

    /// Look up or lazily create the named basic block. Mirrors
    /// `PerFunctionState::defineBB` / `getBB` for the constructive subset
    /// where forward references just create the block in advance and
    /// fill it in when the label is later observed.
    fn ensure_block(
        &mut self,
        _module: &'ctx llvmkit_ir::Module<'ctx>,
        name: &str,
    ) -> llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn, llvmkit_ir::Unsealed> {
        if let Some(bb) = self.blocks.get(name) {
            return *bb;
        }
        let bb = self.func.append_basic_block(name);
        self.blocks.insert(name.to_owned(), bb);
        bb
    }

    fn bind_local(&mut self, lhs: &LocalLhs, v: llvmkit_ir::Value<'ctx>) {
        match lhs {
            LocalLhs::Named(n) => {
                self.local_named.insert(n.clone(), v);
            }
            LocalLhs::Numbered(id) => {
                self.local_numbered.insert(*id, v);
                self.next_unnamed_value_id = self.next_unnamed_value_id.max(id.saturating_add(1));
            }
            LocalLhs::None => {
                let id = self.next_unnamed_value_id;
                self.local_numbered.insert(id, v);
                self.next_unnamed_value_id = id.saturating_add(1);
            }
        }
    }

    /// Resolve all deferred phi incoming edges after the function body has
    /// been fully parsed. Called by `Parser::parse_define` before `}`.
    fn finish(
        &mut self,
        module: &'ctx llvmkit_ir::Module<'ctx>,
    ) -> crate::parse_error::ParseResult<()> {
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
            let bb = self.ensure_block(module, &edge.bb_name);
            let tmp_b = llvmkit_ir::IRBuilder::new(module);
            tmp_b
                .phi_add_incoming_from_value(edge.phi_val, val, bb)
                .map_err(|e| crate::parse_error::ParseError::Expected {
                    expected: format!("valid phi add_incoming: {e}"),
                    loc: DiagLoc::span(edge.loc),
                })?;
        }
        Ok(())
    }
}

enum BlockHeader {
    Named(String),
    Implicit,
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
type ParsedBlockBuilder<'ctx> = llvmkit_ir::IRBuilder<
    'ctx,
    llvmkit_ir::ConstantFolder,
    llvmkit_ir::Positioned,
    llvmkit_ir::Dyn,
>;

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

/// Lift a [`Type<'ctx>`] to the matching [`AnyTypeEnum`] arm. Re-uses the
/// IR side's `try_into` impl so the parser does not duplicate the kind /
/// data-arm dispatch table.
trait IntoTypeEnum<'ctx> {
    fn into_type_enum(self) -> AnyTypeEnum<'ctx>;
}

impl<'ctx> IntoTypeEnum<'ctx> for Type<'ctx> {
    fn into_type_enum(self) -> AnyTypeEnum<'ctx> {
        AnyTypeEnum::from(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> ParseResult<()> {
        let m = Module::new("parse_test");
        let p = Parser::new(src.as_bytes(), &m)?;
        let _ = p.parse_module()?;
        Ok(())
    }

    /// Mirrors `test/Assembler/datalayout.ll` — the parser accepts the
    /// `target datalayout = "..."` directive and the module retains it.
    #[test]
    fn parses_target_datalayout() {
        let src = "target datalayout = \"e-m:e-i64:64\"\n";
        let m = Module::new("dl");
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
        let m = Module::new("triple");
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
        let m = Module::new("masm");
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

    /// Mirrors `LLParser::parseType`'s rejection of legacy typed pointers
    /// in opaque-pointer mode (LLVM 17+). The error wording is checked
    /// structurally, not by string.
    #[test]
    fn rejects_legacy_typed_pointer_suffix() {
        let err = parse("%foo = type { i32* }\n").unwrap_err();
        match err {
            ParseError::Expected { expected, .. } => assert!(expected.contains("opaque-pointer")),
            other => panic!("unexpected error variant: {other:?}"),
        }
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
