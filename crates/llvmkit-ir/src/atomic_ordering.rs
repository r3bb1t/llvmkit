//! Atomic ordering enum. Mirrors `llvm/include/llvm/Support/AtomicOrdering.h`.

use core::fmt;
use core::str::FromStr;

use crate::error::IrError;

/// Memory ordering for atomic ops. Mirrors `enum class AtomicOrdering`
/// in `Support/AtomicOrdering.h`. The discriminator values match the
/// upstream layout (`NotAtomic = 0`, ..., `SequentiallyConsistent = 7`),
/// and `3` (the unused `Consume` slot) is intentionally absent so a
/// future `from_u8` can reject it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtomicOrdering {
    /// Non-atomic. The default for non-atomic loads / stores. Mirrors
    /// `AtomicOrdering::NotAtomic`.
    NotAtomic = 0,
    /// Same value visible to all threads but no synchronization
    /// guarantees. Mirrors `AtomicOrdering::Unordered`.
    Unordered = 1,
    /// `monotonic` (relaxed). Mirrors `AtomicOrdering::Monotonic`.
    Monotonic = 2,
    /// `acquire`. Mirrors `AtomicOrdering::Acquire`.
    Acquire = 4,
    /// `release`. Mirrors `AtomicOrdering::Release`.
    Release = 5,
    /// `acq_rel`. Mirrors `AtomicOrdering::AcquireRelease`.
    AcquireRelease = 6,
    /// `seq_cst`. Mirrors `AtomicOrdering::SequentiallyConsistent`.
    SequentiallyConsistent = 7,
}

impl AtomicOrdering {
    /// Every variant, in discriminant order. Exists so [`FromStr`] can invert
    /// [`as_str`](Self::as_str) by searching this list instead of carrying a
    /// second copy of the spelling table; keep it in step with the enum (the
    /// in-file drift-lock test's exhaustive `match` is the tripwire). Not
    /// derived from the discriminant range, because `3` (the unused `Consume`
    /// slot) is deliberately absent.
    pub const VARIANTS: [Self; 7] = [
        Self::NotAtomic,
        Self::Unordered,
        Self::Monotonic,
        Self::Acquire,
        Self::Release,
        Self::AcquireRelease,
        Self::SequentiallyConsistent,
    ];

    /// IR text spelling. Mirrors `Support/AtomicOrdering.h::toIRString`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotAtomic => "not_atomic",
            Self::Unordered => "unordered",
            Self::Monotonic => "monotonic",
            Self::Acquire => "acquire",
            Self::Release => "release",
            Self::AcquireRelease => "acq_rel",
            Self::SequentiallyConsistent => "seq_cst",
        }
    }
}

impl fmt::Display for AtomicOrdering {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AtomicOrdering {
    type Err = IrError;

    /// Inverse of [`Display`](fmt::Display) / [`AtomicOrdering::as_str`] over
    /// [`VARIANTS`](Self::VARIANTS) — the spellings live only in `as_str`, so
    /// the two directions cannot drift. Mirrors
    /// `LLParser::parseScopeAndOrdering`'s keyword block
    /// (`lib/AsmParser/LLParser.cpp`); note `not_atomic` is
    /// `toIRString`'s spelling for the state a `.ll` file expresses by
    /// omitting the keyword entirely, and is accepted here for round-trip
    /// symmetry.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .into_iter()
            .find(|ordering| ordering.as_str() == s)
            .ok_or_else(|| IrError::InvalidKeyword {
                target: "atomic ordering",
                keyword: s.to_string(),
            })
    }
}

/// Upstream provenance: mirrors `enum class AtomicOrdering` and `toIRString`
/// in `include/llvm/Support/AtomicOrdering.h`.
///
/// The test below is **llvmkit-specific**: it is the `Display`/`FromStr`
/// drift lock, the analogue of `attribute_td_drift.rs`. Upstream's
/// `toIRString` and `LLParser::parseScopeAndOrdering` cannot drift apart —
/// both switch on the same enum — so there is nothing to port.
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: Display/FromStr drift lock for [`AtomicOrdering`].
    /// The exhaustive `match` makes a new variant a compile error here, which
    /// is the prompt to extend `VARIANTS`; the count assertion then fails
    /// until it is extended.
    #[test]
    fn atomic_ordering_display_and_from_str_round_trip() {
        for ordering in AtomicOrdering::VARIANTS {
            match ordering {
                AtomicOrdering::NotAtomic
                | AtomicOrdering::Unordered
                | AtomicOrdering::Monotonic
                | AtomicOrdering::Acquire
                | AtomicOrdering::Release
                | AtomicOrdering::AcquireRelease
                | AtomicOrdering::SequentiallyConsistent => {}
            }
            assert_eq!(ordering.to_string().parse::<AtomicOrdering>(), Ok(ordering));
        }
        // One entry per arm above.
        assert_eq!(AtomicOrdering::VARIANTS.len(), 7);
        // `consume` is the discriminant llvmkit deliberately omits, and the
        // C++ spelling `acquire_release` is not the IR one.
        assert!("consume".parse::<AtomicOrdering>().is_err());
        assert_eq!(
            "acquire_release".parse::<AtomicOrdering>(),
            Err(IrError::InvalidKeyword {
                target: "atomic ordering",
                keyword: "acquire_release".to_string(),
            })
        );
    }
}
