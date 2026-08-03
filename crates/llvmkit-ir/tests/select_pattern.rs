//! The select-pattern vocabulary — tranche 4a of the `ValueTracking.h` port.
//!
//! Sources: `llvm::getSelectPattern`, `getMinMaxPred`, `getMinMaxIntrinsic`,
//! `getInverseMinMaxFlavor`, `getInverseMinMaxIntrinsic` and `getMinMaxLimit`
//! in `llvm/lib/Analysis/ValueTracking.cpp`, plus
//! `SelectPatternResult::isMinOrMax` in the header.
//!
//! These are total functions over an enum, so the tests enumerate rather than
//! sample: every flavour, every predicate.

use llvmkit_ir::{
    ApInt, CmpPredicate, FloatPredicate, IntPredicate, MinMaxIntrinsic, SelectPatternFlavor,
    SelectPatternNaNBehavior, SelectPatternResult, get_select_pattern,
};

const ALL_FLAVORS: [SelectPatternFlavor; 9] = [
    SelectPatternFlavor::Unknown,
    SelectPatternFlavor::SMin,
    SelectPatternFlavor::UMin,
    SelectPatternFlavor::SMax,
    SelectPatternFlavor::UMax,
    SelectPatternFlavor::FMinNum,
    SelectPatternFlavor::FMaxNum,
    SelectPatternFlavor::Abs,
    SelectPatternFlavor::NAbs,
];

const MIN_MAX_INTRINSICS: [MinMaxIntrinsic; 4] = [
    MinMaxIntrinsic::SMin,
    MinMaxIntrinsic::SMax,
    MinMaxIntrinsic::UMin,
    MinMaxIntrinsic::UMax,
];

/// Ports `SelectPatternResult::isMinOrMax`, which upstream spells as
/// `SPF != SPF_UNKNOWN && SPF != SPF_ABS && SPF != SPF_NABS`.
#[test]
fn is_min_or_max_excludes_unknown_and_the_two_absolute_flavors() {
    for flavor in ALL_FLAVORS {
        let expected = !matches!(
            flavor,
            SelectPatternFlavor::Unknown | SelectPatternFlavor::Abs | SelectPatternFlavor::NAbs
        );
        assert_eq!(flavor.is_min_or_max(), expected, "{flavor:?}");
    }
}

/// Ports `llvm::getMinMaxPred`, enumerated over every flavour and both
/// orderings.
///
/// Upstream ends in `llvm_unreachable` for the three flavours that are not a
/// min or max; llvmkit returns `None`, so the two are asserted to agree:
/// exactly the min/max flavours have a predicate.
#[test]
fn min_max_predicate_matches_upstream_for_every_flavor() {
    use FloatPredicate as F;
    use IntPredicate as I;

    let cases: &[(SelectPatternFlavor, bool, CmpPredicate)] = &[
        (SelectPatternFlavor::SMin, false, CmpPredicate::Int(I::Slt)),
        (SelectPatternFlavor::SMin, true, CmpPredicate::Int(I::Slt)),
        (SelectPatternFlavor::UMin, false, CmpPredicate::Int(I::Ult)),
        (SelectPatternFlavor::SMax, false, CmpPredicate::Int(I::Sgt)),
        (SelectPatternFlavor::UMax, false, CmpPredicate::Int(I::Ugt)),
        // Ordered selects the o-prefixed float predicate, unordered the u-.
        (
            SelectPatternFlavor::FMinNum,
            true,
            CmpPredicate::Float(F::Olt),
        ),
        (
            SelectPatternFlavor::FMinNum,
            false,
            CmpPredicate::Float(F::Ult),
        ),
        (
            SelectPatternFlavor::FMaxNum,
            true,
            CmpPredicate::Float(F::Ogt),
        ),
        (
            SelectPatternFlavor::FMaxNum,
            false,
            CmpPredicate::Float(F::Ugt),
        ),
    ];
    for (flavor, ordered, expected) in cases {
        assert_eq!(
            flavor.min_max_predicate(*ordered),
            Some(*expected),
            "{flavor:?} ordered={ordered}"
        );
    }

    // The precondition upstream asserts with llvm_unreachable.
    for flavor in ALL_FLAVORS {
        assert_eq!(
            flavor.min_max_predicate(false).is_some(),
            flavor.is_min_or_max(),
            "{flavor:?}: a predicate exists exactly for the min/max flavours"
        );
    }
}

/// Ports `llvm::getMinMaxIntrinsic`, whose contract is "caller must ensure
/// `SPF` is an integer min or max pattern".
#[test]
fn min_max_intrinsic_covers_exactly_the_integer_flavors() {
    let expected: &[(SelectPatternFlavor, MinMaxIntrinsic)] = &[
        (SelectPatternFlavor::SMin, MinMaxIntrinsic::SMin),
        (SelectPatternFlavor::SMax, MinMaxIntrinsic::SMax),
        (SelectPatternFlavor::UMin, MinMaxIntrinsic::UMin),
        (SelectPatternFlavor::UMax, MinMaxIntrinsic::UMax),
    ];
    for (flavor, intrinsic) in expected {
        assert_eq!(flavor.min_max_intrinsic(), Some(*intrinsic), "{flavor:?}");
        // The mapping is a bijection with the integer flavours.
        assert_eq!(intrinsic.flavor(), *flavor);
    }
    for flavor in ALL_FLAVORS {
        let integer = matches!(
            flavor,
            SelectPatternFlavor::SMin
                | SelectPatternFlavor::SMax
                | SelectPatternFlavor::UMin
                | SelectPatternFlavor::UMax
        );
        assert_eq!(flavor.min_max_intrinsic().is_some(), integer, "{flavor:?}");
    }
}

/// Ports `llvm::getInverseMinMaxFlavor` and the integer arms of
/// `llvm::getInverseMinMaxIntrinsic`.
#[test]
fn inverting_a_min_max_swaps_min_and_max_and_is_an_involution() {
    let pairs: &[(SelectPatternFlavor, SelectPatternFlavor)] = &[
        (SelectPatternFlavor::SMin, SelectPatternFlavor::SMax),
        (SelectPatternFlavor::UMin, SelectPatternFlavor::UMax),
        (SelectPatternFlavor::SMax, SelectPatternFlavor::SMin),
        (SelectPatternFlavor::UMax, SelectPatternFlavor::UMin),
    ];
    for (flavor, inverse) in pairs {
        assert_eq!(flavor.inverse_min_max(), Some(*inverse), "{flavor:?}");
        // Inverting twice is the identity.
        assert_eq!(inverse.inverse_min_max(), Some(*flavor));
    }

    // Upstream's getInverseMinMaxFlavor is llvm_unreachable for everything
    // else -- including the two float flavours, which it does not cover.
    for flavor in ALL_FLAVORS {
        let integer_min_max = pairs.iter().any(|(f, _)| *f == flavor);
        assert_eq!(
            flavor.inverse_min_max().is_some(),
            integer_min_max,
            "{flavor:?}"
        );
    }

    for intrinsic in MIN_MAX_INTRINSICS {
        assert_eq!(intrinsic.inverse().inverse(), intrinsic, "{intrinsic:?}");
        assert_ne!(intrinsic.inverse(), intrinsic);
        // Inverting the intrinsic agrees with inverting the flavour.
        assert_eq!(
            intrinsic.inverse().flavor(),
            intrinsic
                .flavor()
                .inverse_min_max()
                .expect("integer flavour")
        );
    }
}

/// Ports `llvm::getMinMaxLimit`: the extreme value each min/max can produce.
///
/// Upstream is a four-arm switch — `SPF_SMAX` → `getSignedMaxValue`,
/// `SPF_SMIN` → `getSignedMinValue`, `SPF_UMAX` → `getMaxValue`, `SPF_UMIN` →
/// `getMinValue`. Worth stating because the obvious misreading is "identity
/// element", which is the *opposite* end: the identity of `smax` is the signed
/// minimum, not the signed maximum.
#[test]
fn min_max_limit_is_the_extreme_value_of_the_flavor() {
    for width in [1u32, 8, 32, 64] {
        assert_eq!(
            SelectPatternFlavor::SMax.min_max_limit(width),
            Some(ApInt::signed_max_value(width)),
            "smax tops out at the signed maximum"
        );
        assert_eq!(
            SelectPatternFlavor::SMin.min_max_limit(width),
            Some(ApInt::signed_min_value(width))
        );
        assert_eq!(
            SelectPatternFlavor::UMax.min_max_limit(width),
            Some(ApInt::all_ones(width)),
            "getMaxValue is the all-ones pattern"
        );
        assert_eq!(
            SelectPatternFlavor::UMin.min_max_limit(width),
            Some(ApInt::zero(width))
        );
    }

    for flavor in ALL_FLAVORS {
        assert_eq!(
            flavor.min_max_limit(8).is_some(),
            flavor.min_max_intrinsic().is_some(),
            "{flavor:?}: a limit exists exactly for the integer min/max flavours"
        );
    }
}

/// Ports `llvm::getSelectPattern`, enumerated over every comparison predicate.
///
/// The integer arms drop `nan_behavior` and `ordered`; the float arms carry
/// them through. Equality predicates are upstream's `default` arm, commented
/// "Equality", and yield `SPF_UNKNOWN`.
#[test]
fn get_select_pattern_classifies_every_predicate() {
    use FloatPredicate as F;
    use IntPredicate as I;

    let integer: &[(IntPredicate, SelectPatternFlavor)] = &[
        (I::Ugt, SelectPatternFlavor::UMax),
        (I::Uge, SelectPatternFlavor::UMax),
        (I::Sgt, SelectPatternFlavor::SMax),
        (I::Sge, SelectPatternFlavor::SMax),
        (I::Ult, SelectPatternFlavor::UMin),
        (I::Ule, SelectPatternFlavor::UMin),
        (I::Slt, SelectPatternFlavor::SMin),
        (I::Sle, SelectPatternFlavor::SMin),
    ];
    for (predicate, flavor) in integer {
        // Whatever NaN behaviour is passed in, an integer answer discards it.
        let result = get_select_pattern(
            CmpPredicate::Int(*predicate),
            SelectPatternNaNBehavior::ReturnsNaN,
            true,
        );
        assert_eq!(
            result,
            SelectPatternResult {
                flavor: *flavor,
                nan_behavior: SelectPatternNaNBehavior::NotApplicable,
                ordered: false,
            },
            "{predicate:?}"
        );
    }

    for predicate in [I::Eq, I::Ne] {
        assert_eq!(
            get_select_pattern(
                CmpPredicate::Int(predicate),
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )
            .flavor,
            SelectPatternFlavor::Unknown,
            "{predicate:?} is an equality, not a min/max"
        );
    }

    let float: &[(FloatPredicate, SelectPatternFlavor)] = &[
        (F::Ugt, SelectPatternFlavor::FMaxNum),
        (F::Uge, SelectPatternFlavor::FMaxNum),
        (F::Ogt, SelectPatternFlavor::FMaxNum),
        (F::Oge, SelectPatternFlavor::FMaxNum),
        (F::Ult, SelectPatternFlavor::FMinNum),
        (F::Ule, SelectPatternFlavor::FMinNum),
        (F::Olt, SelectPatternFlavor::FMinNum),
        (F::Ole, SelectPatternFlavor::FMinNum),
    ];
    for (predicate, flavor) in float {
        let result = get_select_pattern(
            CmpPredicate::Float(*predicate),
            SelectPatternNaNBehavior::ReturnsOther,
            true,
        );
        assert_eq!(
            result,
            SelectPatternResult {
                flavor: *flavor,
                nan_behavior: SelectPatternNaNBehavior::ReturnsOther,
                ordered: true,
            },
            "{predicate:?} carries NaN behaviour and ordering through"
        );
    }

    // Every remaining float predicate is upstream's `default`.
    for predicate in [
        F::False,
        F::Oeq,
        F::One,
        F::Ord,
        F::Ueq,
        F::Une,
        F::Uno,
        F::True,
    ] {
        assert_eq!(
            get_select_pattern(
                CmpPredicate::Float(predicate),
                SelectPatternNaNBehavior::NotApplicable,
                false,
            )
            .flavor,
            SelectPatternFlavor::Unknown,
            "{predicate:?}"
        );
    }
}

/// The predicate a flavour names classifies back to that same flavour.
///
/// No upstream counterpart as a test, but the round trip is the contract
/// `getMinMaxPred` and `getSelectPattern` jointly define: `getMinMaxPred`
/// returns "the canonical comparison predicate for the specified
/// minimum/maximum flavor", and `getSelectPattern` names the pattern
/// `X Pred Y ? X : Y` implements.
#[test]
fn min_max_predicate_round_trips_through_get_select_pattern() {
    for flavor in ALL_FLAVORS {
        if !flavor.is_min_or_max() {
            continue;
        }
        for ordered in [false, true] {
            let predicate = flavor.min_max_predicate(ordered).expect("min/max flavour");
            let back = get_select_pattern(predicate, SelectPatternNaNBehavior::ReturnsAny, ordered);
            assert_eq!(
                back.flavor, flavor,
                "{flavor:?} ordered={ordered} did not round trip"
            );
        }
    }
}
