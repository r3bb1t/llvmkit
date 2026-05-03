//! Typed parser-error catalogue.
//!
//! Mirrors the diagnostic strings emitted by `LLParser::error` /
//! `LLParser::tokError` in `llvm/lib/AsmParser/LLParser.cpp`. Upstream uses
//! free-form `Twine` strings; we keep the same wording for the subset we
//! ship today but route every variant through structured fields so callers
//! can match on the failure mode without string comparison.
//!
//! The catalogue is intentionally narrow in this session — only the
//! variants that the substrate (lexer pass-through, slot-table integrity,
//! forward-reference resolution, location registry) and the immediate
//! follow-on parser sessions will populate. Sessions 2-3 grow the enum as
//! they add real `parse*` arms; the variants ship now so the parser does
//! not have to relitigate the public error shape later.

use llvmkit_support::Span;

use crate::file_loc::FileLocRange;
use crate::ll_lexer::LexError;
use crate::numbered_values::AddError as SlotAddError;

/// Where in the source a diagnostic points. Carrying both the byte
/// [`Span`] (set by every parser arm) and the optional [`FileLocRange`]
/// projection (populated when the parser is configured to track line/col)
/// keeps low-level tooling and human-facing renderers happy without a
/// second walk over the source buffer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DiagLoc {
    pub span: Span,
    pub file: Option<FileLocRange>,
}

impl DiagLoc {
    /// Construct a diagnostic location pinned to a byte span only.
    #[inline]
    pub const fn span(span: Span) -> Self {
        Self { span, file: None }
    }

    /// Attach a [`FileLocRange`] projection to an existing diagnostic.
    #[inline]
    pub const fn with_file(self, file: FileLocRange) -> Self {
        Self {
            span: self.span,
            file: Some(file),
        }
    }
}

/// Top-level entity kind — distinguishes the namespaces tracked by the
/// parser when it reports symbol errors. Mirrors the four
/// `ForwardRefVals` / `ForwardRefBlocks` / `ForwardRefMDNodes` /
/// `NumberedTypes` tables in `LLParser`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum SymbolKind {
    /// `@name` — function or global variable.
    Global,
    /// `%name` — function-local SSA value or argument.
    Local,
    /// `%name` at the type position — named or numbered struct type.
    Type,
    /// `label %name` — basic block.
    Block,
    /// `!name` — metadata node.
    Metadata,
    /// `#name` — attribute group.
    AttrGroup,
}

/// Symbol identity: either an explicit name (`@foo`, `%bar`) or a slot
/// number (`%0`, `@5`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum SymbolId {
    Named(String),
    Numbered(u32),
}

impl core::fmt::Display for SymbolId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SymbolId::Named(n) => f.write_str(n),
            SymbolId::Numbered(n) => write!(f, "%{n}"),
        }
    }
}

/// Top-level parser error.
///
/// Variants are added phase-by-phase as new parser arms come online.
/// Wording matches `LLParser.cpp` for the cases shipped today; structured
/// fields let callers match without inspecting the rendered string.
#[derive(Clone, PartialEq, Eq, Hash, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// The lexer rejected the next token.
    #[error(transparent)]
    Lex(#[from] LexError),

    /// `LLParser::error` site for "expected X" / "expected Y" diagnostics.
    /// `expected` carries the human-readable description LLParser would
    /// pass to `tokError`.
    #[error("expected {expected} at {loc:?}")]
    Expected { expected: String, loc: DiagLoc },

    /// `redefinition of <symbol>` — mirrors `LLParser::checkValueID` and
    /// the `"redefinition of "` diagnostic site in `LLParser.cpp`.
    #[error("redefinition of {kind:?} '{id}'")]
    Redefinition {
        kind: SymbolKind,
        id: SymbolId,
        loc: DiagLoc,
    },

    /// `use of undefined ...` — mirrors the `"use of undefined "`
    /// diagnostics that `LLParser` emits when a module-level forward
    /// reference is never satisfied. Carries the reference's first-seen
    /// location so renderers can point at the use site.
    #[error("use of undefined {kind:?} '{id}'")]
    UndefinedSymbol {
        kind: SymbolKind,
        id: SymbolId,
        loc: DiagLoc,
    },

    /// `slot mapping rejected slot id` — wraps a [`SlotAddError`] from
    /// [`crate::numbered_values::NumberedValues::add`]. Mirrors the
    /// `assert(ID >= NextUnusedID)` site that `LLParser` triggers when a
    /// `.ll` file uses a non-monotonic slot id.
    #[error("invalid slot id at {loc:?}: {source}")]
    InvalidSlotId {
        #[source]
        source: SlotAddError,
        loc: DiagLoc,
    },

    /// `bitwidth for integer type out of range` — mirrors the upstream
    /// `LLParser::parseType` arm that rejects `iN` for `N` outside
    /// `[1, MAX_INT_BITS]`.
    #[error("integer width {width} out of range (1..={max})")]
    IntegerWidthOutOfRange { width: u64, max: u32, loc: DiagLoc },

    /// I/O failure pulling source bytes. The lexer itself does not perform
    /// I/O; this is for callers using
    /// [`crate::read_to_owned`]-style helpers. The wrapped string is the
    /// `Display` form of the underlying [`std::io::Error`]; we don't keep
    /// the [`std::io::Error`] itself because it lacks `Clone`/`Eq`/`Hash`,
    /// which the rest of [`ParseError`] derives.
    #[error("I/O error reading source: {0}")]
    Io(String),
}

impl From<std::io::Error> for ParseError {
    #[inline]
    fn from(e: std::io::Error) -> Self {
        ParseError::Io(e.to_string())
    }
}

impl ParseError {
    /// The diagnostic location to highlight, when the variant carries one.
    pub fn loc(&self) -> Option<DiagLoc> {
        match self {
            ParseError::Lex(e) => Some(DiagLoc::span(e.span())),
            ParseError::Expected { loc, .. }
            | ParseError::Redefinition { loc, .. }
            | ParseError::UndefinedSymbol { loc, .. }
            | ParseError::InvalidSlotId { loc, .. }
            | ParseError::IntegerWidthOutOfRange { loc, .. } => Some(*loc),
            ParseError::Io(_) => None,
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
            loc: DiagLoc::span(span),
        };
        let loc = err.loc().unwrap();
        assert_eq!(loc.span, span);
        assert!(loc.file.is_none());
    }

    /// Ports the `redefinition of ...` diagnostic family from
    /// `LLParser.cpp`. We assert structural identity, not string identity,
    /// to keep wording flexibility for follow-on sessions.
    #[test]
    fn redefinition_records_symbol() {
        let err = ParseError::Redefinition {
            kind: SymbolKind::Global,
            id: SymbolId::Named("@foo".into()),
            loc: DiagLoc::span(Span::new(0, 4)),
        };
        if let ParseError::Redefinition { kind, id, .. } = &err {
            assert_eq!(*kind, SymbolKind::Global);
            assert_eq!(*id, SymbolId::Named("@foo".into()));
        } else {
            panic!("wrong variant");
        }
    }

    /// llvmkit-specific: lexer errors flow through [`ParseError::Lex`]
    /// without re-encoding. Closest upstream anchor: `LLParser` calling
    /// `Lex.Error(...)` and propagating through `LLParser::error`.
    #[test]
    fn lex_error_passes_through() {
        let lex = LexError::UnknownToken {
            span: Span::new(0, 1),
        };
        let err: ParseError = lex.clone().into();
        assert_eq!(err.loc().map(|l| l.span), Some(lex.span()));
    }

    /// Ports the upstream `parseType`-arm rejection of out-of-range integer
    /// widths (`LLParser.cpp::parseType` checks against `MAX_INT_BITS`).
    #[test]
    fn integer_width_out_of_range_is_typed() {
        let err = ParseError::IntegerWidthOutOfRange {
            width: 1 << 30,
            max: (1 << 24) - 1,
            loc: DiagLoc::span(Span::new(2, 10)),
        };
        let rendered = format!("{err}");
        assert!(rendered.contains("integer width"));
        assert!(rendered.contains("out of range"));
    }
}
