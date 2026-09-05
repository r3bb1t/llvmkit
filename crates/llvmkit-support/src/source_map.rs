//! Line-offset index for translating byte offsets to `(line, column)`.
//!
//! The lexer never needs `(line, column)`: it just produces byte spans. The
//! source map turns those spans into human-readable diagnostics on demand.
//!
//! Construction scans the source once to record line starts. That linear
//! pass is dominated by whatever the caller did to load the source in the
//! first place (file I/O, lexing), so eager init is cheap and removes the
//! `OnceLock` / interior-mutability machinery a lazy design would need.

/// Borrowing source map. Holds the source slice and a precomputed table of
/// line-start offsets.
///
/// Lines are 1-indexed; columns are 1-indexed byte offsets within a line.
/// (Multi-byte UTF-8 characters count as multiple columns. LLVM IR is ASCII in
/// the syntax that matters; non-ASCII only appears inside string constants
/// and quoted identifiers, where character columns aren't a useful unit.)
#[derive(Clone)]
pub struct SourceMap<'src> {
    src: &'src [u8],
    /// Offset of the first byte of each line. `line_starts[0] == 0` always.
    line_starts: Vec<u32>,
}

/// A 1-indexed position in a source buffer, as [`SourceMap::line_col`] reports
/// it.
///
/// Fields are public because this is a plain coordinate pair that callers
/// destructure, not a type with an invariant to protect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineCol {
    /// 1-indexed line number.
    pub line: u32,
    /// 1-indexed byte column within the line.
    pub column: u32,
}

impl core::fmt::Display for LineCol {
    /// `line:column`, the form diagnostics print.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// Prints the *shape* of the map — source length and line count — never the
/// buffer. A `#[derive(Debug)]` would splice a whole `.ll` file into every
/// `dbg!` and every `Debug`-formatted struct that holds one.
impl core::fmt::Debug for SourceMap<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SourceMap")
            .field("bytes", &self.src.len())
            .field("lines", &self.line_starts.len())
            .finish()
    }
}

impl<'src> SourceMap<'src> {
    pub fn new(src: &'src [u8]) -> Self {
        let mut line_starts = Vec::with_capacity(src.len() / 32 + 1);
        line_starts.push(0u32);
        for (i, &b) in src.iter().enumerate() {
            if b == b'\n' {
                // A source longer than `u32::MAX` cannot be addressed by this
                // map -- `Span` and `line_col` are both `u32` -- so stop
                // recording rather than let a truncating cast fold a high
                // offset back onto a wrong line.
                let Ok(next) = u32::try_from(i + 1) else {
                    break;
                };
                // Don't push past EOF — keeps line_text bookkeeping simple.
                if usize::try_from(next).is_ok_and(|n| n <= src.len()) {
                    line_starts.push(next);
                }
            }
        }
        Self { src, line_starts }
    }

    #[inline]
    pub fn source(&self) -> &'src [u8] {
        self.src
    }

    /// Translate an absolute byte offset to a [`LineCol`], both 1-indexed. An
    /// offset `>= src.len()` is reported as if it sat at EOF.
    ///
    /// Returns a named pair rather than `(u32, u32)`: the two halves are the
    /// same type and transposable, and every call site in the workspace
    /// immediately destructured the tuple into differently-named variables
    /// (`let (l, c) = …` in two examples, `let (line, col) = …` in a third),
    /// which is the tell.
    pub fn line_col(&self, offset: u32) -> LineCol {
        // Work in the `u32` domain throughout: `line_starts` is `Vec<u32>` and
        // the parameter is a `u32`, so widening to `usize` and back was what
        // forced the casts. A source longer than `u32::MAX` cannot be addressed
        // by this map at all, so saturating is the honest clamp -- a `u32`
        // offset can never exceed it anyway.
        let len = u32::try_from(self.src.len()).unwrap_or(u32::MAX);
        let off = offset.min(len);
        // Find the largest start <= off via binary search.
        let line_idx = match self.line_starts.binary_search(&off) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line_start = self.line_starts[line_idx];
        LineCol {
            line: u32::try_from(line_idx)
                .unwrap_or(u32::MAX)
                .saturating_add(1),
            column: off.saturating_sub(line_start).saturating_add(1),
        }
    }

    /// Borrow the slice covering a single line by 1-indexed `line` number.
    /// Returns `None` if `line` is out of range. The trailing `\n` (if any) is
    /// stripped; a trailing `\r` is also stripped to make Windows newlines tidy.
    pub fn line_text(&self, line: u32) -> Option<&'src [u8]> {
        if line == 0 {
            return None;
        }
        let i = usize::try_from(line - 1).ok()?;
        let start = usize::try_from(*self.line_starts.get(i)?).ok()?;
        let end = self
            .line_starts
            .get(i + 1)
            .and_then(|&e| usize::try_from(e).ok())
            .unwrap_or(self.src.len());
        // end currently sits *after* the newline. Trim it.
        let mut e = end;
        if e > start && self.src[e - 1] == b'\n' {
            e -= 1;
        }
        if e > start && self.src[e - 1] == b'\r' {
            e -= 1;
        }
        Some(&self.src[start..e])
    }
}

impl<'src> From<&'src [u8]> for SourceMap<'src> {
    #[inline]
    fn from(src: &'src [u8]) -> Self {
        Self::new(src)
    }
}

impl<'src> From<&'src str> for SourceMap<'src> {
    #[inline]
    fn from(src: &'src str) -> Self {
        Self::new(src.as_bytes())
    }
}

/// Upstream provenance: llvmkit-specific support utility. Closest upstream:
/// `llvm/Support/SourceMgr.h::SMLoc` and `SourceMgr::getLineAndColumn`.
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: byte-offset-to-(line,col) mapping. Closest upstream:
    /// `SourceMgr::getLineAndColumn` in `llvm/Support/SourceMgr.h`.
    #[test]
    fn line_col_basic() {
        let sm = SourceMap::from("abc\ndef\nghi");
        assert_eq!(sm.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(sm.line_col(2), LineCol { line: 1, column: 3 });
        assert_eq!(sm.line_col(3), LineCol { line: 1, column: 4 }); // the '\n' is on line 1
        assert_eq!(sm.line_col(4), LineCol { line: 2, column: 1 });
        assert_eq!(sm.line_col(7), LineCol { line: 2, column: 4 });
        assert_eq!(sm.line_col(8), LineCol { line: 3, column: 1 });
        assert_eq!(sm.line_col(10), LineCol { line: 3, column: 3 });
    }

    /// llvmkit-specific: out-of-range clamp. Closest upstream:
    /// `SourceMgr::getLineAndColumn` saturation in `llvm/Support/SourceMgr.h`.
    #[test]
    fn line_col_eof_clamps() {
        let sm = SourceMap::from("ab");
        assert_eq!(sm.line_col(99), LineCol { line: 1, column: 3 });
    }

    /// llvmkit-specific: line-text accessor. Closest upstream:
    /// `SourceMgr::FindLineNumber` / line buffer lookup in `SourceMgr.h`.
    #[test]
    fn line_text_trims_newlines() {
        let sm = SourceMap::from("abc\r\ndef\nghi");
        assert_eq!(sm.line_text(1), Some(&b"abc"[..]));
        assert_eq!(sm.line_text(2), Some(&b"def"[..]));
        assert_eq!(sm.line_text(3), Some(&b"ghi"[..]));
        assert_eq!(sm.line_text(4), None);
    }

    /// llvmkit-specific: empty-source guard. Closest upstream: `SourceMgr`
    /// invariants in `llvm/Support/SourceMgr.h`.
    #[test]
    fn empty_source() {
        let sm = SourceMap::from("");
        assert_eq!(sm.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(sm.line_text(1), Some(&b""[..]));
    }

    /// llvmkit-specific: the `Debug` impl summarises rather than dumping the
    /// buffer. Closest upstream: none — `llvm::SourceMgr` has no `print`.
    #[test]
    fn debug_summarises_without_the_buffer() {
        let sm = SourceMap::from("abc\ndef\n");
        let rendered = format!("{sm:?}");
        assert_eq!(rendered, "SourceMap { bytes: 8, lines: 3 }");
        assert!(!rendered.contains("abc"));
        // Cloning is a plain copy of the same view + table.
        let cloned = sm.clone();
        assert_eq!(format!("{cloned:?}"), rendered);
        assert_eq!(cloned.line_text(2), Some(&b"def"[..]));
    }
}
