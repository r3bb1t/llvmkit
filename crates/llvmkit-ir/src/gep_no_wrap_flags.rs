//! GEP no-wrap flags. Mirrors `llvm/include/llvm/IR/GEPNoWrapFlags.h`.
//!
//! `inbounds` implies `nusw`. The C++ implementation enforces this via an
//! `assert` in the private constructor; this Rust port preserves the
//! invariant in [`GepNoWrapFlags::from_bits_canonical`] and in the named
//! constructors `inbounds()` / `all()` / `nusw()`. Direct manipulation of
//! the underlying bits via `from_bits_truncate` is allowed but the
//! canonicalising helper is preferred.

use core::fmt;

bitflags::bitflags! {
    /// Flags for the `getelementptr` instruction / expression.
    ///
    /// Bit assignments mirror `GEPNoWrapFlags` (`GEPNoWrapFlags.h`):
    ///
    /// ```text
    /// InBoundsFlag = 1 << 0
    /// NUSWFlag     = 1 << 1
    /// NUWFlag      = 1 << 2
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct GepNoWrapFlags: u8 {
        /// `inbounds`. Implies [`NUSW`](Self::NUSW).
        const IN_BOUNDS = 1 << 0;
        /// `nusw` (no unsigned-signed wrap).
        const NUSW      = 1 << 1;
        /// `nuw` (no unsigned wrap).
        const NUW       = 1 << 2;
    }
}

impl GepNoWrapFlags {
    /// Sanitise a raw flag set so `inbounds` always implies `nusw`.
    /// Mirrors the canonicalisation in the private C++ constructor
    /// (`GEPNoWrapFlags.h`).
    #[inline]
    pub const fn from_bits_canonical(raw: u8) -> Self {
        let v = Self::from_bits_truncate(raw);
        if v.contains(Self::IN_BOUNDS) {
            v.union(Self::NUSW)
        } else {
            v
        }
    }

    /// `inbounds` (implies `nusw`). Mirrors `GEPNoWrapFlags::inBounds`
    /// (`GEPNoWrapFlags.h`).
    #[inline]
    pub const fn inbounds() -> Self {
        Self::IN_BOUNDS.union(Self::NUSW)
    }

    /// All three flags set. Mirrors `GEPNoWrapFlags::all`.
    #[inline]
    pub const fn all_flags() -> Self {
        Self::IN_BOUNDS.union(Self::NUSW).union(Self::NUW)
    }

    /// Mirrors `intersectForOffsetAdd` (`GEPNoWrapFlags.h`).
    pub fn intersect_for_offset_add(self, other: Self) -> Self {
        let mut r = self & other;
        if !r.contains(Self::IN_BOUNDS) && r.contains(Self::NUSW) {
            r.remove(Self::NUSW | Self::IN_BOUNDS);
        }
        r
    }

    /// Mirrors `intersectForReassociate` (`GEPNoWrapFlags.h`).
    pub fn intersect_for_reassociate(self, other: Self) -> Self {
        let r = self & other;
        if !r.contains(Self::NUW) {
            Self::empty()
        } else {
            r
        }
    }
}

impl fmt::Display for GepNoWrapFlags {
    /// IR-textual rendering: space-separated keywords in LLVM's canonical
    /// order (`inbounds nusw nuw`). Empty if no flags set.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sep = "";
        let mut emit = |s: &str, f: &mut fmt::Formatter<'_>| -> fmt::Result {
            f.write_str(sep)?;
            f.write_str(s)?;
            sep = " ";
            Ok(())
        };
        if self.contains(Self::IN_BOUNDS) {
            emit("inbounds", f)?;
        }
        if self.contains(Self::NUSW) && !self.contains(Self::IN_BOUNDS) {
            // LLVM only prints `nusw` explicitly when `inbounds` is absent.
            emit("nusw", f)?;
        }
        if self.contains(Self::NUW) {
            emit("nuw", f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbounds_implies_nusw() {
        assert!(GepNoWrapFlags::inbounds().contains(GepNoWrapFlags::NUSW));
        // raw construction with only IN_BOUNDS still canonicalises:
        assert!(
            GepNoWrapFlags::from_bits_canonical(GepNoWrapFlags::IN_BOUNDS.bits())
                .contains(GepNoWrapFlags::NUSW)
        );
    }

    #[test]
    fn intersect_offset_add_drops_orphan_nusw() {
        // x has nusw, y has nusw — intersection has nusw. But the C++ rule
        // says: without inbounds we cannot preserve nusw across an offset add.
        let x = GepNoWrapFlags::NUSW;
        let y = GepNoWrapFlags::NUSW;
        let r = x.intersect_for_offset_add(y);
        assert!(!r.contains(GepNoWrapFlags::NUSW));
    }

    #[test]
    fn intersect_reassociate_requires_nuw() {
        let x = GepNoWrapFlags::inbounds();
        let y = GepNoWrapFlags::inbounds();
        // No nuw -> result is empty.
        assert_eq!(x.intersect_for_reassociate(y), GepNoWrapFlags::empty());

        let x = GepNoWrapFlags::all_flags();
        let y = GepNoWrapFlags::all_flags();
        // With nuw on both -> preserved.
        assert!(x.intersect_for_reassociate(y).contains(GepNoWrapFlags::NUW));
    }

    #[test]
    fn display_inbounds_hides_nusw() {
        let f = GepNoWrapFlags::inbounds();
        assert_eq!(format!("{f}"), "inbounds");
    }

    #[test]
    fn display_nusw_only() {
        let f = GepNoWrapFlags::NUSW;
        assert_eq!(format!("{f}"), "nusw");
    }

    #[test]
    fn display_all() {
        let f = GepNoWrapFlags::all_flags();
        assert_eq!(format!("{f}"), "inbounds nuw");
    }
}
