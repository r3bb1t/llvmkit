//! Fast-math flags. Mirrors `llvm/include/llvm/IR/FMF.h`.
//!
//! LLVM stores these in `Value::SubclassOptionalData` (7 bits). Rust users
//! get a strongly-typed [`bitflags`] struct so the constants are introspectable
//! and union/intersection are normal Rust ops.

use core::fmt;

bitflags::bitflags! {
    /// Fast-math flags.
    ///
    /// Mirrors the bit assignments in
    /// `FastMathFlags` (`FMF.h`):
    ///
    /// ```text
    /// AllowReassoc    = 1 << 0
    /// NoNaNs          = 1 << 1
    /// NoInfs          = 1 << 2
    /// NoSignedZeros   = 1 << 3
    /// AllowReciprocal = 1 << 4
    /// AllowContract   = 1 << 5
    /// ApproxFunc      = 1 << 6
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct FastMathFlags: u8 {
        const ALLOW_REASSOC    = 1 << 0;
        const NO_NANS          = 1 << 1;
        const NO_INFS          = 1 << 2;
        const NO_SIGNED_ZEROS  = 1 << 3;
        const ALLOW_RECIPROCAL = 1 << 4;
        const ALLOW_CONTRACT   = 1 << 5;
        const APPROX_FUNC      = 1 << 6;

        /// All flags set ("fast"). Mirrors `FastMathFlags::AllFlagsMask`.
        const ALL = Self::ALLOW_REASSOC.bits()
                  | Self::NO_NANS.bits()
                  | Self::NO_INFS.bits()
                  | Self::NO_SIGNED_ZEROS.bits()
                  | Self::ALLOW_RECIPROCAL.bits()
                  | Self::ALLOW_CONTRACT.bits()
                  | Self::APPROX_FUNC.bits();
    }
}

impl FastMathFlags {
    /// Returns the convenience "fast" combination (every flag set).
    /// Mirrors `FastMathFlags::getFast` (`FMF.h`).
    #[inline]
    pub const fn fast() -> Self {
        Self::ALL
    }

    /// `true` iff every flag is set. Mirrors `FastMathFlags::isFast`.
    #[inline]
    pub const fn is_fast(self) -> bool {
        self.bits() == Self::ALL.bits()
    }

    /// Bits eligible for `intersectRewrite`. Mirrors `FMF.h`.
    pub const REWRITE_MASK: Self = Self::ALLOW_REASSOC
        .union(Self::ALLOW_RECIPROCAL)
        .union(Self::ALLOW_CONTRACT)
        .union(Self::APPROX_FUNC);

    /// Bits eligible for `unionValue`. Mirrors `FMF.h`.
    pub const VALUE_MASK: Self = Self::NO_NANS
        .union(Self::NO_INFS)
        .union(Self::NO_SIGNED_ZEROS);

    /// Mirrors `FastMathFlags::intersectRewrite` (`FMF.h`).
    #[inline]
    pub const fn intersect_rewrite(lhs: Self, rhs: Self) -> Self {
        let raw = Self::REWRITE_MASK.bits() & lhs.bits() & rhs.bits();
        Self::from_bits_truncate(raw)
    }

    /// Mirrors `FastMathFlags::unionValue` (`FMF.h`).
    #[inline]
    pub const fn union_value(lhs: Self, rhs: Self) -> Self {
        let raw = Self::VALUE_MASK.bits() & (lhs.bits() | rhs.bits());
        Self::from_bits_truncate(raw)
    }
}

impl fmt::Display for FastMathFlags {
    /// Print the IR fast-math suffix, in canonical LLVM order.
    ///
    /// All flags set prints as `fast`; otherwise individual keywords are
    /// emitted in the same order as `FastMathFlags::print` in
    /// `lib/IR/AsmWriter.cpp`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_fast() {
            return f.write_str("fast");
        }
        let mut sep = "";
        let mut emit = |s: &str, f: &mut fmt::Formatter<'_>| -> fmt::Result {
            f.write_str(sep)?;
            f.write_str(s)?;
            sep = " ";
            Ok(())
        };
        if self.contains(Self::ALLOW_REASSOC) {
            emit("reassoc", f)?;
        }
        if self.contains(Self::NO_NANS) {
            emit("nnan", f)?;
        }
        if self.contains(Self::NO_INFS) {
            emit("ninf", f)?;
        }
        if self.contains(Self::NO_SIGNED_ZEROS) {
            emit("nsz", f)?;
        }
        if self.contains(Self::ALLOW_RECIPROCAL) {
            emit("arcp", f)?;
        }
        if self.contains(Self::ALLOW_CONTRACT) {
            emit("contract", f)?;
        }
        if self.contains(Self::APPROX_FUNC) {
            emit("afn", f)?;
        }
        Ok(())
    }
}

/// Upstream provenance: mirrors `FastMathFlags` from
/// `include/llvm/IR/FMF.h`. Closest unit-test:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`.
/// Display assertions track `WriteOptimizationInfo` in
/// `lib/IR/AsmWriter.cpp`.
#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `FastMathFlags::setFast()` / `isFast()` in
    /// `include/llvm/IR/FMF.h`.
    #[test]
    fn fast_is_all() {
        assert!(FastMathFlags::fast().is_fast());
        assert_eq!(FastMathFlags::fast(), FastMathFlags::ALL);
    }

    /// Mirrors `WriteOptimizationInfo` `fast` short-form printer case in
    /// `lib/IR/AsmWriter.cpp`.
    #[test]
    fn display_fast() {
        assert_eq!(format!("{}", FastMathFlags::fast()), "fast");
    }

    /// Mirrors `WriteOptimizationInfo` partial-flag printer in
    /// `lib/IR/AsmWriter.cpp` (each set bit prints its mnemonic).
    #[test]
    fn display_partial() {
        let f = FastMathFlags::NO_NANS | FastMathFlags::NO_INFS;
        assert_eq!(format!("{f}"), "nnan ninf");
    }

    /// Mirrors `WriteOptimizationInfo` empty case in `lib/IR/AsmWriter.cpp`
    /// (no flags -> no output).
    #[test]
    fn display_empty() {
        assert_eq!(format!("{}", FastMathFlags::empty()), "");
    }

    /// Mirrors `FastMathFlags::intersectRewrite` semantics in
    /// `include/llvm/IR/FMF.h` (only rewrite-permitting bits survive).
    #[test]
    fn intersect_rewrite_drops_value_bits() {
        let lhs = FastMathFlags::ALL;
        let rhs = FastMathFlags::ALL;
        let r = FastMathFlags::intersect_rewrite(lhs, rhs);
        assert!(!r.contains(FastMathFlags::NO_NANS));
        assert!(r.contains(FastMathFlags::ALLOW_REASSOC));
    }

    /// Mirrors `FastMathFlags::unionValue` semantics in
    /// `include/llvm/IR/FMF.h` (only value-affecting bits survive).
    #[test]
    fn union_value_drops_rewrite_bits() {
        let lhs = FastMathFlags::ALL;
        let rhs = FastMathFlags::ALL;
        let r = FastMathFlags::union_value(lhs, rhs);
        assert!(r.contains(FastMathFlags::NO_NANS));
        assert!(!r.contains(FastMathFlags::ALLOW_REASSOC));
    }
}
