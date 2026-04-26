//! Lazy line-offset index for translating byte offsets to `(line, column)`.
//!
//! The lexer never needs `(line, column)`: it just produces byte spans. The
//! source map turns those spans into human-readable diagnostics on demand, and
//! amortises the linear scan across requests.

use std::sync::OnceLock;

/// Borrowing source map. Holds the source slice and a lazily-computed cache of
/// line-start offsets.
///
/// Lines are 1-indexed; columns are 1-indexed byte offsets within a line.
/// (Multi-byte UTF-8 characters count as multiple columns. LLVM IR is ASCII in
/// the syntax that matters; non-ASCII only appears inside string constants
/// and quoted identifiers, where character columns aren't a useful unit.)
pub struct SourceMap<'src> {
    src: &'src [u8],
    /// Offset of the first byte of each line. `line_starts[0] == 0` always.
    /// Built lazily on first translation request.
    line_starts: OnceLock<Vec<u32>>,
}

impl<'src> SourceMap<'src> {
    #[inline]
    pub fn new(src: &'src [u8]) -> Self {
        Self {
            src,
            line_starts: OnceLock::new(),
        }
    }

    #[inline]
    pub fn source(&self) -> &'src [u8] {
        self.src
    }

    /// Translate an absolute byte offset to a `(line, column)` pair, both
    /// 1-indexed. An offset `>= src.len()` is reported as if it sat at EOF.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let starts = self.line_starts();
        let off = (offset as usize).min(self.src.len());
        // Find the largest start <= off via binary search.
        let line_idx = match starts.binary_search(&(off as u32)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line_start = starts[line_idx] as usize;
        let column = (off - line_start) as u32 + 1;
        (line_idx as u32 + 1, column)
    }

    /// Borrow the slice covering a single line by 1-indexed `line` number.
    /// Returns `None` if `line` is out of range. The trailing `\n` (if any) is
    /// stripped; a trailing `\r` is also stripped to make Windows newlines tidy.
    pub fn line_text(&self, line: u32) -> Option<&'src [u8]> {
        if line == 0 {
            return None;
        }
        let starts = self.line_starts();
        let i = (line - 1) as usize;
        let start = *starts.get(i)? as usize;
        let end = starts
            .get(i + 1)
            .map(|&e| e as usize)
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

    fn line_starts(&self) -> &Vec<u32> {
        self.line_starts.get_or_init(|| {
            let mut v = Vec::with_capacity(self.src.len() / 32 + 1);
            v.push(0u32);
            for (i, &b) in self.src.iter().enumerate() {
                if b == b'\n' {
                    let next = (i + 1) as u32;
                    // Don't push past EOF — keeps line_text bookkeeping simple.
                    if (next as usize) <= self.src.len() {
                        v.push(next);
                    }
                }
            }
            v
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let sm = SourceMap::from("abc\ndef\nghi");
        assert_eq!(sm.line_col(0), (1, 1));
        assert_eq!(sm.line_col(2), (1, 3));
        assert_eq!(sm.line_col(3), (1, 4)); // the '\n' is on line 1
        assert_eq!(sm.line_col(4), (2, 1));
        assert_eq!(sm.line_col(7), (2, 4));
        assert_eq!(sm.line_col(8), (3, 1));
        assert_eq!(sm.line_col(10), (3, 3));
    }

    #[test]
    fn line_col_eof_clamps() {
        let sm = SourceMap::from("ab");
        assert_eq!(sm.line_col(99), (1, 3));
    }

    #[test]
    fn line_text_trims_newlines() {
        let sm = SourceMap::from("abc\r\ndef\nghi");
        assert_eq!(sm.line_text(1), Some(&b"abc"[..]));
        assert_eq!(sm.line_text(2), Some(&b"def"[..]));
        assert_eq!(sm.line_text(3), Some(&b"ghi"[..]));
        assert_eq!(sm.line_text(4), None);
    }

    #[test]
    fn empty_source() {
        let sm = SourceMap::from("");
        assert_eq!(sm.line_col(0), (1, 1));
        assert_eq!(sm.line_text(1), Some(&b""[..]));
    }

    #[test]
    fn lazy_init_caches() {
        let sm = SourceMap::from("a\nb\nc");
        let first = sm.line_starts() as *const _;
        let second = sm.line_starts() as *const _;
        assert!(std::ptr::eq(first, second));
    }
}
