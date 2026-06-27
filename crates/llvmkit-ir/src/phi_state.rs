//! Construction-lifecycle typestate for [`crate::PhiInst`] (Doctrine
//! D1 -- make invalid states unrepresentable).
//!
//! LLVM's `Verifier::visitPHINode` (`lib/IR/Verifier.cpp`) checks at
//! runtime that a phi has one incoming pair per predecessor edge and
//! that no predecessor was missed. llvmkit gives the common construction path an
//! explicit view state: a freshly-built phi is [`Open`] and accepts
//! [`crate::PhiInst::add_incoming`]; calling [`crate::PhiInst::finish`] returns a
//! [`Closed`] view that no longer exposes `add_incoming`.
//!
//! Open phi handles are linear (`!Copy` / `!Clone`): [`crate::PhiInst::finish`]
//! consumes the only open capability, so retained copies cannot continue adding
//! incoming edges. Runtime-side coherence rules, such as the incoming-block set
//! matching the predecessor set, remain verifier responsibilities.

use crate::value::sealed;

/// Sealed marker trait for the [`crate::PhiInst`] state-machine type
/// parameter. The two implementors are [`Open`] and [`Closed`];
/// external crates cannot invent new states.
pub trait PhiState: sealed::Sealed + 'static {
    /// `true` for [`Closed`], `false` for [`Open`].
    const IS_CLOSED: bool;
}

/// Marker: the phi accepts further `add_incoming` calls.
#[derive(Debug, Clone, Copy)]
pub struct Open;

/// Marker: this phi view has been finalised. `add_incoming` is no longer
/// reachable on the closed view; only read accessors are.
#[derive(Debug, Clone, Copy)]
pub struct Closed;

impl sealed::Sealed for Open {}
impl sealed::Sealed for Closed {}

impl PhiState for Open {
    const IS_CLOSED: bool = false;
}
impl PhiState for Closed {
    const IS_CLOSED: bool = true;
}
