//! Alignment newtype. Mirrors `Support/Alignment.h`.
//!
//! LLVM's `Align` is "alignment in bytes, always a power of two,
//! stored as the log2 shift amount". A bare `u64` would let callers
//! construct invalid alignments (zero, three, …); we wrap it in an
//! `Align` newtype that validates on construction and exposes the
//! original byte count via [`Align::value`].

use core::num::NonZeroU8;

use crate::IrError;
use crate::IrResult;

/// Alignment in bytes, always a power of two. Stored as the log2
/// shift amount (so `Align::new(8)?` stores `3`). Mirrors LLVM's
/// `Align` in `Support/Alignment.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Align(NonZeroU8);

impl Align {
    /// Alignment of 1 (the natural alignment of every value).
    pub const ONE: Align = Align(match NonZeroU8::new(1) {
        Some(v) => v,
        None => unreachable!(),
    });

    /// Construct from a byte count. Errors if `bytes == 0` or
    /// `bytes` is not a power of two. Mirrors LLVM's `Align::Align`
    /// assertion.
    pub fn new(bytes: u64) -> IrResult<Self> {
        if bytes == 0 || !bytes.is_power_of_two() {
            return Err(IrError::InvalidOperation {
                message: "alignment must be a non-zero power of two",
            });
        }
        let shift = bytes.trailing_zeros();
        // LLVM's `Align` stores up to `Value <= 64` (max alignment is
        // 1 << 63 bytes). Our `NonZeroU8` representation accepts the
        // same range plus one (shift = 1..=63 is the legal set; shift
        // = 0 is `Align::ONE`).
        let shift_u8 = u8::try_from(shift + 1).map_err(|_| IrError::InvalidOperation {
            message: "alignment exceeds 1 << 63",
        })?;
        Ok(Align(NonZeroU8::new(shift_u8).unwrap_or_else(|| {
            unreachable!("shift+1 is always >= 1 by construction")
        })))
    }

    /// Alignment in bytes (`1 << log2`).
    #[inline]
    pub fn value(self) -> u64 {
        1u64 << (self.0.get() - 1)
    }

    /// Log2 of the alignment.
    #[inline]
    pub fn log2_value(self) -> u8 {
        self.0.get() - 1
    }
}

/// Optional alignment slot (`alloca`, `load`, `store`). `None` means
/// "use the type's ABI alignment". Mirrors `MaybeAlign` in
/// `Support/Alignment.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MaybeAlign(pub(crate) Option<Align>);

impl MaybeAlign {
    /// `None` — use ABI alignment.
    pub const NONE: MaybeAlign = MaybeAlign(None);

    /// Wrap an explicit alignment.
    #[inline]
    pub const fn new(a: Align) -> Self {
        Self(Some(a))
    }

    /// Underlying alignment, if any.
    #[inline]
    pub fn align(self) -> Option<Align> {
        self.0
    }
}

impl From<Align> for MaybeAlign {
    #[inline]
    fn from(a: Align) -> Self {
        MaybeAlign(Some(a))
    }
}

/// Upstream provenance: mirrors the `Align` / `MaybeAlign` value class
/// from `llvm/include/llvm/Support/Alignment.h`. Each `#[test]` cites the
/// specific constructor or invariant it ports.
#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `Align(uint64_t)` constructor from
    /// `include/llvm/Support/Alignment.h` (`isPowerOf2_64`-protected store).
    #[test]
    fn align_round_trip() -> IrResult<()> {
        for bytes in [1u64, 2, 4, 8, 16, 32, 64, 128, 256, 4096] {
            let a = Align::new(bytes)?;
            assert_eq!(a.value(), bytes);
        }
        Ok(())
    }

    /// Mirrors `Align(0)` assertion from `include/llvm/Support/Alignment.h`
    /// (zero is rejected because alignments must be positive).
    #[test]
    fn align_rejects_zero() {
        assert!(Align::new(0).is_err());
    }

    /// Mirrors `assert(isPowerOf2_64(Value))` in
    /// `include/llvm/Support/Alignment.h::Align`.
    #[test]
    fn align_rejects_non_power_of_two() {
        for bytes in [3u64, 5, 6, 7, 9, 10] {
            assert!(Align::new(bytes).is_err(), "expected error for {bytes}");
        }
    }

    /// Mirrors `MaybeAlign()` default constructor from
    /// `include/llvm/Support/Alignment.h`.
    #[test]
    fn maybe_align_default_is_none() {
        assert_eq!(MaybeAlign::default(), MaybeAlign::NONE);
        assert!(MaybeAlign::default().align().is_none());
    }
}
