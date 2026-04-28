//! Atomic ordering enum. Mirrors `llvm/include/llvm/Support/AtomicOrdering.h`.

use core::fmt;

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
    /// IR text spelling. Mirrors `Support/AtomicOrdering.h::toIRString`.
    pub const fn to_ir_string(self) -> &'static str {
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
        f.write_str(self.to_ir_string())
    }
}
