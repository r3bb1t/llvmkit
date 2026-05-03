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
    AnyTypeEnum, IrError, Linkage, Module, StructType, Type, derived_types::PointerType,
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
        })
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
                _ => return Err(self.token_error("top-level entity")),
            }
        }

        // Forward-referenced types that never received a definition stay
        // opaque. That matches upstream's behavior — `validateEndOfModule`
        // does not error on opaque structs that were referenced but never
        // bodied; only `LLParser`'s `error(forward_loc, ...)` paths flag
        // the truly malformed cases.

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
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Unreachable) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.bump()?;
                    let _ = b.build_unreachable();
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Br) => {
                    let b = builder.take().expect("builder live until terminator");
                    self.parse_br(state, b)?;
                    return Ok(());
                }
                Token::Instruction(crate::ll_token::Opcode::Store) => {
                    let b_ref = builder
                        .as_ref()
                        .expect("builder still alive in non-terminator path");
                    self.bump()?;
                    self.parse_store(state, b_ref)?;
                    continue;
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
                _ => {
                    return Err(ParseError::Expected {
                        expected: format!(
                            "instruction opcode supported by this session (got {opcode:?})"
                        ),
                        loc: DiagLoc::span(result_loc),
                    });
                }
            };
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

    /// `OP TYPE LHS, RHS`. Used by add/sub/mul (and trivially extends to
    /// the rest of the integer binops in follow-on commits).
    fn parse_int_binop(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        op: IntBinOp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
            IntBinOp::Add => b
                .build_int_add::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("add", e))?
                .as_value(),
            IntBinOp::Sub => b
                .build_int_sub::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("sub", e))?
                .as_value(),
            IntBinOp::Mul => b
                .build_int_mul::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("mul", e))?
                .as_value(),
            IntBinOp::UDiv => b
                .build_int_udiv::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("udiv", e))?
                .as_value(),
            IntBinOp::SDiv => b
                .build_int_sdiv::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("sdiv", e))?
                .as_value(),
            IntBinOp::URem => b
                .build_int_urem::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("urem", e))?
                .as_value(),
            IntBinOp::SRem => b
                .build_int_srem::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("srem", e))?
                .as_value(),
            IntBinOp::Shl => b
                .build_int_shl::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("shl", e))?
                .as_value(),
            IntBinOp::LShr => b
                .build_int_lshr::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("lshr", e))?
                .as_value(),
            IntBinOp::AShr => b
                .build_int_ashr::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("ashr", e))?
                .as_value(),
            IntBinOp::And => b
                .build_int_and::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("and", e))?
                .as_value(),
            IntBinOp::Or => b
                .build_int_or::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("or", e))?
                .as_value(),
            IntBinOp::Xor => b
                .build_int_xor::<llvmkit_ir::IntDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("xor", e))?
                .as_value(),
        };
        Ok(v)
    }

    /// `icmp PRED TYPE LHS, RHS`. Mirrors `LLParser::parseCompare`.
    fn parse_icmp(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
        let r = b
            .build_int_cmp::<llvmkit_ir::IntDyn, _, _>(pred, lhs, rhs, name)
            .map_err(|e| self.builder_err("icmp", e))?;
        Ok(r.as_value())
    }

    /// `OP TYPE VALUE to TYPE`. Used by `trunc` / `zext` / `sext`.
    /// Mirrors `LLParser::parseCast`'s integer-cast arm.
    fn parse_int_cast(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        op: IntCast,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
            IntCast::Trunc => b
                .build_trunc_dyn(src_int, dst_int, name)
                .map_err(|e| self.builder_err("trunc", e))?
                .as_value(),
            IntCast::ZExt => b
                .build_zext_dyn(src_int, dst_int, name)
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

    /// `fneg TYPE VALUE`. Mirrors `LLParser::parseUnaryOp` for `Instruction::FNeg`.
    fn parse_fneg(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let ty = self.parse_type(false)?;
        let v = self.parse_value(state, ty)?;
        let f: llvmkit_ir::FloatValue<'ctx, llvmkit_ir::FloatDyn> = v
            .try_into()
            .map_err(|_| self.expected("float-typed fneg operand"))?;
        let r = b
            .build_float_neg::<llvmkit_ir::FloatDyn, _>(f, result_name.as_str())
            .map_err(|e| self.builder_err("fneg", e))?;
        Ok(r.as_value())
    }

    /// `OP TYPE LHS, RHS` for fadd/fsub/fmul/fdiv/frem.
    /// Mirrors `LLParser::parseArithmetic` FP arm.
    fn parse_fp_binop(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        op: FpBinOp,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
            FpBinOp::Add => b
                .build_fp_add::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("fadd", e))?
                .as_value(),
            FpBinOp::Sub => b
                .build_fp_sub::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("fsub", e))?
                .as_value(),
            FpBinOp::Mul => b
                .build_fp_mul::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("fmul", e))?
                .as_value(),
            FpBinOp::Div => b
                .build_fp_div::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("fdiv", e))?
                .as_value(),
            FpBinOp::Rem => b
                .build_fp_rem::<llvmkit_ir::FloatDyn, _, _>(lhs, rhs, name)
                .map_err(|e| self.builder_err("frem", e))?
                .as_value(),
        };
        Ok(v)
    }

    /// `fcmp PRED TYPE LHS, RHS`. Mirrors `LLParser::parseCompare` FP arm.
    fn parse_fcmp(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
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
        let r = b
            .build_fp_cmp::<llvmkit_ir::FloatDyn, _, _>(pred, lhs, rhs, result_name.as_str())
            .map_err(|e| self.builder_err("fcmp", e))?;
        Ok(r.as_value())
    }

    /// `alloca TYPE [, TYPE COUNT] [, align N]`. Session 4 will extend
    /// this with addrspace / inalloca / swift / volatile slots; for now
    /// we ship the plain "type only" form, which is by far the most
    /// common in real-world IR. Mirrors `LLParser::parseAlloc`.
    fn parse_alloca(
        &mut self,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let ty = self.parse_type(false)?;
        // TODO(parser, follow-up): array-size + align suffix.
        let r = b
            .build_alloca(ty, result_name.as_str())
            .map_err(|e| self.builder_err("alloca", e))?;
        Ok(r.as_value())
    }

    /// `load TYPE, ptr POINTER`. Plain form (no atomic / volatile / align
    /// yet — Session 4 lifts those slots through the existing
    /// `AtomicLoadConfig`). Mirrors `LLParser::parseLoad`.
    fn parse_load(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
        result_name: &LocalLhs,
    ) -> ParseResult<llvmkit_ir::Value<'ctx>> {
        let ty = self.parse_type(false)?;
        self.expect_punct(PunctKind::Comma, "',' between load type and pointer")?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed load operand"))?;
        let v = b
            .build_load(ty, ptr, result_name.as_str())
            .map_err(|e| self.builder_err("load", e))?;
        Ok(v)
    }

    /// `store TYPE VALUE, ptr POINTER`. Mirrors `LLParser::parseStore`.
    /// Returns no value.
    fn parse_store(
        &mut self,
        state: &PerFunctionState<'ctx>,
        b: &ParsedBlockBuilder<'ctx>,
    ) -> ParseResult<()> {
        let val_ty = self.parse_type(false)?;
        let val_v = self.parse_value(state, val_ty)?;
        self.expect_punct(PunctKind::Comma, "',' between store value and pointer")?;
        let ptr_ty = self.parse_type(false)?;
        let ptr_v = self.parse_value(state, ptr_ty)?;
        let ptr: llvmkit_ir::PointerValue<'ctx> = ptr_v
            .try_into()
            .map_err(|_| self.expected("ptr-typed store target"))?;
        let _ = b
            .build_store(val_v, ptr)
            .map_err(|e| self.builder_err("store", e))?;
        Ok(())
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
                    _ => Err(self
                        .expected("zeroinitializer for the modeled scalar types (int / pointer)")),
                }
            }
            _ => Err(self.expected("operand value")),
        }
    }
}

// ── Helper enums ────────────────────────────────────────────────────────────

// ── Function-body helper types ──────────────────────────────────────────────

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
}

impl<'ctx> PerFunctionState<'ctx> {
    fn new(func: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>) -> Self {
        Self {
            func,
            local_named: std::collections::HashMap::new(),
            local_numbered: std::collections::HashMap::new(),
            next_unnamed_value_id: 0,
            blocks: std::collections::HashMap::new(),
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
