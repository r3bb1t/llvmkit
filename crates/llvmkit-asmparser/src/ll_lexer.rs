//! `.ll` lexer — direct port of LLVM's textual-IR lexer.
//!
//! Mirrors the union of
//! `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/AsmParser/LLLexer.h`
//! and `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLLexer.cpp`.
//! Token kinds live alongside in [`crate::ll_token`].
//!
//! The lexer borrows from a pre-loaded byte slice and yields
//! [`Spanned<Token>`] values. Numeric semantics (APInt/APFloat decoding) and
//! identifier-table lookups belong to the parser layer.

use llvmkit_support::{Span, Spanned};

use super::ll_token::{
    FpLit, HexFpKind, IntLit, Keyword, NumBase, PrimitiveTy, QuotedNameKind, Sign, Token,
};

// ─── Implementation-detail submodules ─────────────────────────────────────────
//
// These mirror static helpers defined inside `LLLexer.cpp`; they live in
// `src/ll_lexer/` so this file stays focused on the state machine while still
// presenting as a single conceptual module to outside crates.

mod escape;
mod keywords;

// ─── Public constants ─────────────────────────────────────────────────────────

/// Inclusive minimum width of an `iN` integer type. Mirrors
/// `llvm::IntegerType::MIN_INT_BITS`.
pub const INT_TY_MIN_BITS: u64 = 1;

/// Inclusive maximum width of an `iN` integer type. Mirrors
/// `llvm::IntegerType::MAX_INT_BITS`.
pub const INT_TY_MAX_BITS: u64 = (1u64 << 24) - 1; // 16_777_215

// ─── LexError ─────────────────────────────────────────────────────────────────
//
// The lexer does no I/O — it walks a pre-loaded slice — so I/O failures live
// one layer up. Every variant here is a structural lexing failure carrying the
// source [`Span`] of the offending lexeme.

/// All lexer-level failures.
#[derive(Clone, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum LexError {
    #[error("unterminated /* */ block comment")]
    UnterminatedBlockComment { span: Span },

    #[error("end of file in string constant")]
    UnterminatedString { span: Span },

    #[error("end of file in {kind:?} name")]
    UnterminatedQuotedName { kind: QuotedNameKind, span: Span },

    #[error("NUL character is not allowed in names")]
    NulInName { span: Span },

    #[error("invalid value number (does not fit in u32)")]
    IdOverflow { span: Span },

    #[error("constant bigger than 64 bits detected")]
    IntegerOverflow64 { span: Span },

    #[error("constant bigger than 128 bits detected")]
    IntegerOverflow128 { span: Span },

    #[error("bitwidth for integer type out of range (got {width}, must be 1..={max})")]
    IntegerWidthOutOfRange { width: u64, max: u32, span: Span },

    #[error("hexadecimal constant too large for {target:?} (16-bit)")]
    HexFpTooLarge { target: HexFpKind, span: Span },

    #[error("invalid token")]
    UnknownToken { span: Span },

    #[error("expected '*' after '/' to start block comment")]
    StraySlash { span: Span },
}

impl LexError {
    /// The span the diagnostic should highlight. Useful for callers that want
    /// to render `(line, column)` without case-matching every variant.
    pub fn span(&self) -> Span {
        match self {
            LexError::UnterminatedBlockComment { span }
            | LexError::UnterminatedString { span }
            | LexError::UnterminatedQuotedName { span, .. }
            | LexError::NulInName { span }
            | LexError::IdOverflow { span }
            | LexError::IntegerOverflow64 { span }
            | LexError::IntegerOverflow128 { span }
            | LexError::IntegerWidthOutOfRange { span, .. }
            | LexError::HexFpTooLarge { span, .. }
            | LexError::UnknownToken { span }
            | LexError::StraySlash { span } => *span,
        }
    }
}

// ─── Lexer ────────────────────────────────────────────────────────────────────

/// A `.ll` lexer over a borrowed byte buffer.
///
/// `'src` is the lifetime of the underlying bytes. Tokens carrying string
/// payloads (`LabelStr`, `GlobalVar`, …) borrow from `'src` whenever escape
/// decoding is unnecessary; quoted forms with `\xx` escapes allocate.
///
/// `Clone` is provided to support cheap snapshot/lookahead patterns: a clone
/// re-uses the same backing slice and copies just the cursor state.
#[derive(Clone, Debug)]
pub struct Lexer<'src> {
    src: &'src [u8],
    pos: usize,
    tok_start: usize,
    prev_tok_end: usize,
    /// When `true`, a trailing `:` after an identifier is **not** consumed as
    /// a label terminator. Mirrors `LLLexer::IgnoreColonInIdentifiers`.
    pub ignore_colon_in_idents: bool,
}

impl<'src> Lexer<'src> {
    /// Construct a lexer over a byte slice.
    #[inline]
    pub fn new(src: &'src [u8]) -> Self {
        Self {
            src,
            pos: 0,
            tok_start: 0,
            prev_tok_end: 0,
            ignore_colon_in_idents: false,
        }
    }

    /// The full source buffer.
    #[inline]
    pub fn source(&self) -> &'src [u8] {
        self.src
    }

    /// Current byte offset.
    #[inline]
    pub fn position(&self) -> u32 {
        self.pos as u32
    }

    /// Exclusive end of the previous token. Mirrors `LLLexer::PrevTokEnd`.
    #[inline]
    pub fn prev_token_end(&self) -> u32 {
        self.prev_tok_end as u32
    }

    // ── Cursor primitives ────────────────────────────────────────────────────

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    #[inline]
    fn peek_at(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
    }

    #[inline]
    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    #[inline]
    fn span_from(&self, start: usize) -> Span {
        Span::new(start as u32, self.pos as u32)
    }

    #[inline]
    fn current_span(&self) -> Span {
        self.span_from(self.tok_start)
    }

    #[inline]
    fn spanned<T>(&self, value: T, span: Span) -> Spanned<T> {
        Spanned::new(value, span)
    }

    // ── Public driver ────────────────────────────────────────────────────────

    /// Produce the next token. EOF is returned as a real `Token::Eof`; calling
    /// `next_token` after EOF returns more `Eof` tokens (mirroring `LLLexer`'s
    /// "another call to lex will return EOF again" behavior at LLLexer.cpp:188).
    pub fn next_token(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        loop {
            // Per LLLexer::LexToken (LLLexer.cpp:194), PrevTokEnd is set at the
            // start of each *attempt* to lex a token, before whitespace/comment
            // skipping advances the cursor.
            self.prev_tok_end = self.pos;
            self.tok_start = self.pos;

            let Some(c) = self.peek() else {
                let span = Span::empty_at(self.pos as u32);
                return Ok(self.spanned(Token::Eof, span));
            };

            match c {
                // Whitespace + mid-buffer NUL (LLLexer.cpp:182).
                b' ' | b'\t' | b'\n' | b'\r' | 0 => {
                    self.bump();
                    continue;
                }
                // Line comment.
                b';' => {
                    self.skip_line_comment();
                    continue;
                }
                // Block comment.
                b'/' => {
                    self.bump();
                    self.skip_block_comment()?;
                    continue;
                }
                // Single-byte punctuation.
                b'=' => {
                    self.bump();
                    return Ok(self.spanned(Token::Equal, self.current_span()));
                }
                b',' => {
                    self.bump();
                    return Ok(self.spanned(Token::Comma, self.current_span()));
                }
                b'*' => {
                    self.bump();
                    return Ok(self.spanned(Token::Star, self.current_span()));
                }
                b'[' => {
                    self.bump();
                    return Ok(self.spanned(Token::LSquare, self.current_span()));
                }
                b']' => {
                    self.bump();
                    return Ok(self.spanned(Token::RSquare, self.current_span()));
                }
                b'{' => {
                    self.bump();
                    return Ok(self.spanned(Token::LBrace, self.current_span()));
                }
                b'}' => {
                    self.bump();
                    return Ok(self.spanned(Token::RBrace, self.current_span()));
                }
                b'<' => {
                    self.bump();
                    return Ok(self.spanned(Token::Less, self.current_span()));
                }
                b'>' => {
                    self.bump();
                    return Ok(self.spanned(Token::Greater, self.current_span()));
                }
                b'(' => {
                    self.bump();
                    return Ok(self.spanned(Token::LParen, self.current_span()));
                }
                b')' => {
                    self.bump();
                    return Ok(self.spanned(Token::RParen, self.current_span()));
                }
                b'|' => {
                    self.bump();
                    return Ok(self.spanned(Token::Bar, self.current_span()));
                }
                b':' => {
                    self.bump();
                    return Ok(self.spanned(Token::Colon, self.current_span()));
                }

                // Prefix tokens.
                b'+' => return self.lex_positive(),
                b'-' | b'0'..=b'9' => return self.lex_digit_or_negative(),
                b'@' => return self.lex_var(QuotedNameKind::Global),
                b'%' => return self.lex_var(QuotedNameKind::Local),
                b'$' => return self.lex_dollar(),
                b'!' => return self.lex_exclaim(),
                b'^' => return self.lex_caret(),
                b'#' => return self.lex_hash(),
                b'"' => return self.lex_quote(),
                b'.' => return self.lex_dot(),

                c if c.is_ascii_alphabetic() || c == b'_' => return self.lex_identifier(),

                _ => {
                    self.bump();
                    return Err(LexError::UnknownToken {
                        span: self.current_span(),
                    });
                }
            }
        }
    }

    // ── Comments ─────────────────────────────────────────────────────────────

    /// Skip from the current `;` through the end of the line. Mirrors
    /// `LLLexer::SkipLineComment` (LLLexer.cpp:265).
    fn skip_line_comment(&mut self) {
        while let Some(b) = self.peek() {
            if b == b'\n' || b == b'\r' {
                return;
            }
            self.bump();
        }
    }

    /// Skip the body of a `/* … */` comment (the leading `/` was already
    /// consumed). Mirrors `LLLexer::SkipCComment` (LLLexer.cpp:274).
    fn skip_block_comment(&mut self) -> Result<(), LexError> {
        // Expect '*' immediately after the '/'.
        match self.bump() {
            Some(b'*') => {}
            _ => {
                return Err(LexError::StraySlash {
                    span: self.span_from(self.tok_start),
                });
            }
        }
        loop {
            match self.bump() {
                None => {
                    return Err(LexError::UnterminatedBlockComment {
                        span: self.span_from(self.tok_start),
                    });
                }
                Some(b'*') => match self.peek() {
                    Some(b'/') => {
                        self.bump();
                        return Ok(());
                    }
                    Some(_) => continue,
                    None => {
                        return Err(LexError::UnterminatedBlockComment {
                            span: self.span_from(self.tok_start),
                        });
                    }
                },
                Some(_) => {}
            }
        }
    }

    // ── Identifier-prefix lexers ─────────────────────────────────────────────

    /// Lex `@…` (global) or `%…` (local) tokens.
    /// Mirrors `LLLexer::LexVar` (LLLexer.cpp:391).
    fn lex_var(&mut self, kind: QuotedNameKind) -> Result<Spanned<Token<'src>>, LexError> {
        self.bump(); // consume sigil

        // Quoted form: @"…"
        if self.peek() == Some(b'"') {
            return self.lex_quoted_name(kind);
        }

        // Unquoted name: @[-a-zA-Z$._][-a-zA-Z$._0-9]*
        let name_start = self.pos;
        if let Some(b) = self.peek()
            && is_unquoted_name_start(b)
        {
            self.bump();
            while let Some(b) = self.peek() {
                if is_unquoted_name_cont(b) {
                    self.bump();
                } else {
                    break;
                }
            }
            let name = &self.src[name_start..self.pos];
            let token = match kind {
                QuotedNameKind::Global => Token::GlobalVar(std::borrow::Cow::Borrowed(name)),
                QuotedNameKind::Local => Token::LocalVar(std::borrow::Cow::Borrowed(name)),
                QuotedNameKind::Comdat => Token::ComdatVar(std::borrow::Cow::Borrowed(name)),
                QuotedNameKind::String | QuotedNameKind::Metadata => {
                    unreachable!("lex_var handles only Global/Local/Comdat")
                }
            };
            return Ok(self.spanned(token, self.current_span()));
        }

        // Numeric form: @[0-9]+
        if matches!(self.peek(), Some(b'0'..=b'9')) {
            let id = self.lex_uint()?;
            let span = self.current_span();
            let token = match kind {
                QuotedNameKind::Global => Token::GlobalId(id),
                QuotedNameKind::Local => Token::LocalVarId(id),
                _ => unreachable!("only @42 / %42 reach this branch"),
            };
            return Ok(self.spanned(token, span));
        }

        Err(LexError::UnknownToken {
            span: self.current_span(),
        })
    }

    /// Lex `$…` — quoted comdat or label-tail starting with `$`.
    /// Mirrors `LLLexer::LexDollar` (LLLexer.cpp:302).
    fn lex_dollar(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        // Try label-tail rooted at TokStart (which is already at the `$`).
        if let Some(end) = is_label_tail(self.src, self.pos) {
            // end is the position *after* the trailing colon.
            // Label content excludes the colon: [tok_start, end-1).
            let content = &self.src[self.tok_start..end - 1];
            self.pos = end;
            return Ok(self.spanned(
                Token::LabelStr(std::borrow::Cow::Borrowed(content)),
                self.current_span(),
            ));
        }

        // Consume the '$'.
        self.bump();

        // Quoted comdat: $"…"
        if self.peek() == Some(b'"') {
            return self.lex_quoted_name(QuotedNameKind::Comdat);
        }

        // Unquoted comdat: $[-a-zA-Z$._][-a-zA-Z$._0-9]*
        let name_start = self.pos;
        if let Some(b) = self.peek()
            && is_unquoted_name_start(b)
        {
            self.bump();
            while let Some(b) = self.peek() {
                if is_unquoted_name_cont(b) {
                    self.bump();
                } else {
                    break;
                }
            }
            let name = &self.src[name_start..self.pos];
            return Ok(self.spanned(
                Token::ComdatVar(std::borrow::Cow::Borrowed(name)),
                self.current_span(),
            ));
        }

        Err(LexError::UnknownToken {
            span: self.current_span(),
        })
    }

    /// Lex `!…`. Either `!` alone (`Exclaim`) or `!name` (`MetadataVar`).
    /// Mirrors `LLLexer::LexExclaim` (LLLexer.cpp:455).
    fn lex_exclaim(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        self.bump(); // consume '!'

        let name_start = self.pos;
        let Some(first) = self.peek() else {
            return Ok(self.spanned(Token::Exclaim, self.current_span()));
        };
        if !is_metadata_name_start(first) {
            return Ok(self.spanned(Token::Exclaim, self.current_span()));
        }
        self.bump();
        while let Some(b) = self.peek() {
            if is_metadata_name_cont(b) {
                self.bump();
            } else {
                break;
            }
        }
        let raw = &self.src[name_start..self.pos];
        let decoded = escape::unescape(raw);
        Ok(self.spanned(Token::MetadataVar(decoded), self.current_span()))
    }

    /// Lex `^[0-9]+` summary id. Mirrors `LLLexer::LexCaret` (LLLexer.cpp:475).
    fn lex_caret(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        self.bump(); // '^'
        if !matches!(self.peek(), Some(b'0'..=b'9')) {
            return Err(LexError::UnknownToken {
                span: self.current_span(),
            });
        }
        let id = self.lex_uint()?;
        Ok(self.spanned(Token::SummaryId(id), self.current_span()))
    }

    /// Lex `#…` — either `#42` (AttrGrpId) or `#` alone (Hash).
    /// Mirrors `LLLexer::LexHash` (LLLexer.cpp:483).
    fn lex_hash(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        self.bump(); // '#'
        if matches!(self.peek(), Some(b'0'..=b'9')) {
            let id = self.lex_uint()?;
            return Ok(self.spanned(Token::AttrGrpId(id), self.current_span()));
        }
        Ok(self.spanned(Token::Hash, self.current_span()))
    }

    /// Lex a `"…"` token. Either a string constant or, if followed by `:`,
    /// a quoted label.  Mirrors `LLLexer::LexQuote` (LLLexer.cpp:434).
    fn lex_quote(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        let raw = self.read_quoted_payload(QuotedNameKind::String)?;
        let decoded = escape::unescape(raw);

        // Check trailing ':' — that turns this into a label.
        if self.peek() == Some(b':') {
            self.bump();
            if decoded.contains(&0) {
                return Err(LexError::NulInName {
                    span: self.current_span(),
                });
            }
            return Ok(self.spanned(Token::LabelStr(decoded), self.current_span()));
        }

        // Plain string constant: NUL bytes are *allowed*.
        Ok(self.spanned(Token::StringConstant(decoded), self.current_span()))
    }

    /// Lex a quoted name (@"x", %"x", $"x"). The leading sigil is already
    /// consumed; the cursor is currently on the opening `"`.
    fn lex_quoted_name(&mut self, kind: QuotedNameKind) -> Result<Spanned<Token<'src>>, LexError> {
        let raw = self.read_quoted_payload(kind)?;
        let decoded = escape::unescape(raw);
        if decoded.contains(&0) {
            return Err(LexError::NulInName {
                span: self.current_span(),
            });
        }
        let token = match kind {
            QuotedNameKind::Global => Token::GlobalVar(decoded),
            QuotedNameKind::Local => Token::LocalVar(decoded),
            QuotedNameKind::Comdat => Token::ComdatVar(decoded),
            QuotedNameKind::Metadata | QuotedNameKind::String => {
                unreachable!("metadata and string use direct paths")
            }
        };
        Ok(self.spanned(token, self.current_span()))
    }

    /// Consume `"…"` and return the **raw** byte slice of the payload (no
    /// escape decoding). Cursor ends just past the closing `"`. The opening
    /// `"` must be at `self.pos`.
    fn read_quoted_payload(&mut self, kind: QuotedNameKind) -> Result<&'src [u8], LexError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.bump(); // opening '"'
        let start = self.pos;
        loop {
            match self.bump() {
                None => {
                    let span = self.current_span();
                    return Err(match kind {
                        QuotedNameKind::String => LexError::UnterminatedString { span },
                        _ => LexError::UnterminatedQuotedName { kind, span },
                    });
                }
                Some(b'"') => {
                    // self.pos is now one past the closing '"'.
                    return Ok(&self.src[start..self.pos - 1]);
                }
                _ => {}
            }
        }
    }

    // ── Numbers / labels ─────────────────────────────────────────────────────

    /// Consume `[0-9]+` and return as `u32`, erroring if the value overflows.
    /// Used for `@42`, `%42`, `^42`, `#42`, and numeric labels. The cursor
    /// must already be on the first digit (i.e. caller didn't consume it).
    fn lex_uint(&mut self) -> Result<u32, LexError> {
        let digits_start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.bump();
        }
        // The lexer guarantees a non-empty ASCII digit run, so `parse` only
        // fails on overflow (`PosOverflow`). Collapse to one user-facing error.
        let digits = ascii_str(&self.src[digits_start..self.pos]);
        digits.parse::<u32>().map_err(|_| LexError::IdOverflow {
            span: self.span_from(self.tok_start),
        })
    }

    /// Lex `.` — either `...` punctuation or a `.` -prefixed label.
    /// Mirrors LLLexer.cpp:219.
    fn lex_dot(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        // Look-ahead label tail starting at the '.'.
        if let Some(end) = is_label_tail(self.src, self.pos + 1) {
            let content = &self.src[self.pos..end - 1];
            self.pos = end;
            return Ok(self.spanned(
                Token::LabelStr(std::borrow::Cow::Borrowed(content)),
                self.current_span(),
            ));
        }
        // ...
        if self.peek_at(0) == Some(b'.')
            && self.peek_at(1) == Some(b'.')
            && self.peek_at(2) == Some(b'.')
        {
            self.pos += 3;
            return Ok(self.spanned(Token::DotDotDot, self.current_span()));
        }
        // Bare '.' isn't a token in LLVM IR.
        self.bump();
        Err(LexError::UnknownToken {
            span: self.current_span(),
        })
    }

    /// `[0-9]+` or `-[0-9…]` or numeric label. Mirrors
    /// `LLLexer::LexDigitOrNegative` (LLLexer.cpp:1163).
    fn lex_digit_or_negative(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        let first = self.peek().expect("caller verified non-empty");

        // If '-' is followed by a non-digit, this is a label-tail.
        if first == b'-' && !matches!(self.peek_at(1), Some(b'0'..=b'9')) {
            self.bump(); // '-'
            // isLabelTail rooted at the byte right after '-'.
            if let Some(end) = is_label_tail(self.src, self.pos) {
                let content = &self.src[self.tok_start..end - 1];
                self.pos = end;
                return Ok(self.spanned(
                    Token::LabelStr(std::borrow::Cow::Borrowed(content)),
                    self.current_span(),
                ));
            }
            return Err(LexError::UnknownToken {
                span: self.current_span(),
            });
        }

        // Either '-' followed by digits, or already a digit.
        let sign = if first == b'-' {
            self.bump();
            Sign::Neg
        } else {
            Sign::Pos
        };
        let digits_start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.bump();
        }

        // Numeric label: digits then ':'. (Only allowed if we started with a digit.)
        if matches!(sign, Sign::Pos) && self.peek() == Some(b':') {
            // We started with a digit (since sign == Pos). The leading char is
            // included in the label slice.
            let content = &self.src[self.tok_start..self.pos];
            self.bump(); // ':'
            return Ok(self.spanned(
                Token::LabelStr(std::borrow::Cow::Borrowed(content)),
                self.current_span(),
            ));
        }

        // String label, e.g. `-1.foo:` or `1bb:`.
        if matches!(self.peek(), Some(c) if is_label_char(c) || c == b':')
            && let Some(end) = is_label_tail(self.src, self.pos)
        {
            let content = &self.src[self.tok_start..end - 1];
            self.pos = end;
            return Ok(self.spanned(
                Token::LabelStr(std::borrow::Cow::Borrowed(content)),
                self.current_span(),
            ));
        }

        // FP if next char is '.', else int. The 0x… prefix branches into Lex0x.
        if self.peek() != Some(b'.') {
            // Hex FP `0x…`: only valid for digit-start (sign==Pos).
            if matches!(sign, Sign::Pos)
                && self.src.get(self.tok_start) == Some(&b'0')
                && self.src.get(self.tok_start + 1) == Some(&b'x')
            {
                return self.lex_hex_fp();
            }

            let digits = ascii_str(&self.src[digits_start..self.pos]);
            return Ok(self.spanned(
                Token::IntegerLit(IntLit {
                    sign,
                    base: NumBase::Dec,
                    digits,
                }),
                self.current_span(),
            ));
        }

        self.consume_fp_tail()?;
        let lex = ascii_str(&self.src[self.tok_start..self.pos]);
        Ok(self.spanned(Token::FloatLit(FpLit::Decimal(lex)), self.current_span()))
    }

    /// Lex `+1.5` style positive FP literal. Mirrors LLLexer.cpp:1232.
    fn lex_positive(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        self.bump(); // '+'
        if !matches!(self.peek(), Some(b'0'..=b'9')) {
            return Err(LexError::UnknownToken {
                span: self.current_span(),
            });
        }
        // Skip integer part.
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.bump();
        }
        // Must have a '.'.
        if self.peek() != Some(b'.') {
            return Err(LexError::UnknownToken {
                span: self.current_span(),
            });
        }
        self.consume_fp_tail()?;
        let lex = ascii_str(&self.src[self.tok_start..self.pos]);
        Ok(self.spanned(Token::FloatLit(FpLit::Decimal(lex)), self.current_span()))
    }

    /// Consume `\.[0-9]*([eE][-+]?[0-9]+)?`. Cursor is on the `.`. Returns Ok
    /// regardless of whether the exponent is present.
    fn consume_fp_tail(&mut self) -> Result<(), LexError> {
        debug_assert_eq!(self.peek(), Some(b'.'));
        self.bump(); // '.'
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.bump();
        }
        // Optional exponent.
        if matches!(self.peek(), Some(b'e' | b'E')) {
            // Lookahead: digit, or (+/-) then digit.
            let after_e = self.peek_at(1);
            let after_sign = self.peek_at(2);
            let has_exp = matches!(after_e, Some(b'0'..=b'9'))
                || (matches!(after_e, Some(b'+' | b'-'))
                    && matches!(after_sign, Some(b'0'..=b'9')));
            if has_exp {
                self.bump(); // 'e' or 'E'
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.bump();
                }
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.bump();
                }
            }
        }
        Ok(())
    }

    /// Lex `0x[KLMHR]?[0-9A-Fa-f]+` (the `0x` is already at `self.tok_start`).
    /// Mirrors `LLLexer::Lex0x` (LLLexer.cpp:1085).
    fn lex_hex_fp(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        // Cursor sits just past the digit run we already consumed in
        // `lex_digit_or_negative`. Re-park it after the `0x` prefix and reparse.
        self.pos = self.tok_start + 2;

        let kind = match self.peek() {
            Some(b @ (b'K' | b'L' | b'M' | b'H' | b'R')) => {
                self.bump();
                b
            }
            _ => b'J', // plain hex double
        };

        let digits_start = self.pos;
        if !matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
            // Bad token; LLLexer.cpp:1097-1100 rewinds to TokStart+1 and
            // returns an error. Mirror it.
            self.pos = self.tok_start + 1;
            return Err(LexError::UnknownToken {
                span: self.current_span(),
            });
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
            self.bump();
        }
        let digits = ascii_str(&self.src[digits_start..self.pos]);

        let token = match kind {
            b'J' => Token::FloatLit(FpLit::HexDouble(digits)),
            b'K' => Token::FloatLit(FpLit::HexX87(digits)),
            b'L' => Token::FloatLit(FpLit::HexQuad(digits)),
            b'M' => Token::FloatLit(FpLit::HexPpc128(digits)),
            b'H' => {
                self.check_hex16_fits(digits, HexFpKind::Half)?;
                Token::FloatLit(FpLit::HexHalf(digits))
            }
            b'R' => {
                self.check_hex16_fits(digits, HexFpKind::BFloat)?;
                Token::FloatLit(FpLit::HexBFloat(digits))
            }
            _ => unreachable!(),
        };
        Ok(self.spanned(token, self.current_span()))
    }

    /// Reject `0xH` / `0xR` constants whose payload doesn't fit in 16 bits.
    /// Mirrors `llvm::isUInt<16>(Val)` (LLLexer.cpp:1134, :1144).
    fn check_hex16_fits(&self, digits: &str, target: HexFpKind) -> Result<(), LexError> {
        u16::from_str_radix(digits, 16)
            .map(|_| ())
            .map_err(|_| LexError::HexFpTooLarge {
                target,
                span: self.current_span(),
            })
    }

    // ── Identifier / keyword path ────────────────────────────────────────────

    /// Lex a label, integer type, or keyword starting with `[a-zA-Z_]`.
    /// Mirrors `LLLexer::LexIdentifier` (LLLexer.cpp:495).
    fn lex_identifier(&mut self) -> Result<Spanned<Token<'src>>, LexError> {
        let first = self.bump().expect("caller verified non-empty");

        // Walk the label-char run, recording the first non-keyword-char position.
        // (Keyword chars are alnum + `_`; label chars also include `-`, `$`, `.`.)
        // Mirrors LLLexer.cpp:500-507.
        let mut keyword_end: Option<usize> = None;
        while let Some(b) = self.peek() {
            if !is_label_char(b) {
                break;
            }
            if keyword_end.is_none() && !b.is_ascii_alphanumeric() && b != b'_' {
                keyword_end = Some(self.pos);
            }
            self.bump();
        }

        // Label terminator unless we've been told to ignore it (LLLexer.cpp:511).
        if !self.ignore_colon_in_idents && self.peek() == Some(b':') {
            let content = &self.src[self.tok_start..self.pos];
            self.bump(); // ':'
            return Ok(self.spanned(
                Token::LabelStr(std::borrow::Cow::Borrowed(content)),
                self.current_span(),
            ));
        }

        // Integer type `iN` — only meaningful when the leading byte is `i`
        // followed by at least one digit (LLLexer.cpp:518-528).
        if first == b'i' {
            let mut int_end = self.tok_start + 1;
            while int_end < self.pos && self.src[int_end].is_ascii_digit() {
                int_end += 1;
            }
            if int_end > self.tok_start + 1 {
                self.pos = int_end;
                let digits = ascii_str(&self.src[self.tok_start + 1..self.pos]);
                // `digits` is a non-empty ASCII digit run, so `parse` only
                // fails with `PosOverflow`. Treat that as "way out of range";
                // `u64::MAX` makes the range check below fail uniformly.
                let width = digits.parse::<u64>().unwrap_or(u64::MAX);
                if !(INT_TY_MIN_BITS..=INT_TY_MAX_BITS).contains(&width) {
                    return Err(LexError::IntegerWidthOutOfRange {
                        width,
                        max: INT_TY_MAX_BITS as u32,
                        span: self.current_span(),
                    });
                }
                // `width` is now in `1..=16_777_215`, so both the `u32` cast
                // and `NonZeroU32::new` are guaranteed to succeed.
                let nz = std::num::NonZeroU32::new(width as u32)
                    .expect("width validated within 1..=INT_TY_MAX_BITS");
                return Ok(self.spanned(
                    Token::PrimitiveType(PrimitiveTy::Integer(nz)),
                    self.current_span(),
                ));
            }
        }

        // Truncate to keyword_end (the first non-keyword char) so the run we
        // look up only contains alphanumerics and underscores. Cursor is
        // rewound — any `-`/`$`/`.` tail becomes the next token.
        let kw_end = keyword_end.unwrap_or(self.pos);
        self.pos = kw_end;
        let word = &self.src[self.tok_start..self.pos];
        let span = self.current_span();

        // DWARF / DI / CSK / dbg / EmissionKind / NameTableKind / FixedPointKind.
        if let Some(tok) = self.classify_prefixed(word) {
            return Ok(self.spanned(tok, span));
        }

        // Plain keywords.
        if let Some(tok) = keywords::classify_word(word) {
            return Ok(self.spanned(tok, span));
        }

        // [us]0x[hex]+ APSInt.
        if let Some(tok) = self.classify_hex_apsint(word)? {
            return Ok(self.spanned(tok, span));
        }

        // cc<digits> rewind: emit `kw_cc` and rewind cursor to just past `cc`.
        if word.len() > 2 && word.starts_with(b"cc") && word[2..].iter().all(|c| c.is_ascii_digit())
        {
            self.pos = self.tok_start + 2;
            return Ok(self.spanned(
                Token::Kw(Keyword::Cc),
                Span::new(self.tok_start as u32, self.pos as u32),
            ));
        }

        // Truly unknown — rewind to a single byte and emit error (LLLexer.cpp:1073).
        self.pos = self.tok_start + 1;
        Err(LexError::UnknownToken {
            span: self.current_span(),
        })
    }

    /// Classify words with structured prefixes: `DW_..._`, `DIFlag`, `DISPFlag`,
    /// `CSK_`, `dbg_`, plus the small fixed-set tokens for emission kind, etc.
    fn classify_prefixed(&self, word: &'src [u8]) -> Option<Token<'src>> {
        // Interpret word as ASCII (lexer regex guarantees this).
        let s = ascii_str(word);

        // `dbg_<suffix>` — store only the suffix.
        if let Some(suffix) = s.strip_prefix("dbg_") {
            return match suffix {
                "value" | "declare" | "assign" | "label" | "declare_value" => {
                    let suffix_str = ascii_str(&word[4..]);
                    Some(Token::DbgRecordType(suffix_str))
                }
                _ => None,
            };
        }

        // DW_<TYPE>_… — payload is the full keyword.
        if let Some(rest) = s.strip_prefix("DW_") {
            let token = match rest.split_once('_').map(|(t, _)| t) {
                Some("TAG") => Token::DwarfTag(s),
                Some("ATE") => Token::DwarfAttEncoding(s),
                Some("VIRTUALITY") => Token::DwarfVirtuality(s),
                Some("LANG") => Token::DwarfLang(s),
                Some("LNAME") => Token::DwarfSourceLangName(s),
                Some("CC") => Token::DwarfCC(s),
                Some("OP") => Token::DwarfOp(s),
                Some("MACINFO") => Token::DwarfMacinfo(s),
                Some("APPLE") if rest.starts_with("APPLE_ENUM_KIND_") => Token::DwarfEnumKind(s),
                _ => return None,
            };
            return Some(token);
        }

        if s.starts_with("DIFlag") {
            return Some(Token::DiFlag(s));
        }
        if s.starts_with("DISPFlag") {
            return Some(Token::DiSpFlag(s));
        }
        if s.starts_with("CSK_") {
            return Some(Token::ChecksumKind(s));
        }

        match s {
            "NoDebug" | "FullDebug" | "LineTablesOnly" | "DebugDirectivesOnly" => {
                Some(Token::EmissionKind(s))
            }
            "GNU" | "Apple" | "None" | "Default" => Some(Token::NameTableKind(s)),
            "Binary" | "Decimal" | "Rational" => Some(Token::FixedPointKind(s)),
            _ => None,
        }
    }

    /// `[us]0x[0-9A-Fa-f]+` — emit an `IntegerLit` flagged as
    /// hex-signed/unsigned. Returns Ok(None) when the word doesn't fit the
    /// pattern; returns Ok(Some) on a match.
    fn classify_hex_apsint(&self, word: &'src [u8]) -> Result<Option<Token<'src>>, LexError> {
        if word.len() < 4 {
            return Ok(None);
        }
        let base = match word[0] {
            b's' => NumBase::HexSigned,
            b'u' => NumBase::HexUnsigned,
            _ => return Ok(None),
        };
        if word[1] != b'0' || word[2] != b'x' {
            return Ok(None);
        }
        if !word[3..].iter().all(|c| c.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let digits = ascii_str(&word[3..]);
        Ok(Some(Token::IntegerLit(IntLit {
            sign: Sign::Pos,
            base,
            digits,
        })))
    }
}

// ─── Iterator ─────────────────────────────────────────────────────────────────

// ─── Conversions ──────────────────────────────────────────────────────────────

impl<'src> From<&'src [u8]> for Lexer<'src> {
    #[inline]
    fn from(src: &'src [u8]) -> Self {
        Self::new(src)
    }
}

impl<'src> From<&'src str> for Lexer<'src> {
    #[inline]
    fn from(src: &'src str) -> Self {
        Self::new(src.as_bytes())
    }
}

impl<'src> Iterator for Lexer<'src> {
    type Item = Result<Spanned<Token<'src>>, LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_token() {
            Ok(spanned) if matches!(spanned.value, Token::Eof) => None,
            other => Some(other),
        }
    }
}

// ─── Free helpers (LLLexer.cpp top-level statics) ─────────────────────────────

/// `[-a-zA-Z$._0-9]`. Mirrors `isLabelChar` (LLLexer.cpp:153).
#[inline]
fn is_label_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'$' || b == b'.' || b == b'_'
}

/// `[-a-zA-Z$._]` — first character of an unquoted name.
#[inline]
fn is_unquoted_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'-' || b == b'$' || b == b'.' || b == b'_'
}

/// `[-a-zA-Z$._0-9]` — continuation of an unquoted name.
#[inline]
fn is_unquoted_name_cont(b: u8) -> bool {
    is_label_char(b)
}

/// `[-a-zA-Z$._\\]` — first character of a metadata name (escape allowed).
#[inline]
fn is_metadata_name_start(b: u8) -> bool {
    is_unquoted_name_start(b) || b == b'\\'
}

/// `[-a-zA-Z$._0-9\\]` — continuation of a metadata name.
#[inline]
fn is_metadata_name_cont(b: u8) -> bool {
    is_label_char(b) || b == b'\\'
}

/// Treat `bytes` as ASCII text. The lexer's regex guarantees this; the
/// `expect` is intentionally retained as a tripwire (not unsafe).
#[inline]
fn ascii_str(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("lexer regex guarantees ASCII")
}

/// Mirrors `isLabelTail` (LLLexer.cpp:158): scan forward from `pos`, return
/// `Some(end)` where `end` is the offset just *past* the colon if the bytes
/// from `pos` form `[-a-zA-Z$._0-9]*:`. Returns `None` otherwise.
#[inline]
fn is_label_tail(src: &[u8], mut pos: usize) -> Option<usize> {
    while let Some(&b) = src.get(pos) {
        if b == b':' {
            return Some(pos + 1);
        }
        if !is_label_char(b) {
            return None;
        }
        pos += 1;
    }
    None
}

#[cfg(test)]
#[path = "ll_lexer_tests.rs"]
mod tests;
