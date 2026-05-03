//! Line / column source locations for parser diagnostics.
//!
//! Direct port of `llvm/include/llvm/AsmParser/FileLoc.h`. [`FileLoc`] is a
//! `(line, col)` pair with 0-based components; [`FileLocRange`] is the
//! half-open range `[start, end)` produced by `LLParser` while it consumes
//! tokens.
//!
//! These are the *external* diagnostic surface — the lexer keeps working in
//! byte offsets via [`llvmkit_support::Span`], and the parser projects to
//! [`FileLoc`] only when it needs to expose a location to callers. Mirrors
//! upstream's split between `SMLoc` (byte pointer) and `FileLoc` (line/col).

use core::cmp::Ordering;

/// Line / column pair. Both components are **0-based** to match upstream
/// (`FileLoc.h`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct FileLoc {
    /// 0-based line number.
    pub line: u32,
    /// 0-based column number.
    pub col: u32,
}

impl FileLoc {
    /// Construct a [`FileLoc`]. Mirrors the two-argument upstream constructor.
    #[inline]
    pub const fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

impl PartialOrd for FileLoc {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Lexicographic ordering on `(line, col)`. Mirrors the explicit
/// `operator<` / `operator<=` overloads in upstream `FileLoc.h`.
impl Ord for FileLoc {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        match self.line.cmp(&other.line) {
            Ordering::Equal => self.col.cmp(&other.col),
            ord => ord,
        }
    }
}

impl From<(u32, u32)> for FileLoc {
    #[inline]
    fn from((line, col): (u32, u32)) -> Self {
        Self::new(line, col)
    }
}

/// Half-open range `[start, end)` of [`FileLoc`] positions.
///
/// Direct port of upstream `FileLocRange`; constructors enforce
/// `start <= end` via the typed [`FileLocRange::new`] / [`FileLocRange::try_new`]
/// pair (upstream uses `assert(Start <= End)` in the constructor — the
/// fallible `try_new` makes that contract surface-visible without panicking
/// production code).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct FileLocRange {
    pub start: FileLoc,
    pub end: FileLoc,
}

impl FileLocRange {
    /// Construct a [`FileLocRange`] from sorted endpoints.
    ///
    /// Returns `None` if `start > end`. The upstream `FileLocRange(Start, End)`
    /// constructor `assert`s the same invariant; we surface it as an
    /// `Option`-returning constructor so callers must acknowledge the contract
    /// without crashing on malformed inputs.
    #[inline]
    pub const fn try_new(start: FileLoc, end: FileLoc) -> Option<Self> {
        // Manual `<=` because const trait dispatch is unstable.
        let ord = if start.line < end.line {
            true
        } else if start.line == end.line {
            start.col <= end.col
        } else {
            false
        };
        if ord { Some(Self { start, end }) } else { None }
    }

    /// Construct a [`FileLocRange`] when the caller has already validated
    /// `start <= end`. Mirrors upstream's debug-asserted constructor; in
    /// release builds the upstream form silently constructs an inverted range,
    /// so this Rust analogue clamps to an empty range at `start` for the
    /// inverted case to keep downstream invariants intact.
    #[inline]
    pub fn new(start: FileLoc, end: FileLoc) -> Self {
        match Self::try_new(start, end) {
            Some(r) => r,
            None => Self { start, end: start },
        }
    }

    /// Empty range `[start, start)`.
    #[inline]
    pub const fn empty_at(start: FileLoc) -> Self {
        Self { start, end: start }
    }

    /// `true` iff `loc` falls inside the half-open range. Mirrors upstream
    /// `bool contains(FileLoc) const`.
    #[inline]
    pub fn contains_loc(&self, loc: FileLoc) -> bool {
        self.start <= loc && loc < self.end
    }

    /// `true` iff `other` is fully contained. Mirrors upstream
    /// `bool contains(FileLocRange) const`.
    #[inline]
    pub fn contains_range(&self, other: FileLocRange) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ports the `operator<` / `operator<=` / `operator==` semantics declared
    /// in `llvm/include/llvm/AsmParser/FileLoc.h`. Upstream has no dedicated
    /// unit test for these inline operators; the header itself is the
    /// canonical source.
    #[test]
    fn file_loc_orderings() {
        let a = FileLoc::new(1, 4);
        let b = FileLoc::new(1, 5);
        let c = FileLoc::new(2, 0);

        assert!(a < b);
        assert!(b < c);
        assert!(a <= a);
        assert!(a == FileLoc::new(1, 4));
        assert!(b > a && c > b);
    }

    /// Ports the `FileLocRange::contains(FileLoc)` arm of `FileLoc.h`.
    #[test]
    fn range_contains_loc_is_half_open() {
        let r = FileLocRange::new(FileLoc::new(1, 0), FileLoc::new(1, 5));
        assert!(r.contains_loc(FileLoc::new(1, 0)));
        assert!(r.contains_loc(FileLoc::new(1, 4)));
        // Half-open: end is *not* included.
        assert!(!r.contains_loc(FileLoc::new(1, 5)));
        assert!(!r.contains_loc(FileLoc::new(0, 9)));
    }

    /// Ports the `FileLocRange::contains(FileLocRange)` arm.
    #[test]
    fn range_contains_range() {
        let outer = FileLocRange::new(FileLoc::new(1, 0), FileLoc::new(3, 0));
        let inner = FileLocRange::new(FileLoc::new(1, 2), FileLoc::new(2, 8));
        assert!(outer.contains_range(inner));
        assert!(outer.contains_range(outer));

        let crossing = FileLocRange::new(FileLoc::new(2, 0), FileLoc::new(4, 0));
        assert!(!outer.contains_range(crossing));
    }

    /// llvmkit-specific: surfaces the upstream `assert(Start <= End)` as an
    /// `Option`-returning constructor. Closest upstream anchor:
    /// `FileLocRange::FileLocRange(FileLoc, FileLoc)` in `FileLoc.h`.
    #[test]
    fn try_new_rejects_inverted_range() {
        let s = FileLoc::new(2, 0);
        let e = FileLoc::new(1, 0);
        assert!(FileLocRange::try_new(s, e).is_none());
        // Equal endpoints are an empty range, not an inverted one.
        assert_eq!(FileLocRange::try_new(s, s), Some(FileLocRange::empty_at(s)));
    }
}
