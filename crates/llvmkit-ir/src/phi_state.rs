//! Construction-lifecycle typestate for [`crate::PhiInst`] (Doctrine
//! D1 -- make invalid states unrepresentable).
//!
//! LLVM's `Verifier::visitPHINode` (`lib/IR/Verifier.cpp`) checks at
//! runtime that a phi has one incoming pair per predecessor edge and
//! that no predecessor was missed. llvmkit moves the closing-time
//! check to compile time: a freshly-built phi is [`Open`] and accepts
//! [`crate::PhiInst::add_incoming`]; calling [`crate::PhiInst::finish`]
//! consumes the open phi and produces a [`Closed`] view that no
//! longer exposes `add_incoming`.
//!
//! The runtime-side coherence rule (incoming-block set matches
//! predecessor set) is still enforced by [`crate::Module::verify`] for
//! parsed modules, where the phi is born `Closed`.

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

/// Marker: the phi has been finalised. `add_incoming` is no longer
/// reachable; only read accessors are.
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
