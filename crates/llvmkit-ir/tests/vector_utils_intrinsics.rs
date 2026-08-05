//! The `VectorUtils` intrinsic classifiers.
//!
//! **No upstream counterpart.** `unittests/Analysis/VectorUtilsTest.cpp` covers
//! the shuffle-mask and splat surface and stops: not one of
//! `isTriviallyVectorizable`, `isTriviallyScalarizable`,
//! `isVectorIntrinsicWithStructReturnOverloadAtField`,
//! `getInterleaveIntrinsicFactor` or `getDeinterleaveIntrinsicFactor` has a
//! unit test anywhere under `unittests/`. Upstream exercises them only through
//! whole-pass lit tests for the scalarizer and the loop vectorizer, which are
//! not an oracle for a predicate. These tests are therefore llvmkit's own, and
//! say so rather than implying a port.
//!
//! # The one with teeth
//!
//! llvmkit spells `case Intrinsic::sqrt:` as a match on
//! [`IntrinsicId::base_name`], because it mints per-intrinsic constants only
//! for the intrinsics its own analyses need. That trades a compile error for a
//! silent wrong answer: upstream cannot misspell an enumerator, but a string
//! can be misspelled and will then simply never match, forever.
//!
//! [`the_vectorizable_table_holds_exactly_the_upstream_names`] is the guard.
//! Counting the ids across the whole 16k-entry intrinsic space that answer
//! `true` must equal the number of names the C++ switch lists — a misspelling
//! matches no intrinsic and drops the count. It is checked by construction, not
//! by re-reading the list.

use llvmkit_ir::{
    IntrinsicId, deinterleave_intrinsic_factor, interleave_intrinsic_factor,
    is_trivially_scalarizable, is_trivially_vectorizable,
    is_vector_intrinsic_with_struct_return_overload_at_field,
};

/// How many `case` labels `llvm::isTriviallyVectorizable` lists before its
/// `return true` — 20 integer bit-manipulation intrinsics and 51
/// floating-point ones.
///
/// Re-derive when syncing LLVM: the labels between `switch (ID) {` and
/// `return true;` in `VectorUtils.cpp`.
const UPSTREAM_TRIVIALLY_VECTORIZABLE_COUNT: usize = 71;

/// How many `case` labels `llvm::isTriviallyScalarizable` adds on top of the
/// ones it inherits by calling `isTriviallyVectorizable` first — the six
/// `with.overflow` intrinsics.
const UPSTREAM_SCALARIZABLE_ONLY_COUNT: usize = 6;

/// Every intrinsic id llvmkit knows, which is the whole generated space rather
/// than a subset — the property that makes the counts below meaningful.
fn all_intrinsics() -> impl Iterator<Item = IntrinsicId> {
    IntrinsicId::all()
}

/// Exactly as many intrinsics answer `true` as the upstream switch has labels.
///
/// This is the misspelling guard the module docs describe: a name that matches
/// no intrinsic contributes nothing, so the count falls. It also catches the
/// reverse — a base name that collides with a target intrinsic would push the
/// count up.
#[test]
fn the_vectorizable_table_holds_exactly_the_upstream_names() {
    let matched: Vec<&str> = all_intrinsics()
        .filter(|id| is_trivially_vectorizable(*id))
        .map(IntrinsicId::base_name)
        .collect();

    assert_eq!(
        matched.len(),
        UPSTREAM_TRIVIALLY_VECTORIZABLE_COUNT,
        "is_trivially_vectorizable matched {} intrinsics, not the {} labels \
         upstream lists; a misspelled name matches nothing, so a shortfall \
         names the typo. Matched: {matched:?}",
        matched.len(),
        UPSTREAM_TRIVIALLY_VECTORIZABLE_COUNT,
    );
}

/// The same guard for the six intrinsics `isTriviallyScalarizable` adds.
#[test]
fn scalarizable_adds_exactly_the_six_with_overflow_intrinsics() {
    let extra: Vec<&str> = all_intrinsics()
        .filter(|id| is_trivially_scalarizable(*id) && !is_trivially_vectorizable(*id))
        .map(IntrinsicId::base_name)
        .collect();

    assert_eq!(
        extra.len(),
        UPSTREAM_SCALARIZABLE_ONLY_COUNT,
        "expected exactly the six with.overflow intrinsics, got {extra:?}"
    );
    for name in &extra {
        assert!(
            name.ends_with(".with.overflow"),
            "{name} is not a with.overflow intrinsic"
        );
    }
}

/// `isTriviallyVectorizable` implies `isTriviallyScalarizable` — the note
/// upstream's header states outright, checked across the whole intrinsic space
/// rather than on a sample.
#[test]
fn vectorizable_implies_scalarizable() {
    for id in all_intrinsics() {
        if is_trivially_vectorizable(id) {
            assert!(
                is_trivially_scalarizable(id),
                "{} is vectorizable but not scalarizable",
                id.base_name()
            );
        }
    }
}

/// A sample of the intrinsics the table names, one from each group upstream's
/// comments mark, plus intrinsics that must answer `false`.
#[test]
fn the_named_groups_answer_true_and_others_false() {
    let yes = [
        "llvm.abs.i32",                  // integer bit-manipulation
        "llvm.smul.fix.sat.i32",         // fixed-point
        "llvm.sqrt.f32",                 // floating-point
        "llvm.is.fpclass.f64",           // predicate-shaped
        "llvm.scmp.i8.i32",              // three-way compare
        "llvm.vector.reduce.fmax.v4f32", // control: see below
    ];
    for (index, name) in yes.iter().enumerate() {
        let id = IntrinsicId::lookup(name).unwrap_or_else(|| panic!("{name} is an intrinsic"));
        // The last entry is the control: a vector intrinsic that is *not* on
        // the list, because reductions are not elementwise.
        let expected = index + 1 != yes.len();
        assert_eq!(
            is_trivially_vectorizable(id),
            expected,
            "{name} answered wrongly"
        );
    }

    // Neither memory operations nor target intrinsics are elementwise.
    for name in ["llvm.memcpy.p0.p0.i64", "llvm.assume", "llvm.amdgcn.kill"] {
        let id = IntrinsicId::lookup(name).unwrap_or_else(|| panic!("{name} is an intrinsic"));
        assert!(!is_trivially_vectorizable(id), "{name} is not elementwise");
        assert!(!is_trivially_scalarizable(id), "{name} is not scalarizable");
    }
}

/// `llvm.vector.interleaveN` answers `N` for 2 through 8, and nothing else
/// answers at all.
///
/// The sweep over every intrinsic is what makes "nothing else" a claim rather
/// than a hope.
#[test]
fn interleave_factors_cover_two_through_eight_and_nothing_else() {
    let mut answered: Vec<(u32, &str)> = all_intrinsics()
        .filter_map(|id| interleave_intrinsic_factor(id).map(|factor| (factor, id.base_name())))
        .collect();
    answered.sort_unstable();

    let expected: Vec<(u32, &str)> = vec![
        (2, "llvm.vector.interleave2"),
        (3, "llvm.vector.interleave3"),
        (4, "llvm.vector.interleave4"),
        (5, "llvm.vector.interleave5"),
        (6, "llvm.vector.interleave6"),
        (7, "llvm.vector.interleave7"),
        (8, "llvm.vector.interleave8"),
    ];
    assert_eq!(answered, expected);
}

/// The deinterleave half, and that the two tables do not answer for each
/// other's intrinsics — upstream keeps them as two switches for that reason.
#[test]
fn deinterleave_factors_cover_two_through_eight_and_nothing_else() {
    let mut answered: Vec<(u32, &str)> = all_intrinsics()
        .filter_map(|id| deinterleave_intrinsic_factor(id).map(|factor| (factor, id.base_name())))
        .collect();
    answered.sort_unstable();

    let expected: Vec<(u32, &str)> = vec![
        (2, "llvm.vector.deinterleave2"),
        (3, "llvm.vector.deinterleave3"),
        (4, "llvm.vector.deinterleave4"),
        (5, "llvm.vector.deinterleave5"),
        (6, "llvm.vector.deinterleave6"),
        (7, "llvm.vector.deinterleave7"),
        (8, "llvm.vector.deinterleave8"),
    ];
    assert_eq!(answered, expected);

    for id in all_intrinsics() {
        assert!(
            interleave_intrinsic_factor(id).is_none()
                || deinterleave_intrinsic_factor(id).is_none(),
            "{} answered as both an interleave and a deinterleave",
            id.base_name()
        );
    }
}

/// `frexp` returns `{ significand, exponent }` and is overloaded on both
/// fields; every other intrinsic is overloaded on field 0 only.
#[test]
fn only_frexp_is_overloaded_beyond_the_first_struct_field() {
    let frexp = IntrinsicId::lookup("llvm.frexp.f32.i32").expect("frexp intrinsic");
    assert!(is_vector_intrinsic_with_struct_return_overload_at_field(
        frexp, 0
    ));
    assert!(is_vector_intrinsic_with_struct_return_overload_at_field(
        frexp, 1
    ));
    assert!(!is_vector_intrinsic_with_struct_return_overload_at_field(
        frexp, 2
    ));

    let sincos = IntrinsicId::lookup("llvm.sincos.f32").expect("sincos intrinsic");
    assert!(is_vector_intrinsic_with_struct_return_overload_at_field(
        sincos, 0
    ));
    assert!(!is_vector_intrinsic_with_struct_return_overload_at_field(
        sincos, 1
    ));
}

/// The field index is signed upstream, and `-1` — the spelling
/// `isVectorIntrinsicWithOverloadTypeAtArg` gives the return type — is not a
/// struct field, so it answers `false` even for `frexp`.
#[test]
fn a_negative_field_index_is_not_a_struct_field() {
    let frexp = IntrinsicId::lookup("llvm.frexp.f32.i32").expect("frexp intrinsic");
    assert!(!is_vector_intrinsic_with_struct_return_overload_at_field(
        frexp, -1
    ));
}
