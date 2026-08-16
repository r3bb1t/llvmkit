//! The TableGen lexer.
//!
//! Ports `llvm/lib/TableGen/TGLexer.cpp`.

use crate::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Ident(String),
    BangIdent(String),
    String(String),
    Int(i64),
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Less,
    Greater,
    Colon,
    Semi,
    Comma,
    Equal,
    Dot,
    Ellipsis,
    Hash,
    Question,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) col: usize,
}

pub(crate) struct Lexer<'a> {
    pub(crate) file: &'a str,
    pub(crate) text: &'a str,
    pub(crate) pos: usize,
    pub(crate) line: usize,
    pub(crate) col: usize,
}

impl<'a> Lexer<'a> {
    pub(crate) fn new(file: &'a str, text: &'a str) -> Self {
        Self {
            file,
            text,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub(crate) fn tokenize(mut self) -> GenResult<Vec<Token>> {
        let mut out = Vec::new();
        while let Some(ch) = self.peek_char() {
            if ch.is_whitespace() {
                self.bump_char();
                continue;
            }
            if ch == '/' && self.peek_next_char() == Some('/') {
                self.bump_char();
                self.bump_char();
                while let Some(c) = self.peek_char() {
                    self.bump_char();
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            if ch == '/' && self.peek_next_char() == Some('*') {
                self.bump_char();
                self.bump_char();
                loop {
                    match self.peek_char() {
                        Some('*') if self.peek_next_char() == Some('/') => {
                            self.bump_char();
                            self.bump_char();
                            break;
                        }
                        Some(_) => {
                            self.bump_char();
                        }
                        None => return Err(self.error("unterminated block comment")),
                    }
                }
                continue;
            }
            if ch == '#' && self.is_preprocessor_directive() && self.is_preprocessor_keyword() {
                while let Some(c) = self.peek_char() {
                    self.bump_char();
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }

            let file = self.file.to_owned();
            let line = self.line;
            let col = self.col;
            let kind = match ch {
                '{' => {
                    self.bump_char();
                    TokenKind::LBrace
                }
                '}' => {
                    self.bump_char();
                    TokenKind::RBrace
                }
                '(' => {
                    self.bump_char();
                    TokenKind::LParen
                }
                ')' => {
                    self.bump_char();
                    TokenKind::RParen
                }
                '[' => {
                    self.bump_char();
                    TokenKind::LBracket
                }
                ']' => {
                    self.bump_char();
                    TokenKind::RBracket
                }
                '<' => {
                    self.bump_char();
                    TokenKind::Less
                }
                '>' => {
                    self.bump_char();
                    TokenKind::Greater
                }
                ':' => {
                    self.bump_char();
                    TokenKind::Colon
                }
                ';' => {
                    self.bump_char();
                    TokenKind::Semi
                }
                ',' => {
                    self.bump_char();
                    TokenKind::Comma
                }
                '=' => {
                    self.bump_char();
                    TokenKind::Equal
                }
                '?' => {
                    self.bump_char();
                    TokenKind::Question
                }
                '#' => {
                    self.bump_char();
                    TokenKind::Hash
                }
                '.' => {
                    if self.starts_with("...") {
                        self.bump_char();
                        self.bump_char();
                        self.bump_char();
                        TokenKind::Ellipsis
                    } else {
                        self.bump_char();
                        TokenKind::Dot
                    }
                }
                '"' => TokenKind::String(self.lex_string()?),
                '!' => {
                    self.bump_char();
                    let ident = self.lex_ident_tail()?;
                    TokenKind::BangIdent(ident)
                }
                '-' | '0'..='9' => TokenKind::Int(self.lex_int()?),
                _ if is_ident_start(ch) => TokenKind::Ident(self.lex_ident()),
                _ => return Err(self.error(format!("unexpected character `{ch}`"))),
            };
            out.push(Token {
                kind,
                file,
                line,
                col,
            });
        }
        Ok(out)
    }

    pub(crate) fn is_preprocessor_directive(&self) -> bool {
        let before = &self.text[..self.pos];
        before
            .rsplit_once('\n')
            .map_or(before, |(_, tail)| tail)
            .trim()
            .is_empty()
    }

    pub(crate) fn is_preprocessor_keyword(&self) -> bool {
        self.starts_with("#if")
            || self.starts_with("#ifdef")
            || self.starts_with("#ifndef")
            || self.starts_with("#endif")
            || self.starts_with("#define")
            || self.starts_with("#undef")
    }

    pub(crate) fn lex_string(&mut self) -> GenResult<String> {
        self.expect_char('"')?;
        let mut s = String::new();
        while let Some(ch) = self.peek_char() {
            self.bump_char();
            match ch {
                '"' => return Ok(s),
                '\\' => {
                    let escaped = self
                        .peek_char()
                        .ok_or_else(|| self.error("unterminated string escape"))?;
                    self.bump_char();
                    let decoded = match escaped {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '\\' => '\\',
                        '"' => '"',
                        other => other,
                    };
                    s.push(decoded);
                }
                other => s.push(other),
            }
        }
        Err(self.error("unterminated string literal"))
    }

    pub(crate) fn lex_int(&mut self) -> GenResult<i64> {
        let start = self.pos;
        if self.peek_char() == Some('-') {
            self.bump_char();
        }
        if self.starts_with("0x") || self.starts_with("0X") {
            self.bump_char();
            self.bump_char();
            while self.peek_char().is_some_and(|c| c.is_ascii_hexdigit()) {
                self.bump_char();
            }
        } else {
            while self.peek_char().is_some_and(|c| c.is_ascii_digit()) {
                self.bump_char();
            }
        }
        let raw = &self.text[start..self.pos];
        let negative = raw.starts_with('-');
        let digits = raw.trim_start_matches('-');
        let parsed = if let Some(hex) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            i64::from_str_radix(hex, 16)
        } else {
            digits.parse::<i64>()
        };
        parsed
            .map(|v| if negative { -v } else { v })
            .map_err(|source| self.error(format!("invalid integer `{raw}`: {source}")))
    }

    pub(crate) fn lex_ident_tail(&mut self) -> GenResult<String> {
        if !self.peek_char().is_some_and(is_ident_start) {
            return Err(self.error("expected identifier after `!`"));
        }
        Ok(self.lex_ident())
    }

    pub(crate) fn lex_ident(&mut self) -> String {
        let start = self.pos;
        self.bump_char();
        while self.peek_char().is_some_and(is_ident_continue) {
            self.bump_char();
        }
        self.text[start..self.pos].to_owned()
    }

    pub(crate) fn expect_char(&mut self, expected: char) -> GenResult<()> {
        match self.peek_char() {
            Some(ch) if ch == expected => {
                self.bump_char();
                Ok(())
            }
            _ => Err(self.error(format!("expected `{expected}`"))),
        }
    }

    pub(crate) fn starts_with(&self, s: &str) -> bool {
        self.text[self.pos..].starts_with(s)
    }

    pub(crate) fn peek_char(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    pub(crate) fn peek_next_char(&self) -> Option<char> {
        let mut chars = self.text[self.pos..].chars();
        chars.next()?;
        chars.next()
    }

    pub(crate) fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    pub(crate) fn error<M>(&self, message: M) -> TableGenError
    where
        M: Into<String>,
    {
        TableGenError::new(format!(
            "{}:{}:{}: {}",
            self.file,
            self.line,
            self.col,
            message.into()
        ))
    }
}

pub(crate) fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

pub(crate) fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}
