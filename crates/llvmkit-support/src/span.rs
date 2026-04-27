//! Source-range bookkeeping.
//!
//! Mirrors `llvm::SMRange` (`llvm/include/llvm/Support/SMLoc.h`) as a half-open
//! `[start, end)` byte range. We use `u32` byte offsets — IR files larger than
//! 4 GiB are not a realistic input — to keep `Token<'src>` compact (saves 8
//! bytes per token vs. `usize`).

use std::ops::Range;

/// Half-open byte range `[start, end)` into a source buffer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    /// Construct a span. `start <= end` is *not* enforced; callers that build
    /// spans by accumulation may briefly hold an inverted span before patching it.
    #[inline]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Single-byte span at `offset`. Useful for "unexpected character here"
    /// diagnostics where there is nothing to widen.
    #[inline]
    pub const fn single(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset + 1,
        }
    }

    /// Empty span at `offset`. Good for "expected something *here*" diagnostics.
    #[inline]
    pub const fn empty_at(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    #[inline]
    pub const fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// View as a `Range<usize>` for slice indexing.
    #[inline]
    pub const fn as_range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

impl From<Span> for Range<usize> {
    #[inline]
    fn from(s: Span) -> Self {
        s.as_range()
    }
}

/// A value carrying its source span.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    #[inline]
    pub const fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }

    #[inline]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            value: f(self.value),
            span: self.span,
        }
    }

    #[inline]
    pub fn as_ref(&self) -> Spanned<&T> {
        Spanned {
            value: &self.value,
            span: self.span,
        }
    }
}

/// Upstream provenance: llvmkit-specific support utility. Closest upstream:
/// `llvm/Support/SourceMgr.h::SMLoc` / `SMRange` (LLVM uses raw pointers
/// where llvmkit uses byte offsets).
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: span constructors. Closest upstream:
    /// `SMRange` / `SMLoc` in `llvm/Support/SourceMgr.h`.
    #[test]
    fn span_basics() {
        let s = Span::new(3, 7);
        assert_eq!(s.len(), 4);
        assert!(!s.is_empty());
        assert_eq!(s.as_range(), 3..7);

        let single = Span::single(5);
        assert_eq!(single.len(), 1);
        assert_eq!(single.as_range(), 5..6);

        let empty = Span::empty_at(5);
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
    }

    /// llvmkit-specific: span as slice index. Closest upstream:
    /// `SMRange::Start` / `End` in `llvm/Support/SourceMgr.h`.
    #[test]
    fn span_indexes_slice() {
        let src = b"hello world";
        let s = Span::new(6, 11);
        assert_eq!(&src[s.as_range()], b"world");
        let r: std::ops::Range<usize> = s.into();
        assert_eq!(r, 6..11);
    }

    /// llvmkit-specific: `Spanned<T>::map`. Closest upstream:
    /// `llvm::SMLoc`-tagged value patterns in the LLVM source.
    #[test]
    fn spanned_map_preserves_span() {
        let sp = Spanned::new(7u32, Span::new(2, 4));
        let mapped = sp.map(|n| n * 3);
        assert_eq!(mapped.value, 21);
        assert_eq!(mapped.span, Span::new(2, 4));
    }

    /// llvmkit-specific: `Spanned<T>::as_ref` borrow. Closest upstream:
    /// no direct equivalent; mirrors LLVM's diag `SMLoc`-tag pattern.
    #[test]
    fn spanned_as_ref_borrows() {
        let sp = Spanned::new(String::from("hi"), Span::new(0, 2));
        let r = sp.as_ref();
        assert_eq!(r.value, "hi");
        assert_eq!(r.span, sp.span);
    }
}
