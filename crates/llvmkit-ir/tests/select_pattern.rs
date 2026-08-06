//! The select-pattern vocabulary — tranche 4a of the `ValueTracking.h` port.
//!
//! Sources: `llvm::getSelectPattern`, `getMinMaxPred`, `getMinMaxIntrinsic`,
//! `getInverseMinMaxFlavor`, `getInverseMinMaxIntrinsic` and `getMinMaxLimit`
//! in `llvm/lib/Analysis/ValueTracking.cpp`, plus
//! `SelectPatternResult::isMinOrMax` in the header.
//!
//! These are total functions over an enum, so the tests enumerate rather than
//! sample: every flavour, every predicate.

use std::collections::BTreeSet;

use llvmkit_ir::{
    ApInt, CmpPredicate, FloatPredicate, IntPredicate, MinMaxIntrinsic, MinMaxKind,
    MinMaxOperation, SelectPatternFlavor, SelectPatternNanBehavior, SelectPatternResult,
    select_pattern,
};

const ALL_FLAVORS: [SelectPatternFlavor; 9] = [
    SelectPatternFlavor::Unknown,
    SelectPatternFlavor::Smin,
    SelectPatternFlavor::Umin,
    SelectPatternFlavor::Smax,
    SelectPatternFlavor::Umax,
    SelectPatternFlavor::FminNum,
    SelectPatternFlavor::FmaxNum,
    SelectPatternFlavor::Abs,
    SelectPatternFlavor::Nabs,
];

const MIN_MAX_INTRINSICS: [MinMaxIntrinsic; 4] = [
    MinMaxIntrinsic::Smin,
    MinMaxIntrinsic::Smax,
    MinMaxIntrinsic::Umin,
    MinMaxIntrinsic::Umax,
];

const MIN_MAX_KINDS: [MinMaxKind; 6] = [
    MinMaxKind::Minimum,
    MinMaxKind::Maximum,
    MinMaxKind::MinimumNum,
    MinMaxKind::MaximumNum,
    MinMaxKind::MinNum,
    MinMaxKind::MaxNum,
];

/// Ports `SelectPatternResult::isMinOrMax`, which upstream spells as
/// `SPF != SPF_UNKNOWN && SPF != SPF_ABS && SPF != SPF_NABS`.
#[test]
fn is_min_or_max_excludes_unknown_and_the_two_absolute_flavors() {
    for flavor in ALL_FLAVORS {
        let expected = !matches!(
            flavor,
            SelectPatternFlavor::Unknown | SelectPatternFlavor::Abs | SelectPatternFlavor::Nabs
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
        (SelectPatternFlavor::Smin, false, CmpPredicate::Int(I::Slt)),
        (SelectPatternFlavor::Smin, true, CmpPredicate::Int(I::Slt)),
        (SelectPatternFlavor::Umin, false, CmpPredicate::Int(I::Ult)),
        (SelectPatternFlavor::Smax, false, CmpPredicate::Int(I::Sgt)),
        (SelectPatternFlavor::Umax, false, CmpPredicate::Int(I::Ugt)),
        // Ordered selects the o-prefixed float predicate, unordered the u-.
        (
            SelectPatternFlavor::FminNum,
            true,
            CmpPredicate::Float(F::Olt),
        ),
        (
            SelectPatternFlavor::FminNum,
            false,
            CmpPredicate::Float(F::Ult),
        ),
        (
            SelectPatternFlavor::FmaxNum,
            true,
            CmpPredicate::Float(F::Ogt),
        ),
        (
            SelectPatternFlavor::FmaxNum,
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
        (SelectPatternFlavor::Smin, MinMaxIntrinsic::Smin),
        (SelectPatternFlavor::Smax, MinMaxIntrinsic::Smax),
        (SelectPatternFlavor::Umin, MinMaxIntrinsic::Umin),
        (SelectPatternFlavor::Umax, MinMaxIntrinsic::Umax),
    ];
    for (flavor, intrinsic) in expected {
        assert_eq!(flavor.min_max_intrinsic(), Some(*intrinsic), "{flavor:?}");
        // The mapping is a bijection with the integer flavours.
        assert_eq!(intrinsic.flavor(), *flavor);
    }
    for flavor in ALL_FLAVORS {
        let integer = matches!(
            flavor,
            SelectPatternFlavor::Smin
                | SelectPatternFlavor::Smax
                | SelectPatternFlavor::Umin
                | SelectPatternFlavor::Umax
        );
        assert_eq!(flavor.min_max_intrinsic().is_some(), integer, "{flavor:?}");
    }
}

/// Ports `llvm::getInverseMinMaxFlavor` and the integer arms of
/// `llvm::getInverseMinMaxIntrinsic`.
#[test]
fn inverting_a_min_max_swaps_min_and_max_and_is_an_involution() {
    let pairs: &[(SelectPatternFlavor, SelectPatternFlavor)] = &[
        (SelectPatternFlavor::Smin, SelectPatternFlavor::Smax),
        (SelectPatternFlavor::Umin, SelectPatternFlavor::Umax),
        (SelectPatternFlavor::Smax, SelectPatternFlavor::Smin),
        (SelectPatternFlavor::Umax, SelectPatternFlavor::Umin),
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

/// Ports the six floating-point arms of `llvm::getInverseMinMaxIntrinsic` —
/// `minimum`/`maximum`, `minimumnum`/`maximumnum` and `minnum`/`maxnum` — and
/// `MinMaxOperation::inverse` over the whole ten-element domain that function
/// covers.
///
/// Upstream's `default` arm is `llvm_unreachable("Unexpected intrinsic")`,
/// which is what a flat `Intrinsic::ID` domain costs. Here the domain is two
/// closed enums, so there is no rejected case left to assert about; what the
/// enumeration checks instead is that every arm maps where upstream maps it.
#[test]
fn inverting_a_floating_point_min_max_swaps_minimum_and_maximum() {
    let pairs: &[(MinMaxKind, MinMaxKind)] = &[
        (MinMaxKind::Minimum, MinMaxKind::Maximum),
        (MinMaxKind::Maximum, MinMaxKind::Minimum),
        (MinMaxKind::MinimumNum, MinMaxKind::MaximumNum),
        (MinMaxKind::MaximumNum, MinMaxKind::MinimumNum),
        (MinMaxKind::MinNum, MinMaxKind::MaxNum),
        (MinMaxKind::MaxNum, MinMaxKind::MinNum),
    ];
    for (kind, inverse) in pairs {
        assert_eq!(kind.inverse(), *inverse, "{kind:?}");
    }

    for kind in MIN_MAX_KINDS {
        // Inverting twice is the identity, and no arm is its own inverse.
        assert_eq!(kind.inverse().inverse(), kind, "{kind:?}");
        assert_ne!(kind.inverse(), kind, "{kind:?}");
        // A minimum inverts to a maximum.
        assert_eq!(kind.inverse().is_maximum(), kind.is_minimum(), "{kind:?}");
        // Inverting swaps the extremum, never the IEEE form or the NaN rule.
        assert_eq!(
            kind.inverse().is_ieee_754_2019_form(),
            kind.is_ieee_754_2019_form(),
            "{kind:?}"
        );
        assert_eq!(
            kind.inverse().returns_the_non_nan_operand(),
            kind.returns_the_non_nan_operand(),
            "{kind:?}"
        );
    }

    // The sum over both halves delegates, and inverting never crosses between
    // the integer and floating-point arms.
    for intrinsic in MIN_MAX_INTRINSICS {
        assert_eq!(
            MinMaxOperation::Integer(intrinsic).inverse(),
            MinMaxOperation::Integer(intrinsic.inverse()),
        );
    }
    for kind in MIN_MAX_KINDS {
        assert_eq!(
            MinMaxOperation::Float(kind).inverse(),
            MinMaxOperation::Float(kind.inverse()),
        );
    }

    // The two arms name ten distinct intrinsics -- the disjointness the sum
    // type's documentation claims.
    let names: Vec<&str> = MIN_MAX_INTRINSICS
        .into_iter()
        .map(MinMaxOperation::Integer)
        .chain(MIN_MAX_KINDS.into_iter().map(MinMaxOperation::Float))
        .map(MinMaxOperation::name)
        .collect();
    let distinct: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(distinct.len(), 10, "{names:?}");
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
            SelectPatternFlavor::Smax.min_max_limit(width),
            Some(ApInt::signed_max_value(width)),
            "smax tops out at the signed maximum"
        );
        assert_eq!(
            SelectPatternFlavor::Smin.min_max_limit(width),
            Some(ApInt::signed_min_value(width))
        );
        assert_eq!(
            SelectPatternFlavor::Umax.min_max_limit(width),
            Some(ApInt::all_ones(width)),
            "getMaxValue is the all-ones pattern"
        );
        assert_eq!(
            SelectPatternFlavor::Umin.min_max_limit(width),
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
        (I::Ugt, SelectPatternFlavor::Umax),
        (I::Uge, SelectPatternFlavor::Umax),
        (I::Sgt, SelectPatternFlavor::Smax),
        (I::Sge, SelectPatternFlavor::Smax),
        (I::Ult, SelectPatternFlavor::Umin),
        (I::Ule, SelectPatternFlavor::Umin),
        (I::Slt, SelectPatternFlavor::Smin),
        (I::Sle, SelectPatternFlavor::Smin),
    ];
    for (predicate, flavor) in integer {
        // Whatever NaN behaviour is passed in, an integer answer discards it.
        let result = select_pattern(
            CmpPredicate::Int(*predicate),
            SelectPatternNanBehavior::ReturnsNaN,
            true,
        );
        assert_eq!(
            result,
            SelectPatternResult {
                flavor: *flavor,
                nan_behavior: SelectPatternNanBehavior::NotApplicable,
                ordered: false,
            },
            "{predicate:?}"
        );
    }

    for predicate in [I::Eq, I::Ne] {
        assert_eq!(
            select_pattern(
                CmpPredicate::Int(predicate),
                SelectPatternNanBehavior::NotApplicable,
                false,
            )
            .flavor,
            SelectPatternFlavor::Unknown,
            "{predicate:?} is an equality, not a min/max"
        );
    }

    let float: &[(FloatPredicate, SelectPatternFlavor)] = &[
        (F::Ugt, SelectPatternFlavor::FmaxNum),
        (F::Uge, SelectPatternFlavor::FmaxNum),
        (F::Ogt, SelectPatternFlavor::FmaxNum),
        (F::Oge, SelectPatternFlavor::FmaxNum),
        (F::Ult, SelectPatternFlavor::FminNum),
        (F::Ule, SelectPatternFlavor::FminNum),
        (F::Olt, SelectPatternFlavor::FminNum),
        (F::Ole, SelectPatternFlavor::FminNum),
    ];
    for (predicate, flavor) in float {
        let result = select_pattern(
            CmpPredicate::Float(*predicate),
            SelectPatternNanBehavior::ReturnsOther,
            true,
        );
        assert_eq!(
            result,
            SelectPatternResult {
                flavor: *flavor,
                nan_behavior: SelectPatternNanBehavior::ReturnsOther,
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
            select_pattern(
                CmpPredicate::Float(predicate),
                SelectPatternNanBehavior::NotApplicable,
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
            let back = select_pattern(predicate, SelectPatternNanBehavior::ReturnsAny, ordered);
            assert_eq!(
                back.flavor, flavor,
                "{flavor:?} ordered={ordered} did not round trip"
            );
        }
    }
}
