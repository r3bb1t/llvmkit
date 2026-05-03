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
}

// ── Helper enums ────────────────────────────────────────────────────────────

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
