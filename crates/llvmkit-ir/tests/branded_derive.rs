//! Expansion tests for `#[derive(Branded)]` — the bound-free derive that lets
//! `ModuleBrand` drop its supertraits.
//!
//! Every fixture here is local; production types migrate in a later slice.
//! The load-bearing case is [`NoTraits`]: a type parameter that implements
//! nothing at all still yields `Copy + Eq + Hash + Debug` containers, which is
//! exactly what std derive cannot express and why this macro exists.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use llvmkit_macros::Branded;

/// Implements nothing — no `Clone`, no `Debug`, nothing. Standing in for a
/// bare `struct LiftedBin;` brand.
struct NoTraits;

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Invoke the derived `Clone` impl through a generic seam. On `Copy` types a
/// literal `.clone()` call is (correctly) rejected by `clippy::clone_on_copy`;
/// this still dispatches to `<T as Clone>::clone`, which is what the tests
/// verify.
fn clone_of<T: Clone>(value: &T) -> T {
    value.clone()
}

// -- named struct, full six, phantom + real fields --------------------------

#[derive(Branded)]
struct Named<B> {
    x: u32,
    _brand: PhantomData<fn(B) -> B>,
}

#[test]
fn named_struct_full_six() {
    let a = Named::<NoTraits> {
        x: 5,
        _brand: PhantomData,
    };
    let b = a; // Copy: `a` stays usable
    assert!(a == b, "copies compare equal");
    assert!(a == clone_of(&a), "clone equals source");
    assert_eq!(hash_of(&a), hash_of(&b), "equal values hash equal");

    let c = Named::<NoTraits> {
        x: 6,
        _brand: PhantomData,
    };
    assert!(a != c);

    let dbg = format!("{a:?}");
    assert!(dbg.contains("x: 5"), "real field printed: {dbg}");
    assert!(!dbg.contains("PhantomData"), "phantom skipped: {dbg}");
    assert!(!dbg.contains("_brand"), "phantom name skipped: {dbg}");
}

// -- tuple struct ------------------------------------------------------------

#[derive(Branded)]
struct Tup<B>(u32, PhantomData<fn(B) -> B>);

#[test]
fn tuple_struct_debug_skips_phantom() {
    let t = Tup::<NoTraits>(7, PhantomData);
    assert_eq!(format!("{t:?}"), "Tup(7)");
    let u = t;
    assert!(t == u);
    assert_eq!(hash_of(&t), hash_of(&u));
}

// -- enum: unit + tuple variants, phantom carrier ----------------------------

#[derive(Branded)]
enum Guest<B> {
    Idle,
    Pair(u32, u32),
    Ghost(PhantomData<fn(B) -> B>),
}

#[test]
fn enum_full_six() {
    let idle = Guest::<NoTraits>::Idle;
    let p12 = Guest::<NoTraits>::Pair(1, 2);
    let p13 = Guest::<NoTraits>::Pair(1, 3);
    let ghost = Guest::<NoTraits>::Ghost(PhantomData);

    let copy = p12; // Copy
    assert!(p12 == copy);
    assert!(p12 == clone_of(&p12));
    assert!(p12 != p13, "same variant, different payload");
    assert!(idle != p12, "different variants differ");
    assert_eq!(hash_of(&p12), hash_of(&copy), "equal values hash equal");

    assert_eq!(format!("{idle:?}"), "Idle");
    assert_eq!(format!("{p12:?}"), "Pair(1, 2)");
    assert_eq!(
        format!("{ghost:?}"),
        "Ghost",
        "all-phantom variant prints bare"
    );
}

// -- subset: Clone without Copy, over a genuinely non-Copy field -------------

#[derive(Branded)]
#[branded(Debug, Clone)]
struct Deep<B> {
    text: String,
    _brand: PhantomData<fn(B) -> B>,
}

#[test]
fn subset_clone_is_field_wise() {
    let a = Deep::<NoTraits> {
        text: "own".to_string(),
        _brand: PhantomData,
    };
    let b = a.clone();
    assert_eq!(a.text, b.text);
    assert!(format!("{a:?}").contains("own"));
}

// -- subset with Default -----------------------------------------------------

#[derive(Branded)]
#[branded(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct Settings<B> {
    n: u32,
    _brand: PhantomData<fn(B) -> B>,
}

#[test]
fn default_subset() {
    let d = Settings::<NoTraits>::default();
    assert_eq!(d.n, 0);
    assert!(d == d.clone());
}

// -- the whole point: two unbounded parameters, both hosted by a type that
// -- implements nothing, and the container is still Copy + Eq + Hash + Debug

#[derive(Branded)]
struct G<T, B> {
    x: u32,
    _elem: PhantomData<fn(T) -> T>,
    _brand: PhantomData<fn(B) -> B>,
}

#[test]
fn unbounded_parameters_still_work() {
    let g = G::<NoTraits, NoTraits> {
        x: 9,
        _elem: PhantomData,
        _brand: PhantomData,
    };
    let h = g; // Copy despite NoTraits: !Clone
    assert!(g == h);
    assert_eq!(hash_of(&g), hash_of(&h));
    assert_eq!(format!("{g:?}"), "G { x: 9 }");
}
