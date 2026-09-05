//! Select-pattern classification — the min/max/abs idioms a `select` can spell.
//!
//! Mirrors the `SelectPatternResult` slice of
//! `llvm/include/llvm/Analysis/ValueTracking.h` and its implementation in
//! `llvm/lib/Analysis/ValueTracking.cpp`.
//!
//! This module is the *vocabulary*: the flavours, the result record, and the
//! total functions that map between a flavour and its predicate, intrinsic,
//! inverse and saturating limit. Matching an actual `select` against these
//! flavours (`matchSelectPattern` and friends) is a separate piece of work and
//! is recorded in the parity ledger until it lands.

use crate::ap_int::ApInt;
use crate::cmp_predicate::{CmpPredicate, FloatPredicate, IntPredicate};
use crate::fp_class::MinMaxKind;
use crate::intrinsics::descriptor_for_callee;

/// Which min/max/abs idiom a `select` implements.
///
/// Ports `llvm::SelectPatternFlavor`. `SPF_UNKNOWN` is spelled [`Self::Unknown`]
/// rather than dropped, because [`SelectPatternResult`] is the return type of a
/// classification that legitimately fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectPatternFlavor {
    /// Not a recognised pattern.
    Unknown,
    /// Signed minimum.
    Smin,
    /// Unsigned minimum.
    Umin,
    /// Signed maximum.
    Smax,
    /// Unsigned maximum.
    Umax,
    /// Floating-point `minnum`.
    FminNum,
    /// Floating-point `maxnum`.
    FmaxNum,
    /// Absolute value.
    Abs,
    /// Negated absolute value.
    Nabs,
}

impl SelectPatternFlavor {
    /// Whether this flavour is a minimum or a maximum.
    ///
    /// Ports `SelectPatternResult::isMinOrMax`, which lives on the result
    /// upstream but reads only the flavour.
    #[inline]
    pub const fn is_min_or_max(self) -> bool {
        !matches!(self, Self::Unknown | Self::Abs | Self::Nabs)
    }

    /// The canonical comparison predicate for this minimum/maximum.
    ///
    /// Ports `llvm::getMinMaxPred`. `ordered` selects between the ordered and
    /// unordered float predicate and is ignored by the integer flavours.
    ///
    /// Upstream ends in `llvm_unreachable` for the three non-min/max flavours;
    /// here that precondition is the `None`, so a caller cannot read a
    /// predicate that was never defined.
    #[inline]
    pub const fn min_max_predicate(self, ordered: bool) -> Option<CmpPredicate> {
        Some(match self {
            Self::Smin => CmpPredicate::Int(IntPredicate::Slt),
            Self::Umin => CmpPredicate::Int(IntPredicate::Ult),
            Self::Smax => CmpPredicate::Int(IntPredicate::Sgt),
            Self::Umax => CmpPredicate::Int(IntPredicate::Ugt),
            Self::FminNum => CmpPredicate::Float(if ordered {
                FloatPredicate::Olt
            } else {
                FloatPredicate::Ult
            }),
            Self::FmaxNum => CmpPredicate::Float(if ordered {
                FloatPredicate::Ogt
            } else {
                FloatPredicate::Ugt
            }),
            Self::Unknown | Self::Abs | Self::Nabs => return None,
        })
    }

    /// The integer min/max intrinsic equivalent to this flavour.
    ///
    /// Ports `llvm::getMinMaxIntrinsic`, whose doc says "Caller must ensure
    /// `SPF` is an integer min or max pattern" and whose `default` arm is
    /// `llvm_unreachable`. That precondition is the `None` here.
    #[inline]
    pub const fn min_max_intrinsic(self) -> Option<MinMaxIntrinsic> {
        Some(match self {
            Self::Smin => MinMaxIntrinsic::Smin,
            Self::Smax => MinMaxIntrinsic::Smax,
            Self::Umin => MinMaxIntrinsic::Umin,
            Self::Umax => MinMaxIntrinsic::Umax,
            _ => return None,
        })
    }

    /// The opposite minimum/maximum: signed minimum inverts to signed maximum,
    /// and so on.
    ///
    /// Ports `llvm::getInverseMinMaxFlavor`, which handles the four integer
    /// flavours and is `llvm_unreachable` for the rest — including the two
    /// float ones, which it does *not* cover.
    #[inline]
    pub const fn inverse_min_max(self) -> Option<Self> {
        Some(match self {
            Self::Smin => Self::Smax,
            Self::Smax => Self::Smin,
            Self::Umin => Self::Umax,
            Self::Umax => Self::Umin,
            _ => return None,
        })
    }

    /// The extreme value this minimum/maximum can produce at `bit_width`.
    ///
    /// Ports `llvm::getMinMaxLimit`, "the minimum or maximum constant value
    /// for the specified integer min/max flavor and type": a signed maximum
    /// tops out at `signed_max_value`, an unsigned minimum bottoms out at
    /// zero. Note this is the *limit*, not the identity element — the identity
    /// of `smax` is the signed **minimum**, which is the opposite end.
    #[inline]
    pub fn min_max_limit(self, bit_width: u32) -> Option<ApInt> {
        Some(match self {
            Self::Smax => ApInt::signed_max_value(bit_width),
            Self::Smin => ApInt::signed_min_value(bit_width),
            Self::Umax => ApInt::all_ones(bit_width),
            Self::Umin => ApInt::zero(bit_width),
            _ => return None,
        })
    }
}

/// The four integer min/max intrinsics.
///
/// A dedicated enum rather than llvmkit's crate-internal intrinsic semantic,
/// so the mapping can be part of the public API. It is also exactly the range
/// of `llvm::getMinMaxIntrinsic`, which makes [`Self::inverse`] total where
/// upstream's `getInverseMinMaxIntrinsic` needs an `llvm_unreachable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MinMaxIntrinsic {
    /// `llvm.smin`.
    Smin,
    /// `llvm.smax`.
    Smax,
    /// `llvm.umin`.
    Umin,
    /// `llvm.umax`.
    Umax,
}

impl MinMaxIntrinsic {
    /// The intrinsic computing the opposite extremum.
    ///
    /// Ports the integer arms of `llvm::getInverseMinMaxIntrinsic`. The six
    /// floating-point arms it also covers are [`MinMaxKind::inverse`], and
    /// [`MinMaxOperation::inverse`] is the whole function over both.
    #[inline]
    pub const fn inverse(self) -> Self {
        match self {
            Self::Smin => Self::Smax,
            Self::Smax => Self::Smin,
            Self::Umin => Self::Umax,
            Self::Umax => Self::Umin,
        }
    }

    /// The flavour this intrinsic implements.
    #[inline]
    pub const fn flavor(self) -> SelectPatternFlavor {
        match self {
            Self::Smin => SelectPatternFlavor::Smin,
            Self::Smax => SelectPatternFlavor::Smax,
            Self::Umin => SelectPatternFlavor::Umin,
            Self::Umax => SelectPatternFlavor::Umax,
        }
    }

    /// The intrinsic's base name, as it appears in `.ll` text.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Smin => "llvm.smin",
            Self::Smax => "llvm.smax",
            Self::Umin => "llvm.umin",
            Self::Umax => "llvm.umax",
        }
    }
}

/// A min/max intrinsic, integer or floating-point.
///
/// Upstream spells this as an `Intrinsic::ID` — one flat type naming every
/// intrinsic there is — and narrows it with a `switch` whose `default` is
/// `llvm_unreachable`. llvmkit has no public intrinsic-id type, and the two
/// halves of the min/max family are already closed enums that exist for their
/// own reasons: [`MinMaxIntrinsic`] is exactly the range of
/// `llvm::getMinMaxIntrinsic`, and [`MinMaxKind`] ports
/// `KnownFPClass::MinMaxKind`, which is deliberately independent of the IR.
///
/// This is their sum, for the two upstream functions whose domain or range
/// spans both: `getInverseMinMaxIntrinsic` and `canConvertToMinOrMaxIntrinsic`.
/// The arms are disjoint — the four integer intrinsics and the six
/// floating-point ones, ten in all, and no intrinsic is named by both. Because
/// the domain *is* those ten, every mapping over it is total and there is no
/// unreachable arm to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MinMaxOperation {
    /// `llvm.smin`, `llvm.smax`, `llvm.umin` or `llvm.umax`.
    Integer(MinMaxIntrinsic),
    /// One of the six floating-point forms — `llvm.minimum` and `llvm.maximum`,
    /// `llvm.minimumnum` and `llvm.maximumnum`, `llvm.minnum` and `llvm.maxnum`.
    Float(MinMaxKind),
}

impl MinMaxOperation {
    /// The min/max computing the opposite extremum.
    ///
    /// Ports `llvm::getInverseMinMaxIntrinsic` over its whole domain, by
    /// delegating to [`MinMaxIntrinsic::inverse`] and [`MinMaxKind::inverse`].
    /// Inverting never crosses the integer/floating-point boundary, which is
    /// why the sum can be taken apart and put back together unchanged.
    #[inline]
    pub const fn inverse(self) -> Self {
        match self {
            Self::Integer(intrinsic) => Self::Integer(intrinsic.inverse()),
            Self::Float(kind) => Self::Float(kind.inverse()),
        }
    }

    /// The intrinsic's base name, as it appears in `.ll` text.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Integer(intrinsic) => intrinsic.name(),
            Self::Float(kind) => kind.name(),
        }
    }
}

/// What a floating-point min/max does when given one NaN and one non-NaN.
///
/// Ports `llvm::SelectPatternNaNBehavior`. Only meaningful when the flavour is
/// [`SelectPatternFlavor::FminNum`] or [`SelectPatternFlavor::FmaxNum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectPatternNanBehavior {
    /// NaN behaviour does not apply — upstream's `SPNB_NA`.
    NotApplicable,
    /// Given one NaN input, returns the NaN.
    ReturnsNaN,
    /// Given one NaN input, returns the non-NaN.
    ReturnsOther,
    /// May return either, or no operand can be NaN.
    ReturnsAny,
}

/// The classification of a `select`.
///
/// Ports `llvm::SelectPatternResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectPatternResult {
    /// Which idiom was recognised.
    pub flavor: SelectPatternFlavor,
    /// NaN behaviour; only applicable to the two float flavours.
    pub nan_behavior: SelectPatternNanBehavior,
    /// Whether implementing this min/max as `fcmp; select` needs the `fcmp`
    /// to be ordered.
    pub ordered: bool,
}

impl SelectPatternResult {
    /// The "no pattern" answer, upstream's `{SPF_UNKNOWN, SPNB_NA, false}`.
    #[inline]
    pub const fn unknown() -> Self {
        Self {
            flavor: SelectPatternFlavor::Unknown,
            nan_behavior: SelectPatternNanBehavior::NotApplicable,
            ordered: false,
        }
    }

    /// Whether the recognised flavour is a minimum or a maximum.
    #[inline]
    pub const fn is_min_or_max(self) -> bool {
        self.flavor.is_min_or_max()
    }
}

/// The pattern `X <predicate> Y ? X : Y` implements.
///
/// Ports `llvm::getSelectPattern`. `nan_behavior` and `ordered` are carried
/// through to the result for the float predicates and ignored for the integer
/// ones, exactly as upstream does.
///
/// Equality predicates select one operand regardless of order, so they are not
/// a min/max and fall into [`SelectPatternFlavor::Unknown`] — upstream's
/// `default` arm, commented "Equality".
pub fn select_pattern(
    predicate: CmpPredicate,
    nan_behavior: SelectPatternNanBehavior,
    ordered: bool,
) -> SelectPatternResult {
    let integer = |flavor| SelectPatternResult {
        flavor,
        nan_behavior: SelectPatternNanBehavior::NotApplicable,
        ordered: false,
    };
    match predicate {
        CmpPredicate::Int(IntPredicate::Ugt | IntPredicate::Uge) => {
            integer(SelectPatternFlavor::Umax)
        }
        CmpPredicate::Int(IntPredicate::Sgt | IntPredicate::Sge) => {
            integer(SelectPatternFlavor::Smax)
        }
        CmpPredicate::Int(IntPredicate::Ult | IntPredicate::Ule) => {
            integer(SelectPatternFlavor::Umin)
        }
        CmpPredicate::Int(IntPredicate::Slt | IntPredicate::Sle) => {
            integer(SelectPatternFlavor::Smin)
        }
        CmpPredicate::Float(
            FloatPredicate::Ugt | FloatPredicate::Uge | FloatPredicate::Ogt | FloatPredicate::Oge,
        ) => SelectPatternResult {
            flavor: SelectPatternFlavor::FmaxNum,
            nan_behavior,
            ordered,
        },
        CmpPredicate::Float(
            FloatPredicate::Ult | FloatPredicate::Ule | FloatPredicate::Olt | FloatPredicate::Ole,
        ) => SelectPatternResult {
            flavor: SelectPatternFlavor::FminNum,
            nan_behavior,
            ordered,
        },
        _ => SelectPatternResult::unknown(),
    }
}

// --------------------------------------------------------------------------
// Matching a `select` against the flavours above
// --------------------------------------------------------------------------

use crate::ap_float::ApFloatCmpResult;
use crate::constant::{Constant, ConstantData};
use crate::constants::{ConstantFloatValue, ConstantIntValue};
use crate::float_kind::FloatDyn;
use crate::fmf::FastMathFlags;
use crate::instr_types::CastOpcode;
use crate::instruction::{InstructionKindData, InstructionView};
use crate::int_width::IntDyn;
use crate::module::{ModuleBrand, ModuleRef};
use crate::operator::is_supported_floating_point_type;
use crate::value::{Value, ValueKindData, ValueSlot};
use crate::value_tracking::{
    MAX_ANALYSIS_RECURSION_DEPTH, NswRequirement, PoisonPolicy, ValueTrackingQuery,
    is_known_negation,
};
use crate::{ApFloat, IrResult};

/// A matched `select`: which idiom, and the two values it chooses between.
///
/// Ports the shape `llvm::matchSelectPattern` returns through its `Value *&LHS`
/// / `Value *&RHS` out-parameters plus its `SelectPatternResult`. Upstream's own
/// comment on those out-parameters — "Assume success. If there's no match,
/// callers should not use these anyway" — is why the whole record sits behind an
/// `Option` here: a caller that did not match cannot read operands that were
/// never meaningfully set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectPatternMatch<'ctx, B: ModuleBrand> {
    /// The recognised idiom. Never [`SelectPatternFlavor::Unknown`] — that is
    /// the `None` of the enclosing `Option`.
    pub result: SelectPatternResult,
    /// Upstream's `LHS`.
    pub lhs: Value<'ctx, B>,
    /// Upstream's `RHS`.
    pub rhs: Value<'ctx, B>,
    /// The cast that was looked through, when the caller asked for one and one
    /// was found. Upstream's `Instruction::CastOps *CastOp` out-parameter.
    pub cast: Option<CastOpcode>,
}

/// Recognise the min/max/abs idiom a `select` implements.
///
/// Ports `llvm::matchSelectPattern`. `look_through_cast` is upstream's `CastOp`
/// pointer as a `bool`: passing `true` lets the match see through a cast on the
/// select arms and reports which cast in [`SelectPatternMatch::cast`], where
/// passing a null `CastOp` upstream disables that path.
///
/// Fast-math flags written on the `select` itself are read, as upstream reads
/// them: `isa<FPMathOperator>(SI) ? SI->getFastMathFlags() : FastMathFlags()`.
/// `nsz` is the flag that only ever reaches the matcher this way or through the
/// `fptosi`/`fptoui` cast path — `matchDecomposedSelectPattern` takes `nnan`
/// from the `fcmp` but never `nsz`.
pub fn match_select_pattern<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    look_through_cast: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<Option<SelectPatternMatch<'ctx, B>>> {
    if depth >= MAX_ANALYSIS_RECURSION_DEPTH {
        return Ok(None);
    }
    let Some(InstructionKindData::Select(select)) = instruction_kind(value) else {
        return Ok(None);
    };
    let condition = value_from_slot(value, select.cond.get());
    let Ok(condition) = InstructionView::try_from(condition) else {
        return Ok(None);
    };
    if !matches!(
        instruction_kind(condition.to_erased()),
        Some(InstructionKindData::Icmp(_) | InstructionKindData::Fcmp(_))
    ) {
        return Ok(None);
    }
    let true_value = value_from_slot(value, select.true_val.get());
    let false_value = value_from_slot(value, select.false_val.get());
    // `isa<FPMathOperator>(SI) ? SI->getFastMathFlags() : FastMathFlags()`.
    // `FPMathOperator::classof`'s `Select` arm is
    // `isSupportedFloatingPointType(V->getType())`, which is wider than
    // `isFPOrFPVectorTy` — a homogeneous FP struct qualifies too.
    let fast_math_flags = if is_supported_floating_point_type(value.ty()) {
        select.fmf.get()
    } else {
        FastMathFlags::empty()
    };
    match_decomposed_select_pattern(
        &condition,
        true_value,
        false_value,
        fast_math_flags,
        look_through_cast,
        query,
        depth,
    )
}

/// [`match_select_pattern`] with the `select` already taken apart.
///
/// Ports `llvm::matchDecomposedSelectPattern`, which exists so a caller holding
/// the compare and the two arms separately — InstCombine mid-rewrite — need not
/// build a `select` to ask.
pub fn match_decomposed_select_pattern<'a, 'ctx, B: ModuleBrand + 'ctx>(
    compare: &InstructionView<'ctx, B>,
    true_value: Value<'ctx, B>,
    false_value: Value<'ctx, B>,
    fast_math_flags: FastMathFlags,
    look_through_cast: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<Option<SelectPatternMatch<'ctx, B>>> {
    let anchor = compare.to_erased();
    let Some((predicate, compare_lhs, compare_rhs)) = compare_parts(anchor) else {
        return Ok(None);
    };
    let mut fast_math_flags = fast_math_flags;
    if let Some(InstructionKindData::Fcmp(data)) = instruction_kind(anchor)
        && data.fmf.contains(FastMathFlags::NO_NANS)
    {
        fast_math_flags |= FastMathFlags::NO_NANS;
    }

    // Bail out early: an equality compare picks one operand regardless of
    // order, so it is never a min/max.
    let is_equality = match predicate {
        CmpPredicate::Int(predicate) => predicate.is_equality(),
        CmpPredicate::Float(predicate) => predicate.is_equality(),
    };
    if is_equality {
        return Ok(None);
    }

    // Deal with type mismatches.
    if look_through_cast && compare_lhs.ty().id() != true_value.ty().id() {
        for (cast_side, other_side, cast_is_true_arm) in [
            (true_value, false_value, true),
            (false_value, true_value, false),
        ] {
            let Some((cast_opcode, other)) = look_through_cast_arm(anchor, cast_side, other_side)
            else {
                continue;
            };
            let Some(source) = cast_source(cast_side) else {
                continue;
            };
            // A potential fmin/fmax with a cast to integer has no -0.0 to
            // preserve, so signed zeros stop mattering.
            let mut fast_math_flags = fast_math_flags;
            if matches!(cast_opcode, CastOpcode::FpToSi | CastOpcode::FpToUi) {
                fast_math_flags |= FastMathFlags::NO_SIGNED_ZEROS;
            }
            let (true_value, false_value) = if cast_is_true_arm {
                (source, other)
            } else {
                (other, source)
            };
            let matched = match_select_pattern_core(
                CompareParts {
                    predicate,
                    lhs: compare_lhs,
                    rhs: compare_rhs,
                },
                fast_math_flags,
                true_value,
                false_value,
                query,
                depth,
            )?;
            return Ok(matched.map(|mut matched| {
                matched.cast = Some(cast_opcode);
                matched
            }));
        }
    }

    match_select_pattern_core(
        CompareParts {
            predicate,
            lhs: compare_lhs,
            rhs: compare_rhs,
        },
        fast_math_flags,
        true_value,
        false_value,
        query,
        depth,
    )
}

/// A decomposed comparison: upstream's `Pred` / `CmpLHS` / `CmpRHS` triple,
/// which travels together through every function in this family.
#[derive(Clone, Copy)]
struct CompareParts<'ctx, B: ModuleBrand> {
    predicate: CmpPredicate,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
}

/// Ports the static `matchSelectPattern(Pred, FMF, CmpLHS, CmpRHS, TrueVal,
/// FalseVal, LHS, RHS, Depth)`.
fn match_select_pattern_core<'a, 'ctx, B: ModuleBrand + 'ctx>(
    compare: CompareParts<'ctx, B>,
    fast_math_flags: FastMathFlags,
    true_value: Value<'ctx, B>,
    false_value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<Option<SelectPatternMatch<'ctx, B>>> {
    let mut predicate = compare.predicate;
    let mut compare_lhs = compare.lhs;
    let mut compare_rhs = compare.rhs;

    // IEEE-754 ignores the sign of 0.0 in comparisons, so when the select has
    // exactly one 0.0 operand, the compare's 0.0 operands are set to that same
    // value for the purpose of identifying min/max. Vector constants with
    // undefined elements are disregarded because they cannot be
    // back-propagated for analysis.
    let mut has_mismatched_zeros = false;
    if matches!(predicate, CmpPredicate::Float(_)) {
        let output_zero = if is_any_zero_fp(true_value) && !is_any_zero_fp(false_value) {
            Some(true_value)
        } else if is_any_zero_fp(false_value) && !is_any_zero_fp(true_value) {
            Some(false_value)
        } else {
            None
        };
        if let Some(output_zero) = output_zero {
            if is_any_zero_fp(compare_lhs) && compare_lhs != output_zero {
                has_mismatched_zeros = true;
                compare_lhs = output_zero;
            }
            if is_any_zero_fp(compare_rhs) && compare_rhs != output_zero {
                has_mismatched_zeros = true;
                compare_rhs = output_zero;
            }
        }
    }

    // Signed zero may give inconsistent results between implementations:
    //   (0.0 <= -0.0) ? 0.0 : -0.0   returns 0.0
    //   minNum(0.0, -0.0)            may return either (IEEE 754-2008 5.3.1)
    // so proceed only when one operand is known non-zero, or signed zeros do
    // not matter.
    if let CmpPredicate::Float(float_predicate) = predicate {
        let strict = matches!(
            float_predicate,
            FloatPredicate::Ogt | FloatPredicate::Olt | FloatPredicate::Ugt | FloatPredicate::Ult
        );
        let non_strict = matches!(
            float_predicate,
            FloatPredicate::Oge | FloatPredicate::Ole | FloatPredicate::Uge | FloatPredicate::Ule
        );
        if ((strict && has_mismatched_zeros) || non_strict)
            && !fast_math_flags.contains(FastMathFlags::NO_SIGNED_ZEROS)
            && !is_known_non_zero_float(compare_lhs)
            && !is_known_non_zero_float(compare_rhs)
        {
            return Ok(None);
        }
    }

    let mut nan_behavior = SelectPatternNanBehavior::NotApplicable;
    let mut ordered = false;

    // Given one NaN and one non-NaN input:
    //   - maxnum/minnum (C99 fmaxf/fminf) return the non-NaN input.
    //   - A simple C99 `a < b ? a : b` returns `b`, which could be either,
    //     because the ordered comparison fails.
    // so discover exactly what NaN behaviour is required or accepted.
    if let CmpPredicate::Float(float_predicate) = predicate {
        let lhs_safe = is_known_non_nan(compare_lhs, fast_math_flags);
        let rhs_safe = is_known_non_nan(compare_rhs, fast_math_flags);
        if lhs_safe && rhs_safe {
            nan_behavior = SelectPatternNanBehavior::ReturnsAny;
            ordered = float_predicate.is_ordered();
        } else if float_predicate.is_ordered() {
            // An ordered comparison is false given a NaN, so it returns the RHS.
            ordered = true;
            nan_behavior = if lhs_safe {
                SelectPatternNanBehavior::ReturnsNaN
            } else if rhs_safe {
                SelectPatternNanBehavior::ReturnsOther
            } else {
                return Ok(None);
            };
        } else {
            // An unordered comparison is true given a NaN, so it returns the LHS.
            ordered = false;
            nan_behavior = if lhs_safe {
                SelectPatternNanBehavior::ReturnsOther
            } else if rhs_safe {
                SelectPatternNanBehavior::ReturnsNaN
            } else {
                return Ok(None);
            };
        }
    }

    if true_value == compare_rhs && false_value == compare_lhs {
        core::mem::swap(&mut compare_lhs, &mut compare_rhs);
        predicate = swapped_predicate(predicate);
        nan_behavior = match nan_behavior {
            SelectPatternNanBehavior::ReturnsNaN => SelectPatternNanBehavior::ReturnsOther,
            SelectPatternNanBehavior::ReturnsOther => SelectPatternNanBehavior::ReturnsNaN,
            other => other,
        };
        ordered = !ordered;
    }

    // `([if]cmp X, Y) ? X : Y`.
    if true_value == compare_lhs && false_value == compare_rhs {
        let result = select_pattern(predicate, nan_behavior, ordered);
        return Ok(matched(result, compare_lhs, compare_rhs));
    }

    // Upstream's call is `isKnownNegation(TrueVal, FalseVal)`, both defaults:
    // no `nsw` required, poison lanes allowed.
    if is_known_negation(
        true_value,
        false_value,
        NswRequirement::NotRequired,
        PoisonPolicy::Allow,
    ) && let Some(found) =
        match_abs(predicate, compare_lhs, compare_rhs, true_value, false_value)
    {
        return Ok(Some(found));
    }

    if let CmpPredicate::Int(int_predicate) = predicate {
        return match_min_max(
            int_predicate,
            compare_lhs,
            compare_rhs,
            true_value,
            false_value,
            query,
            depth,
        );
    }

    // Per IEEE 754-2008 5.3.1, `minNum(0.0, -0.0)` and friends may return
    // either sign of zero, so an `fcmp`/`select` pair has stricter semantics
    // than `minnum`. Be conservative.
    if nan_behavior != SelectPatternNanBehavior::ReturnsAny
        || (!fast_math_flags.contains(FastMathFlags::NO_SIGNED_ZEROS)
            && !is_known_non_zero_float(compare_lhs)
            && !is_known_non_zero_float(compare_rhs))
    {
        return Ok(None);
    }
    let CmpPredicate::Float(float_predicate) = predicate else {
        return Ok(None);
    };
    Ok(match_fast_float_clamp(
        float_predicate,
        compare_lhs,
        compare_rhs,
        true_value,
        false_value,
    ))
}

/// Ports the `isKnownNegation(TrueVal, FalseVal)` arm: `abs` and `nabs`.
///
/// Upstream matches the arm against `CmpLHS` *or* `sext(CmpLHS)`, because
/// sign-extending a value does not change its sign.
fn match_abs<'ctx, B: ModuleBrand + 'ctx>(
    predicate: CmpPredicate,
    compare_lhs: Value<'ctx, B>,
    compare_rhs: Value<'ctx, B>,
    true_value: Value<'ctx, B>,
    false_value: Value<'ctx, B>,
) -> Option<SelectPatternMatch<'ctx, B>> {
    let CmpPredicate::Int(predicate) = predicate else {
        return None;
    };
    let zero_or_all_ones = int_constant(compare_rhs)
        .is_some_and(|constant| constant.is_zero() || constant.is_all_ones());
    let zero_or_one =
        int_constant(compare_rhs).is_some_and(|constant| constant.is_zero() || constant.is_one());

    // `TrueVal` matches `CmpLHS` (possibly sign-extended).
    if is_compare_lhs_or_its_sext(true_value, compare_lhs) {
        // If the compare uses the negated value (`-X >s 0`), swap the reported
        // operands, because the negated value is always `RHS`.
        let (lhs, rhs) = if is_negation_of(compare_lhs, false_value) {
            (false_value, true_value)
        } else {
            (true_value, false_value)
        };
        let flavor = match predicate {
            // (X >s 0) ? X : -X   /  (X >s -1) ? X : -X   --> ABS(X)
            IntPredicate::Sgt if zero_or_all_ones => SelectPatternFlavor::Abs,
            // (X >=s 0) ? X : -X  /  (X >=s 1) ? X : -X   --> ABS(X)
            IntPredicate::Sge if zero_or_one => SelectPatternFlavor::Abs,
            // (X <s 0) ? X : -X   /  (X <s 1) ? X : -X    --> NABS(X)
            IntPredicate::Slt if zero_or_one => SelectPatternFlavor::Nabs,
            _ => return None,
        };
        return matched(
            SelectPatternResult {
                flavor,
                nan_behavior: SelectPatternNanBehavior::NotApplicable,
                ordered: false,
            },
            lhs,
            rhs,
        );
    }

    // `FalseVal` matches `CmpLHS` instead.
    if is_compare_lhs_or_its_sext(false_value, compare_lhs) {
        let (lhs, rhs) = if is_negation_of(compare_lhs, true_value) {
            (true_value, false_value)
        } else {
            (false_value, true_value)
        };
        let flavor = match predicate {
            // (X >s 0) ? -X : X   /  (X >s -1) ? -X : X   --> NABS(X)
            IntPredicate::Sgt if zero_or_all_ones => SelectPatternFlavor::Nabs,
            // (X <s 0) ? -X : X   /  (X <s 1) ? -X : X    --> ABS(X)
            IntPredicate::Slt if zero_or_one => SelectPatternFlavor::Abs,
            _ => return None,
        };
        return matched(
            SelectPatternResult {
                flavor,
                nan_behavior: SelectPatternNanBehavior::NotApplicable,
                ordered: false,
            },
            lhs,
            rhs,
        );
    }

    None
}

/// Ports the static `matchMinMax(Pred, CmpLHS, CmpRHS, TrueVal, FalseVal, LHS,
/// RHS, Depth)`.
fn match_min_max<'a, 'ctx, B: ModuleBrand + 'ctx>(
    predicate: IntPredicate,
    compare_lhs: Value<'ctx, B>,
    compare_rhs: Value<'ctx, B>,
    true_value: Value<'ctx, B>,
    false_value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<Option<SelectPatternMatch<'ctx, B>>> {
    // Upstream's "assume success" sets `LHS`/`RHS` to the select arms up front;
    // every arm below that matches reports the same pair.
    let report = |flavor: SelectPatternFlavor| {
        matched(
            SelectPatternResult {
                flavor,
                nan_behavior: SelectPatternNanBehavior::NotApplicable,
                ordered: false,
            },
            true_value,
            false_value,
        )
    };

    if let Some(flavor) = match_clamp(predicate, compare_lhs, compare_rhs, true_value, false_value)
    {
        return Ok(report(flavor));
    }

    if let Some(flavor) = match_min_max_of_min_max(
        predicate,
        compare_lhs,
        compare_rhs,
        true_value,
        false_value,
        query,
        depth,
    )? {
        return Ok(report(flavor));
    }

    // Look through `not` operations to find a disguised min/max.
    // (X > Y) ? ~X : ~Y ==> (~X < ~Y) ? ~X : ~Y ==> MIN(~X, ~Y)
    // (X < Y) ? ~X : ~Y ==> (~X > ~Y) ? ~X : ~Y ==> MAX(~X, ~Y)
    if not_value(true_value).is_some_and(|not| not == compare_lhs)
        && not_value(false_value).is_some_and(|not| not == compare_rhs)
    {
        let flavor = match predicate {
            IntPredicate::Sgt => Some(SelectPatternFlavor::Smin),
            IntPredicate::Slt => Some(SelectPatternFlavor::Smax),
            IntPredicate::Ugt => Some(SelectPatternFlavor::Umin),
            IntPredicate::Ult => Some(SelectPatternFlavor::Umax),
            _ => None,
        };
        if let Some(flavor) = flavor {
            return Ok(report(flavor));
        }
    }

    // (X > Y) ? ~Y : ~X ==> (~X < ~Y) ? ~Y : ~X ==> MAX(~Y, ~X)
    // (X < Y) ? ~Y : ~X ==> (~X > ~Y) ? ~Y : ~X ==> MIN(~Y, ~X)
    if not_value(false_value).is_some_and(|not| not == compare_lhs)
        && not_value(true_value).is_some_and(|not| not == compare_rhs)
    {
        let flavor = match predicate {
            IntPredicate::Sgt => Some(SelectPatternFlavor::Smax),
            IntPredicate::Slt => Some(SelectPatternFlavor::Smin),
            IntPredicate::Ugt => Some(SelectPatternFlavor::Umax),
            IntPredicate::Ult => Some(SelectPatternFlavor::Umin),
            _ => None,
        };
        if let Some(flavor) = flavor {
            return Ok(report(flavor));
        }
    }

    if !matches!(predicate, IntPredicate::Sgt | IntPredicate::Slt) {
        return Ok(None);
    }
    let Some(c1) = int_constant(compare_rhs) else {
        return Ok(None);
    };

    // An unsigned min/max can be written with a signed compare.
    let (arm_is_true_value, c2) = if compare_lhs == true_value {
        (true, int_constant(false_value))
    } else if compare_lhs == false_value {
        (false, int_constant(true_value))
    } else {
        return Ok(None);
    };
    let Some(c2) = c2 else {
        return Ok(None);
    };

    // Is the sign bit set?
    // (X <s 0) ? X : MAXVAL ==> (X >u MAXVAL) ? X : MAXVAL ==> UMAX
    // (X <s 0) ? MAXVAL : X ==> (X >u MAXVAL) ? MAXVAL : X ==> UMIN
    if predicate == IntPredicate::Slt && c1.is_zero() && c2.is_max_signed_value() {
        return Ok(report(if arm_is_true_value {
            SelectPatternFlavor::Umax
        } else {
            SelectPatternFlavor::Umin
        }));
    }

    // Is the sign bit clear?
    // (X >s -1) ? MINVAL : X ==> (X <u MINVAL) ? MINVAL : X ==> UMAX
    // (X >s -1) ? X : MINVAL ==> (X <u MINVAL) ? X : MINVAL ==> UMIN
    if predicate == IntPredicate::Sgt && c1.is_all_ones() && c2.is_min_signed_value() {
        return Ok(report(if arm_is_true_value {
            SelectPatternFlavor::Umin
        } else {
            SelectPatternFlavor::Umax
        }));
    }

    Ok(None)
}

/// Ports the static `matchClamp`: a min/max whose other arm is itself a
/// saturating min/max against a constant.
fn match_clamp<'ctx, B: ModuleBrand + 'ctx>(
    predicate: IntPredicate,
    compare_lhs: Value<'ctx, B>,
    compare_rhs: Value<'ctx, B>,
    true_value: Value<'ctx, B>,
    false_value: Value<'ctx, B>,
) -> Option<SelectPatternFlavor> {
    // Swap the select operands and predicate to match the patterns below.
    let (predicate, true_value, false_value) = if compare_rhs == true_value {
        (predicate, true_value, false_value)
    } else {
        (predicate.swapped(), false_value, true_value)
    };
    if compare_rhs != true_value {
        return None;
    }
    let c1 = int_constant(compare_rhs)?;

    for (wanted_predicate, inner, flavor, ordered) in [
        // (X <s C1) ? C1 : SMIN(X, C2) ==> SMAX(SMIN(X, C2), C1)
        (
            IntPredicate::Slt,
            SelectPatternFlavor::Smin,
            SelectPatternFlavor::Smax,
            Signedness::Signed,
        ),
        // (X >s C1) ? C1 : SMAX(X, C2) ==> SMIN(SMAX(X, C2), C1)
        (
            IntPredicate::Sgt,
            SelectPatternFlavor::Smax,
            SelectPatternFlavor::Smin,
            Signedness::Signed,
        ),
        // (X <u C1) ? C1 : UMIN(X, C2) ==> UMAX(UMIN(X, C2), C1)
        (
            IntPredicate::Ult,
            SelectPatternFlavor::Umin,
            SelectPatternFlavor::Umax,
            Signedness::Unsigned,
        ),
        // (X >u C1) ? C1 : UMAX(X, C2) ==> UMIN(UMAX(X, C2), C1)
        (
            IntPredicate::Ugt,
            SelectPatternFlavor::Umax,
            SelectPatternFlavor::Umin,
            Signedness::Unsigned,
        ),
    ] {
        if predicate != wanted_predicate {
            continue;
        }
        let Some(c2) = int_min_max_against_constant(false_value, compare_lhs, inner) else {
            continue;
        };
        let inside = match (ordered, wanted_predicate) {
            (Signedness::Signed, IntPredicate::Slt) => c1.slt(&c2),
            (Signedness::Signed, _) => c1.sgt(&c2),
            (Signedness::Unsigned, IntPredicate::Ult) => c1.ult(&c2),
            (Signedness::Unsigned, _) => c1.ugt(&c2),
        };
        if inside {
            return Some(flavor);
        }
    }
    None
}

/// Which comparison family [`match_clamp`] should use for its constant test.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Signedness {
    Signed,
    Unsigned,
}

/// Ports the static `matchMinMaxOfMinMax`: `x pred y ? min(a, b) : min(c, d)`,
/// where the compare lines up with the inner min/max operands.
fn match_min_max_of_min_max<'a, 'ctx, B: ModuleBrand + 'ctx>(
    predicate: IntPredicate,
    compare_lhs: Value<'ctx, B>,
    compare_rhs: Value<'ctx, B>,
    true_value: Value<'ctx, B>,
    false_value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> IrResult<Option<SelectPatternFlavor>> {
    let Some(left) = match_select_pattern(true_value, false, query, depth + 1)? else {
        return Ok(None);
    };
    if !left.result.is_min_or_max() {
        return Ok(None);
    }
    let Some(right) = match_select_pattern(false_value, false, query, depth + 1)? else {
        return Ok(None);
    };
    if left.result.flavor != right.result.flavor {
        return Ok(None);
    }

    // Make sure the compare predicate lines up with the inner flavour, swapping
    // the compare operands if it is the mirror image.
    let (wanted_swapped, wanted_direct): ([IntPredicate; 2], [IntPredicate; 2]) =
        match left.result.flavor {
            SelectPatternFlavor::Smin => (
                [IntPredicate::Sgt, IntPredicate::Sge],
                [IntPredicate::Slt, IntPredicate::Sle],
            ),
            SelectPatternFlavor::Smax => (
                [IntPredicate::Slt, IntPredicate::Sle],
                [IntPredicate::Sgt, IntPredicate::Sge],
            ),
            SelectPatternFlavor::Umin => (
                [IntPredicate::Ugt, IntPredicate::Uge],
                [IntPredicate::Ult, IntPredicate::Ule],
            ),
            SelectPatternFlavor::Umax => (
                [IntPredicate::Ult, IntPredicate::Ule],
                [IntPredicate::Ugt, IntPredicate::Uge],
            ),
            _ => return Ok(None),
        };
    let (compare_lhs, compare_rhs) = if wanted_swapped.contains(&predicate) {
        (compare_rhs, compare_lhs)
    } else if wanted_direct.contains(&predicate) {
        (compare_lhs, compare_rhs)
    } else {
        return Ok(None);
    };

    let (a, b) = (left.lhs, left.rhs);
    let (c, d) = (right.lhs, right.rhs);

    // If there is a common operand in the already matched min/max and the other
    // min/max operands match the compare operands — directly or inverted — then
    // this is a min/max of the same flavour.
    // Upstream's four `if` blocks, each accepting the compare either directly
    // or with both sides inverted:
    //     (CmpLHS == first && CmpRHS == other)
    //  || (other == ~CmpLHS && first == ~CmpRHS)
    let lines_up = |first: Value<'ctx, B>, other: Value<'ctx, B>| {
        (compare_lhs == first && compare_rhs == other)
            || (not_value(other).is_some_and(|not| not == compare_lhs)
                && not_value(first).is_some_and(|not| not == compare_rhs))
    };

    // a pred c ? m(a, b) : m(c, b)   b pred d ? m(a, b) : m(a, d)   and the two
    // mirror arrangements.
    let matches_any = (d == b && lines_up(a, c))
        || (c == b && lines_up(a, d))
        || (d == a && lines_up(b, c))
        || (c == a && lines_up(b, d));

    Ok(matches_any.then_some(left.result.flavor))
}

/// Ports the static `matchFastFloatClamp`:
///   X < C1 ? C1 : Min(X, C2) --> Max(C1, Min(X, C2))
///   X > C1 ? C1 : Max(X, C2) --> Min(C1, Max(X, C2))
fn match_fast_float_clamp<'ctx, B: ModuleBrand + 'ctx>(
    predicate: FloatPredicate,
    compare_lhs: Value<'ctx, B>,
    compare_rhs: Value<'ctx, B>,
    true_value: Value<'ctx, B>,
    false_value: Value<'ctx, B>,
) -> Option<SelectPatternMatch<'ctx, B>> {
    // First, check whether the select has inverse order.
    let (predicate, true_value, false_value) = if compare_rhs == false_value {
        (predicate.inverse(), false_value, true_value)
    } else {
        (predicate, true_value, false_value)
    };

    if compare_rhs != true_value {
        return None;
    }
    let c1 = float_constant(compare_rhs)?;
    if !c1.is_finite() {
        return None;
    }

    let (wanted_flavor, result_flavor) = match predicate {
        FloatPredicate::Olt | FloatPredicate::Ole | FloatPredicate::Ult | FloatPredicate::Ule => {
            (SelectPatternFlavor::FminNum, SelectPatternFlavor::FmaxNum)
        }
        FloatPredicate::Ogt | FloatPredicate::Oge | FloatPredicate::Ugt | FloatPredicate::Uge => {
            (SelectPatternFlavor::FmaxNum, SelectPatternFlavor::FminNum)
        }
        _ => return None,
    };

    let c2 = float_min_max_against_constant(false_value, compare_lhs, wanted_flavor)?;
    let ordering = c1.compare(&c2);
    let inside = match wanted_flavor {
        SelectPatternFlavor::FminNum => ordering == ApFloatCmpResult::LessThan,
        _ => ordering == ApFloatCmpResult::GreaterThan,
    };
    if !inside {
        return None;
    }
    matched(
        SelectPatternResult {
            flavor: result_flavor,
            nan_behavior: SelectPatternNanBehavior::ReturnsAny,
            ordered: false,
        },
        true_value,
        false_value,
    )
}

/// Which min/max intrinsic (or `fcmp`/`select` idiom) the values in `values`
/// could all be rewritten as, and whether every select condition is used only
/// by its select.
///
/// Ports `llvm::canConvertToMinOrMaxIntrinsic`. Upstream returns
/// `{Intrinsic::not_intrinsic, false}` for "no", which is the `None` here.
///
/// The answer spans both halves of the min/max family, so it is a
/// [`MinMaxOperation`]: upstream's switch maps the four integer flavours to
/// `smin`/`smax`/`umin`/`umax` and `SPF_FMAXNUM` / `SPF_FMINNUM` to
/// `maxnum` / `minnum`.
pub fn can_convert_to_min_or_max_intrinsic<'a, 'ctx, B, Values>(
    values: Values,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> IrResult<Option<(MinMaxOperation, bool)>>
where
    B: ModuleBrand + 'ctx,
    Values: IntoIterator<Item = Value<'ctx, B>>,
{
    let mut flavor: Option<SelectPatternFlavor> = None;
    let mut all_compares_single_use = true;

    for value in values {
        let Some(current) = match_select_pattern(value, false, query, 0)? else {
            return Ok(None);
        };
        if !current.result.is_min_or_max() {
            return Ok(None);
        }
        match flavor {
            Some(known) if known != current.result.flavor => return Ok(None),
            _ => flavor = Some(current.result.flavor),
        }
        all_compares_single_use &= select_condition_has_one_use(value);
    }

    Ok(flavor
        .and_then(min_max_operation)
        .map(|operation| (operation, all_compares_single_use)))
}

/// The min/max intrinsic `flavor` converts to, integer or floating-point.
///
/// Ports the six-arm `switch` inside `llvm::canConvertToMinOrMaxIntrinsic`,
/// which reaches wider than `getMinMaxIntrinsic`: it also maps `SPF_FMAXNUM`
/// and `SPF_FMINNUM`. Upstream's `default` is `llvm_unreachable`, guarded by
/// the `isMinOrMax` check its caller has already made; the three flavours that
/// check rejects are the `None` here, so the guard is carried by the return
/// type rather than by the caller remembering to look.
fn min_max_operation(flavor: SelectPatternFlavor) -> Option<MinMaxOperation> {
    Some(match flavor {
        SelectPatternFlavor::Smin => MinMaxOperation::Integer(MinMaxIntrinsic::Smin),
        SelectPatternFlavor::Smax => MinMaxOperation::Integer(MinMaxIntrinsic::Smax),
        SelectPatternFlavor::Umin => MinMaxOperation::Integer(MinMaxIntrinsic::Umin),
        SelectPatternFlavor::Umax => MinMaxOperation::Integer(MinMaxIntrinsic::Umax),
        SelectPatternFlavor::FminNum => MinMaxOperation::Float(MinMaxKind::MinNum),
        SelectPatternFlavor::FmaxNum => MinMaxOperation::Float(MinMaxKind::MaxNum),
        SelectPatternFlavor::Unknown | SelectPatternFlavor::Abs | SelectPatternFlavor::Nabs => {
            return None;
        }
    })
}

// --------------------------------------------------------------------------
// Matching helpers
// --------------------------------------------------------------------------

/// Wrap a successful classification. `Unknown` is the `None`, so a caller
/// cannot read operands the match never set.
fn matched<'ctx, B: ModuleBrand + 'ctx>(
    result: SelectPatternResult,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
) -> Option<SelectPatternMatch<'ctx, B>> {
    (result.flavor != SelectPatternFlavor::Unknown).then_some(SelectPatternMatch {
        result,
        lhs,
        rhs,
        cast: None,
    })
}

/// Ports the static `isKnownNonNaN(V, FMF)`.
fn is_known_non_nan<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    fast_math_flags: FastMathFlags,
) -> bool {
    if fast_math_flags.contains(FastMathFlags::NO_NANS) {
        return true;
    }
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Float(_)) => {
            float_constant(value).is_some_and(|constant| !constant.is_nan())
        }
        // Upstream's `ConstantDataVector` arm, plus its `ConstantAggregateZero`
        // arm: llvmkit stores both as an aggregate of element constants.
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            if !value.ty().is_vector() {
                return false;
            }
            elements.iter().all(|element| {
                let element = value_from_slot(value, *element);
                matches!(
                    &element.data().kind,
                    ValueKindData::Constant(ConstantData::Float(_))
                ) && float_constant(element).is_some_and(|constant| !constant.is_nan())
            })
        }
        _ => false,
    }
}

/// Ports the file-local `static bool isKnownNonZero(const Value *V)`
/// (`ValueTracking.cpp`) — the **one-argument** overload that sits beside
/// `matchSelectPattern`, *not* `llvm::isKnownNonZero`, which is the known-bits
/// walk.
///
/// Upstream carries both names in one file and tells them apart by arity: the
/// float min/max arms write `isKnownNonZero(CmpLHS)` and reach this one, which
/// reads float constants only and answers `false` for everything else — it
/// never consults known bits. llvmkit called the known-bits routine here, which
/// answered `false` for a non-zero float constant like `1.0` where upstream
/// answers `true`, so a signed-zero guard declined matches upstream accepts.
fn is_known_non_zero_float<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Float(_)) => {
            float_constant(value).is_some_and(|constant| !constant.is_zero())
        }
        // Upstream's `ConstantDataVector` arm. llvmkit stores one as an
        // aggregate of element constants, so upstream's
        // `getElementType()->isFloatingPointTy()` guard becomes the per-element
        // `Float` check, and its early `return false` on a zero element is the
        // `all` below.
        ValueKindData::Constant(ConstantData::Aggregate(elements)) => {
            if !value.ty().is_vector() {
                return false;
            }
            elements.iter().all(|element| {
                let element = value_from_slot(value, *element);
                matches!(
                    &element.data().kind,
                    ValueKindData::Constant(ConstantData::Float(_))
                ) && float_constant(element).is_some_and(|constant| !constant.is_zero())
            })
        }
        _ => false,
    }
}

/// Ports the static `getNotValue(V)`.
///
/// Upstream's second arm mints `ConstantInt::get(V->getType(), ~*C)` for a
/// constant operand; minting a constant is a module mutation, so this reports
/// only the `xor X, -1` form. The effect is that a `not` written as a folded
/// constant is not recognised, which forgoes a match rather than inventing one.
fn not_value<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    let Some(InstructionKindData::Xor(data)) = instruction_kind(value) else {
        return None;
    };
    let lhs = value_from_slot(value, data.lhs.get());
    let rhs = value_from_slot(value, data.rhs.get());
    if int_constant(rhs).is_some_and(|constant| constant.is_all_ones()) {
        return Some(lhs);
    }
    int_constant(lhs)
        .is_some_and(|constant| constant.is_all_ones())
        .then_some(rhs)
}

/// Whether `value` is `sub 0, negated` — upstream's `m_Neg(m_Specific(..))`.
fn is_negation_of<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    negated: Value<'ctx, B>,
) -> bool {
    let Some(InstructionKindData::Sub(data)) = instruction_kind(value) else {
        return false;
    };
    int_constant(value_from_slot(value, data.lhs.get())).is_some_and(|constant| constant.is_zero())
        && data.rhs.get() == negated.slot()
}

/// Upstream's `m_CombineOr(m_Specific(CmpLHS), m_SExt(m_Specific(CmpLHS)))`:
/// sign-extending a value does not change its sign, so an arm may match either.
fn is_compare_lhs_or_its_sext<'ctx, B: ModuleBrand + 'ctx>(
    arm: Value<'ctx, B>,
    compare_lhs: Value<'ctx, B>,
) -> bool {
    if arm == compare_lhs {
        return true;
    }
    matches!(
        instruction_kind(arm),
        Some(InstructionKindData::Cast(data))
            if data.kind == CastOpcode::Sext && data.src.get() == compare_lhs.slot()
    )
}

/// The constant an integer min/max compares `x` against, when `value` is
/// `flavor(x, C)`.
///
/// Ports the `m_SMin(m_Specific(CmpLHS), m_APInt(C2))` family
/// [`match_clamp`] uses — the **non**-commutative spelling, so `C` must be the
/// second operand.
fn int_min_max_against_constant<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    expected_operand: Value<'ctx, B>,
    flavor: SelectPatternFlavor,
) -> Option<ApInt> {
    let (left, right) = int_min_max_operands(value, flavor)?;
    (left == expected_operand).then(|| int_constant(right))?
}

/// Whether `value` is `flavor(expected, other)` in *either* operand order, and
/// `other` if so.
///
/// Ports the `m_c_SMax(m_Specific(LHS), m_Value())` family — the commutative
/// spelling of [`int_min_max_operands`]'s matcher, which is how every use site
/// in `isTruePredicate` reads it.
pub(crate) fn int_min_max_over<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    expected: Value<'ctx, B>,
    flavor: SelectPatternFlavor,
) -> Option<Value<'ctx, B>> {
    let (left, right) = int_min_max_operands(value, flavor)?;
    if left == expected {
        return Some(right);
    }
    (right == expected).then_some(left)
}

/// The two operands of an integer min/max of the given `flavor`, in upstream's
/// binding order.
///
/// Ports `MaxMin_match<IcmpInst, LHS, RHS, Pred_t>` (`PatternMatch.h`), what
/// `m_SMin` / `m_SMax` / `m_UMin` / `m_UMax` expand to. It matches **two**
/// shapes: a call to the matching `llvm.{s,u}{min,max}` intrinsic, and the
/// structural `select(icmp PRED L, R, L, R)` — a select whose condition
/// compares the very values the select returns.
///
/// For the select shape the operands come back in the *compare's* order, not
/// the select's arms: upstream binds `L` to `Cmp->getOperand(0)` and inverts
/// the predicate when the true arm is the compare's right-hand side.
fn int_min_max_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    flavor: SelectPatternFlavor,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    if let Some(operands) = min_max_intrinsic_operands(value, flavor) {
        return Some(operands);
    }
    let (left, right, predicate) = select_over_icmp_of_its_own_arms(value)?;
    min_max_predicate_matches(flavor, predicate).then_some((left, right))
}

/// The `dyn_cast<IntrinsicInst>` arm of `MaxMin_match`: a direct call to
/// `llvm.smin` / `llvm.smax` / `llvm.umin` / `llvm.umax`.
fn min_max_intrinsic_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    flavor: SelectPatternFlavor,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    let Some(InstructionKindData::Call(call)) = instruction_kind(value) else {
        return None;
    };
    let callee = value_from_slot(value, call.callee.get());
    let descriptor = descriptor_for_callee(callee)?;
    let wanted = match flavor {
        SelectPatternFlavor::Smin => MinMaxIntrinsic::Smin,
        SelectPatternFlavor::Smax => MinMaxIntrinsic::Smax,
        SelectPatternFlavor::Umin => MinMaxIntrinsic::Umin,
        SelectPatternFlavor::Umax => MinMaxIntrinsic::Umax,
        _ => return None,
    };
    if descriptor.id().base_name() != wanted.name() {
        return None;
    }
    let (first, second) = (call.args.first()?, call.args.get(1)?);
    Some((
        value_from_slot(value, first.get()),
        value_from_slot(value, second.get()),
    ))
}

/// Whether `predicate`, read in `MaxMin_match`'s normalised direction, selects
/// `flavor`. Ports the four `Pred_t::match` specialisations.
fn min_max_predicate_matches(flavor: SelectPatternFlavor, predicate: IntPredicate) -> bool {
    matches!(
        (flavor, predicate),
        (
            SelectPatternFlavor::Smin,
            IntPredicate::Slt | IntPredicate::Sle
        ) | (
            SelectPatternFlavor::Smax,
            IntPredicate::Sgt | IntPredicate::Sge
        ) | (
            SelectPatternFlavor::Umin,
            IntPredicate::Ult | IntPredicate::Ule
        ) | (
            SelectPatternFlavor::Umax,
            IntPredicate::Ugt | IntPredicate::Uge
        )
    )
}

/// The `select(icmp PRED L, R, L, R)` shape `MaxMin_match` looks for: a select
/// whose condition compares the very values the select returns.
///
/// Returns the operands as upstream binds them — `L` is the *compare's* first
/// operand — together with the predicate `MaxMin_match` tests, which is the
/// compare's own when its left operand is the true arm and the **inverse**
/// otherwise.
fn select_over_icmp_of_its_own_arms<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>, IntPredicate)> {
    let Some(InstructionKindData::Select(select)) = instruction_kind(value) else {
        return None;
    };
    let condition = value_from_slot(value, select.cond.get());
    let Some(InstructionKindData::Icmp(compare)) = instruction_kind(condition) else {
        return None;
    };
    let compare_lhs = value_from_slot(condition, compare.lhs.get());
    let compare_rhs = value_from_slot(condition, compare.rhs.get());
    let true_value = value_from_slot(value, select.true_val.get());
    let false_value = value_from_slot(value, select.false_val.get());

    if compare_lhs == true_value && compare_rhs == false_value {
        Some((compare_lhs, compare_rhs, compare.predicate))
    } else if compare_lhs == false_value && compare_rhs == true_value {
        Some((compare_lhs, compare_rhs, compare.predicate.inverse()))
    } else {
        None
    }
}

/// The constant a floating-point min/max compares `x` against, when `value` is
/// the `fcmp`/`select` form of `flavor(x, C)`.
///
/// Ports the `m_OrdOrUnordFMin(m_Specific(CmpLHS), m_APFloat(C2))` family,
/// which matches the structural `select(fcmp PRED L, R, L, R)` shape directly
/// rather than going through `matchSelectPattern`.
fn float_min_max_against_constant<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    expected_operand: Value<'ctx, B>,
    flavor: SelectPatternFlavor,
) -> Option<ApFloat> {
    let Some(InstructionKindData::Select(select)) = instruction_kind(value) else {
        return None;
    };
    let condition = value_from_slot(value, select.cond.get());
    let Some(InstructionKindData::Fcmp(compare)) = instruction_kind(condition) else {
        return None;
    };
    let compare_lhs = value_from_slot(condition, compare.lhs.get());
    let compare_rhs = value_from_slot(condition, compare.rhs.get());
    let true_value = value_from_slot(value, select.true_val.get());
    let false_value = value_from_slot(value, select.false_val.get());

    // The compared values have to be the ones the select returns.
    let (left, right, predicate) = if compare_lhs == true_value && compare_rhs == false_value {
        (compare_lhs, compare_rhs, compare.predicate)
    } else if compare_lhs == false_value && compare_rhs == true_value {
        (compare_rhs, compare_lhs, compare.predicate.swapped())
    } else {
        return None;
    };

    let is_min = matches!(
        predicate,
        FloatPredicate::Olt | FloatPredicate::Ole | FloatPredicate::Ult | FloatPredicate::Ule
    );
    let is_max = matches!(
        predicate,
        FloatPredicate::Ogt | FloatPredicate::Oge | FloatPredicate::Ugt | FloatPredicate::Uge
    );
    let wanted = match flavor {
        SelectPatternFlavor::FminNum if is_min => true,
        SelectPatternFlavor::FmaxNum if is_max => true,
        _ => false,
    };
    if !wanted {
        return None;
    }

    if left == expected_operand {
        return float_constant(right);
    }
    (right == expected_operand).then(|| float_constant(left))?
}

/// Ports the static `lookThroughCast(CmpI, V1, V2, CastOp)` for the two
/// arrangements it accepts without folding a constant.
///
/// Upstream's third arm calls `lookThroughCastConst`, which builds a *new*
/// constant with `ConstantExpr::getTrunc` / `ConstantFoldCastOperand` and
/// checks it round-trips. Minting a constant is a module mutation, so that arm
/// is not ported; the two shapes that need no new value are.
fn look_through_cast_arm<'ctx, B: ModuleBrand + 'ctx>(
    compare: Value<'ctx, B>,
    first: Value<'ctx, B>,
    second: Value<'ctx, B>,
) -> Option<(CastOpcode, Value<'ctx, B>)> {
    let Some(InstructionKindData::Cast(cast)) = instruction_kind(first) else {
        return None;
    };
    let source_ty = value_from_slot(first, cast.src.get()).ty().id();

    // If both arms are the same cast from the same type, look through both.
    if let Some(InstructionKindData::Cast(other)) = instruction_kind(second) {
        let other_source = value_from_slot(second, other.src.get());
        if cast.kind == other.kind && other_source.ty().id() == source_ty {
            return Some((cast.kind, other_source));
        }
        return None;
    }

    // `trunc` of a value the compare already widened:
    //   %y_ext = sext iK %y to iN
    //   %cond  = cmp iN %x, %y_ext
    //   %tr    = trunc iN %x to iK
    //   %sel   = select i1 %cond, iK %tr, iK %y
    if cast.kind != CastOpcode::Trunc {
        return None;
    }
    let (_, _, compare_rhs) = compare_parts(compare)?;
    let Some(InstructionKindData::Cast(widened)) = instruction_kind(compare_rhs) else {
        return None;
    };
    if !matches!(widened.kind, CastOpcode::Sext | CastOpcode::Zext)
        || widened.src.get() != second.slot()
    {
        return None;
    }
    Some((cast.kind, compare_rhs))
}

/// The value a cast instruction casts.
fn cast_source<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    let Some(InstructionKindData::Cast(data)) = instruction_kind(value) else {
        return None;
    };
    Some(value_from_slot(value, data.src.get()))
}

/// The predicate and operands of an `icmp` or `fcmp`.
fn compare_parts<'ctx, B: ModuleBrand + 'ctx>(
    compare: Value<'ctx, B>,
) -> Option<(CmpPredicate, Value<'ctx, B>, Value<'ctx, B>)> {
    match instruction_kind(compare)? {
        InstructionKindData::Icmp(data) => Some((
            CmpPredicate::Int(data.predicate),
            value_from_slot(compare, data.lhs.get()),
            value_from_slot(compare, data.rhs.get()),
        )),
        InstructionKindData::Fcmp(data) => Some((
            CmpPredicate::Float(data.predicate),
            value_from_slot(compare, data.lhs.get()),
            value_from_slot(compare, data.rhs.get()),
        )),
        _ => None,
    }
}

/// `CmpInst::getSwappedPredicate` over either predicate family.
fn swapped_predicate(predicate: CmpPredicate) -> CmpPredicate {
    match predicate {
        CmpPredicate::Int(predicate) => CmpPredicate::Int(predicate.swapped()),
        CmpPredicate::Float(predicate) => CmpPredicate::Float(predicate.swapped()),
    }
}

/// Upstream's `m_AnyZeroFP()`: `+0.0` or `-0.0`.
fn is_any_zero_fp<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    float_constant(value).is_some_and(|constant| constant.is_zero())
}

/// Whether every user of `value`'s `select` condition is that select.
/// Ports the `m_Select(m_OneUse(m_Value()), ..)` half of
/// `canConvertToMinOrMaxIntrinsic`.
fn select_condition_has_one_use<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    let Some(InstructionKindData::Select(select)) = instruction_kind(value) else {
        return false;
    };
    value_from_slot(value, select.cond.get()).users().count() == 1
}

/// The `ApInt` behind a scalar integer constant.
fn int_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApInt> {
    ConstantIntValue::<IntDyn, B>::try_from(Constant::try_from(value).ok()?)
        .ok()
        .map(|constant| constant.ap_int())
}

/// The `ApFloat` behind a scalar floating-point constant.
fn float_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApFloat> {
    ConstantFloatValue::<FloatDyn, B>::try_from(Constant::try_from(value).ok()?)
        .ok()
        .map(|constant| constant.ap_float())
}

fn instruction_kind<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<&'ctx InstructionKindData> {
    match &value.data().kind {
        ValueKindData::Instruction(instruction) => Some(&instruction.kind),
        _ => None,
    }
}

fn value_from_slot<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    slot: ValueSlot,
) -> Value<'ctx, B> {
    let module = ModuleRef::<B>::new(anchor.module().core_ref());
    let data = module.value_data(slot);
    Value::from_parts(slot, module, data.ty)
}
