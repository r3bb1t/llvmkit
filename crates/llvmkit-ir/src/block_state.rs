//! Construction-lifecycle typestate for [`crate::BasicBlock`]
//! (Doctrine D1 — make invalid states unrepresentable).
//!
//! LLVM enforces "every well-formed basic block ends in exactly one
//! terminator and contains nothing after that terminator" at runtime
//! in `Verifier::visitBasicBlock` (`lib/IR/Verifier.cpp`). llvmkit
//! moves the rule to compile time: an [`Unsealed`] block has no
//! terminator and accepts further instructions; a [`Sealed`] block has
//! a terminator and is closed. Once the IRBuilder consumes its
//! insertion block via a terminator-emitting build, the block is
//! returned in the `Sealed` state and can only be referenced (e.g. as
//! a branch target or phi predecessor) -- never appended to.
//!
//! The seal state is *informational*: [`crate::BasicBlock`] stays
//! `Copy` so the same block id can be passed as a phi predecessor
//! after it was sealed. The compile-time guarantee comes from
//! [`crate::IRBuilder::position_at_end`] only accepting an
//! [`Unsealed`] block.

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
