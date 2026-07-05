//! Construction-lifecycle typestate for [`crate::BasicBlock`]
//! (Doctrine D1 — make invalid states unrepresentable).
//!
//! LLVM enforces "every well-formed basic block ends in exactly one
//! terminator and contains nothing after that terminator" at runtime
//! in `Verifier::visitBasicBlock` (`lib/IR/Verifier.cpp`). llvmkit
//! models the common builder path with a termination-state view: an
//! [`Unterminated`] block handle can be positioned for appends, and
//! terminator builders return a [`Terminated`] view of the insertion
//! block. Code that follows that returned view cannot append via
//! [`crate::IRBuilder::position_at_end`].
//!
//! Cranelift calls this state "filled"; llvmkit uses LLVM's own
//! "terminator" vocabulary. The word "sealed" is reserved for the
//! Braun-SSA predecessor-set sense used by `SsaBuilder`.
//!
//! `BasicBlock` is a linear insertion-capability handle (`!Copy` / `!Clone`).
//! Copyable cross-block references use [`crate::BasicBlockLabel`], so a
//! retained label can still name a predecessor after termination without
//! being accepted by [`crate::IRBuilder::position_at_end`].

use crate::value::sealed;

/// Sealed marker trait for the [`crate::BasicBlock`] termination-state
/// type parameter. The two implementors are [`Unterminated`] and
/// [`Terminated`]; external crates cannot invent new states.
pub trait BlockTerminationState: sealed::Sealed + 'static {
    /// `true` for [`Terminated`], `false` for [`Unterminated`]. Useful for
    /// formatter / diagnostic helpers that need to distinguish at
    /// runtime without per-state generics.
    const IS_TERMINATED: bool;
}

/// Marker: the block has no terminator yet. `IRBuilder` may be
/// positioned at this block and emit instructions into it.
#[derive(Debug, Clone, Copy)]
pub struct Unterminated;

/// Marker: the block has a terminator. No further instructions can
/// be appended via `IRBuilder`.
#[derive(Debug, Clone, Copy)]
pub struct Terminated;

impl sealed::Sealed for Unterminated {}
impl sealed::Sealed for Terminated {}

impl BlockTerminationState for Unterminated {
    const IS_TERMINATED: bool = false;
}
impl BlockTerminationState for Terminated {
    const IS_TERMINATED: bool = true;
}
