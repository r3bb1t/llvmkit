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

    /// `true` iff `offset` falls inside the half-open range. Mirrors the
    /// `bool contains(SMLoc) const` shape LLVM gives `SMRange`'s callers;
    /// llvmkit's own parsers hand-rolled `self.start <= x && x < self.end`
    /// (see `FileLocRange::contains_loc` in `llvmkit-asmparser`).
    #[inline]
    pub const fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    /// Smallest span covering both `self` and `other` — the hull, so any gap
    /// between them is swallowed. Diagnostics build a multi-token range this
    /// way (first token's span joined with the last token's).
    ///
    /// Inverted inputs are tolerated the same way [`Self::new`] tolerates
    /// them: the result is the componentwise `min` / `max`, never normalised.
    #[inline]
    pub const fn join(self, other: Self) -> Self {
        Self {
            start: if self.start <= other.start {
                self.start
            } else {
                other.start
            },
            end: if self.end >= other.end {
                self.end
            } else {
                other.end
            },
        }
    }
}

impl From<Span> for Range<usize> {
    #[inline]
    fn from(s: Span) -> Self {
        s.as_range()
    }
}

/// A value carrying its source span.
///
/// The ordering is **span-first**: see the hand-written [`Ord`] /
/// [`PartialOrd`] impls below. A `#[derive]` would order by declaration
/// order — `value` before `span` — so sorting a batch of spanned tokens
/// would group them by token rather than by position in the file, which is
/// never what a caller sorting source-tagged data wants.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

/// Span-first ordering, `value` only as the tie-break. Comparing both fields
/// keeps `cmp(a, b) == Equal` exactly when `a == b`, as [`Ord`] requires of
/// the derived [`PartialEq`].
impl<T: Ord> Ord for Spanned<T> {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.span
            .cmp(&other.span)
            .then_with(|| self.value.cmp(&other.value))
    }
}

/// Span-first, matching [`Ord`]. A `T` with a partial order can still leave
/// two same-span values incomparable, so the tie-break is delegated rather
/// than forced.
impl<T: PartialOrd> PartialOrd for Spanned<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.span.cmp(&other.span) {
            std::cmp::Ordering::Equal => self.value.partial_cmp(&other.value),
            ordering => Some(ordering),
        }
    }
}

impl<T> Spanned<T> {
    #[inline]
    pub const fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }

    #[inline]
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Spanned<U> {
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

    /// llvmkit-specific: half-open containment. Closest upstream:
    /// `FileLocRange::contains_loc` in `llvmkit-asmparser`, itself the shape
    /// of `bool contains(SMLoc) const` on `llvm::SMRange`.
    #[test]
    fn span_contains_is_half_open() {
        let s = Span::new(3, 7);
        assert!(!s.contains(2));
        assert!(s.contains(3));
        assert!(s.contains(6));
        assert!(!s.contains(7));
        // An empty span contains nothing, not even its own offset.
        assert!(!Span::empty_at(4).contains(4));
    }

    /// llvmkit-specific: span hull. Closest upstream: the `SMRange(Start,
    /// End)` widening `LLLexer`/`LLParser` do by hand when a diagnostic spans
    /// several tokens.
    #[test]
    fn span_join_is_the_hull() {
        assert_eq!(Span::new(2, 4).join(Span::new(8, 9)), Span::new(2, 9));
        assert_eq!(Span::new(8, 9).join(Span::new(2, 4)), Span::new(2, 9));
        // Nesting keeps the outer span; joining with self is identity.
        assert_eq!(Span::new(0, 10).join(Span::new(3, 5)), Span::new(0, 10));
        let s = Span::new(3, 5);
        assert_eq!(s.join(s), s);
    }

    /// llvmkit-specific: `Spanned<T>` sorts by source position, not by value.
    /// Closest upstream: no equivalent — LLVM tags diagnostics with `SMLoc`
    /// but never sorts the pairs.
    #[test]
    fn spanned_orders_by_span_first() {
        let mut items = [
            Spanned::new(9u32, Span::new(10, 11)),
            Spanned::new(1u32, Span::new(20, 21)),
            Spanned::new(5u32, Span::new(0, 1)),
        ];
        items.sort();
        assert_eq!(
            items.iter().map(|s| s.span.start).collect::<Vec<_>>(),
            [0, 10, 20]
        );
        // Same span falls back to the value, so the order stays total.
        let span = Span::new(4, 6);
        assert!(Spanned::new(1u32, span) < Spanned::new(2u32, span));
    }
}
