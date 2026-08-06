//! What an `fcmp` proves about the floating-point class of its operand.
//!
//! Ports `llvm::FloatingPointPredicateUtils`
//! (`llvm/Analysis/FloatingPointPredicateUtils.h`), which is the IR
//! instantiation of the template `llvm::GenericFloatingPointPredicateUtils`
//! (`llvm/IR/GenericFloatingPointPredicateUtils.h`). Upstream shares that
//! template with the MIR world through `MachineFloatingPointPredicateUtils`;
//! llvmkit has no machine layer, so the generic parameters are resolved to the
//! IR forms here and the template is not reproduced.
//!
//! The entry point is [`fcmp_implies_class`]: given `fcmp <pred> lhs, rhs`, it
//! answers which classes `lhs` may belong to when the comparison is true and
//! when it is false. [`fcmp_to_class_test`] is the stricter question — the
//! single `llvm.is.fpclass` mask equivalent to the comparison, which exists only
//! when the two answers are complements.
//!
//! Upstream threads a `const Function &` through every entry point, but only
//! `queryDenormalMode` reads it — for the `denormal-fp-math` attribute, and
//! only in the arm comparing against a zero. That function is a parameter here
//! too, because it is the function holding the *comparison*, which is not
//! always the one holding the value being asked about.

use crate::Branded;
use crate::ap_float::ApFloatSemantics;
use crate::cmp_predicate::FloatPredicate;
use crate::constant::ConstantData;
use crate::denormal_mode::{DenormalMode, DenormalModeKind};
use crate::fp_class::FpClassTest;
use crate::function::FunctionValue;
use crate::instruction::InstructionKindData;
use crate::intrinsics::descriptor_for_callee;
use crate::marker::Dyn;
use crate::module::{ModuleBrand, ModuleRef};
use crate::r#type::{Type, TypeKind};
use crate::value::{Value, ValueKindData, ValueSlot};
use crate::{ApFloat, ApInt};

/// What an `fcmp` proves about the class of the value it tests.
///
/// Ports the `std::tuple<ValueRefT, FPClassTest, FPClassTest>` that
/// `fcmpImpliesClass` returns. Upstream signals "nothing was proved" by leaving
/// the first element null and both masks `fcAllFlags`, and every caller checks
/// the value before reading the masks; that sentinel becomes `None` on the
/// functions returning this type, so a value in hand always carries a real
/// answer.
#[derive(Branded)]
#[branded(Debug)]
pub struct ImpliedFpClasses<'ctx, B: ModuleBrand> {
    tested: Value<'ctx, B>,
    if_true: FpClassTest,
    if_false: FpClassTest,
}

// A `derive` would bound `B: Clone + Copy`, which a bare brand does not satisfy.
impl<B: ModuleBrand> Clone for ImpliedFpClasses<'_, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: ModuleBrand> Copy for ImpliedFpClasses<'_, B> {}

impl<'ctx, B: ModuleBrand + 'ctx> ImpliedFpClasses<'ctx, B> {
    /// The value the classes describe.
    ///
    /// This is the comparison's left-hand side, or — when the caller asked to
    /// look through it and it was an `llvm.fabs` — that call's operand.
    pub fn tested(self) -> Value<'ctx, B> {
        self.tested
    }

    /// The classes [`Self::tested`] may belong to when the comparison is true.
    pub fn if_true(self) -> FpClassTest {
        self.if_true
    }

    /// The classes [`Self::tested`] may belong to when the comparison is false.
    pub fn if_false(self) -> FpClassTest {
        self.if_false
    }

    /// The classes for one branch of the comparison.
    pub fn if_condition_is(self, condition_is_true: bool) -> FpClassTest {
        if condition_is_true {
            self.if_true
        } else {
            self.if_false
        }
    }

    /// Whether the comparison is an exact class test — the two answers are
    /// complements, so it decides membership rather than merely narrowing it.
    ///
    /// Ports the `ClassIfTrue == ~ClassIfFalse` check in `fcmpToClassTest`.
    pub fn is_exact(self) -> bool {
        self.if_true == self.if_false.complement()
    }
}

/// Ports the private `exactClass` helper: a comparison that decides membership
/// answers `M` when true and everything else when false.
fn exact_class<'ctx, B: ModuleBrand + 'ctx>(
    tested: Value<'ctx, B>,
    mask: FpClassTest,
) -> Option<ImpliedFpClasses<'ctx, B>> {
    Some(ImpliedFpClasses {
        tested,
        if_true: mask,
        if_false: mask.complement(),
    })
}

/// A non-exact answer, where the true and false masks are independent.
fn implied<'ctx, B: ModuleBrand + 'ctx>(
    tested: Value<'ctx, B>,
    if_true: FpClassTest,
    if_false: FpClassTest,
) -> Option<ImpliedFpClasses<'ctx, B>> {
    Some(ImpliedFpClasses {
        tested,
        if_true,
        if_false,
    })
}

/// Which classes `lhs` may belong to given `fcmp <predicate> lhs, rhs`.
///
/// Ports the `(CmpInst::Predicate, const FunctionT &, ValueRefT LHS, ValueRefT
/// RHS, bool)` overload of `fcmpImpliesClass`. `None` where upstream returns a
/// null tested value — nothing was proved.
///
/// `rhs` must be a floating-point constant; upstream's own `TODO` to call
/// `computeKnownFPClass` on a non-constant right-hand side is inherited.
///
/// With `look_through_source` set, an `llvm.fabs` on the left-hand side is
/// seen through: the answer then describes that call's operand, which
/// [`ImpliedFpClasses::tested`] reports.
pub fn fcmp_implies_class<'ctx, B: ModuleBrand + 'ctx>(
    predicate: FloatPredicate,
    function: FunctionValue<'ctx, Dyn, B>,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    look_through_source: bool,
) -> Option<ImpliedFpClasses<'ctx, B>> {
    let constant_rhs = match_constant_float(rhs)?;
    fcmp_implies_class_of_constant(predicate, function, lhs, &constant_rhs, look_through_source)
}

/// [`fcmp_implies_class`] against a constant already in hand.
///
/// Ports the `const APFloat &ConstRHS` overload of `fcmpImpliesClass`, whose
/// job is to recognise the two comparisons against the smallest normal value
/// that `__builtin_isnormal` expands to; everything else forwards to the
/// class-keyed form.
pub fn fcmp_implies_class_of_constant<'ctx, B: ModuleBrand + 'ctx>(
    predicate: FloatPredicate,
    function: FunctionValue<'ctx, Dyn, B>,
    lhs: Value<'ctx, B>,
    constant_rhs: &ApFloat,
    look_through_source: bool,
) -> Option<ImpliedFpClasses<'ctx, B>> {
    // Checks against the smallest normal — equivalently the largest subnormal —
    // refine to an exact class test.
    if !constant_rhs.is_negative() && constant_rhs.is_smallest_normalized() {
        let (source, is_fabs) = look_through_fabs(lhs, look_through_source);

        let mut mask = match predicate {
            // fcmp olt x, smallest_normal       -> fcNegInf|fcNegNormal|fcSubnormal|fcZero
            // fcmp olt fabs(x), smallest_normal -> fcSubnormal|fcZero
            // fcmp uge x, smallest_normal       -> fcNan|fcPosNormal|fcPosInf
            // fcmp uge fabs(x), smallest_normal -> ~(fcSubnormal|fcZero)
            FloatPredicate::Olt | FloatPredicate::Uge => {
                let mut mask = FpClassTest::ZERO.union(FpClassTest::SUBNORMAL);
                if !is_fabs {
                    mask |= FpClassTest::NEGATIVE_NORMAL.union(FpClassTest::NEGATIVE_INFINITY);
                }
                mask
            }
            // fcmp oge x, smallest_normal       -> fcPosNormal | fcPosInf
            // fcmp oge fabs(x), smallest_normal -> fcInf | fcNormal
            // fcmp ult x, smallest_normal       -> ~(fcPosNormal | fcPosInf)
            // fcmp ult fabs(x), smallest_normal -> ~(fcInf | fcNormal)
            FloatPredicate::Oge | FloatPredicate::Ult => {
                let mut mask = FpClassTest::POSITIVE_INFINITY.union(FpClassTest::POSITIVE_NORMAL);
                if is_fabs {
                    mask |= FpClassTest::NEGATIVE_INFINITY.union(FpClassTest::NEGATIVE_NORMAL);
                }
                mask
            }
            _ => {
                return fcmp_implies_class_of_class(
                    predicate,
                    function,
                    lhs,
                    FpClassTest::of(constant_rhs),
                    look_through_source,
                );
            }
        };

        // Invert the comparison for the unordered cases.
        if predicate.is_unordered() {
            mask = mask.complement();
        }

        return exact_class(source, mask);
    }

    fcmp_implies_class_of_class(
        predicate,
        function,
        lhs,
        FpClassTest::of(constant_rhs),
        look_through_source,
    )
}

/// [`fcmp_implies_class`] against a right-hand side reduced to its class.
///
/// Ports the `FPClassTest RHSClass` overload of `fcmpImpliesClass`, which is
/// where the reasoning lives. Upstream asserts `RHSClass != fcNone`; an empty
/// class describes no value at all, so that precondition becomes `None`.
pub fn fcmp_implies_class_of_class<'ctx, B: ModuleBrand + 'ctx>(
    predicate: FloatPredicate,
    function: FunctionValue<'ctx, Dyn, B>,
    lhs: Value<'ctx, B>,
    rhs_class: FpClassTest,
    look_through_source: bool,
) -> Option<ImpliedFpClasses<'ctx, B>> {
    if rhs_class.is_none() {
        return None;
    }

    let source = lhs;

    if predicate == FloatPredicate::True {
        return exact_class(source, FpClassTest::ALL);
    }
    if predicate == FloatPredicate::False {
        return exact_class(source, FpClassTest::NONE);
    }

    let original_class = rhs_class;

    let is_negative_rhs = rhs_class.intersection(FpClassTest::NEGATIVE) == rhs_class;
    let is_positive_rhs = rhs_class.intersection(FpClassTest::POSITIVE) == rhs_class;
    let is_nan = rhs_class.difference(FpClassTest::NAN).is_none();

    if is_nan {
        // fcmp o__ x, nan -> false
        // fcmp u__ x, nan -> true
        return exact_class(
            source,
            if predicate.is_ordered() {
                FpClassTest::NONE
            } else {
                FpClassTest::ALL
            },
        );
    }

    // fcmp ord x, zero|normal|subnormal|inf -> ~fcNan
    if predicate == FloatPredicate::Ord {
        return exact_class(source, FpClassTest::NAN.complement());
    }

    // fcmp uno x, zero|normal|subnormal|inf -> fcNan
    if predicate == FloatPredicate::Uno {
        return exact_class(source, FpClassTest::NAN);
    }

    let (source, is_fabs) = look_through_fabs(lhs, look_through_source);
    let rhs_class = if is_fabs {
        rhs_class.inverse_absolute()
    } else {
        rhs_class
    };

    let is_zero = original_class.intersection(FpClassTest::ZERO) == original_class;
    if is_zero {
        // A comparison against zero is exact only when input denormals reach the
        // comparison intact. Upstream's `TODO` to handle flush-to-zero by
        // expanding the masks over the subnormal cases is inherited.
        if query_denormal_mode(function, lhs).input() != DenormalModeKind::Ieee {
            return None;
        }

        // The `fabs` fold that the sign-sensitive arms need; the arms above it
        // are already sign-symmetric, so upstream applies it only here.
        let folded = |mask: FpClassTest| {
            exact_class(
                source,
                if is_fabs {
                    mask.inverse_absolute()
                } else {
                    mask
                },
            )
        };

        // `True`/`False` returned above, and `Ord`/`Uno` have their own arms, so
        // every predicate is accounted for and the match needs no fallback —
        // where upstream writes `llvm_unreachable("all compare types are
        // handled")`, exhaustiveness proves the same thing at compile time.
        return match predicate {
            // Match x == 0.0
            FloatPredicate::Oeq => exact_class(source, FpClassTest::ZERO),
            // Match isnan(x) || (x == 0.0)
            FloatPredicate::Ueq => exact_class(source, FpClassTest::ZERO.union(FpClassTest::NAN)),
            // Match (x != 0.0)
            FloatPredicate::Une => exact_class(source, FpClassTest::ZERO.complement()),
            // Match !isnan(x) && x != 0.0
            FloatPredicate::One => exact_class(
                source,
                FpClassTest::NAN
                    .complement()
                    .intersection(FpClassTest::ZERO.complement()),
            ),
            // The canonical spelling of ord/uno is against a zero. Upstream
            // notes it could also handle other non-NaN constants, or LHS == RHS.
            FloatPredicate::Ord => exact_class(source, FpClassTest::NAN.complement()),
            FloatPredicate::Uno => exact_class(source, FpClassTest::NAN),
            // x > 0
            FloatPredicate::Ogt => folded(
                FpClassTest::POSITIVE_SUBNORMAL
                    .union(FpClassTest::POSITIVE_NORMAL)
                    .union(FpClassTest::POSITIVE_INFINITY),
            ),
            // isnan(x) || x > 0
            FloatPredicate::Ugt => folded(
                FpClassTest::POSITIVE_SUBNORMAL
                    .union(FpClassTest::POSITIVE_NORMAL)
                    .union(FpClassTest::POSITIVE_INFINITY)
                    .union(FpClassTest::NAN),
            ),
            // x >= 0
            FloatPredicate::Oge => folded(FpClassTest::POSITIVE.union(FpClassTest::NEGATIVE_ZERO)),
            // isnan(x) || x >= 0
            FloatPredicate::Uge => folded(
                FpClassTest::POSITIVE
                    .union(FpClassTest::NEGATIVE_ZERO)
                    .union(FpClassTest::NAN),
            ),
            // x < 0
            FloatPredicate::Olt => folded(
                FpClassTest::NEGATIVE_SUBNORMAL
                    .union(FpClassTest::NEGATIVE_NORMAL)
                    .union(FpClassTest::NEGATIVE_INFINITY),
            ),
            // isnan(x) || x < 0
            FloatPredicate::Ult => folded(
                FpClassTest::NEGATIVE_SUBNORMAL
                    .union(FpClassTest::NEGATIVE_NORMAL)
                    .union(FpClassTest::NEGATIVE_INFINITY)
                    .union(FpClassTest::NAN),
            ),
            // x <= 0
            FloatPredicate::Ole => folded(FpClassTest::NEGATIVE.union(FpClassTest::POSITIVE_ZERO)),
            // isnan(x) || x <= 0
            FloatPredicate::Ule => folded(
                FpClassTest::NEGATIVE
                    .union(FpClassTest::POSITIVE_ZERO)
                    .union(FpClassTest::NAN),
            ),
            FloatPredicate::True | FloatPredicate::False => None,
        };
    }

    let is_denormal_rhs = original_class.intersection(FpClassTest::SUBNORMAL) == original_class;

    let is_infinity = original_class.intersection(FpClassTest::INFINITY) == original_class;
    if is_infinity {
        // As in the zero arm, every predicate is covered, so the match is total.
        let mut mask = match predicate {
            // Match the __builtin_isinf patterns:
            //
            //   fcmp oeq x, +inf       -> is_fpclass x, fcPosInf
            //   fcmp oeq fabs(x), +inf -> is_fpclass x, fcInf
            //   fcmp oeq x, -inf       -> is_fpclass x, fcNegInf
            //   fcmp oeq fabs(x), -inf -> is_fpclass x, 0 -> false
            //
            //   fcmp une x, +inf       -> is_fpclass x, ~fcPosInf
            //   fcmp une fabs(x), +inf -> is_fpclass x, ~fcInf
            //   fcmp une x, -inf       -> is_fpclass x, ~fcNegInf
            //   fcmp une fabs(x), -inf -> is_fpclass x, fcAllFlags -> true
            FloatPredicate::Oeq | FloatPredicate::Une => {
                if is_negative_rhs {
                    if is_fabs {
                        FpClassTest::NONE
                    } else {
                        FpClassTest::NEGATIVE_INFINITY
                    }
                } else {
                    let mut mask = FpClassTest::POSITIVE_INFINITY;
                    if is_fabs {
                        mask |= FpClassTest::NEGATIVE_INFINITY;
                    }
                    mask
                }
            }
            // Match the __builtin_isinf patterns:
            //   fcmp one x, -inf       -> is_fpclass x, fcNegInf
            //   fcmp one fabs(x), -inf -> is_fpclass x, ~fcNegInf & ~fcNan
            //   fcmp one x, +inf       -> is_fpclass x, ~fcNegInf & ~fcNan
            //   fcmp one fabs(x), +inf -> is_fpclass x, ~fcInf & fcNan
            //
            //   fcmp ueq x, +inf       -> is_fpclass x, fcPosInf|fcNan
            //   fcmp ueq (fabs x), +inf -> is_fpclass x, fcInf|fcNan
            //   fcmp ueq x, -inf       -> is_fpclass x, fcNegInf|fcNan
            //   fcmp ueq fabs(x), -inf -> is_fpclass x, fcNan
            FloatPredicate::One | FloatPredicate::Ueq => {
                if is_negative_rhs {
                    if is_fabs {
                        FpClassTest::NAN.complement()
                    } else {
                        FpClassTest::NEGATIVE_INFINITY
                            .complement()
                            .intersection(FpClassTest::NAN.complement())
                    }
                } else {
                    let mut mask = FpClassTest::POSITIVE_INFINITY
                        .complement()
                        .intersection(FpClassTest::NAN.complement());
                    if is_fabs {
                        mask = mask.intersection(FpClassTest::NEGATIVE_INFINITY.complement());
                    }
                    mask
                }
            }
            FloatPredicate::Olt | FloatPredicate::Uge => {
                if is_negative_rhs {
                    // No value is ordered and less than negative infinity, and
                    // every value is unordered with or at least it.
                    // fcmp olt x, -inf -> false
                    // fcmp uge x, -inf -> true
                    FpClassTest::NONE
                } else {
                    // fcmp olt fabs(x), +inf -> fcFinite
                    // fcmp uge fabs(x), +inf -> ~fcFinite
                    // fcmp olt x, +inf       -> fcFinite|fcNegInf
                    // fcmp uge x, +inf       -> ~(fcFinite|fcNegInf)
                    let mut mask = FpClassTest::FINITE;
                    if !is_fabs {
                        mask |= FpClassTest::NEGATIVE_INFINITY;
                    }
                    mask
                }
            }
            FloatPredicate::Oge | FloatPredicate::Ult => {
                if is_negative_rhs {
                    // fcmp oge x, -inf       -> ~fcNan
                    // fcmp oge fabs(x), -inf -> ~fcNan
                    // fcmp ult x, -inf       -> fcNan
                    // fcmp ult fabs(x), -inf -> fcNan
                    FpClassTest::NAN.complement()
                } else {
                    // fcmp oge fabs(x), +inf -> fcInf
                    // fcmp oge x, +inf       -> fcPosInf
                    // fcmp ult fabs(x), +inf -> ~fcInf
                    // fcmp ult x, +inf       -> ~fcPosInf
                    let mut mask = FpClassTest::POSITIVE_INFINITY;
                    if is_fabs {
                        mask |= FpClassTest::NEGATIVE_INFINITY;
                    }
                    mask
                }
            }
            FloatPredicate::Ogt | FloatPredicate::Ule => {
                if is_negative_rhs {
                    // fcmp ogt x, -inf       -> fcmp one x, -inf
                    // fcmp ogt fabs(x), -inf -> fcmp ord x, x
                    // fcmp ule x, -inf       -> fcmp ueq x, -inf
                    // fcmp ule fabs(x), -inf -> fcmp uno x, x
                    if is_fabs {
                        FpClassTest::NAN.complement()
                    } else {
                        FpClassTest::NEGATIVE_INFINITY
                            .union(FpClassTest::NAN)
                            .complement()
                    }
                } else {
                    // No value is ordered and greater than infinity.
                    FpClassTest::NONE
                }
            }
            FloatPredicate::Ole | FloatPredicate::Ugt => {
                if is_negative_rhs {
                    if is_fabs {
                        FpClassTest::NONE
                    } else {
                        FpClassTest::NEGATIVE_INFINITY
                    }
                } else {
                    // fcmp ole x, +inf       -> fcmp ord x, x
                    // fcmp ole fabs(x), +inf -> fcmp ord x, x
                    // fcmp ole x, -inf       -> fcmp oeq x, -inf
                    // fcmp ole fabs(x), -inf -> false
                    FpClassTest::NAN.complement()
                }
            }
            FloatPredicate::Ord
            | FloatPredicate::Uno
            | FloatPredicate::True
            | FloatPredicate::False => return None,
        };

        // Invert the comparison for the unordered cases.
        if predicate.is_unordered() {
            mask = mask.complement();
        }

        return exact_class(source, mask);
    }

    if predicate == FloatPredicate::Oeq {
        return implied(source, rhs_class, FpClassTest::ALL);
    }

    if predicate == FloatPredicate::Ueq {
        return implied(
            source,
            rhs_class.union(FpClassTest::NAN),
            FpClassTest::NAN.complement(),
        );
    }

    if predicate == FloatPredicate::One {
        return implied(
            source,
            FpClassTest::NAN.complement(),
            rhs_class.union(FpClassTest::NAN),
        );
    }

    if predicate == FloatPredicate::Une {
        return implied(source, FpClassTest::ALL, rhs_class);
    }

    // Upstream asserts here that the remaining right-hand classes are exactly
    // the normal and subnormal ones, everything else having been recognised as
    // an exact class test above.

    if is_negative_rhs {
        // Upstream's `TODO: Handle fneg(fabs)` is inherited.
        if is_fabs {
            // fabs(x) o> -k -> fcmp ord x, x
            // fabs(x) u> -k -> true
            // fabs(x) o< -k -> false
            // fabs(x) u< -k -> fcmp uno x, x
            return match predicate {
                FloatPredicate::Ogt | FloatPredicate::Oge => {
                    implied(source, FpClassTest::NAN.complement(), FpClassTest::NAN)
                }
                FloatPredicate::Ugt | FloatPredicate::Uge => {
                    implied(source, FpClassTest::ALL, FpClassTest::NONE)
                }
                FloatPredicate::Olt | FloatPredicate::Ole => {
                    implied(source, FpClassTest::NONE, FpClassTest::ALL)
                }
                FloatPredicate::Ult | FloatPredicate::Ule => {
                    implied(source, FpClassTest::NAN, FpClassTest::NAN.complement())
                }
                _ => None,
            };
        }

        let mut classes_le = FpClassTest::NEGATIVE_INFINITY.union(FpClassTest::NEGATIVE_NORMAL);
        let mut classes_ge = FpClassTest::POSITIVE
            .union(FpClassTest::NEGATIVE_ZERO)
            .union(FpClassTest::NEGATIVE_SUBNORMAL);

        if is_denormal_rhs {
            classes_le |= FpClassTest::NEGATIVE_SUBNORMAL;
        } else {
            classes_ge |= FpClassTest::NEGATIVE_NORMAL;
        }

        return ordered_bound(predicate, source, rhs_class, classes_ge, classes_le);
    } else if is_positive_rhs {
        let mut classes_ge = FpClassTest::POSITIVE_NORMAL.union(FpClassTest::POSITIVE_INFINITY);
        let mut classes_le = FpClassTest::NEGATIVE
            .union(FpClassTest::POSITIVE_ZERO)
            .union(FpClassTest::POSITIVE_SUBNORMAL);

        if is_denormal_rhs {
            classes_ge |= FpClassTest::POSITIVE_SUBNORMAL;
        } else {
            classes_le |= FpClassTest::POSITIVE_NORMAL;
        }

        if is_fabs {
            classes_ge = classes_ge.inverse_absolute();
            classes_le = classes_le.inverse_absolute();
        }

        return ordered_bound(predicate, source, rhs_class, classes_ge, classes_le);
    }

    None
}

/// The relational tail shared by the negative and positive right-hand cases:
/// the same four predicate groups against a pair of already-computed bounds.
fn ordered_bound<'ctx, B: ModuleBrand + 'ctx>(
    predicate: FloatPredicate,
    source: Value<'ctx, B>,
    rhs_class: FpClassTest,
    classes_ge: FpClassTest,
    classes_le: FpClassTest,
) -> Option<ImpliedFpClasses<'ctx, B>> {
    // The false mask keeps the right-hand class itself: `x > k` being false
    // leaves `x <= k`, and `x == k` is in that.
    let ordered = |bound: FpClassTest| implied(source, bound, bound.complement().union(rhs_class));
    let unordered = |bound: FpClassTest| {
        let bound = bound.union(FpClassTest::NAN);
        implied(source, bound, bound.complement().union(rhs_class))
    };

    match predicate {
        FloatPredicate::Ogt | FloatPredicate::Oge => ordered(classes_ge),
        FloatPredicate::Ugt | FloatPredicate::Uge => unordered(classes_ge),
        FloatPredicate::Olt | FloatPredicate::Ole => ordered(classes_le),
        FloatPredicate::Ult | FloatPredicate::Ule => unordered(classes_le),
        _ => None,
    }
}

/// The single `llvm.is.fpclass` mask equivalent to `fcmp <predicate> lhs, rhs`.
///
/// Ports `fcmpToClassTest`. It is the strict form of [`fcmp_implies_class`]:
/// where that narrows the class either way the comparison goes, this succeeds
/// only when the comparison *decides* membership — upstream's example is that
/// `x > 0` implies positive but `x > 1` does not.
pub fn fcmp_to_class_test<'ctx, B: ModuleBrand + 'ctx>(
    predicate: FloatPredicate,
    function: FunctionValue<'ctx, Dyn, B>,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    look_through_source: bool,
) -> Option<(Value<'ctx, B>, FpClassTest)> {
    let constant_rhs = match_constant_float(rhs)?;
    fcmp_to_class_test_of_constant(predicate, function, lhs, &constant_rhs, look_through_source)
}

/// [`fcmp_to_class_test`] against a constant already in hand.
///
/// Ports the `const APFloat &ConstRHS` overload of `fcmpToClassTest`.
pub fn fcmp_to_class_test_of_constant<'ctx, B: ModuleBrand + 'ctx>(
    predicate: FloatPredicate,
    function: FunctionValue<'ctx, Dyn, B>,
    lhs: Value<'ctx, B>,
    constant_rhs: &ApFloat,
    look_through_source: bool,
) -> Option<(Value<'ctx, B>, FpClassTest)> {
    let implied = fcmp_implies_class_of_constant(
        predicate,
        function,
        lhs,
        constant_rhs,
        look_through_source,
    )?;
    implied
        .is_exact()
        .then(|| (implied.tested(), implied.if_true()))
}

/// The denormal mode `function` gives to `value`'s element type.
///
/// Ports `FloatingPointPredicateUtils::queryDenormalMode`, which is
/// `F.getDenormalMode(Val->getType()->getScalarType()->getFltSemantics())`. Note
/// which operand supplies what: the *type* comes from the value, the *mode*
/// from the function the caller passes — which is the enclosing function of the
/// comparison, not of the value. A value that is a bare argument still gets a
/// mode this way.
fn query_denormal_mode<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, Dyn, B>,
    value: Value<'ctx, B>,
) -> DenormalMode {
    // Upstream's `getFltSemantics()` asserts on a non-float type; only float
    // comparisons reach here, so the fallback is unreachable in practice and
    // takes the answer that teaches nothing.
    match scalar_semantics(value.ty()) {
        Some(semantics) => function.denormal_mode(semantics),
        None => DenormalMode::dynamic(),
    }
}

/// The denormal mode governing an *instruction's* own result type.
///
/// Ports the `const Function *F = I->getFunction(); F ? F->getDenormalMode(...)
/// : DenormalMode::getDynamic()` idiom that `computeKnownFPClass` repeats in
/// every arm needing one. A value outside any function has no attribute to read,
/// which is exactly upstream's null-`F` case.
pub(crate) fn denormal_mode_of<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> DenormalMode {
    match enclosing_function(value) {
        Some(function) => query_denormal_mode(function, value),
        None => DenormalMode::dynamic(),
    }
}

/// The function `value` is computed in, for callers outside this module.
pub(crate) fn enclosing_function_of<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<FunctionValue<'ctx, Dyn, B>> {
    enclosing_function(value)
}

/// See through an `llvm.fabs`, reporting whether one was there.
///
/// Ports `FloatingPointPredicateUtils::lookThroughFAbs` together with its
/// `LookThroughSrc &&` guard at every call site.
fn look_through_fabs<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    look_through_source: bool,
) -> (Value<'ctx, B>, bool) {
    if !look_through_source {
        return (value, false);
    }
    match fabs_operand(value) {
        Some(source) => (source, true),
        None => (value, false),
    }
}

/// The operand of an `llvm.fabs` call. Ports `m_FAbs(m_Value(Src))`.
fn fabs_operand<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    let ValueKindData::Instruction(instruction) = &value.data().kind else {
        return None;
    };
    let InstructionKindData::Call(data) = &instruction.kind else {
        return None;
    };
    let callee = value_from_slot(value, data.callee.get());
    if descriptor_for_callee(callee)?.id().base_name() != "llvm.fabs" {
        return None;
    }
    data.args
        .first()
        .map(|argument| value_from_slot(value, argument.get()))
}

/// The floating-point constant `value` is, if it is one.
///
/// Ports `FloatingPointPredicateUtils::matchConstantFloat`, which is
/// `m_APFloatAllowPoison`. llvmkit stores a scalar float constant as its raw bit
/// pattern, so the splat-with-poison-elements half of that matcher has nothing
/// to look at here and only the scalar case is recognised.
fn match_constant_float<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApFloat> {
    let ValueKindData::Constant(ConstantData::Float(bits)) = &value.data().kind else {
        return None;
    };
    let semantics = scalar_semantics(value.ty())?;
    // The same decode `ConstantFloatValue::ap_float` performs: the stored `u128`
    // is the raw bit pattern, low word first.
    let low = u64::try_from(*bits & 0xffff_ffff_ffff_ffff).ok()?;
    let high = u64::try_from(*bits >> 64).ok()?;
    let pattern = ApInt::from_words(semantics.bit_width(), &[low, high]);
    ApFloat::from_bits(semantics, &pattern).ok()
}

/// The function `value` is computed in. Ports `Instruction::getFunction`.
fn enclosing_function<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<FunctionValue<'ctx, Dyn, B>> {
    let ValueKindData::Instruction(data) = &value.data().kind else {
        return None;
    };
    let block = value_from_slot(value, data.parent.get());
    let ValueKindData::BasicBlock(block_data) = &block.data().kind else {
        return None;
    };
    let parent = (*block_data.parent.borrow())?;
    FunctionValue::try_from(value_from_slot(value, parent)).ok()
}

/// The `ApFloat` semantics of a scalar or per-lane floating-point type.
fn scalar_semantics<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Option<ApFloatSemantics> {
    let kind = match ty.data().as_vector() {
        Some((element, _, _)) => Type::new(element, ty.module()).kind(),
        None => ty.kind(),
    };
    Some(match kind {
        TypeKind::Half => ApFloatSemantics::IeeeHalf,
        TypeKind::BFloat => ApFloatSemantics::BFloat,
        TypeKind::Float => ApFloatSemantics::IeeeSingle,
        TypeKind::Double => ApFloatSemantics::IeeeDouble,
        TypeKind::Fp128 => ApFloatSemantics::IeeeQuad,
        TypeKind::X86Fp80 => ApFloatSemantics::X87DoubleExtended,
        TypeKind::PpcFp128 => ApFloatSemantics::PpcDoubleDouble,
        _ => return None,
    })
}

/// Re-anchor a slot as a value in the same module.
fn value_from_slot<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    slot: ValueSlot,
) -> Value<'ctx, B> {
    let module: ModuleRef<B> = ModuleRef::new(anchor.module().core_ref());
    let data = module.value_data(slot);
    Value::from_parts(slot, module, data.ty)
}
