//! Capability-typed handles (D1, D8): a module's verification state decides the
//! capability of the handles it mints.
//!
//! llvmkit-specific: LLVM has no verification typestate and no capability on a
//! `Value *`, so nothing upstream can be ported here.

use core::any::TypeId;
use llvmkit_ir::{ModuleState, Mutable, ReadOnly, Unverified, Verified};

fn capability_of<S: ModuleState>() -> TypeId {
    TypeId::of::<S::Capability>()
}

/// An unverified module mints `Mutable` handles and a verified one `ReadOnly`
/// handles — the mapping every `Module::view` reads. llvmkit-specific (D8).
#[test]
fn a_modules_state_decides_the_capability_of_its_handles() {
    assert_eq!(capability_of::<Unverified>(), TypeId::of::<Mutable>());
    assert_eq!(capability_of::<Verified>(), TypeId::of::<ReadOnly>());
}
