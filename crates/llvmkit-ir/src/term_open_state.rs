//! Open / Closed typestate for variable-arity terminators.
//!
//! Mirrors the convention from `lib/IR/Instructions.cpp` where
//! `SwitchInst` / `IndirectBrInst` / `CatchSwitchInst` accept new
//! cases / destinations / handlers via `addCase` / `addDestination` /
//! `addHandler` until the IR is committed to a verifier pass.
//!
//! `Open`: this handle view can still edit the variable-arity list.
//! `Closed`: this handle view exposes the read-only API.
//!
//! The state lives only on the *handle* (Doctrine D1); the underlying storage
//! uses interior mutability so that an attached terminator's case list can grow
//! through `&self`.

mod sealed {
    pub trait Sealed {}
}

/// Sealed marker trait for terminator-state markers.
pub trait TermOpenState: sealed::Sealed {}

/// This handle view can still edit the variable-arity list.
#[derive(Debug, Clone, Copy)]
pub struct Open(());
/// This handle view treats the variable-arity list as frozen.
#[derive(Debug, Clone, Copy)]
pub struct Closed(());

impl sealed::Sealed for Open {}
impl sealed::Sealed for Closed {}
impl TermOpenState for Open {}
impl TermOpenState for Closed {}
