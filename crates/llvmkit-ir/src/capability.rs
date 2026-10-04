//! What a handle may do with the module it was minted from (D1, D8).
//!
//! Every borrowing handle embeds a [`ModuleRef`](crate::module::ModuleRef), and
//! the reference carries a capability: [`Mutable`] when it was minted from
//! `&Module<B, Unverified>`, [`ReadOnly`] when it was minted from a
//! `Module<B, Verified>`, a [`ModuleView`](crate::ModuleView) or a read-only
//! pass context. Readers exist at either capability; a mutator requires
//! [`CanMutate`], which only [`Mutable`] implements, so "mutate a verified
//! module" and "mutate from an `Inspect` pass" are type errors rather than
//! conventions a token parameter had to enforce.
//!
//! No upstream counterpart: LLVM has no verification typestate and no
//! capability on a `Value *`.

mod sealed {
    pub trait Sealed {}
}

/// A handle's capability. Sealed: [`Mutable`] and [`ReadOnly`] are the only two.
pub trait Capability: sealed::Sealed + 'static {}

/// May read and mutate. Minted only from `&Module<B, Unverified>`.
#[derive(Debug)]
pub enum Mutable {}

/// May only read. Minted from a `Module<B, Verified>`, a
/// [`ModuleView`](crate::ModuleView), or a read-only pass context.
#[derive(Debug)]
pub enum ReadOnly {}

impl sealed::Sealed for Mutable {}
impl sealed::Sealed for ReadOnly {}
impl Capability for Mutable {}
impl Capability for ReadOnly {}

/// The capability that grants mutation: [`Mutable`] alone. Every mutator is
/// bounded `where C: CanMutate`.
#[diagnostic::on_unimplemented(
    message = "a `{Self}` handle cannot mutate the module it reads",
    label = "this handle was minted read-only",
    note = "mutation needs a handle minted from `&Module<B, Unverified>` — `Module::view` on an \
            unverified module; a `Module<B, Verified>`, a `ModuleView` and a read-only pass \
            context mint `ReadOnly` handles"
)]
pub trait CanMutate: Capability {}
impl CanMutate for Mutable {}

/// The capability a handle was minted with — a type-level read for tests and
/// for generic code that has to name it. Every capability-carrying handle
/// implements it.
pub trait CapabilityOf {
    type Capability: Capability;
}

/// A module's verification state, and the capability of the handles it mints:
/// an unverified module mints [`Mutable`] handles, a verified one [`ReadOnly`]
/// handles. Sealed to the two states.
pub trait ModuleState: sealed::Sealed + 'static {
    type Capability: Capability;
}
impl sealed::Sealed for crate::module::Unverified {}
impl sealed::Sealed for crate::module::Verified {}
impl ModuleState for crate::module::Unverified {
    type Capability = Mutable;
}
impl ModuleState for crate::module::Verified {
    type Capability = ReadOnly;
}
