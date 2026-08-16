//! Whether one boolean condition forces another.
//!
//! Ports the `isImpliedCondition` / `isImpliedByDomCondition` family of
//! `llvm/lib/Analysis/ValueTracking.cpp` together with the static helpers they
//! rest on (`isImpliedCondICmps`, `isImpliedCondFCmps`, `isImpliedCondAndOr`,
//! `isImpliedCondCommonOperandWithCR`, `isImpliedCondOperands`,
//! `isTruePredicate`, `getDomPredecessorCondition`).
//!
//! The answer is three-valued and the `Option` says which: `Some(true)` — the
//! left condition being as claimed forces the right one true; `Some(false)` —
//! forces it false; `None` — nothing can be inferred. Upstream's own comment
//! notes that implication has the same truth table as `<=u` on `i1`.
//!
//! # What is not modeled, and why
//!
//! - **The floating-point half is partial.** `isImpliedCondFCmps` reaches its
//!   constant-versus-constant conclusion through `ConstantFPRange`
//!   (`llvm/IR/ConstantFPRange.h`), which llvmkit does not port. Everything
//!   that does not need it — normalisation, the matching-operands bit-test —
//!   is here; the `makeExactFCmpRegion` arm answers `None`. That is the
//!   conservative direction.

use crate::ApInt;
use crate::ap_int::Signedness;
use crate::assumptions::{single_predecessor, terminator_of_block};
use crate::cmp_predicate::{CmpPredicate, IntPredicate, PredicateWithSameSign};
use crate::constant::ConstantData;
use crate::constant_range::ConstantRange;
use crate::data_layout::DataLayout;
use crate::instr_types::{BinaryOpData, BranchKind, CastOpcode};
use crate::instruction::{InstructionKindData, InstructionView};
use crate::module::{ModuleBrand, ModuleRef};
use crate::select_pattern::{SelectPatternFlavor, int_min_max_over};
use crate::r#type::TypeKind;
use crate::value::{Value, ValueKindData, ValueSlot};
use crate::value_tracking::{
    MAX_ANALYSIS_RECURSION_DEPTH, ValueTrackingQuery, compute_constant_range,
};

// --------------------------------------------------------------------------
// Entry points
// --------------------------------------------------------------------------

/// Whether `lhs` holding as claimed forces `rhs`.
///
/// Ports the `(const Value *LHS, const Value *RHS, ...)` overload of
/// `llvm::isImpliedCondition`. `lhs_is_true` is upstream's `LHSIsTrue`, which
/// defaults to `true`: pass `false` to ask what `!lhs` forces.
///
/// Both conditions must be `i1` or a vector of `i1`.
pub fn is_implied_condition<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    data_layout: &DataLayout,
    lhs_is_true: bool,
) -> Option<bool> {
    is_implied_condition_at_depth(lhs, rhs, data_layout, lhs_is_true, 0)
}

/// [`is_implied_condition`] with the right-hand condition already taken apart
/// into a predicate and two operands.
///
/// Ports the `(const Value *LHS, CmpPredicate RHSPred, const Value *RHSOp0,
/// const Value *RHSOp1, ...)` overload of `llvm::isImpliedCondition`. A caller
/// that has the pieces but no `icmp` instruction holding them — a fold about to
/// be built, say — uses this rather than materialising one.
pub fn is_implied_condition_decomposed<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs_predicate: PredicateWithSameSign,
    rhs_op0: Value<'ctx, B>,
    rhs_op1: Value<'ctx, B>,
    data_layout: &DataLayout,
    lhs_is_true: bool,
) -> Option<bool> {
    is_implied_condition_decomposed_at_depth(
        lhs,
        rhs_predicate,
        &CompareOperand::Value(rhs_op0),
        &CompareOperand::Value(rhs_op1),
        data_layout,
        lhs_is_true,
        0,
    )
}

/// The value of `condition` at `context`, when a dominating branch settles it.
///
/// Ports the `(const Value *Cond, const Instruction *ContextI, const DataLayout
/// &DL)` overload of `llvm::isImpliedByDomCondition`.
///
/// Upstream's own `TODO` — that reaching only the single predecessor is "a
/// poor/cheap way to determine dominance" and a `DominatorTree` would do better
/// — is inherited along with the behaviour.
pub fn is_implied_by_dom_condition<'ctx, B: ModuleBrand + 'ctx>(
    condition: Value<'ctx, B>,
    context: &InstructionView<'ctx, B>,
    data_layout: &DataLayout,
) -> Option<bool> {
    let (predecessor_condition, is_true) = dom_predecessor_condition(context)?;
    is_implied_condition(predecessor_condition, condition, data_layout, is_true)
}

/// [`is_implied_by_dom_condition`] with the condition taken apart.
///
/// Ports the `(CmpPredicate Pred, const Value *LHS, const Value *RHS, ...)`
/// overload of `llvm::isImpliedByDomCondition`.
pub fn is_implied_by_dom_condition_decomposed<'ctx, B: ModuleBrand + 'ctx>(
    predicate: PredicateWithSameSign,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    context: &InstructionView<'ctx, B>,
    data_layout: &DataLayout,
) -> Option<bool> {
    let (predecessor_condition, is_true) = dom_predecessor_condition(context)?;
    is_implied_condition_decomposed(
        predecessor_condition,
        predicate,
        lhs,
        rhs,
        data_layout,
        is_true,
    )
}

// --------------------------------------------------------------------------
// The recursive core
// --------------------------------------------------------------------------

/// Ports the value/value `isImpliedCondition` at an explicit recursion depth.
fn is_implied_condition_at_depth<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    data_layout: &DataLayout,
    lhs_is_true: bool,
    depth: u32,
) -> Option<bool> {
    // LHS ==> RHS by definition.
    if lhs == rhs {
        return Some(lhs_is_true);
    }

    // Match not.
    let mut rhs = rhs;
    let mut invert_rhs = false;
    if let Some(inner) = not_operand(rhs) {
        if lhs == inner {
            return Some(!lhs_is_true);
        }
        rhs = inner;
        invert_rhs = true;
    }

    let flip = |implied: bool| if invert_rhs { !implied } else { implied };

    if let Some(compare) = int_compare_parts(rhs).or_else(|| float_compare_parts(rhs)) {
        return is_implied_condition_decomposed_at_depth(
            lhs,
            compare.predicate,
            &CompareOperand::Value(compare.lhs),
            &CompareOperand::Value(compare.rhs),
            data_layout,
            lhs_is_true,
            depth,
        )
        .map(flip);
    }
    if let Some(source) = nuw_trunc_source(rhs) {
        return is_implied_condition_decomposed_at_depth(
            lhs,
            PredicateWithSameSign::int(IntPredicate::Ne),
            &CompareOperand::Value(source),
            &zero_like(source)?,
            data_layout,
            lhs_is_true,
            depth,
        )
        .map(flip);
    }

    if depth == MAX_ANALYSIS_RECURSION_DEPTH {
        return None;
    }

    // LHS ==> (RHS1 || RHS2) if LHS ==> RHS1 or LHS ==> RHS2.
    if let Some((rhs1, rhs2)) = logical_or_operands(rhs) {
        for arm in [rhs1, rhs2] {
            if is_implied_condition_at_depth(lhs, arm, data_layout, lhs_is_true, depth + 1)
                == Some(true)
            {
                return Some(!invert_rhs);
            }
        }
    }
    // LHS ==> !(RHS1 && RHS2) if LHS ==> !RHS1 or LHS ==> !RHS2.
    if let Some((rhs1, rhs2)) = logical_and_operands(rhs) {
        for arm in [rhs1, rhs2] {
            if is_implied_condition_at_depth(lhs, arm, data_layout, lhs_is_true, depth + 1)
                == Some(false)
            {
                return Some(invert_rhs);
            }
        }
    }

    None
}

/// Ports the decomposed `isImpliedCondition` at an explicit recursion depth.
fn is_implied_condition_decomposed_at_depth<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs_predicate: PredicateWithSameSign,
    rhs_op0: &CompareOperand<'ctx, B>,
    rhs_op1: &CompareOperand<'ctx, B>,
    data_layout: &DataLayout,
    lhs_is_true: bool,
    depth: u32,
) -> Option<bool> {
    // Bail out when we hit the limit.
    if depth == MAX_ANALYSIS_RECURSION_DEPTH {
        return None;
    }

    // A mismatch occurs when a scalar compare is weighed against a vector one.
    // A bare literal is never a vector.
    let rhs_is_vector = rhs_op0.value().is_some_and(is_vector);
    if rhs_is_vector != is_vector(lhs) {
        return None;
    }

    // Upstream asserts `LHS->getType()->isIntOrIntVectorTy(1)`. A caller that
    // hands over a non-boolean condition has broken the contract; declining is
    // the honest answer where upstream aborts, and no arm below could fire.
    if !is_int_or_int_vector_of_width_one(lhs) {
        return None;
    }

    // Match not.
    let (lhs, lhs_is_true) = match not_operand(lhs) {
        Some(inner) => (inner, !lhs_is_true),
        None => (lhs, lhs_is_true),
    };

    let right = PredicateSide {
        predicate: rhs_predicate,
        op0: rhs_op0.clone(),
        op1: rhs_op1.clone(),
    };

    if rhs_op0.is_int_or_pointer_scalar() {
        if let Some(compare) = int_compare_parts(lhs) {
            return implied_by_int_compares(
                CompareSides {
                    left: PredicateSide {
                        predicate: compare.predicate,
                        op0: CompareOperand::Value(compare.lhs),
                        op1: CompareOperand::Value(compare.rhs),
                    },
                    right,
                },
                data_layout,
                lhs_is_true,
            );
        }
        if let Some(source) = nuw_trunc_source(lhs) {
            return implied_by_int_compares(
                CompareSides {
                    left: PredicateSide {
                        predicate: PredicateWithSameSign::int(IntPredicate::Ne),
                        op0: CompareOperand::Value(source),
                        op1: zero_like(source)?,
                    },
                    right,
                },
                data_layout,
                lhs_is_true,
            );
        }
    } else {
        // Upstream asserts the type is floating-point here.
        if let Some(compare) = float_compare_parts(lhs) {
            return implied_by_float_compares(
                CompareSides {
                    left: PredicateSide {
                        predicate: compare.predicate,
                        op0: CompareOperand::Value(compare.lhs),
                        op1: CompareOperand::Value(compare.rhs),
                    },
                    right,
                },
                lhs_is_true,
            );
        }
    }

    // The left-hand side may still be an `and`, an `or` or a `select`; the
    // right-hand side is expected to be an `icmp`. Upstream's own `FIXME`
    // records that and/or/select on the right is not handled.
    implied_by_and_or(
        lhs,
        rhs_predicate,
        rhs_op0,
        rhs_op1,
        data_layout,
        lhs_is_true,
        depth,
    )
}

/// A comparison operand: a value in the module, or a bare integer.
///
/// Upstream builds `ConstantInt::get(V->getType(), 0)` for the two `m_NUWTrunc`
/// arms and passes it as an ordinary operand. llvmkit declines to mint a
/// constant from an analysis — an analysis that edits the IR it is asked about
/// is a different kind of thing — so the literal travels beside the values.
///
/// Equality follows LLVM's constant uniquing rather than llvmkit's raw value
/// identity: a literal equals a constant value holding the same bits, because
/// upstream those *are* the same object.
enum CompareOperand<'ctx, B: ModuleBrand> {
    /// A value in the module.
    Value(Value<'ctx, B>),
    /// An integer with no `Value` behind it.
    Literal(ApInt),
}

impl<'ctx, B: ModuleBrand + 'ctx> CompareOperand<'ctx, B> {
    /// The value, or `None` for a bare literal.
    fn value(&self) -> Option<Value<'ctx, B>> {
        match self {
            Self::Value(value) => Some(*value),
            Self::Literal(_) => None,
        }
    }

    /// The integer this operand holds, if it is one.
    fn constant(&self) -> Option<ApInt> {
        match self {
            Self::Value(value) => constant_int(*value),
            Self::Literal(bits) => Some(bits.clone()),
        }
    }

    /// Whether the operand is any constant. Ports `m_ImmConstant`.
    fn is_constant(&self) -> bool {
        match self {
            Self::Value(value) => is_constant(*value),
            Self::Literal(_) => true,
        }
    }

    /// Whether the operand's scalar type is an integer or a pointer. Ports
    /// `getScalarType()->isIntOrPtrTy()`; a bare literal is always an integer.
    fn is_int_or_pointer_scalar(&self) -> bool {
        match self {
            Self::Value(value) => matches!(
                scalar_kind(*value),
                Some(TypeKind::Integer { .. } | TypeKind::Pointer { .. })
            ),
            Self::Literal(_) => true,
        }
    }

    /// The range `computeConstantRange` gives this operand, at the depth
    /// upstream passes (`MaxAnalysisRecursionDepth - 1`) and with no context.
    fn range(&self, for_signed: bool, data_layout: &DataLayout) -> Option<ConstantRange> {
        match self {
            Self::Value(value) => {
                let query: ValueTrackingQuery<'_, 'ctx, B> = ValueTrackingQuery::new(data_layout)
                    .with_max_depth(MAX_ANALYSIS_RECURSION_DEPTH.saturating_sub(1));
                compute_constant_range(
                    *value,
                    if for_signed {
                        Signedness::Signed
                    } else {
                        Signedness::Unsigned
                    },
                    &query,
                )
                .ok()
            }
            Self::Literal(bits) => Some(ConstantRange::single(bits.clone())),
        }
    }
}

// Hand-written: a derived `Clone` would bound `B: Clone`, which a bare brand
// unit struct need not satisfy, and the compiler would blame the use site.
impl<'ctx, B: ModuleBrand + 'ctx> Clone for CompareOperand<'ctx, B> {
    fn clone(&self) -> Self {
        match self {
            Self::Value(value) => Self::Value(*value),
            Self::Literal(bits) => Self::Literal(bits.clone()),
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> PartialEq for CompareOperand<'ctx, B> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Value(a), Self::Value(b)) => a == b,
            (Self::Literal(a), Self::Literal(b)) => a == b,
            (Self::Value(value), Self::Literal(bits))
            | (Self::Literal(bits), Self::Value(value)) => {
                constant_int(*value).is_some_and(|held| held == *bits)
            }
        }
    }
}

/// One side of an implication: a predicate and the two operands it compares.
struct PredicateSide<'ctx, B: ModuleBrand> {
    predicate: PredicateWithSameSign,
    op0: CompareOperand<'ctx, B>,
    op1: CompareOperand<'ctx, B>,
}

/// Both sides of an implication.
///
/// Upstream passes six loose parameters (`LPred, L0, L1, RPred, R0, R1`);
/// bundling them keeps the argument count inside clippy's limit and keeps the
/// two sides from being transposed at a call site.
struct CompareSides<'ctx, B: ModuleBrand> {
    left: PredicateSide<'ctx, B>,
    right: PredicateSide<'ctx, B>,
}

/// Ports `isImpliedCondICmps`.
fn implied_by_int_compares<'ctx, B: ModuleBrand + 'ctx>(
    sides: CompareSides<'ctx, B>,
    data_layout: &DataLayout,
    lhs_is_true: bool,
) -> Option<bool> {
    let mut left_predicate = sides.left.predicate;
    let mut right_predicate = sides.right.predicate;
    let (mut l0, mut l1) = (sides.left.op0, sides.left.op1);
    let (mut r0, mut r1) = (sides.right.op0, sides.right.op1);

    // The rest of the logic assumes the left condition is true; invert the
    // predicate when it is not.
    if !lhs_is_true {
        left_predicate = left_predicate.inverse();
    }

    // Operands can be non-canonical, so normalise any common one to L0/R0.
    if l0 == r1 {
        core::mem::swap(&mut r0, &mut r1);
        right_predicate = right_predicate.swapped();
    }
    if r0 == l1 {
        core::mem::swap(&mut l0, &mut l1);
        left_predicate = left_predicate.swapped();
    }
    if l1 == r1 {
        // With L0 == R0 and L1 == R1, make L1/R1 the constants.
        if l0 != r0 || l0.is_constant() {
            core::mem::swap(&mut l0, &mut l1);
            left_predicate = left_predicate.swapped();
            core::mem::swap(&mut r0, &mut r1);
            right_predicate = right_predicate.swapped();
        }
    }

    let left_int = left_predicate.as_int()?;
    let right_int = right_predicate.as_int()?;

    // Operand 0 matches and at least one side has a constant.
    if l0 == r0 && (l1.constant().is_some() || r1.constant().is_some()) {
        let left_range = l1.range(left_int.is_signed(), data_layout);
        let right_range = r1.range(right_int.is_signed(), data_layout);
        if let (Some(left_range), Some(right_range)) = (left_range, right_range) {
            if let Some(result) = implied_by_common_operand_ranges(
                left_predicate,
                &left_range,
                right_predicate,
                &right_range,
            ) {
                return Some(result);
            }
            // Both were exact constant ranges and nothing came of it; nothing
            // further can be deduced.
            if l1.constant().is_some() && r1.constant().is_some() {
                return None;
            }
        }
    }

    // Both compares over exactly the same operands.
    if l0 == r0 && l1 == r1 {
        return PredicateWithSameSign::implied_by_matching_comparison(
            left_predicate,
            right_predicate,
        );
    }

    // "X - Y must be positive if X >= Y and no overflow." Taking SGT as the
    // example: L0:x > L1:y and C >= 0 ==> R0:(x -nsw y) < R1:(-C) is false.
    let signed_left = left_predicate.preferred_signed_predicate();
    if matches!(
        signed_left,
        CmpPredicate::Int(IntPredicate::Sgt | IntPredicate::Sge)
    ) && nsw_sub_of(&r0, &l0, &l1)
        && r1
            .constant()
            .is_some_and(|c| c.is_negative() || c.is_zero())
        && PredicateWithSameSign::implied_by_matching_comparison(
            with_predicate(left_predicate, signed_left),
            right_predicate,
        ) == Some(false)
    {
        return Some(false);
    }
    // And SLT: L0:x < L1:y and C <= 0 ==> R0:(x -nsw y) < R1:(-C) is true.
    if matches!(
        signed_left,
        CmpPredicate::Int(IntPredicate::Slt | IntPredicate::Sle)
    ) && nsw_sub_of(&r0, &l0, &l1)
        && r1.constant().is_some_and(|c| !c.is_negative())
        && PredicateWithSameSign::implied_by_matching_comparison(
            with_predicate(left_predicate, signed_left),
            right_predicate,
        ) == Some(true)
    {
        return Some(true);
    }

    // a - b == NonZero -> a != b, also through `ptrtoint`/`ptrtoaddr`.
    if left_int == IntPredicate::Eq
        && right_int.is_equality()
        && l1.constant().is_some_and(|c| !c.is_zero())
        && let Some(l0_value) = l0.value()
        && let Some((a, b)) = sub_operands(l0_value)
    {
        let a = CompareOperand::Value(a);
        let b = CompareOperand::Value(b);
        let through_pointer = |operand: &CompareOperand<'ctx, B>,
                               target: &CompareOperand<'ctx, B>| {
            operand
                .value()
                .and_then(ptr_to_int_or_addr_source)
                .map(CompareOperand::Value)
                .is_some_and(|source| source == *target)
        };
        if (a == r0 && b == r1)
            || (a == r1 && b == r0)
            || (through_pointer(&a, &r0) && through_pointer(&b, &r1))
            || (through_pointer(&a, &r1) && through_pointer(&b, &r0))
        {
            return Some(right_predicate.drop_same_sign() == CmpPredicate::Int(IntPredicate::Ne));
        }
    }

    // L0 = R0 = L1 + R1: L0 >=u L1 implies R0 >=u R1, L0 <u L1 implies R0 <u R1.
    if l0 == r0
        && matches!(left_int, IntPredicate::Ult | IntPredicate::Uge)
        && matches!(right_int, IntPredicate::Ult | IntPredicate::Uge)
        && is_commutative_add_of(&l0, &l1, &r1)
    {
        return Some(PredicateWithSameSign::matching(left_predicate, right_predicate).is_some());
    }

    let matching = PredicateWithSameSign::matching(left_predicate, right_predicate)?;
    implied_by_operands(
        matching.as_int()?,
        l0.value()?,
        l1.value()?,
        r0.value()?,
        r1.value()?,
    )
}

/// Ports `isImpliedCondCommonOperandWithCR`: "icmp LPred X, LCR" against
/// "icmp RPred X, RCR", both sides given as ranges.
fn implied_by_common_operand_ranges(
    left_predicate: PredicateWithSameSign,
    left_range: &ConstantRange,
    right_predicate: PredicateWithSameSign,
    right_range: &ConstantRange,
) -> Option<bool> {
    let range_implies = |range: &ConstantRange, predicate: IntPredicate| -> Option<bool> {
        // Every value true for the left is true for the right: implied true.
        if range.icmp(predicate, right_range) {
            return Some(true);
        }
        // No overlap at all: implied false.
        if range.icmp(predicate.inverse(), right_range) {
            return Some(false);
        }
        None
    };

    let left_int = left_predicate.as_int()?;
    let right_int = right_predicate.as_int()?;
    let allowed = ConstantRange::make_allowed_icmp_region(left_int, left_range);
    if let Some(result) = range_implies(&allowed, right_int) {
        return Some(result);
    }

    // Exactly one side claims `samesign`: read it in the other's signedness and
    // try once more.
    if left_predicate.has_same_sign() != right_predicate.has_same_sign() {
        let left_int = if left_predicate.has_same_sign() {
            left_int.flip_signedness()
        } else {
            left_int
        };
        let right_int = if right_predicate.has_same_sign() {
            right_int.flip_signedness()
        } else {
            right_int
        };
        let allowed = ConstantRange::make_allowed_icmp_region(left_int, left_range);
        return range_implies(&allowed, right_int);
    }
    None
}

/// Ports `isImpliedCondFCmps`.
///
/// The `ConstantFPRange` arm is not reachable — see the module header — so this
/// is normalisation plus the matching-operands bit test.
fn implied_by_float_compares<'ctx, B: ModuleBrand + 'ctx>(
    sides: CompareSides<'ctx, B>,
    lhs_is_true: bool,
) -> Option<bool> {
    let mut left_predicate = sides.left.predicate;
    let mut right_predicate = sides.right.predicate;
    let (mut l0, mut l1) = (sides.left.op0, sides.left.op1);
    let (mut r0, mut r1) = (sides.right.op0, sides.right.op1);

    if !lhs_is_true {
        left_predicate = left_predicate.inverse();
    }

    if l0 == r1 {
        core::mem::swap(&mut r0, &mut r1);
        right_predicate = right_predicate.swapped();
    }
    if r0 == l1 {
        core::mem::swap(&mut l0, &mut l1);
        left_predicate = left_predicate.swapped();
    }
    if l1 == r1 && (l0 != r0 || l0.is_constant()) {
        core::mem::swap(&mut l0, &mut l1);
        left_predicate = left_predicate.swapped();
        core::mem::swap(&mut r0, &mut r1);
        right_predicate = right_predicate.swapped();
    }

    // Matching operands: the predicates are four-bit masks, so containment is
    // a bit test. Upstream writes it as `(LPred & RPred) == LPred`.
    if l0 == r0 && l1 == r1 {
        let left = left_predicate.as_float()?.as_raw();
        let right = right_predicate.as_float()?.as_raw();
        if left & right == left {
            return Some(true);
        }
        if left & !right == left {
            return Some(false);
        }
    }

    None
}

/// Ports `isImpliedCondAndOr`: when the left condition is an `and`, an `or` or a
/// `select`, each leg may carry the implication on its own.
fn implied_by_and_or<'ctx, B: ModuleBrand + 'ctx>(
    lhs: Value<'ctx, B>,
    rhs_predicate: PredicateWithSameSign,
    rhs_op0: &CompareOperand<'ctx, B>,
    rhs_op1: &CompareOperand<'ctx, B>,
    data_layout: &DataLayout,
    lhs_is_true: bool,
    depth: u32,
) -> Option<bool> {
    // If an `or` is false both its legs are false; if an `and` is true both its
    // legs are true. Either way the leg inherits `lhs_is_true`.
    let legs = if lhs_is_true {
        logical_and_operands(lhs)
    } else {
        logical_or_operands(lhs)
    }?;

    for leg in [legs.0, legs.1] {
        if let Some(implication) = is_implied_condition_decomposed_at_depth(
            leg,
            rhs_predicate,
            rhs_op0,
            rhs_op1,
            data_layout,
            lhs_is_true,
            depth + 1,
        ) {
            return Some(implication);
        }
    }
    None
}

/// Ports `isImpliedCondOperands`: `icmp Pred BLHS BRHS` is true whenever
/// `icmp Pred ALHS ARHS` is.
fn implied_by_operands<'ctx, B: ModuleBrand + 'ctx>(
    predicate: IntPredicate,
    a_lhs: Value<'ctx, B>,
    a_rhs: Value<'ctx, B>,
    b_lhs: Value<'ctx, B>,
    b_rhs: Value<'ctx, B>,
) -> Option<bool> {
    let (order, left, right) = match predicate {
        IntPredicate::Slt | IntPredicate::Sle => {
            (IntPredicate::Sle, (b_lhs, a_lhs), (a_rhs, b_rhs))
        }
        IntPredicate::Sgt | IntPredicate::Sge => {
            (IntPredicate::Sle, (a_lhs, b_lhs), (b_rhs, a_rhs))
        }
        IntPredicate::Ult | IntPredicate::Ule => {
            (IntPredicate::Ule, (b_lhs, a_lhs), (a_rhs, b_rhs))
        }
        IntPredicate::Ugt | IntPredicate::Uge => {
            (IntPredicate::Ule, (a_lhs, b_lhs), (b_rhs, a_rhs))
        }
        _ => return None,
    };
    (is_true_predicate(order, left.0, left.1) && is_true_predicate(order, right.0, right.1))
        .then_some(true)
}

/// Ports `isTruePredicate`: whether `icmp Pred LHS RHS` is *statically* true,
/// by structure rather than by value.
fn is_true_predicate<'ctx, B: ModuleBrand + 'ctx>(
    predicate: IntPredicate,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
) -> bool {
    if is_true_when_equal(predicate) && lhs == rhs {
        return true;
    }

    match predicate {
        IntPredicate::Sle => {
            // LHS s<= LHS +nsw C and LHS s<= LHS | C, both if C >= 0.
            if let Some(constant) = nsw_add_or_or_constant(rhs, lhs) {
                return !constant.is_negative();
            }
            // LHS s<= smax(LHS, V) for any V.
            if int_min_max_over(rhs, lhs, SelectPatternFlavor::Smax).is_some() {
                return true;
            }
            // smin(RHS, V) s<= RHS for any V.
            if int_min_max_over(lhs, rhs, SelectPatternFlavor::Smin).is_some() {
                return true;
            }
            // A = X +nsw CA and B = X +nsw CB, with CA s<= CB.
            if let Some((x, left_constant)) = nsw_add_like_constant(lhs)
                && let Some((y, right_constant)) = nsw_add_like_constant(rhs)
                && x == y
            {
                return left_constant.sle(&right_constant);
            }
            false
        }
        IntPredicate::Ule => {
            // LHS u<= LHS +nuw V, and LHS u<= LHS | V, for any V.
            if nuw_add_over(rhs, lhs).is_some() || or_over(rhs, lhs).is_some() {
                return true;
            }
            // LHS u<= umax(LHS, V) for any V.
            if int_min_max_over(rhs, lhs, SelectPatternFlavor::Umax).is_some() {
                return true;
            }
            // RHS >> V u<= RHS for any V.
            if lshr_of(lhs, rhs) {
                return true;
            }
            // RHS u/ C u<= RHS when C u> 1.
            if let Some(constant) = udiv_of_by_constant(lhs, rhs)
                && constant.ugt(&ApInt::one_bit_set(constant.bit_width(), 0))
            {
                return true;
            }
            // RHS & V u<= RHS, and umin(RHS, V) u<= RHS, for any V.
            if and_over(lhs, rhs).is_some()
                || int_min_max_over(lhs, rhs, SelectPatternFlavor::Umin).is_some()
            {
                return true;
            }
            // A = X +nuw CA and B = X +nuw CB, with CA u<= CB.
            if let Some((x, left_constant)) = nuw_add_like_constant(lhs)
                && let Some((y, right_constant)) = nuw_add_like_constant(rhs)
                && x == y
            {
                return left_constant.ule(&right_constant);
            }
            false
        }
        _ => false,
    }
}

/// Ports `CmpInst::isTrueWhenEqual` for the integer predicates.
fn is_true_when_equal(predicate: IntPredicate) -> bool {
    matches!(
        predicate,
        IntPredicate::Eq
            | IntPredicate::Uge
            | IntPredicate::Ule
            | IntPredicate::Sge
            | IntPredicate::Sle
    )
}

/// Ports `getDomPredecessorCondition`: the condition of the conditional branch
/// in `context`'s single predecessor, and whether `context`'s block is its true
/// successor.
fn dom_predecessor_condition<'ctx, B: ModuleBrand + 'ctx>(
    context: &InstructionView<'ctx, B>,
) -> Option<(Value<'ctx, B>, bool)> {
    let anchor = context.to_erased();
    let context_block = context.parent().slot();
    let predecessor = single_predecessor(anchor, context_block)?;

    let terminator = terminator_of_block(anchor, predecessor)?;
    let InstructionKindData::Br(data) = instruction_kind(terminator)? else {
        return None;
    };
    let (condition, then_block, else_block) = match &*data.kind.borrow() {
        BranchKind::Unconditional(_) => return None,
        BranchKind::Conditional {
            cond,
            then_bb,
            else_bb,
        } => (cond.get(), *then_bb, *else_bb),
    };

    // A branch whose arms are the same block should simplify; upstream declines
    // to reason about it.
    if then_block == else_block {
        return None;
    }

    // Upstream asserts one of the two arms is the context block.
    if then_block != context_block && else_block != context_block {
        return None;
    }

    Some((
        value_from_slot(anchor, condition),
        then_block == context_block,
    ))
}

// --------------------------------------------------------------------------
// Pattern helpers
// --------------------------------------------------------------------------

/// The predicate and operands of a comparison.
struct ComparePartsOf<'ctx, B: ModuleBrand> {
    predicate: PredicateWithSameSign,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
}

/// The parts of an `icmp`, carrying its `samesign` flag.
fn int_compare_parts<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<ComparePartsOf<'ctx, B>> {
    let InstructionKindData::Icmp(data) = instruction_kind(value)? else {
        return None;
    };
    let predicate = if data.samesign {
        PredicateWithSameSign::int_same_sign(data.predicate)
    } else {
        PredicateWithSameSign::int(data.predicate)
    };
    Some(ComparePartsOf {
        predicate,
        lhs: value_from_slot(value, data.lhs.get()),
        rhs: value_from_slot(value, data.rhs.get()),
    })
}

/// The parts of an `fcmp`.
fn float_compare_parts<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<ComparePartsOf<'ctx, B>> {
    let InstructionKindData::Fcmp(data) = instruction_kind(value)? else {
        return None;
    };
    Some(ComparePartsOf {
        predicate: PredicateWithSameSign::float(data.predicate),
        lhs: value_from_slot(value, data.lhs.get()),
        rhs: value_from_slot(value, data.rhs.get()),
    })
}

/// Replace the predicate of `flagged`, keeping its `samesign` claim.
fn with_predicate(
    flagged: PredicateWithSameSign,
    predicate: CmpPredicate,
) -> PredicateWithSameSign {
    match (predicate, flagged.has_same_sign()) {
        (CmpPredicate::Int(predicate), true) => PredicateWithSameSign::int_same_sign(predicate),
        (CmpPredicate::Int(predicate), false) => PredicateWithSameSign::int(predicate),
        (CmpPredicate::Float(predicate), _) => PredicateWithSameSign::float(predicate),
    }
}

/// The operand of `xor X, -1`. Ports `m_Not`.
fn not_operand<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Xor(data) = instruction_kind(value)? else {
        return None;
    };
    let (lhs, rhs) = binary_operands(value, data);
    if constant_int(rhs).is_some_and(|c| c.is_all_ones()) {
        return Some(lhs);
    }
    constant_int(lhs)
        .is_some_and(|c| c.is_all_ones())
        .then_some(rhs)
}

/// The source of a `trunc nuw`. Ports `m_NUWTrunc`.
fn nuw_trunc_source<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Cast(data) = instruction_kind(value)? else {
        return None;
    };
    (data.kind == CastOpcode::Trunc && data.nuw.get())
        .then(|| value_from_slot(value, data.src.get()))
}

/// The source of a `ptrtoint` or `ptrtoaddr`. Ports `m_PtrToIntOrAddr`.
fn ptr_to_int_or_addr_source<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Cast(data) = instruction_kind(value)? else {
        return None;
    };
    matches!(data.kind, CastOpcode::PtrToInt | CastOpcode::PtrToAddr)
        .then(|| value_from_slot(value, data.src.get()))
}

/// The two operands of a logical `and`. Ports `m_LogicalAnd`.
fn logical_and_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    logical_operands(value, true)
}

/// The two operands of a logical `or`. Ports `m_LogicalOr`.
fn logical_or_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    logical_operands(value, false)
}

/// Ports `LogicalOp_match`: the bitwise spelling on an `i1`, or the
/// poison-blocking `select` spelling — `L ? R : false` for `and`, `L ? true : R`
/// for `or`.
fn logical_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    want_and: bool,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    if !is_int_or_int_vector_of_width_one(value) {
        return None;
    }
    match instruction_kind(value)? {
        InstructionKindData::And(data) if want_and => Some(binary_operands(value, data)),
        InstructionKindData::Or(data) if !want_and => Some(binary_operands(value, data)),
        InstructionKindData::Select(data) => {
            let condition = value_from_slot(value, data.cond.get());
            // Don't match a scalar select of bool vectors.
            if condition.ty().id() != value.ty().id() {
                return None;
            }
            let true_value = value_from_slot(value, data.true_val.get());
            let false_value = value_from_slot(value, data.false_val.get());
            if want_and {
                constant_int(false_value)
                    .is_some_and(|c| c.is_zero())
                    .then_some((condition, true_value))
            } else {
                constant_int(true_value)
                    .is_some_and(|c| c.is_all_ones())
                    .then_some((condition, false_value))
            }
        }
        _ => None,
    }
}

/// Whether `value` is `sub nsw lhs, rhs`. Ports
/// `m_NSWSub(m_Specific(L0), m_Specific(L1))`.
fn nsw_sub_of<'ctx, B: ModuleBrand + 'ctx>(
    value: &CompareOperand<'ctx, B>,
    lhs: &CompareOperand<'ctx, B>,
    rhs: &CompareOperand<'ctx, B>,
) -> bool {
    let (Some(value), Some(lhs), Some(rhs)) = (value.value(), lhs.value(), rhs.value()) else {
        return false;
    };
    let Some(InstructionKindData::Sub(data)) = instruction_kind(value) else {
        return false;
    };
    data.no_signed_wrap && data.lhs.get() == lhs.slot() && data.rhs.get() == rhs.slot()
}

/// The operands of a `sub`. Ports `m_Sub(m_Value(A), m_Value(B))`.
fn sub_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    let InstructionKindData::Sub(data) = instruction_kind(value)? else {
        return None;
    };
    Some(binary_operands(value, data))
}

/// Whether `value` is `add` of `a` and `b` in either order. Ports
/// `m_c_Add(m_Specific(L1), m_Specific(R1))`.
fn is_commutative_add_of<'ctx, B: ModuleBrand + 'ctx>(
    value: &CompareOperand<'ctx, B>,
    a: &CompareOperand<'ctx, B>,
    b: &CompareOperand<'ctx, B>,
) -> bool {
    let (Some(value), Some(a), Some(b)) = (value.value(), a.value(), b.value()) else {
        return false;
    };
    let Some(InstructionKindData::Add(data)) = instruction_kind(value) else {
        return false;
    };
    let (lhs, rhs) = (data.lhs.get(), data.rhs.get());
    (lhs == a.slot() && rhs == b.slot()) || (lhs == b.slot() && rhs == a.slot())
}

/// The constant `C` when `value` is `expected +nsw C` or `expected | C`. Ports
/// the two `m_NSWAdd` / `m_Or` alternatives of the `ICMP_SLE` arm, both of which
/// upstream writes non-commutatively.
fn nsw_add_or_or_constant<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    expected: Value<'ctx, B>,
) -> Option<ApInt> {
    let data = match instruction_kind(value)? {
        InstructionKindData::Add(data) if data.no_signed_wrap => data,
        InstructionKindData::Or(data) => data,
        _ => return None,
    };
    (data.lhs.get() == expected.slot())
        .then(|| constant_int(value_from_slot(value, data.rhs.get())))?
}

/// The other operand when `value` is `add nuw` involving `expected`. Ports
/// `m_c_Add(m_Specific(LHS), m_Value())` guarded by `hasNoUnsignedWrap`.
fn nuw_add_over<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    expected: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Add(data) = instruction_kind(value)? else {
        return None;
    };
    data.no_unsigned_wrap
        .then(|| commutative_other_operand(value, data, expected))?
}

/// The other operand when `value` is `or` involving `expected`. Ports
/// `m_c_Or(m_Specific(LHS), m_Value())`.
fn or_over<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    expected: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Or(data) = instruction_kind(value)? else {
        return None;
    };
    commutative_other_operand(value, data, expected)
}

/// The other operand when `value` is `and` involving `expected`. Ports
/// `m_c_And(m_Specific(RHS), m_Value())`.
fn and_over<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    expected: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::And(data) = instruction_kind(value)? else {
        return None;
    };
    commutative_other_operand(value, data, expected)
}

/// Whether `value` is `lshr expected, V` for any V. Ports
/// `m_LShr(m_Specific(RHS), m_Value())`.
fn lshr_of<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>, expected: Value<'ctx, B>) -> bool {
    matches!(
        instruction_kind(value),
        Some(InstructionKindData::Lshr(data)) if data.lhs.get() == expected.slot()
    )
}

/// The divisor when `value` is `udiv expected, C`. Ports
/// `m_UDiv(m_Specific(RHS), m_APInt(C))`.
fn udiv_of_by_constant<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    expected: Value<'ctx, B>,
) -> Option<ApInt> {
    let InstructionKindData::Udiv(data) = instruction_kind(value)? else {
        return None;
    };
    (data.lhs.get() == expected.slot())
        .then(|| constant_int(value_from_slot(value, data.rhs.get())))?
}

/// The base and constant when `value` is `X +nsw C` or `or disjoint X, C`.
/// Ports `m_NSWAddLike(m_Value(X), m_APInt(C))`.
fn nsw_add_like_constant<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, ApInt)> {
    add_like_constant(value, |data| data.no_signed_wrap)
}

/// The base and constant when `value` is `X +nuw C` or `or disjoint X, C`.
/// Ports `m_NUWAddLike(m_Value(X), m_APInt(C))`.
fn nuw_add_like_constant<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, ApInt)> {
    add_like_constant(value, |data| data.no_unsigned_wrap)
}

/// Shared body of the two `*AddLike` matchers: an `add` carrying the wanted
/// no-wrap flag, or an `or disjoint`, which cannot wrap either way.
fn add_like_constant<'ctx, B, F>(
    value: Value<'ctx, B>,
    wraps_ok: F,
) -> Option<(Value<'ctx, B>, ApInt)>
where
    B: ModuleBrand + 'ctx,
    F: FnOnce(&BinaryOpData) -> bool,
{
    let data = match instruction_kind(value)? {
        InstructionKindData::Add(data) if wraps_ok(data) => data,
        InstructionKindData::Or(data) if data.disjoint => data,
        _ => return None,
    };
    let (lhs, rhs) = binary_operands(value, data);
    Some((lhs, constant_int(rhs)?))
}

/// The operand that is not `expected`, for a commutative binary operator.
fn commutative_other_operand<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
    expected: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    if data.lhs.get() == expected.slot() {
        return Some(value_from_slot(anchor, data.rhs.get()));
    }
    (data.rhs.get() == expected.slot()).then(|| value_from_slot(anchor, data.lhs.get()))
}

/// Both operands of a binary operator, as values.
fn binary_operands<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &BinaryOpData,
) -> (Value<'ctx, B>, Value<'ctx, B>) {
    (
        value_from_slot(anchor, data.lhs.get()),
        value_from_slot(anchor, data.rhs.get()),
    )
}

/// A zero as wide as `value`'s scalar integer type.
///
/// Ports the `ConstantInt::get(V->getType(), 0)` upstream builds for the two
/// `m_NUWTrunc` arms — as a [`CompareOperand::Literal`], because minting a
/// constant would mean an analysis editing the IR it was asked about.
fn zero_like<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<CompareOperand<'ctx, B>> {
    let TypeKind::Integer { bits } = scalar_kind(value)? else {
        return None;
    };
    Some(CompareOperand::Literal(ApInt::zero(bits)))
}

/// Whether `value` is a constant. Ports `m_ImmConstant` at the granularity
/// llvmkit models — every constant here is materialised, so there is no
/// `ConstantExpr` to exclude.
fn is_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    matches!(value.data().kind, ValueKindData::Constant(_))
}

/// The scalar integer constant `value` holds.
fn constant_int<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApInt> {
    let TypeKind::Integer { bits } = value.ty().kind() else {
        return None;
    };
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Int(words)) => Some(ApInt::from_words(bits, words)),
        _ => None,
    }
}

/// Whether the value's type is a vector.
fn is_vector<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    value.ty().data().as_vector().is_some()
}

/// Whether the value's type is `i1` or a vector of `i1`. Ports
/// `Type::isIntOrIntVectorTy(1)`.
fn is_int_or_int_vector_of_width_one<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    matches!(scalar_kind(value), Some(TypeKind::Integer { bits: 1 }))
}

/// The kind of the value's scalar type, peeling one vector layer.
fn scalar_kind<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<TypeKind> {
    let ty = value.ty();
    Some(match ty.data().as_vector() {
        Some((element, _, _)) => crate::r#type::Type::new(element, ty.module()).kind(),
        None => ty.kind(),
    })
}

/// The instruction payload behind `value`, or `None` when it is not one.
fn instruction_kind<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<&'ctx InstructionKindData> {
    match &value.data().kind {
        ValueKindData::Instruction(instruction) => Some(&instruction.kind),
        _ => None,
    }
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
