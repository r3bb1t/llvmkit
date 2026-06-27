//! Construction-lifecycle typestate for [`crate::BasicBlock`]
//! (Doctrine D1 — make invalid states unrepresentable).
//!
//! LLVM enforces "every well-formed basic block ends in exactly one
//! terminator and contains nothing after that terminator" at runtime
//! in `Verifier::visitBasicBlock` (`lib/IR/Verifier.cpp`). llvmkit
//! models the common builder path with a seal-state view: an [`Unsealed`] block
//! handle can be positioned for appends, and terminator builders return a
//! [`Sealed`] view of the insertion block. Code that follows that returned view
//! cannot append via [`crate::IRBuilder::position_at_end`].
//!
//! `BasicBlock` is a linear insertion-capability handle (`!Copy` / `!Clone`).
//! Copyable cross-block references use [`crate::BasicBlockLabel`], so a retained
//! label can still name a predecessor after sealing without being accepted by
//! [`crate::IRBuilder::position_at_end`].

use crate::value::sealed;

/// Sealed marker trait for the [`crate::BasicBlock`] seal-state
/// type parameter. The two implementors are [`Unsealed`] and
/// [`Sealed`]; external crates cannot invent new states.
pub trait BlockSealState: sealed::Sealed + 'static {
    /// `true` for [`Sealed`], `false` for [`Unsealed`]. Useful for
    /// formatter / diagnostic helpers that need to distinguish at
    /// runtime without per-state generics.
    const IS_SEALED: bool;
}

/// Marker: the block has no terminator yet. `IRBuilder` may be
/// positioned at this block and emit instructions into it.
#[derive(Debug, Clone, Copy)]
pub struct Unsealed;

/// Marker: the block has a terminator. No further instructions can
/// be appended via `IRBuilder`.
#[derive(Debug, Clone, Copy)]
pub struct Sealed;

impl sealed::Sealed for Unsealed {}
impl sealed::Sealed for Sealed {}

impl BlockSealState for Unsealed {
    const IS_SEALED: bool = false;
}
impl BlockSealState for Sealed {
    const IS_SEALED: bool = true;
}
