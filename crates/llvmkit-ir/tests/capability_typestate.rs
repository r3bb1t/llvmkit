//! Capability-typed handles (D1, D8): a module's verification state decides the
//! capability of the handles it mints.
//!
//! llvmkit-specific: LLVM has no verification typestate and no capability on a
//! `Value *`, so nothing upstream can be ported here.

use core::any::TypeId;
use llvmkit_ir::{CapabilityOf, Module, ModuleState, Mutable, ReadOnly, Unverified, Verified};

fn capability_of<S: ModuleState>() -> TypeId {
    TypeId::of::<S::Capability>()
}

fn capability<T: CapabilityOf>(_: T) -> TypeId {
    TypeId::of::<T::Capability>()
}

/// An unverified module mints `Mutable` handles and a verified one `ReadOnly`
/// handles — the mapping every `Module::view` reads. llvmkit-specific (D8).
#[test]
fn a_modules_state_decides_the_capability_of_its_handles() {
    assert_eq!(capability_of::<Unverified>(), TypeId::of::<Mutable>());
    assert_eq!(capability_of::<Verified>(), TypeId::of::<ReadOnly>());
}

/// A type minted through a `ModuleView` is read-only, and a type reached from
/// it stays read-only — the module view is the read grade of a module,
/// whatever its state. Positive control: the unverified module itself mints
/// `Mutable` types. The view's constructor also takes the `Mutable` element as
/// an operand, which a capability-blind bound would refuse. llvmkit-specific
/// (D1, D8).
#[test]
fn a_type_minted_through_a_module_view_is_read_only() {
    let m = Module::dynamic("m");
    assert_eq!(capability(m.i32_type()), TypeId::of::<Mutable>());
    assert_eq!(capability(m.as_view().i32_type()), TypeId::of::<ReadOnly>());
    let array = m.as_view().array_type(m.i32_type(), 4);
    assert_eq!(capability(array), TypeId::of::<ReadOnly>());
    assert_eq!(capability(array.element()), TypeId::of::<ReadOnly>());
}
