//! Registry internals that the public surface cannot reach.
//!
//! The public brand behaviour lives in `tests/module_brands.rs`; what needs
//! crate-private access is the poisoning-recovery path, because nothing a user
//! can call runs inside the registry's critical section.

use super::{BRANDS, BrandState, Module, ModuleBrand, lock_brands};

/// Poison [`BRANDS`] by panicking on a thread that holds its guard.
///
/// This is the only way to reach the poisoned state at all: every real critical
/// section is a single infallible map operation, so the registry cannot poison
/// itself. Recovery still has to work — otherwise one unrelated panic anywhere
/// in a process would brick every later module construction.
fn poison_the_registry() {
    let poisoning = std::thread::spawn(|| {
        let _guard = lock_brands();
        panic!("deliberately poisoning the brand registry");
    })
    .join();
    assert!(poisoning.is_err(), "the poisoning thread must panic");
    assert!(BRANDS.is_poisoned(), "the registry should now be poisoned");
}

/// After poisoning, claiming and releasing brands still works: both sides take
/// the lock with `unwrap_or_else(PoisonError::into_inner)`, which is sound here
/// because a single map operation has no partially-applied state to observe.
#[test]
fn a_poisoned_registry_still_claims_and_releases() {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct AfterPoison;
    impl ModuleBrand for AfterPoison {}

    poison_the_registry();

    // Claim ...
    let module = Module::branded::<AfterPoison>("after-poison").expect("claim despite poisoning");
    assert_eq!(module.name(), "after-poison");
    assert!(matches!(
        lock_brands().get(&core::any::TypeId::of::<AfterPoison>()),
        Some(BrandState::InUse)
    ));

    // ... uniqueness still enforced ...
    assert!(Module::branded::<AfterPoison>("again").is_err());

    // ... and release still works.
    drop(module);
    assert!(
        lock_brands()
            .get(&core::any::TypeId::of::<AfterPoison>())
            .is_none(),
        "the release must remove the key, not leave it InUse",
    );
    let _reclaimed = Module::branded::<AfterPoison>("again").expect("reclaim after release");
}

/// `branded_once` leaves the key present as `Retired` rather than removing it —
/// the state distinction the public API surfaces as `BrandInUse` vs
/// `BrandRetired`.
#[test]
fn retirement_leaves_the_key_behind() {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct RetiredKey;
    impl ModuleBrand for RetiredKey {}

    let key = core::any::TypeId::of::<RetiredKey>();

    let module = Module::branded_once::<RetiredKey>("once").expect("claim");
    assert!(matches!(lock_brands().get(&key), Some(BrandState::InUse)));

    drop(module);
    assert!(matches!(lock_brands().get(&key), Some(BrandState::Retired)));
}

/// `Module::dynamic` is registry-exempt: sixteen live `DynBrand` modules leave
/// the registry with no `DynBrand` key at all.
#[test]
fn dyn_brand_never_reaches_the_registry() {
    let key = core::any::TypeId::of::<super::DynBrand>();
    let modules: Vec<_> = (0..16).map(|i| Module::dynamic(format!("d{i}"))).collect();

    assert_eq!(modules.len(), 16);
    assert!(
        lock_brands().get(&key).is_none(),
        "DynBrand must never be registered",
    );
}
