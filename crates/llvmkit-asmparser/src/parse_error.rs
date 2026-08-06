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
            SymbolKind::Global => '@',
            SymbolKind::Local | SymbolKind::Type | SymbolKind::Block => '%',
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
            SymbolKind::Local => "value",
            SymbolKind::Type => "type",
            SymbolKind::Block => "label",
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
/// No message embeds its [`DiagLoc`]: a location is data for a renderer to
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
        loc: DiagLoc,
    },

    /// `redefinition of <symbol>` — mirrors `LLParser::checkValueID` and
    /// the `"redefinition of "` diagnostic site in `LLParser.cpp`.
    #[error("redefinition of {kind} '{sigil}{id}'", sigil = .kind.sigil())]
    Redefinition {
        kind: SymbolKind,
        id: SymbolId,
        loc: DiagLoc,
    },

    /// `use of undefined ...` — mirrors the `"use of undefined "`
    /// diagnostics that `LLParser` emits when a module-level forward
    /// reference is never satisfied. Carries the reference's first-seen
    /// location so renderers can point at the use site.
    #[error("use of undefined {kind} '{sigil}{id}'", sigil = .kind.sigil())]
    UndefinedSymbol {
        kind: SymbolKind,
        id: SymbolId,
        loc: DiagLoc,
    },

    /// `slot mapping rejected slot id` — wraps a [`SlotAddError`] from
    /// [`crate::numbered_values::NumberedValues::add`]. Mirrors the
    /// `assert(ID >= NextUnusedID)` site that `LLParser` triggers when a
    /// `.ll` file uses a non-monotonic slot id.
    #[error("invalid slot id: {source}")]
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
    /// I/O; this is for the file-reading entry points and callers using
    /// [`crate::read_to_owned`]-style helpers. `message` is the `Display`
    /// form of the underlying [`std::io::Error`]; we don't keep the
    /// [`std::io::Error`] itself because it lacks `Clone`/`Eq`/`Hash`,
    /// which the rest of [`ParseError`] derives. `kind` is kept beside it
    /// because [`std::io::ErrorKind`] *is* `Copy + Eq + Hash`, so a caller
    /// can still tell `NotFound` from `PermissionDenied` without parsing
    /// the message back.
    #[error("I/O error reading source: {message}")]
    Io {
        kind: std::io::ErrorKind,
        message: String,
    },

    /// A live module already holds the brand requested by
    /// [`crate::parse_branded`] / [`crate::parse_file_branded`]. Mirrors
    /// [`llvmkit_ir::IrError::BrandInUse`]; the registry-exempt
    /// [`crate::parse_dynamic`] entry points can never produce it.
    #[error("module brand `{brand}` is already held by a live module")]
    BrandInUse { brand: &'static str },

    /// The brand requested by [`crate::parse_branded`] /
    /// [`crate::parse_file_branded`] was permanently retired by a
    /// [`llvmkit_ir::Module::branded_once`] module. Mirrors
    /// [`llvmkit_ir::IrError::BrandRetired`].
    #[error("module brand `{brand}` was permanently retired")]
    BrandRetired { brand: &'static str },
}

impl From<std::io::Error> for ParseError {
    #[inline]
    fn from(e: std::io::Error) -> Self {
        ParseError::Io {
            kind: e.kind(),
            message: e.to_string(),
        }
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
            ParseError::Io { .. }
            | ParseError::BrandInUse { .. }
            | ParseError::BrandRetired { .. } => None,
        }
    }
}

/// `Result` alias parameterised on [`ParseError`].
pub type ParseResult<T> = Result<T, ParseError>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ll_lexer::UnknownTokenReason;

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
    /// to keep wording flexibility for later revisions.
    #[test]
    fn redefinition_records_symbol() {
        let err = ParseError::Redefinition {
            kind: SymbolKind::Global,
            id: SymbolId::Named("foo".into()),
            loc: DiagLoc::span(Span::new(0, 4)),
        };
        if let ParseError::Redefinition { kind, id, .. } = &err {
            assert_eq!(*kind, SymbolKind::Global);
            assert_eq!(*id, SymbolId::Named("foo".into()));
        } else {
            panic!("wrong variant");
        }
    }

    /// Mirrors the exact diagnostic `LLParser::parseNamedGlobal` emits —
    /// `error(NameLoc, "redefinition of global '@" + Name + "'")` in
    /// `LLParser.cpp` — and its `"use of undefined "` sibling in
    /// `LLParser::validateEndOfModule`. The sigil comes from the namespace
    /// ([`SymbolKind::sigil`]), so a numbered metadata slot renders `!0` and
    /// not `%0`; the [`DiagLoc`] stays out of the prose, since upstream also
    /// carries its `LocTy` beside the message rather than inside it.
    #[test]
    fn diagnostics_match_upstream_wording() {
        let redefinition = ParseError::Redefinition {
            kind: SymbolKind::Global,
            id: SymbolId::Named("foo".into()),
            loc: DiagLoc::span(Span::new(0, 4)),
        };
        assert_eq!(redefinition.to_string(), "redefinition of global '@foo'");

        let undefined = ParseError::UndefinedSymbol {
            kind: SymbolKind::Metadata,
            id: SymbolId::Numbered(0),
            loc: DiagLoc::span(Span::new(0, 2)),
        };
        assert_eq!(undefined.to_string(), "use of undefined metadata '!0'");

        let undefined_local = ParseError::UndefinedSymbol {
            kind: SymbolKind::Local,
            id: SymbolId::Named("x".into()),
            loc: DiagLoc::span(Span::new(0, 2)),
        };
        assert_eq!(undefined_local.to_string(), "use of undefined value '%x'");

        let expected = ParseError::Expected {
            expected: "type".into(),
            loc: DiagLoc::span(Span::new(5, 9)),
        };
        assert_eq!(expected.to_string(), "expected type");
    }

    /// llvmkit-specific (no upstream counterpart: `llvm::SMDiagnostic` keeps
    /// no `std::error_code`): an I/O failure keeps the
    /// [`std::io::ErrorKind`] beside its message, so `NotFound` stays
    /// matchable without parsing the rendered string back.
    #[test]
    fn io_errors_keep_their_kind() {
        let err: ParseError =
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such file").into();
        match &err {
            ParseError::Io { kind, message } => {
                assert_eq!(*kind, std::io::ErrorKind::NotFound);
                assert_eq!(message, "no such file");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert_eq!(err.to_string(), "I/O error reading source: no such file");
        assert_eq!(err.loc(), None);
    }

    /// llvmkit-specific: lexer errors flow through [`ParseError::Lex`]
    /// without re-encoding. Closest upstream anchor: `LLParser` calling
    /// `Lex.Error(...)` and propagating through `LLParser::error`.
    #[test]
    fn lex_error_passes_through() {
        let lex = LexError::UnknownToken {
            reason: UnknownTokenReason::StrayByte { byte: b'?' },
            span: Span::new(0, 1),
        };
        let err: ParseError = lex.clone().into();
        assert_eq!(err.loc().map(|l| l.span), Some(lex.span()));
        // The reason survives the conversion rather than being flattened to a
        // generic string, so a caller can still match on it.
        assert!(format!("{err}").contains("no token starts with '?'"));
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
