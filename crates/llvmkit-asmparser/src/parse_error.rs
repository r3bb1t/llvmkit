//! Typed parser-error catalogue.
//!
//! Mirrors the diagnostic strings emitted by `LLParser::error` /
//! `LLParser::tokError` in `llvm/lib/AsmParser/LLParser.cpp`. Upstream uses
//! free-form `Twine` strings; we keep the same wording for the subset we
//! ship today but route every variant through structured fields so callers
//! can match on the failure mode without string comparison.
//!
//! The catalogue is intentionally narrow for now — only the
//! variants that the substrate (lexer pass-through, slot-table integrity,
//! forward-reference resolution, location registry) and the immediate
//! follow-on parser work will populate. Later revisions grow the enum as
//! they add real `parse*` arms; the variants ship now so the parser does
//! not have to relitigate the public error shape later.

use std::borrow::Cow;

use llvmkit_support::Span;

use crate::ll_lexer::LexError;
use crate::numbered_values::AddError as SlotAddError;

/// Top-level entity kind — distinguishes the namespaces tracked by the
/// parser when it reports symbol errors. Mirrors the four
/// `ForwardRefVals` / `ForwardRefBlocks` / `ForwardRefMDNodes` /
/// `NumberedTypes` tables in `LLParser`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum SymbolKind {
    /// `@name` — function or global variable, as a *definition*.
    Global,
    /// `@name` — the same namespace, as an unsatisfied *use*.
    ///
    /// Upstream words the two sides differently and llvmkit reproduces the
    /// split rather than smoothing it: `checkValueID` and the redefinition
    /// sites are handed the noun `"global"`, while `validateEndOfModule`'s
    /// leftover sweep hard-codes `"use of undefined value '@" + Name`. Same
    /// sigil, different noun.
    GlobalValue,
    /// `%name` — function-local SSA value or argument.
    Local,
    /// `%name` at the type position — named or numbered struct type.
    Type,
    /// `!name` — metadata node.
    Metadata,
    /// `$name` — comdat.
    Comdat,
    /// `#name` — attribute group.
    AttrGroup,
}

impl SymbolKind {
    /// The `.ll` sigil that introduces an identifier in this namespace.
    ///
    /// Diagnostics pair it with a [`SymbolId`], which carries the bare
    /// identity. That split is upstream's own:
    /// `LLParser::checkValueID(Loc, Kind, Prefix, ...)` takes the noun and
    /// the prefix as separate arguments and glues them together per
    /// message, which is why `"redefinition of global '@" + Name + "'"`
    /// spells the `@` beside the word `global` and not inside the name.
    #[inline]
    pub const fn sigil(self) -> char {
        match self {
            SymbolKind::Global | SymbolKind::GlobalValue => '@',
            SymbolKind::Local | SymbolKind::Type => '%',
            SymbolKind::Comdat => '$',
            SymbolKind::Metadata => '!',
            SymbolKind::AttrGroup => '#',
        }
    }
}

/// The noun `LLParser.cpp` uses for this namespace in its diagnostics.
///
/// Taken from the `Kind` arguments upstream passes to
/// `LLParser::checkValueID` (`"global"`, `"label"`) and from the fixed
/// phrases around them: `"use of undefined value '%x'"`
/// (`LLParser::PerFunctionState::finishFunction`) is why the `%`-namespace
/// is `value` rather than `local`, and `"use of undefined metadata '!0'"`
/// (`LLParser::validateEndOfModule`) fixes the metadata spelling.
impl core::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            SymbolKind::Global => "global",
            SymbolKind::GlobalValue => "value",
            SymbolKind::Local => "value",
            SymbolKind::Type => "type",
            SymbolKind::Comdat => "comdat",
            SymbolKind::Metadata => "metadata",
            SymbolKind::AttrGroup => "attribute group",
        })
    }
}

/// Symbol identity: either an explicit name (`foo`, `bar`) or a slot
/// number (`0`, `5`) — in both cases the bare identity, without the
/// namespace's sigil.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum SymbolId {
    Named(String),
    Numbered(u32),
}

/// The bare identity, with no sigil: the namespace supplies that through
/// [`SymbolKind::sigil`], which is the only thing that knows whether `0`
/// means `%0`, `!0`, or `#0`.
impl core::fmt::Display for SymbolId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SymbolId::Named(n) => f.write_str(n),
            SymbolId::Numbered(n) => write!(f, "{n}"),
        }
    }
}

/// Top-level parser error.
///
/// Variants are added phase-by-phase as new parser arms come online.
/// Wording matches `LLParser.cpp` for the cases shipped today; structured
/// fields let callers match without inspecting the rendered string.
///
/// No message embeds its location: a location is data for a renderer to
/// place, not prose, and every variant that has one hands it over through
/// [`ParseError::loc`]. Upstream is the same shape — `LLParser::error`
/// carries the `LocTy` beside the `Twine`, and `SMDiagnostic` decides how
/// to print it.
#[derive(Clone, PartialEq, Eq, Hash, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// The lexer rejected the next token.
    #[error(transparent)]
    Lex(#[from] LexError),

    /// `LLParser::error` site for "expected X" / "expected Y" diagnostics.
    /// `expected` carries the human-readable description LLParser would
    /// pass to `tokError`. Almost every site names a fixed grammar
    /// production, so the description is a [`Cow`] and the common case
    /// borrows a `&'static str` instead of allocating per diagnostic.
    #[error("expected {expected}")]
    Expected {
        expected: Cow<'static, str>,
        loc: Span,
    },

    /// A diagnostic whose wording is *not* of the `expected <production>`
    /// shape — rendered verbatim, with nothing prepended.
    ///
    /// [`ParseError::Expected`] exists because the overwhelming majority of
    /// upstream's messages start with `expected `, and storing the bare
    /// production keeps those sites short. But `LLParser` also emits ~100
    /// diagnostics that are ordinary prose — `udiv constexprs are no longer
    /// supported`, `constant ptrauth base pointer must be a pointer`,
    /// `alignment is not a power of two`. Routing those through `Expected`
    /// rendered them as `expected udiv constexprs are no longer supported`,
    /// which is not upstream's text.
    ///
    /// This variant is the counterpart of `LLParser::error` /
    /// `LLParser::tokError` for exactly that set: whatever the parser stores
    /// is what a reader sees. Choose between the two by asking what upstream
    /// prints — if the message begins with `expected `, store the remainder
    /// in `Expected`; otherwise store the whole sentence here.
    #[error("{message}")]
    Message {
        message: Cow<'static, str>,
        loc: Span,
    },

    /// `redefinition of <symbol>` — mirrors `LLParser::checkValueID` and
    /// the `"redefinition of "` diagnostic site in `LLParser.cpp`.
    #[error("redefinition of {kind} '{sigil}{id}'", sigil = .kind.sigil())]
    Redefinition {
        kind: SymbolKind,
        id: SymbolId,
        loc: Span,
    },

    /// `use of undefined ...` — mirrors the `"use of undefined "`
    /// diagnostics that `LLParser` emits when a module-level forward
    /// reference is never satisfied. Carries the reference's first-seen
    /// location so renderers can point at the use site.
    #[error("use of undefined {kind} '{sigil}{id}'", sigil = .kind.sigil())]
    UndefinedSymbol {
        kind: SymbolKind,
        id: SymbolId,
        loc: Span,
    },

    /// `'%x' defined with type 'T' but expected 'U'` — mirrors the
    /// non-label arm of `LLParser::checkValidVariableType`, reached when a
    /// name already bound in the function — or minted as a forward reference
    /// at an earlier use — is referenced at a different type. `name` carries
    /// the sigil, because upstream glues it on before the quotes.
    #[error("'{name}' defined with type '{defined}' but expected '{expected}'")]
    DefinedWithWrongType {
        name: String,
        defined: String,
        expected: String,
        loc: Span,
    },

    /// `'%x' is not a basic block` — the label arm of
    /// `LLParser::checkValidVariableType`: a `label` operand named something
    /// that is bound to an ordinary value.
    #[error("'{name}' is not a basic block")]
    NotABasicBlock { name: String, loc: Span },

    /// `instruction forward referenced with type '<T>'` — mirrors
    /// `LLParser::PerFunctionState::setInstName`, where the definition of a
    /// name disagrees with the type its earlier forward reference demanded.
    /// The type named is the *forward reference's*, as upstream spells it.
    #[error("instruction forward referenced with type '{ty}'")]
    InstructionForwardReferencedWithType { ty: String, loc: Span },

    /// `slot mapping rejected slot id` — wraps a [`SlotAddError`] from
    /// [`crate::numbered_values::NumberedValues::add`]. Mirrors the
    /// `assert(ID >= NextUnusedID)` site that `LLParser` triggers when a
    /// `.ll` file uses a non-monotonic slot id.
    #[error("invalid slot id: {source}")]
    InvalidSlotId {
        #[source]
        source: SlotAddError,
        loc: Span,
    },

    /// `iN` for `N` outside `[MIN_INT_BITS, MAX_INT_BITS]`.
    ///
    /// Upstream rejects this in `LLLexer::LexIdentifier`, whose wording this
    /// reproduces; `width` and `max` remain as structured fields for callers
    /// that want the numbers, since the rendered text names neither.
    #[error("bitwidth for integer type out of range")]
    IntegerWidthOutOfRange { width: u64, max: u32, loc: Span },

    /// A specialized `DI*` node named a field its class does not declare.
    /// Mirrors the fall-through arm of `LLParser`'s `PARSE_MD_FIELDS` macro
    /// (`LLParser.cpp`), which reports `invalid field '...'` once every
    /// `PARSE_MD_FIELD` in the class's `VISIT_MD_FIELDS` block has failed to
    /// match. The accepted set is
    /// [`llvmkit_ir::metadata::SpecializedMetadataKind::declared_fields`].
    #[error("invalid field '{field}'")]
    InvalidMetadataField {
        kind: &'static str,
        field: String,
        loc: Span,
    },

    /// A specialized `DI*` node repeated a field. Mirrors
    /// `LLParser::parseMDField`'s `Result.Seen` guard (`LLParser.cpp`).
    #[error("field '{field}' cannot be specified more than once")]
    DuplicateMetadataField {
        kind: &'static str,
        field: String,
        loc: Span,
    },

    /// A specialized `DI*` node omitted a field its class declares `REQUIRED`.
    /// Mirrors the `REQUIRE_FIELD` expansion in `LLParser`'s `PARSE_MD_FIELDS`
    /// macro (`LLParser.cpp`), which — like this — reports against the closing
    /// `)` rather than the node's opening token. The required set is
    /// [`llvmkit_ir::metadata::SpecializedMetadataKind::required_fields`].
    #[error("missing required field '{field}'")]
    MissingRequiredMetadataField {
        kind: &'static str,
        field: &'static str,
        loc: Span,
    },

    /// A `DW_*` / `DIFlag*` / kind keyword that its family's table does not
    /// contain. `what` is upstream's own wording for the family, so the
    /// rendered message matches `LLParser::parseMDField`'s byte for byte —
    /// `invalid DWARF tag 'x'`, `invalid debug info flag 'x'`,
    /// `invalid checksum kind 'x'`, and the eleven siblings.
    #[error("invalid {what} '{value}'")]
    InvalidMetadataFieldValue {
        what: &'static str,
        value: String,
        loc: Span,
    },

    /// An unsigned metadata field over its declared maximum. Mirrors
    /// `LLParser::parseMDField(MDUnsignedField&)`; the limit is the one the
    /// field's type carries (`LineField` is `UINT32_MAX`, `ColumnField`
    /// `UINT16_MAX`, and a bare `MDUnsignedField` may narrow further).
    #[error("value for '{field}' too large, limit is {limit}")]
    MetadataFieldValueTooLarge {
        field: String,
        limit: u64,
        loc: Span,
    },

    /// A signed metadata field under its declared minimum. Mirrors
    /// `LLParser::parseMDField(MDSignedField&)`.
    #[error("value for '{field}' too small, limit is {limit}")]
    MetadataFieldValueTooSmall {
        field: String,
        limit: i64,
        loc: Span,
    },

    /// `null` given for an `MDField` upstream declares `(/* AllowNull */
    /// false)`.
    #[error("'{field}' cannot be null")]
    MetadataFieldCannotBeNull { field: String, loc: Span },

    /// `""` given for an `MDStringField` upstream declares
    /// `EmptyIs::Error`.
    #[error("'{field}' cannot be empty")]
    MetadataFieldCannotBeEmpty { field: String, loc: Span },
}

impl ParseError {
    /// The diagnostic location to highlight.
    ///
    /// Every `ParseError` is a diagnostic about a token, so this is total —
    /// the `Option` it used to return existed only for the variants (`Io`,
    /// `BrandInUse`, `BrandRetired`) that were not diagnostics at all, and
    /// all three are gone. Doctrine D1: a two-state value does not carry its
    /// state in a predicate.
    pub fn loc(&self) -> Span {
        match self {
            ParseError::Lex(e) => e.span(),
            ParseError::Expected { loc, .. }
            | ParseError::Message { loc, .. }
            | ParseError::Redefinition { loc, .. }
            | ParseError::UndefinedSymbol { loc, .. }
            | ParseError::DefinedWithWrongType { loc, .. }
            | ParseError::NotABasicBlock { loc, .. }
            | ParseError::InstructionForwardReferencedWithType { loc, .. }
            | ParseError::InvalidSlotId { loc, .. }
            | ParseError::IntegerWidthOutOfRange { loc, .. }
            | ParseError::InvalidMetadataField { loc, .. }
            | ParseError::DuplicateMetadataField { loc, .. }
            | ParseError::MissingRequiredMetadataField { loc, .. }
            | ParseError::InvalidMetadataFieldValue { loc, .. }
            | ParseError::MetadataFieldValueTooLarge { loc, .. }
            | ParseError::MetadataFieldValueTooSmall { loc, .. }
            | ParseError::MetadataFieldCannotBeNull { loc, .. }
            | ParseError::MetadataFieldCannotBeEmpty { loc, .. } => *loc,
        }
    }
}

/// `Result` alias parameterised on [`ParseError`].
pub type ParseResult<T> = Result<T, ParseError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Ports the wording used by `LLParser::tokError("expected ...")` in
    /// `LLParser.cpp`. The Rust analogue routes the message through a
    /// structured field so tests anchor on the data, not a free-form
    /// string match.
    #[test]
    fn expected_carries_location() {
        let span = Span::new(5, 9);
        let err = ParseError::Expected {
            expected: "type".into(),
            loc: span,
        };
        let loc = err.loc();
        assert_eq!(loc, span);
    }

    /// Ports the `redefinition of ...` diagnostic family from
    /// `LLParser.cpp`.
    ///
    /// This asserts the structured fields *and* the rendered text. It used to
    /// assert only the fields, on the reasoning that wording should stay
    /// flexible — but upstream's wording is contractual (every
    /// `test/Assembler` negative pins it with a `FileCheck` line), and
    /// field-only assertions are precisely what hid `ParseError::Expected`
    /// prepending `expected ` to messages upstream prints bare.
    #[test]
    fn redefinition_records_symbol() {
        let err = ParseError::Redefinition {
            kind: SymbolKind::Global,
            id: SymbolId::Named("foo".into()),
            loc: Span::new(0, 4),
        };
        if let ParseError::Redefinition { kind, id, .. } = &err {
            assert_eq!(*kind, SymbolKind::Global);
            assert_eq!(*id, SymbolId::Named("foo".into()));
        } else {
            panic!("wrong variant");
        }
        assert_eq!(err.to_string(), "redefinition of global '@foo'");
    }

    /// Mirrors the exact diagnostic `LLParser::parseNamedGlobal` emits —
    /// `error(NameLoc, "redefinition of global '@" + Name + "'")` in
    /// `LLParser.cpp` — and its `"use of undefined "` sibling in
    /// `LLParser::validateEndOfModule`. The sigil comes from the namespace
    /// ([`SymbolKind::sigil`]), so a numbered metadata slot renders `!0` and
    /// not `%0`; the location stays out of the prose, since upstream also
    /// carries its `LocTy` beside the message rather than inside it.
    #[test]
    fn diagnostics_match_upstream_wording() {
        let redefinition = ParseError::Redefinition {
            kind: SymbolKind::Global,
            id: SymbolId::Named("foo".into()),
            loc: Span::new(0, 4),
        };
        assert_eq!(redefinition.to_string(), "redefinition of global '@foo'");

        let undefined = ParseError::UndefinedSymbol {
            kind: SymbolKind::Metadata,
            id: SymbolId::Numbered(0),
            loc: Span::new(0, 2),
        };
        assert_eq!(undefined.to_string(), "use of undefined metadata '!0'");

        let undefined_local = ParseError::UndefinedSymbol {
            kind: SymbolKind::Local,
            id: SymbolId::Named("x".into()),
            loc: Span::new(0, 2),
        };
        assert_eq!(undefined_local.to_string(), "use of undefined value '%x'");

        let expected = ParseError::Expected {
            expected: "type".into(),
            loc: Span::new(5, 9),
        };
        assert_eq!(expected.to_string(), "expected type");
    }

    /// llvmkit-specific (**no upstream counterpart**): lexer errors flow
    /// through [`ParseError::Lex`] without re-encoding. Closest upstream
    /// anchor: `LLLexer::LexError` recording at `ErrorPriority::Lexer`, which
    /// outranks the parser's own message for exactly this set of failures.
    #[test]
    fn lex_error_passes_through() {
        let lex = LexError::UnterminatedString {
            span: Span::new(0, 4),
        };
        let err: ParseError = lex.clone().into();
        assert_eq!(err.loc(), lex.span());
        // The variant survives the conversion rather than being flattened to
        // a generic string, so a caller can still match on it.
        assert!(matches!(
            err,
            ParseError::Lex(LexError::UnterminatedString { .. })
        ));
        assert_eq!(format!("{err}"), "end of file in string constant");
    }

    /// Ports the out-of-range `iN` rejection, whose text upstream fixes in
    /// `LLLexer::LexIdentifier` (`test/Assembler/invalid-inttype.ll` pins it).
    ///
    /// The width and the limit stay reachable as fields — that is llvmkit's
    /// addition — but they are deliberately absent from the rendered text,
    /// because upstream's message names neither.
    #[test]
    fn integer_width_out_of_range_is_typed() {
        let err = ParseError::IntegerWidthOutOfRange {
            width: 1 << 30,
            max: llvmkit_ir::MAX_INT_BITS,
            loc: Span::new(2, 10),
        };
        assert_eq!(err.to_string(), "bitwidth for integer type out of range");
        assert!(matches!(
            err,
            ParseError::IntegerWidthOutOfRange { width, max, .. }
                if width == 1 << 30 && max == llvmkit_ir::MAX_INT_BITS
        ));
    }
}
