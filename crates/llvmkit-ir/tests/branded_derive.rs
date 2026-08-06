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

// -- opt-in ordering: struct, lexicographic over all fields ------------------

#[derive(Branded)]
#[branded(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Ranked<B> {
    major: u32,
    minor: u32,
    _brand: PhantomData<fn(B) -> B>,
}

fn ranked(major: u32, minor: u32) -> Ranked<NoTraits> {
    Ranked {
        major,
        minor,
        _brand: PhantomData,
    }
}

/// llvmkit-specific: the derive's ordering is field-order lexicographic and
/// still needs no bound on the unbounded brand. No upstream counterpart —
/// `#[derive(Branded)]` exists because std's derive cannot express this.
#[test]
fn struct_ordering_is_lexicographic() {
    assert!(ranked(1, 0) < ranked(1, 1), "second field breaks the tie");
    assert!(ranked(1, 9) < ranked(2, 0), "first field dominates");
    assert_eq!(ranked(3, 4).cmp(&ranked(3, 4)), std::cmp::Ordering::Equal);
    assert_eq!(
        ranked(3, 4).partial_cmp(&ranked(3, 5)),
        Some(std::cmp::Ordering::Less)
    );

    let mut v = [ranked(2, 1), ranked(1, 7), ranked(2, 0)];
    v.sort();
    assert_eq!(
        v.iter().map(|r| (r.major, r.minor)).collect::<Vec<_>>(),
        [(1, 7), (2, 0), (2, 1)]
    );
}

/// llvmkit-specific: `Ord` and the derived `PartialEq` agree — `cmp` returns
/// `Equal` exactly when `eq` is true, which is the contract `Ord` states and
/// what a `BTreeMap` keyed by a branded id relies on. No upstream counterpart.
#[test]
fn struct_ordering_agrees_with_equality() {
    let values = [ranked(1, 1), ranked(1, 2), ranked(2, 1)];
    for a in values {
        for b in values {
            assert_eq!(
                a.cmp(&b) == std::cmp::Ordering::Equal,
                a == b,
                "cmp/eq disagree on {a:?} vs {b:?}"
            );
            assert_eq!(a.partial_cmp(&b), Some(a.cmp(&b)));
        }
    }
}

// -- opt-in ordering: enum, declaration order then payload -------------------

#[derive(Branded)]
#[branded(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Rung<B> {
    Low,
    Mid(u32),
    High(PhantomData<fn(B) -> B>),
}

/// llvmkit-specific: enum ordering ranks by declaration order first, then by
/// payload — matching the std derive's semantics without its bounds, and
/// without an `as`-cast on the discriminant. No upstream counterpart.
#[test]
fn enum_ordering_ranks_by_declaration_then_payload() {
    let low = Rung::<NoTraits>::Low;
    let mid1 = Rung::<NoTraits>::Mid(1);
    let mid2 = Rung::<NoTraits>::Mid(2);
    let high = Rung::<NoTraits>::High(PhantomData);

    assert!(low < mid1, "earlier variant sorts first");
    assert!(mid1 < mid2, "same variant compares payloads");
    assert!(mid2 < high, "later variant sorts last");
    assert_eq!(low.cmp(&low), std::cmp::Ordering::Equal);
    assert_eq!(
        high.cmp(&high),
        std::cmp::Ordering::Equal,
        "an all-phantom variant ties with itself"
    );

    let mut v = [high, mid2, low, mid1];
    v.sort();
    assert_eq!(v, [low, mid1, mid2, high]);
}

// -- partial orders stay partial ---------------------------------------------

#[derive(Branded)]
#[branded(Clone, Copy, Debug, PartialEq, PartialOrd)]
struct Measured<B> {
    value: f64,
    _brand: PhantomData<fn(B) -> B>,
}

/// llvmkit-specific: `PartialOrd` alone is derivable, and an incomparable
/// field (NaN) propagates `None` rather than being forced into a total order.
/// No upstream counterpart.
#[test]
fn partial_ord_without_ord_stays_partial() {
    let nan = Measured::<NoTraits> {
        value: f64::NAN,
        _brand: PhantomData,
    };
    let one = Measured::<NoTraits> {
        value: 1.0,
        _brand: PhantomData,
    };
    assert_eq!(one.partial_cmp(&one), Some(std::cmp::Ordering::Equal));
    assert_eq!(nan.partial_cmp(&one), None);
    assert_eq!(nan.partial_cmp(&nan), None);
}
