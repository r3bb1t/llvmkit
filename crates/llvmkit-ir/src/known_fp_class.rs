//! Which floating-point classes a value can belong to.
//!
//! Ports `llvm::computeKnownFPClass` (`llvm/lib/Analysis/ValueTracking.cpp`)
//! over the [`KnownFpClass`] lattice. It stands to [`fp_class`](crate::fp_class)
//! as [`compute_known_bits`](crate::value_tracking::compute_known_bits) stands
//! to [`KnownBits`].
//!
//! # What is not modeled, and why
//!
//! Upstream's dispatch is one `switch` over ~30 opcodes and ~40 intrinsics.
//! **The arms below are not consulted yet**, and each one's absence leaves the
//! answer at `fcAllFlags` for that shape — "could be anything", which is the
//! conservative direction, so no caller is misled. They are listed rather than
//! silently missing so the gap is legible:
//!
//! - **The remaining intrinsics** — `sin`, `cos`, `powi`, `ldexp`, `frexp`,
//!   `arithmetic_fence`, `vector_reverse`, `fptrunc_round`, and every
//!   `experimental_constrained_*` / target-specific (`amdgcn_*`) variant.
//!
//! The `bitcast` arm used to diverge: it discarded `depth` and entered known
//! bits as a fresh top-level query, so a deep chain was answered more precisely
//! here than upstream answers it. It threads `depth + 1` onto the shared budget
//! now, as `computeKnownBits(Src, DemandedElts, Bits, Q, Depth + 1)` does.
//!
//! What *is* here: the constant and poison leaves, the fast-math-flag
//! refinement, `nofpclass` on a call return or a parameter, the context arm
//! (assumptions, dominating branches and an injected condition, through
//! [`fp_predicate`](crate::fp_predicate)), `select`, `fneg`, the arithmetic
//! arms `fadd` / `fsub` / `fmul` / `fdiv` / `frem`, the vector arms
//! `extractelement` / `insertelement` / `shufflevector` / `extractvalue` /
//! `bitcast` / `phi`, the `fabs`/`copysign`/`canonicalize`/`sqrt` intrinsics,
//! `fma`/`fmuladd`, the six min/max intrinsics, the four reducing min/max
//! intrinsics, the seven rounding intrinsics, the `exp` and `log` families,
//! `fpext`, `fptrunc`, `sitofp` and `uitofp`.
//!
//! **`fdiv` and `frem` have no upstream unit test.**
//! `ComputeKnownFPClassTest` covers `FAdd`, `FSub`, `FMul` and `FMulNoZero`
//! and stops there, so those two arms are ported from the implementation with
//! no fixture of their own to check them against — worth knowing before
//! trusting a subtle answer from either.

use crate::ap_float::{ApFloatSemantics, ApFloatSign, BinaryExponent};
use crate::assumptions::{AssumptionSource, is_valid_assume_for_context};
use crate::attributes::AttrIndex;
use crate::cmp_predicate::{FloatPredicate, IntPredicate};
use crate::constant::ConstantData;
use crate::denormal_mode::{DenormalMode, DenormalModeKind};
use crate::fmf::FastMathFlags;
use crate::fp_class::{FpClassTest, KnownFpClass, MinMaxKind};
use crate::fp_predicate::{denormal_mode_of, enclosing_function_of, fcmp_implies_class};
use crate::instr_types::{
    BranchKind, CastOpcode, ExtractElementInstData, InsertElementInstData, PhiData,
    ShuffleVectorInstData,
};
use crate::instruction::{InstructionKindData, InstructionView};
use crate::intrinsics::descriptor_for_callee;
use crate::known_bits::KnownBits;
use crate::module::{ModuleBrand, ModuleRef};
use crate::r#type::{Type, TypeKind};
use crate::r#use::Use;
use crate::value::{Value, ValueKindData, ValueSlot};
use crate::value_tracking::{
    MAX_ANALYSIS_RECURSION_DEPTH, ValueTrackingQuery, assume_argument, compute_known_bits_at_depth,
    is_known_not_undef, is_sign_bit_check, logical_op_parts, not_operand, parent_block,
    shuffle_source_demands,
};
use crate::vector_utils::splat_value;
use crate::{ApFloat, ApInt};

/// Which floating-point classes `value` may belong to.
///
/// Ports the `(const Value *V, FPClassTest InterestedClasses, const
/// SimplifyQuery &SQ, unsigned Depth)` overload of `llvm::computeKnownFPClass`.
///
/// `interested_classes` is upstream's compile-time hint: an arm may skip work
/// for a class nobody asked about. Upstream's own comment is the contract —
/// "Queries not specified in `InterestedClasses` should be reliable if they are
/// determined during the query" — so passing [`FpClassTest::ALL`] is always
/// correct and only ever slower.
pub fn compute_known_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> KnownFpClass {
    known_fp_class(value, interested_classes, query, 0)
}

/// [`compute_known_fp_class`] over every class.
///
/// Ports the `(const Value *V, const DataLayout &DL, ...)` overload at its
/// defaulted `InterestedClasses = fcAllFlags`.
pub fn compute_known_fp_class_all<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> KnownFpClass {
    compute_known_fp_class(value, FpClassTest::ALL, query)
}

/// [`compute_known_fp_class`] with the fast-math flags of the *use* folded in.
///
/// Ports the `(const Value *V, FastMathFlags FMF, FPClassTest, const
/// SimplifyQuery &, unsigned)` overload: a use site that carries `nnan` or
/// `ninf` can rule those out even when the definition does not.
pub fn compute_known_fp_class_with_flags<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    flags: FastMathFlags,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> KnownFpClass {
    let mut interested_classes = interested_classes;
    if flags.contains(FastMathFlags::NO_NANS) {
        interested_classes = interested_classes.difference(FpClassTest::NAN);
    }
    if flags.contains(FastMathFlags::NO_INFS) {
        interested_classes = interested_classes.difference(FpClassTest::INFINITY);
    }

    let mut known = compute_known_fp_class(value, interested_classes, query);
    if flags.contains(FastMathFlags::NO_NANS) {
        known.known_not(FpClassTest::NAN);
    }
    if flags.contains(FastMathFlags::NO_INFS) {
        known.known_not(FpClassTest::INFINITY);
    }
    known
}

/// Ports `computeKnownFPClass` at an explicit recursion depth.
fn known_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    // The constant leaves, which answer exactly and return.
    if let Some(known) = constant_fp_class(value) {
        return known;
    }

    let kind = instruction_kind(value);

    // Flags on the operator itself rule classes out regardless of the arm, and
    // upstream applies them on the way *out* — through a `scope_exit` — so an
    // arm that learns nothing still benefits.
    // Upstream's `KnownNotFromFlags` opens with the attribute, before the
    // flags: a call's return `nofpclass` or an argument's parameter one. The
    // mask *is* what is ruled out, so it joins directly.
    let mut ruled_out = no_fp_class_of(value);
    if let Some(flags) = kind.and_then(fast_math_flags) {
        if flags.contains(FastMathFlags::NO_NANS) {
            ruled_out |= FpClassTest::NAN;
        }
        if flags.contains(FastMathFlags::NO_INFS) {
            ruled_out |= FpClassTest::INFINITY;
        }
    }

    // Context-dependent facts join the flags: whatever the context proves the
    // value is *not* is ruled out on the same way out, and its sign bit fills in
    // one the arms did not determine.
    let assumed = known_fp_class_from_context(value, query);
    ruled_out |= assumed.classes().complement();

    // Nothing need be learned from inputs that the flags already settle.
    let interested_classes = interested_classes.difference(ruled_out);

    // Ports the `scope_exit` that upstream runs on every return path below.
    let finish = |mut known: KnownFpClass| {
        known.known_not(ruled_out);
        if known.sign_bit().is_none()
            && let Some(sign) = assumed.sign_bit()
        {
            if sign {
                known.sign_bit_must_be_one();
            } else {
                known.sign_bit_must_be_zero();
            }
        }
        known
    };

    // Upstream's `if (!Op) return;`, which sits *after* the context arm: a value
    // that is no operator at all — an argument, say — has no arm to dispatch to,
    // but the flags and the context still apply to it.
    let Some(kind) = kind else {
        return finish(KnownFpClass::unknown());
    };

    // All recursive arms must come after this.
    if depth == MAX_ANALYSIS_RECURSION_DEPTH.min(query.max_depth()) {
        return finish(KnownFpClass::unknown());
    }

    finish(dispatch(value, kind, interested_classes, query, depth))
}

/// The opcode switch. Arms not listed here leave the answer unknown; the module
/// header names each one.
fn dispatch<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    kind: &'ctx InstructionKindData,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    match kind {
        InstructionKindData::Fneg(data) => {
            let source = value_from_slot(value, data.src.get());
            let mut known = known_fp_class(source, interested_classes, query, depth + 1);
            known.negate();
            known
        }
        InstructionKindData::Cast(data) => cast_fp_class(
            value,
            data.kind,
            data.src.get(),
            interested_classes,
            query,
            depth,
        ),
        InstructionKindData::Select(data) => {
            let condition = value_from_slot(value, data.cond.get());
            let for_arm = |slot: ValueSlot, invert: bool| {
                let arm = value_from_slot(value, slot);
                let known = known_fp_class(arm, interested_classes, query, depth + 1);
                adjust_known_fp_class_for_select_arm(known, condition, arm, invert, query, depth)
            };
            // Only known if known in both the true and the false arm.
            for_arm(data.true_val.get(), false).intersect_with(for_arm(data.false_val.get(), true))
        }
        InstructionKindData::Fadd(data) => add_or_subtract_fp_class(
            value,
            data.lhs.get(),
            data.rhs.get(),
            true,
            interested_classes,
            query,
            depth,
        ),
        InstructionKindData::Fsub(data) => add_or_subtract_fp_class(
            value,
            data.lhs.get(),
            data.rhs.get(),
            false,
            interested_classes,
            query,
            depth,
        ),
        InstructionKindData::Fmul(data) => {
            multiply_fp_class(value, data.lhs.get(), data.rhs.get(), query, depth)
        }
        InstructionKindData::Fdiv(data) => divide_or_remainder_fp_class(
            value,
            data.lhs.get(),
            data.rhs.get(),
            true,
            interested_classes,
            query,
            depth,
        ),
        InstructionKindData::Frem(data) => divide_or_remainder_fp_class(
            value,
            data.lhs.get(),
            data.rhs.get(),
            false,
            interested_classes,
            query,
            depth,
        ),
        InstructionKindData::ExtractElement(data) => {
            extract_element_fp_class(value, data, interested_classes, query, depth)
        }
        InstructionKindData::InsertElement(data) => {
            insert_element_fp_class(value, data, interested_classes, query, depth)
        }
        InstructionKindData::ShuffleVector(data) => {
            shuffle_vector_fp_class(value, data, interested_classes, query, depth)
        }
        InstructionKindData::ExtractValue(data) => {
            // Upstream first looks through a `frexp` result at index 0; that
            // intrinsic is not modeled, so what remains is the fallthrough,
            // which forwards to the aggregate operand.
            known_fp_class(
                value_from_slot(value, data.aggregate.get()),
                interested_classes,
                query,
                depth + 1,
            )
        }
        InstructionKindData::Phi(data) => {
            phi_fp_class(value, data, interested_classes, query, depth)
        }
        InstructionKindData::Call(_) => {
            intrinsic_fp_class(value, kind, interested_classes, query, depth)
        }
        _ => KnownFpClass::unknown(),
    }
}

/// The `ExtractElement` arm.
///
/// Ports `case Instruction::ExtractElement:`: a constant, in-range index
/// demands only the lane it names; anything else demands them all.
fn extract_element_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &ExtractElementInstData,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    let vector = value_from_slot(value, data.vector.get());
    let Some((lanes, false)) = vector_lanes(vector) else {
        // Upstream's `else` branch: a non-fixed-vector operand demands the one
        // element it has.
        return known_fp_class(vector, interested_classes, query, depth + 1);
    };
    let index = value_from_slot(value, data.index.get());
    let demanded = constant_lane_index(index, lanes).map_or_else(
        || ApInt::all_ones(lanes),
        |index| ApInt::one_bit_set(lanes, index),
    );
    let subquery = query.with_temporary_demanded_elements(&demanded);
    known_fp_class(vector, interested_classes, &subquery, depth + 1)
}

/// The `InsertElement` arm.
///
/// Ports `case Instruction::InsertElement:`: the answer is the union of the
/// inserted element and whatever lanes of the source vector are still demanded
/// after the inserted lane is cleared.
fn insert_element_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &InsertElementInstData,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    // Upstream returns immediately for a scalable vector.
    let Some((lanes, false)) = vector_lanes(value) else {
        return KnownFpClass::unknown();
    };
    let demanded = demanded_lanes(query, lanes);
    let mut demanded_vector = demanded.clone();
    let mut needs_element = true;
    if let Some(index) = constant_lane_index(value_from_slot(value, data.index.get()), lanes) {
        demanded_vector.clear_bit(index);
        needs_element = demanded.bit(index);
    }

    let mut known = if needs_element {
        let element = known_fp_class(
            value_from_slot(value, data.value.get()),
            interested_classes,
            query,
            depth + 1,
        );
        // Upstream's early out: nothing more to learn once the element alone
        // is unknown.
        if element.is_unknown() {
            return element;
        }
        element
    } else {
        KnownFpClass::from_classes(FpClassTest::NONE)
    };

    if !demanded_vector.is_zero() {
        let subquery = query.with_temporary_demanded_elements(&demanded_vector);
        known.union_in_place(known_fp_class(
            value_from_slot(value, data.vector.get()),
            interested_classes,
            &subquery,
            depth + 1,
        ));
    }
    known
}

/// Whether `semantics` is one of the IEEE-like formats upstream's
/// `Type::isIEEELikeFPTy` accepts.
///
/// The exclusions matter: `x86_fp80` carries an explicit integer bit, so the
/// "all exponent bits plus one fraction bit set means NaN" reasoning below
/// does not hold for it, and upstream says so in a note. `ppc_fp128` is a
/// double-double pair rather than a single IEEE field layout.
fn is_ieee_like(semantics: ApFloatSemantics) -> bool {
    match semantics {
        ApFloatSemantics::IeeeHalf
        | ApFloatSemantics::Bfloat
        | ApFloatSemantics::IeeeSingle
        | ApFloatSemantics::IeeeDouble
        | ApFloatSemantics::IeeeQuad => true,
        ApFloatSemantics::X87DoubleExtended | ApFloatSemantics::PpcDoubleDouble => false,
    }
}

/// The `BitCast` arm.
///
/// Ports `case Instruction::BitCast:`, which is the one arm that reasons in
/// *integer* terms: it runs known bits over an element-wise bitcast of an
/// integer source and reads the float's fields back out of them.
///
/// Upstream's `m_ElementWiseBitCast` requires the cast not to change the
/// element count, so a `<2 x float>` to `i64` bitcast — which reinterprets
/// lanes — is declined rather than misread.
fn bitcast_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    source: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    // `m_ElementWiseBitCast`: same lane count on both sides, and the source
    // must be an integer or integer vector.
    if vector_lanes(value) != vector_lanes(source) {
        return KnownFpClass::unknown();
    }
    if !matches!(scalar_kind(source.ty()), TypeKind::Integer { .. }) {
        return KnownFpClass::unknown();
    }
    let Some(semantics) = scalar_semantics(value.ty()) else {
        return KnownFpClass::unknown();
    };

    // `computeKnownBits(Src, DemandedElts, Bits, Q, Depth + 1)` — the shared
    // budget, so a bitcast reached late in the FP walk hands known bits a query
    // already at the recursion limit.
    let Ok(bits) = compute_known_bits_at_depth(source, query, depth + 1) else {
        return KnownFpClass::unknown();
    };

    let mut known = KnownFpClass::unknown();
    // The sign bit transfers directly.
    if bits.is_non_negative() {
        known.sign_bit_must_be_zero();
    } else if bits.is_negative() {
        known.sign_bit_must_be_one();
    }

    if !is_ieee_like(semantics) {
        return known;
    }

    // An IEEE float is NaN when every exponent bit and at least one fraction
    // bit is set. So: reading the unknown bits as 0 and still getting a NaN
    // means it is always a NaN; reading them as 1 and *not* getting a NaN
    // means it never is.
    if ApFloat::from_bits(semantics, bits.one_mask()).is_ok_and(|float| float.is_nan()) {
        known = KnownFpClass::from_classes(FpClassTest::NAN);
    } else if ApFloat::from_bits(semantics, &!bits.zero_mask()).is_ok_and(|float| !float.is_nan()) {
        known.known_not(FpClassTest::NAN);
    }

    // Infinity and zero are single bit patterns up to sign, so comparing
    // against them with the sign bit masked out settles both directions.
    for (pattern, class) in [
        (
            ApFloat::inf(semantics, ApFloatSign::Positive).to_bits(),
            FpClassTest::INFINITY,
        ),
        (
            ApFloat::zero(semantics, ApFloatSign::Positive).to_bits(),
            FpClassTest::ZERO,
        ),
    ] {
        // `makeConstant` then `Zero.clearSignBit()`: every bit is pinned to the
        // pattern except the sign, which is left unknown so the comparison
        // ignores it.
        let mut zero = !&pattern;
        zero.clear_sign_bit();
        let Ok(reference) = KnownBits::from_zero_one(zero, pattern) else {
            continue;
        };
        match KnownBits::eq(&bits, &reference) {
            // A definite answer here is always `false`: the sign bit was
            // cleared from the reference's zero mask, so an exact match
            // cannot be proven this way, only a mismatch.
            Some(_) => known.known_not(class),
            None if bits == reference => known = KnownFpClass::from_classes(class),
            None => {}
        }
    }

    known
}

/// The `ShuffleVector` arm.
///
/// Ports `case Instruction::ShuffleVector:`: the union of whatever lanes the
/// mask takes from each source. A poison lane among the demanded ones says
/// nothing about the result's common state, which is
/// [`shuffle_demanded_elements`]' `None`.
///
/// The `getSplatValue` fast path runs first, as upstream's does. It is what
/// keeps a splat mask carrying poison lanes — `<0, poison, 0, 0>`, which
/// `m_ZeroMask` accepts — from being answered as "nothing known": the
/// demanded-lane path below sees a demanded poison lane and gives up, while
/// the splat match reads straight through to the scalar.
fn shuffle_vector_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &ShuffleVectorInstData,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    // Handle vector splat idiom.
    //
    // Upstream recurses through the `computeKnownFPClass` overload that takes
    // no `DemandedElts`, resetting the demanded set. `query` is passed along
    // unchanged here because the answer is the same: the splat is a scalar, and
    // `demanded_elements_for` yields `None` for anything that is not a vector.
    if let Some(splat) = splat_value(value) {
        return known_fp_class(splat, interested_classes, query, depth + 1);
    }

    let Some((lhs, lhs_demand, rhs, rhs_demand)) =
        shuffle_source_demands(value, data, query, false)
    else {
        return KnownFpClass::unknown();
    };

    let mut known = if lhs_demand.is_zero() {
        KnownFpClass::from_classes(FpClassTest::NONE)
    } else {
        let subquery = query.with_temporary_demanded_elements(&lhs_demand);
        let left = known_fp_class(lhs, interested_classes, &subquery, depth + 1);
        // Upstream's early out.
        if left.is_unknown() {
            return left;
        }
        left
    };

    if !rhs_demand.is_zero() {
        let subquery = query.with_temporary_demanded_elements(&rhs_demand);
        known.union_in_place(known_fp_class(
            rhs,
            interested_classes,
            &subquery,
            depth + 1,
        ));
    }
    known
}

/// The `PHI` arm.
///
/// Ports `case Instruction::PHI:`: the union over the incoming values, with
/// direct self references skipped and the recursion capped two levels below
/// the general limit, because a loop would otherwise be walked repeatedly for
/// no gain.
fn phi_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &PhiData,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    // Unreachable blocks may have zero-operand phi nodes.
    let incoming: Vec<ValueSlot> = data
        .incoming
        .borrow()
        .iter()
        .map(|(incoming, _)| incoming.get())
        .collect();
    if incoming.is_empty() {
        return KnownFpClass::unknown();
    }

    let recursion_limit = MAX_ANALYSIS_RECURSION_DEPTH.saturating_sub(2);
    if depth >= recursion_limit {
        return KnownFpClass::unknown();
    }

    // `None` until the first non-self incoming is folded in. A phi whose every
    // incoming is a self reference leaves it `None` and answers unknown, which
    // is where upstream's `hasConstantValue()` undef guard lands too.
    let mut result: Option<KnownFpClass> = None;
    for slot in incoming {
        // Skip direct self references.
        if slot == value.slot() {
            continue;
        }
        // Upstream recurses *at* the limit rather than at `depth + 1`, which
        // is what caps the walk at two levels regardless of how deep the phi
        // itself sits.
        let source = known_fp_class(
            value_from_slot(value, slot),
            interested_classes,
            query,
            recursion_limit,
        );
        result = Some(match result {
            Some(known) => known.union_with(source),
            None => source,
        });
        if result.is_some_and(|known| known.classes() == FpClassTest::ALL) {
            break;
        }
    }
    result.unwrap_or_else(KnownFpClass::unknown)
}

/// The lane count and scalability of `value`'s type, or `None` for a scalar.
fn vector_lanes<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<(u32, bool)> {
    value
        .ty()
        .data()
        .as_vector()
        .map(|(_, lanes, scalable)| (lanes, scalable))
}

/// A constant, in-range lane index, if the operand is one.
fn constant_lane_index<'ctx, B: ModuleBrand + 'ctx>(
    index: Value<'ctx, B>,
    lanes: u32,
) -> Option<u32> {
    let ValueKindData::Constant(ConstantData::Int(words)) = &index.data().kind else {
        return None;
    };
    let TypeKind::Integer { bits } = index.ty().kind() else {
        return None;
    };
    ApInt::from_words(bits, words)
        .try_zext_u64()
        .and_then(|index| u32::try_from(index).ok())
        .filter(|index| *index < lanes)
}

/// The lanes this query demands at `lanes` wide, defaulting to all of them.
fn demanded_lanes<'a, 'ctx, B: ModuleBrand + 'ctx>(
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    lanes: u32,
) -> ApInt {
    query
        .demanded_elements()
        .filter(|demanded| demanded.bit_width() == lanes)
        .cloned()
        .unwrap_or_else(|| ApInt::all_ones(lanes))
}

/// The classes a `nofpclass` attribute rules out for `value`.
///
/// Ports the two reads that open `computeKnownFPClass`'s `KnownNotFromFlags`:
/// `CallBase::getRetNoFPClass` for a call, and `Argument::getNoFPClass` for a
/// parameter. Anything else carries no such attribute, which is
/// [`FpClassTest::NONE`] — nothing ruled out.
fn no_fp_class_of<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> FpClassTest {
    let mask = match &value.data().kind {
        ValueKindData::Argument { parent_fn, slot } => {
            function_no_fp_class(value, *parent_fn, AttrIndex::Param(*slot))
        }
        ValueKindData::Instruction(instruction) => match &instruction.kind {
            InstructionKindData::Call(call) => {
                // Only a direct call names a function whose return attributes
                // can be read; upstream's `getRetNoFPClass` answers the empty
                // mask for an indirect one too.
                function_no_fp_class(value, call.callee.get(), AttrIndex::Return)
            }
            _ => None,
        },
        _ => None,
    };
    mask.unwrap_or(FpClassTest::NONE)
}

/// The `nofpclass` mask at `index` on the function in `function_slot`, if that
/// slot really holds a function and the attribute is present.
fn function_no_fp_class<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    function_slot: ValueSlot,
    index: AttrIndex,
) -> Option<FpClassTest> {
    let function = value_from_slot(anchor, function_slot);
    let ValueKindData::Function(data) = &function.data().kind else {
        return None;
    };
    data.attributes.borrow().no_fp_class(index)
}

/// Whether `value` is provably not `undef`.
///
/// Ports the `isGuaranteedNotToBeUndef` guard the arithmetic arms put on their
/// "both operands are the same value" special cases — without it, `fadd x, x`
/// on an `undef` `x` is not `2 * x`, because each use may read a different
/// value.
///
/// `is_known_not_undef` is fallible where `computeKnownFPClass` is not, so an
/// error answers `false`: the special case is skipped and the arm falls back to
/// the general operand-by-operand reasoning, which is the weaker answer and
/// never the wrong one.
fn is_definitely_not_undef<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> bool {
    is_known_not_undef(value, query).unwrap_or(false)
}

/// The `FAdd` and `FSub` arms.
///
/// Ports the shared `case Instruction::FAdd: case Instruction::FSub:` block of
/// `computeKnownFPClassFromOperator`.
fn add_or_subtract_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    lhs_slot: ValueSlot,
    rhs_slot: ValueSlot,
    is_add: bool,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    let mut known = KnownFpClass::unknown();
    let want_negative =
        is_add && interested_classes.intersects(KnownFpClass::ORDERED_LESS_THAN_ZERO);
    let want_nan = interested_classes.intersects(FpClassTest::NAN);
    let want_negative_zero = interested_classes.intersects(FpClassTest::NEGATIVE_ZERO);

    if !want_nan && !want_negative && !want_negative_zero {
        return known;
    }

    let mut interested_sources = interested_classes;
    if want_negative {
        interested_sources |= KnownFpClass::ORDERED_LESS_THAN_ZERO;
    }
    if interested_classes.intersects(FpClassTest::NAN) {
        interested_sources |= FpClassTest::INFINITY;
    }

    let lhs = value_from_slot(value, lhs_slot);
    let rhs = value_from_slot(value, rhs_slot);
    let known_rhs = known_fp_class(rhs, interested_sources, query, depth + 1);

    // `fadd x, x` is the canonical form of `fmul x, 2`.
    let self_add = lhs_slot == rhs_slot && is_definitely_not_undef(lhs, query);
    let mut known_lhs = if self_add {
        known_rhs
    } else {
        KnownFpClass::unknown()
    };

    if !((want_nan && known_rhs.is_known_never_nan())
        || (want_negative && known_rhs.cannot_be_ordered_less_than_zero())
        || want_negative_zero
        || !is_add)
    {
        return known;
    }

    if !self_add {
        // The right-hand side is canonically cheaper to compute, so the
        // left-hand side is only inspected once there is a point.
        known_lhs = known_fp_class(lhs, interested_sources, query, depth + 1);
    }

    // Adding positive and negative infinity produces NaN.
    if known_lhs.is_known_never_nan()
        && known_rhs.is_known_never_nan()
        && (known_lhs.is_known_never_infinity() || known_rhs.is_known_never_infinity())
    {
        known.known_not(FpClassTest::NAN);
    }

    if is_add {
        if known_lhs.cannot_be_ordered_less_than_zero()
            && known_rhs.cannot_be_ordered_less_than_zero()
        {
            known.known_not(KnownFpClass::ORDERED_LESS_THAN_ZERO);
        }
        if known_lhs.cannot_be_ordered_greater_than_zero()
            && known_rhs.cannot_be_ordered_greater_than_zero()
        {
            known.known_not(KnownFpClass::ORDERED_GREATER_THAN_ZERO);
        }
    }

    let Some(mode) = scalar_denormal_mode(value) else {
        return known;
    };

    if is_add {
        // Doubling zero gives the same zero.
        if self_add
            && known_rhs.is_known_never_logical_positive_zero(mode)
            && match mode.output() {
                DenormalModeKind::Ieee => true,
                DenormalModeKind::PreserveSign => known_rhs.is_known_never_positive_subnormal(),
                DenormalModeKind::PositiveZero => known_rhs.is_known_never_subnormal(),
                DenormalModeKind::Dynamic => false,
            }
        {
            known.known_not(FpClassTest::POSITIVE_ZERO);
        }

        // `fadd x, 0.0` returns `+0.0`, never `-0.0`.
        if (known_lhs.is_known_never_logical_negative_zero(mode)
            || known_rhs.is_known_never_logical_negative_zero(mode))
            // A negative denormal output must not be able to flush to `-0`.
            && matches!(
                mode.output(),
                DenormalModeKind::Ieee | DenormalModeKind::PositiveZero
            )
        {
            known.known_not(FpClassTest::NEGATIVE_ZERO);
        }
    } else if (known_lhs.is_known_never_logical_negative_zero(mode)
        || known_rhs.is_known_never_logical_positive_zero(mode))
        && matches!(
            mode.output(),
            DenormalModeKind::Ieee | DenormalModeKind::PositiveZero
        )
    {
        // Only `fsub -0, +0` can return `-0`.
        known.known_not(FpClassTest::NEGATIVE_ZERO);
    }

    known
}

/// The `FMul` arm.
///
/// Ports `case Instruction::FMul:`, which does its work in
/// `KnownFPClass::fmul` and `KnownFPClass::square` — both already ported — and
/// adds the denormal-scaling refinement on a constant right-hand side.
fn multiply_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    lhs_slot: ValueSlot,
    rhs_slot: ValueSlot,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    let mode = scalar_denormal_mode(value).unwrap_or_else(DenormalMode::dynamic);
    let lhs = value_from_slot(value, lhs_slot);
    let rhs = value_from_slot(value, rhs_slot);

    // `x * x` is non-negative or NaN. Upstream carries a FIXME that this
    // should check `isGuaranteedNotToBeUndef`; it does not, and neither does
    // this, because squaring an `undef` is still non-negative-or-NaN whichever
    // values the two uses read.
    if lhs_slot == rhs_slot {
        let known_source = known_fp_class(lhs, FpClassTest::ALL, query, depth + 1);
        return KnownFpClass::square(known_source, mode);
    }

    // A constant right-hand side whose exponent is at least the mantissa width
    // scales away any subnormal. Upstream's own note: this mirrors `ldexp`, and
    // a general `ConstantFPRange` analysis would subsume it.
    let mut cannot_be_subnormal = false;
    let known_rhs = match (constant_ap_float(rhs), scalar_semantics(value.ty())) {
        (Some(constant), Some(semantics)) => {
            let mantissa_bits = i32::try_from(semantics.precision().saturating_sub(1)).unwrap_or(0);
            // Upstream compares `ilogb`'s `int`, whose out-of-band answers are
            // sentinels at the extremes: `IEK_Inf` is `INT_MAX` and clears any
            // threshold, while `IEK_Zero` and `IEK_NaN` sit at the `INT_MIN`
            // end and clear none. [`BinaryExponent`] spells those as variants,
            // so the comparison has to name them.
            cannot_be_subnormal = match constant.ilogb() {
                BinaryExponent::Finite(exponent) => exponent >= mantissa_bits,
                // `x * inf` is an infinity or a NaN, never a subnormal.
                BinaryExponent::Infinity => true,
                BinaryExponent::Zero | BinaryExponent::Nan => false,
            };
            KnownFpClass::of(&constant)
        }
        _ => known_fp_class(rhs, FpClassTest::ALL, query, depth + 1),
    };
    let known_lhs = known_fp_class(lhs, FpClassTest::ALL, query, depth + 1);

    let mut known = KnownFpClass::fmul(known_lhs, known_rhs, mode);
    if cannot_be_subnormal {
        known.known_not(FpClassTest::SUBNORMAL);
    }
    known
}

/// The `FDiv` and `FRem` arms.
///
/// Ports the shared `case Instruction::FDiv: case Instruction::FRem:` block of
/// `computeKnownFPClassFromOperator`.
fn divide_or_remainder_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    lhs_slot: ValueSlot,
    rhs_slot: ValueSlot,
    is_divide: bool,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    let mut known = KnownFpClass::unknown();
    let want_nan = interested_classes.intersects(FpClassTest::NAN);
    let lhs = value_from_slot(value, lhs_slot);
    let rhs = value_from_slot(value, rhs_slot);

    if lhs_slot == rhs_slot && is_definitely_not_undef(lhs, query) {
        // `x / x` is exactly `1.0` or NaN; `x % x` is exactly `±0.0` or NaN.
        known = KnownFpClass::from_classes(if is_divide {
            FpClassTest::NAN | FpClassTest::POSITIVE_NORMAL
        } else {
            FpClassTest::NAN | FpClassTest::ZERO
        });
        if !want_nan {
            return known;
        }

        let known_source = known_fp_class(
            lhs,
            FpClassTest::NAN | FpClassTest::INFINITY | FpClassTest::ZERO | FpClassTest::SUBNORMAL,
            query,
            depth + 1,
        );
        let mode = scalar_denormal_mode(value).unwrap_or_else(DenormalMode::dynamic);
        if known_source.is_known_never_infinity_or_nan()
            && known_source.is_known_never_logical_zero(mode)
        {
            known.known_not(FpClassTest::NAN);
        } else if known_source.is_known_never(FpClassTest::SIGNALING_NAN) {
            known.known_not(FpClassTest::SIGNALING_NAN);
        }
        return known;
    }

    let want_negative = interested_classes.intersects(FpClassTest::NEGATIVE);
    let want_positive = !is_divide && interested_classes.intersects(FpClassTest::POSITIVE);
    if !want_nan && !want_negative && !want_positive {
        return known;
    }

    let known_rhs = known_fp_class(
        rhs,
        FpClassTest::NAN | FpClassTest::INFINITY | FpClassTest::ZERO | FpClassTest::NEGATIVE,
        query,
        depth + 1,
    );
    let knows_something_useful = known_rhs.is_known_never_nan()
        || known_rhs.is_known_never(FpClassTest::NEGATIVE)
        || known_rhs.is_known_never(FpClassTest::POSITIVE);

    let known_lhs = if knows_something_useful || want_positive {
        known_fp_class(lhs, FpClassTest::ALL, query, depth + 1)
    } else {
        KnownFpClass::unknown()
    };

    // Upstream reads the denormal mode through a possibly-null `Function`, and
    // every use below is guarded by that null check; `None` here is the same
    // guard.
    let mode = scalar_denormal_mode(value);

    if is_divide {
        // Only `0/0` and `Inf/Inf` produce NaN.
        if known_lhs.is_known_never_nan()
            && known_rhs.is_known_never_nan()
            && (known_lhs.is_known_never_infinity() || known_rhs.is_known_never_infinity())
            && mode.is_some_and(|mode| {
                known_lhs.is_known_never_logical_zero(mode)
                    || known_rhs.is_known_never_logical_zero(mode)
            })
        {
            known.known_not(FpClassTest::NAN);
        }

        // The sign is the exclusive-or of the operand signs: `X / -0.0` is
        // `-Inf` (or NaN), and `+X / +X` is `+X`.
        if (known_lhs.is_known_never(FpClassTest::NEGATIVE)
            && known_rhs.is_known_never(FpClassTest::NEGATIVE))
            || (known_lhs.is_known_never(FpClassTest::POSITIVE)
                && known_rhs.is_known_never(FpClassTest::POSITIVE))
        {
            known.known_not(FpClassTest::NEGATIVE);
        }
        if (known_lhs.is_known_never(FpClassTest::POSITIVE)
            && known_rhs.is_known_never(FpClassTest::NEGATIVE))
            || (known_lhs.is_known_never(FpClassTest::NEGATIVE)
                && known_rhs.is_known_never(FpClassTest::POSITIVE))
        {
            known.known_not(FpClassTest::POSITIVE);
        }

        // `0 / x` is zero or NaN.
        if known_lhs.is_known_always(FpClassTest::ZERO) {
            known.known_not(FpClassTest::SUBNORMAL | FpClassTest::NORMAL | FpClassTest::INFINITY);
        }
        // `x / 0` is NaN or infinity.
        if known_rhs.is_known_always(FpClassTest::ZERO) {
            known.known_not(FpClassTest::FINITE);
        }
    } else {
        // `Inf % x` and `x % 0` produce NaN.
        if known_lhs.is_known_never_nan()
            && known_rhs.is_known_never_nan()
            && known_lhs.is_known_never_infinity()
            && mode.is_some_and(|mode| known_rhs.is_known_never_logical_zero(mode))
        {
            known.known_not(FpClassTest::NAN);
        }

        // `frem` takes its sign from the first operand.
        if known_lhs.cannot_be_ordered_less_than_zero() {
            known.known_not(KnownFpClass::ORDERED_LESS_THAN_ZERO);
        }
        if known_lhs.cannot_be_ordered_greater_than_zero() {
            known.known_not(KnownFpClass::ORDERED_GREATER_THAN_ZERO);
        }
        if known_lhs.is_known_never(FpClassTest::NEGATIVE) {
            known.known_not(FpClassTest::NEGATIVE);
        }
        if known_lhs.is_known_never(FpClassTest::POSITIVE) {
            known.known_not(FpClassTest::POSITIVE);
        }
    }

    known
}

/// The `FPExt` / `FPTrunc` / `SIToFP` / `UIToFP` arms.
fn cast_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    opcode: CastOpcode,
    source_slot: ValueSlot,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    let source = value_from_slot(value, source_slot);
    match opcode {
        CastOpcode::FpExt => {
            let known_source = known_fp_class(source, interested_classes, query, depth + 1);
            let (Some(destination), Some(from)) =
                (scalar_semantics(value.ty()), scalar_semantics(source.ty()))
            else {
                return KnownFpClass::unknown();
            };
            KnownFpClass::fpext(known_source, destination, from)
        }
        CastOpcode::FpTrunc => fp_trunc_class(source, interested_classes, query, depth),
        CastOpcode::BitCast => bitcast_fp_class(value, source, query, depth),
        CastOpcode::SiToFp | CastOpcode::UiToFp => {
            let mut known = KnownFpClass::unknown();
            // An integer conversion cannot produce a NaN, and an integer is
            // never subnormal.
            known.known_not(FpClassTest::NAN);
            known.known_not(FpClassTest::SUBNORMAL);
            // Both turn a zero into `+0`.
            known.known_not(FpClassTest::NEGATIVE_ZERO);
            if opcode == CastOpcode::UiToFp {
                known.sign_bit_must_be_zero();
            }

            if interested_classes.intersects(FpClassTest::INFINITY) {
                // The magnitude of the widest integer, one bit narrower when
                // signed. Upstream's comment: this still works for a signed
                // minimum, because the largest float is scaled by a fraction
                // close to 2.0.
                let TypeKind::Integer { bits } = scalar_kind(source.ty()) else {
                    return known;
                };
                let Ok(mut integer_size) = i32::try_from(bits) else {
                    return known;
                };
                if opcode == CastOpcode::SiToFp {
                    integer_size -= 1;
                }
                // Upstream asks `ilogb(APFloat::getLargest(sem)) >= IntSize`;
                // the exponent of the largest finite value *is* `maxExponent`.
                if let Some(semantics) = scalar_semantics(value.ty())
                    && semantics.max_exponent() >= integer_size
                {
                    known.known_not(FpClassTest::INFINITY);
                }
            }
            known
        }
        _ => KnownFpClass::unknown(),
    }
}

/// Ports `computeKnownFPClassForFPTrunc`.
fn fp_trunc_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    source: Value<'ctx, B>,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    let mut known = KnownFpClass::unknown();
    if interested_classes
        .intersection(KnownFpClass::ORDERED_LESS_THAN_ZERO.union(FpClassTest::NAN))
        .is_none()
    {
        return known;
    }

    let known_source = known_fp_class(source, interested_classes, query, depth + 1);

    // The sign is preserved. Upstream's `TODO: Handle cannot be ordered greater
    // than zero` is inherited.
    if known_source.cannot_be_ordered_less_than_zero() {
        known.known_not(KnownFpClass::ORDERED_LESS_THAN_ZERO);
    }
    known.propagate_nan(known_source, true);
    // Upstream's closing comment: "Infinity needs a range check", which it does
    // not do either.
    known
}

/// The `call` arm's intrinsic switch.
fn intrinsic_fp_class<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    kind: &'ctx InstructionKindData,
    interested_classes: FpClassTest,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    let InstructionKindData::Call(data) = kind else {
        return KnownFpClass::unknown();
    };
    let callee = value_from_slot(value, data.callee.get());
    let Some(descriptor) = descriptor_for_callee(callee) else {
        return KnownFpClass::unknown();
    };
    let name = descriptor.id().base_name();
    let argument = |index: usize| -> Option<Value<'ctx, B>> {
        data.args
            .get(index)
            .map(|arg| value_from_slot(value, arg.get()))
    };
    // The enclosing function's `denormal-fp-math` for the result's element
    // type, and upstream's `getDynamic()` where there is no enclosing function.
    let mode = denormal_mode_of(value);

    match name {
        // `fma` / `fmuladd`, which only learn anything about the sign.
        "llvm.fma" | "llvm.fmuladd" => {
            let mut known = KnownFpClass::unknown();
            if !interested_classes.intersects(FpClassTest::NEGATIVE) {
                return known;
            }
            let (Some(lhs), Some(rhs), Some(addend)) = (argument(0), argument(1), argument(2))
            else {
                return known;
            };
            // Upstream carries a FIXME that this should check
            // `isGuaranteedNotToBeUndef`; it does not, and neither does this.
            if lhs != rhs {
                return known;
            }
            // `x * x` cannot be `-0`, so neither can the sum.
            known.known_not(FpClassTest::NEGATIVE_ZERO);
            // And `x * x + y` is non-negative when `y` is.
            if known_fp_class(addend, interested_classes, query, depth + 1)
                .cannot_be_ordered_less_than_zero()
            {
                known.known_not(FpClassTest::NEGATIVE);
            }
            known
        }
        // The reducing min/max intrinsics pick one of the vector's elements,
        // so whatever is common to every element carries to the result.
        "llvm.vector.reduce.fmax"
        | "llvm.vector.reduce.fmin"
        | "llvm.vector.reduce.fmaximum"
        | "llvm.vector.reduce.fminimum" => {
            let Some(source) = argument(0) else {
                return KnownFpClass::unknown();
            };
            let mut known = compute_known_fp_class_with_flags(
                source,
                fast_math_flags(kind).unwrap_or_else(FastMathFlags::empty),
                interested_classes,
                query,
            );
            // The sign only carries when the result cannot be a NaN.
            if !known.is_known_never_nan() {
                known.reset_sign_bit();
            }
            known
        }
        "llvm.fabs" => {
            let mut known = KnownFpClass::unknown();
            // Caring only about the sign bit means the operand need not be
            // inspected at all.
            if interested_classes.intersects(FpClassTest::NAN.union(FpClassTest::POSITIVE))
                && let Some(source) = argument(0)
            {
                known = known_fp_class(source, interested_classes, query, depth + 1);
            }
            known.absolute();
            known
        }
        "llvm.copysign" => {
            let (Some(magnitude), Some(sign)) = (argument(0), argument(1)) else {
                return KnownFpClass::unknown();
            };
            let mut known = known_fp_class(magnitude, interested_classes, query, depth + 1);
            let known_sign = known_fp_class(sign, interested_classes, query, depth + 1);
            known.copy_sign(known_sign);
            known
        }
        "llvm.sqrt" => {
            let Some(source) = argument(0) else {
                return KnownFpClass::unknown();
            };
            let mut interested_sources = interested_classes;
            if interested_classes.intersects(FpClassTest::NAN) {
                interested_sources |= KnownFpClass::ORDERED_LESS_THAN_ZERO;
            }
            let known_source = known_fp_class(source, interested_sources, query, depth + 1);

            // Upstream consults `nsz` on the call to decide whether the
            // denormal mode matters at all. It reads it through `Q.IIQ`, so a
            // query told to ignore instruction flags must not see it.
            let has_no_signed_zeros = query.uses_instruction_info()
                && data
                    .attrs
                    .fast_math_flags_value()
                    .contains(FastMathFlags::NO_SIGNED_ZEROS);
            let mut known = KnownFpClass::sqrt(
                known_source,
                if has_no_signed_zeros {
                    DenormalMode::dynamic()
                } else {
                    mode
                },
            );
            if has_no_signed_zeros {
                known.known_not(FpClassTest::NEGATIVE_ZERO);
            }
            known
        }
        "llvm.minnum" | "llvm.maxnum" | "llvm.minimum" | "llvm.maximum" | "llvm.minimumnum"
        | "llvm.maximumnum" => {
            let (Some(lhs), Some(rhs)) = (argument(0), argument(1)) else {
                return KnownFpClass::unknown();
            };
            let known_lhs = known_fp_class(lhs, interested_classes, query, depth + 1);
            let known_rhs = known_fp_class(rhs, interested_classes, query, depth + 1);
            let Some(min_max) = min_max_kind(name) else {
                return KnownFpClass::unknown();
            };
            KnownFpClass::min_max_like(known_lhs, known_rhs, min_max, mode)
        }
        "llvm.canonicalize" => {
            let Some(source) = argument(0) else {
                return KnownFpClass::unknown();
            };
            let known_source = known_fp_class(source, interested_classes, query, depth + 1);
            KnownFpClass::canonicalize(known_source, mode)
        }
        "llvm.trunc" | "llvm.floor" | "llvm.ceil" | "llvm.rint" | "llvm.nearbyint"
        | "llvm.round" | "llvm.roundeven" => {
            let Some(source) = argument(0) else {
                return KnownFpClass::unknown();
            };
            let known_source = known_fp_class(source, interested_classes, query, depth + 1);
            KnownFpClass::round_to_integral(
                known_source,
                name == "llvm.trunc",
                is_multi_unit_float_type(value.ty()),
            )
        }
        "llvm.exp" | "llvm.exp2" | "llvm.exp10" => {
            let Some(source) = argument(0) else {
                return KnownFpClass::unknown();
            };
            let known_source = known_fp_class(source, interested_classes, query, depth + 1);
            KnownFpClass::exp(known_source)
        }
        "llvm.log" | "llvm.log2" | "llvm.log10" => {
            let Some(source) = argument(0) else {
                return KnownFpClass::unknown();
            };
            // Upstream skips the whole arm when neither NaN nor infinity is
            // asked about, because that is all `log` can teach.
            if !interested_classes.intersects(FpClassTest::NAN.union(FpClassTest::INFINITY)) {
                return KnownFpClass::unknown();
            }
            let mut interested_sources = interested_classes;
            if interested_classes.intersects(FpClassTest::NEGATIVE_INFINITY) {
                interested_sources |= FpClassTest::ZERO.union(FpClassTest::SUBNORMAL);
            }
            if interested_classes.intersects(FpClassTest::NAN) {
                interested_sources |= FpClassTest::NAN.union(FpClassTest::NEGATIVE);
            }
            let known_source = known_fp_class(source, interested_sources, query, depth + 1);
            KnownFpClass::log(known_source, mode)
        }
        _ => KnownFpClass::unknown(),
    }
}

// --------------------------------------------------------------------------
// Context-dependent facts
// --------------------------------------------------------------------------

/// Adjust `known` for the select arm `arm` with what `condition` implies.
///
/// Ports `llvm::adjustKnownFPClassForSelectArm`. `invert` picks the false arm —
/// the condition is then assumed false rather than true.
///
/// Its known-bits sibling
/// [`adjust_known_bits_for_select_arm`](crate::adjust_known_bits_for_select_arm)
/// checks that the arm is not `undef` before trusting the refinement; upstream
/// leaves a `TODO` asking whether this one should too, and that question is
/// inherited rather than answered.
pub fn adjust_known_fp_class_for_select_arm<'a, 'ctx, B: ModuleBrand + 'ctx>(
    known: KnownFpClass,
    condition: Value<'ctx, B>,
    arm: Value<'ctx, B>,
    invert: bool,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) -> KnownFpClass {
    let mut known = known;
    known_fp_class_from_cond(arm, condition, !invert, &mut known, query, depth + 1);
    known
}

/// Ports `computeKnownFPClassFromContext`.
///
/// Three sources feed it, each attached to the query separately: an injected
/// condition ([`ValueTrackingQuery::with_condition_context`]), the dominating
/// branch conditions ([`ValueTrackingQuery::with_dominating_conditions`], which
/// also needs a dominator tree), and the `@llvm.assume` calls
/// ([`ValueTrackingQuery::with_assumptions`], which also needs a context
/// instruction). A query carrying none of them proves nothing.
fn known_fp_class_from_context<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> KnownFpClass {
    let mut known = KnownFpClass::unknown();

    // Handle the injected condition.
    if let Some(context) = query.condition_context()
        && context.affects(value)
    {
        known_fp_class_from_cond(
            value,
            context.condition(),
            !context.is_inverted(),
            &mut known,
            query,
            0,
        );
    }

    let Some(context_instruction) = query.context_instruction() else {
        return known;
    };

    // Handle dominating conditions.
    if let (Some(cache), Some(dominator_tree), Some(context_block)) = (
        query.dominating_conditions(),
        query.dominator_tree(),
        parent_block(context_instruction),
    ) {
        for branch in cache.conditions_for(value) {
            let Some(InstructionKindData::Br(data)) = instruction_kind(branch) else {
                continue;
            };
            let (condition, then_block, else_block) = match &*data.kind.borrow() {
                BranchKind::Unconditional(_) => continue,
                BranchKind::Conditional {
                    cond,
                    then_bb,
                    else_bb,
                } => (cond.get(), *then_bb, *else_bb),
            };
            let Some(branch_block) = parent_block(branch) else {
                continue;
            };
            let condition = value_from_slot(branch, condition);
            for (successor, condition_is_true) in [(then_block, true), (else_block, false)] {
                if dominator_tree.dominates_edge_slots(branch_block, successor, context_block) {
                    known_fp_class_from_cond(
                        value,
                        condition,
                        condition_is_true,
                        &mut known,
                        query,
                        0,
                    );
                }
            }
        }
    }

    let Some(cache) = query.assumptions() else {
        return known;
    };
    let Ok(context_view) = InstructionView::try_from(context_instruction) else {
        return known;
    };

    for assumption in cache.assumptions_for(value) {
        // The operand-bundle half needs `getKnowledgeFromBundle`, which is not
        // ported; see the [`assumptions`](crate::assumptions) module header.
        if assumption.source() != AssumptionSource::Condition {
            continue;
        }
        let Some(assume) = assumption.assume(module_ref(value)) else {
            continue;
        };
        let Some(argument) = assume_argument(assume.to_erased()) else {
            continue;
        };
        if !is_valid_assume_for_context(&assume, &context_view, query.dominator_tree(), false) {
            continue;
        }
        known_fp_class_from_cond(value, argument, true, &mut known, query, 0);
    }

    known
}

/// Ports `computeKnownFPClassFromCond`.
///
/// Upstream also takes the context instruction, but never reads it; the
/// parameter is not reproduced.
fn known_fp_class_from_cond<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    condition: Value<'ctx, B>,
    condition_is_true: bool,
    known: &mut KnownFpClass,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
    depth: u32,
) {
    // `and` splits a true condition into two true conditions, `or` a false one
    // into two false ones; either way both halves hold.
    if depth < query.max_depth()
        && let Some((a, b, is_and)) = logical_op_parts(condition)
        && is_and == condition_is_true
    {
        known_fp_class_from_cond(value, a, condition_is_true, known, query, depth + 1);
        known_fp_class_from_cond(value, b, condition_is_true, known, query, depth + 1);
        return;
    }

    if depth < query.max_depth()
        && let Some(negated) = not_operand(condition)
    {
        known_fp_class_from_cond(value, negated, !condition_is_true, known, query, depth + 1);
        return;
    }

    if let Some((predicate, lhs, rhs)) = float_compare_parts(condition) {
        // Upstream passes `*cast<Instruction>(Cond)->getParent()->getParent()`:
        // the function holding the *condition*, which is what supplies the
        // denormal mode. A condition that is not an instruction would trip that
        // cast, so it teaches nothing here.
        let Some(function) = enclosing_function_of(condition) else {
            return;
        };
        // `LookThroughSrc` is upstream's `LHS != V`: an `fabs` is only worth
        // seeing through when the comparison's own operand is not already the
        // value being asked about.
        let Some(implied) = fcmp_implies_class(predicate, function, lhs, rhs, lhs != value) else {
            return;
        };
        if implied.tested() == value {
            known.known_not(implied.if_condition_is(condition_is_true).complement());
        }
        return;
    }

    if let Some((tested, mask)) = is_fpclass_call_parts(condition) {
        if tested == value {
            known.known_not(if condition_is_true {
                mask.complement()
            } else {
                mask
            });
        }
        return;
    }

    // An `icmp` against the value's own bit pattern can be a sign-bit test.
    if let Some((predicate, lhs, rhs)) = int_compare_parts(condition)
        && element_wise_bitcast_source(lhs) == Some(value)
        && let Some(rhs) = constant_int(rhs)
        && let Some(true_if_signed) = is_sign_bit_check(predicate, &rhs)
    {
        if true_if_signed == condition_is_true {
            known.sign_bit_must_be_one();
        } else {
            known.sign_bit_must_be_zero();
        }
    }
}

// --------------------------------------------------------------------------
// The convenience predicates
// --------------------------------------------------------------------------

/// Whether `value` is provably never a NaN.
///
/// Ports `llvm::isKnownNeverNaN`.
pub fn is_known_never_nan<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> bool {
    compute_known_fp_class(value, FpClassTest::NAN, query).is_known_never_nan()
}

/// Whether `value` is provably never an infinity.
///
/// Ports `llvm::isKnownNeverInfinity`.
pub fn is_known_never_infinity<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> bool {
    compute_known_fp_class(value, FpClassTest::INFINITY, query).is_known_never_infinity()
}

/// Whether `value` is provably neither an infinity nor a NaN.
///
/// Ports `llvm::isKnownNeverInfOrNaN`. Upstream asks the lattice both questions
/// separately rather than using `isKnownNeverInfOrNaN`, and that is reproduced.
pub fn is_known_never_infinity_or_nan<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> bool {
    let known = compute_known_fp_class(value, FpClassTest::INFINITY.union(FpClassTest::NAN), query);
    known.is_known_never_nan() && known.is_known_never_infinity()
}

/// Whether `value` is provably never `-0.0`.
///
/// Ports `llvm::cannotBeNegativeZero`. Upstream's own caution applies: this is
/// the *literal* `-0.0`, so a caller under a `PreserveSign` denormal mode must
/// think about subnormals separately.
pub fn cannot_be_negative_zero<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> bool {
    compute_known_fp_class(value, FpClassTest::NEGATIVE_ZERO, query).is_known_never_negative_zero()
}

/// Whether `value` is provably NaN or never less than `-0.0`.
///
/// Ports `llvm::cannotBeOrderedLessThanZero`.
pub fn cannot_be_ordered_less_than_zero<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> bool {
    compute_known_fp_class(value, KnownFpClass::ORDERED_LESS_THAN_ZERO, query)
        .cannot_be_ordered_less_than_zero()
}

/// `value`'s sign bit, when it is known.
///
/// Ports `llvm::computeKnownFPSignBit`: `Some(false)` for a provably clear sign
/// bit, `Some(true)` for a provably set one, `None` otherwise.
pub fn compute_known_fp_sign_bit<'a, 'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    query: &ValueTrackingQuery<'a, 'ctx, B>,
) -> Option<bool> {
    compute_known_fp_class(value, FpClassTest::ALL, query).sign_bit()
}

/// Whether the user reached through `use_edge` is indifferent to the sign of a
/// zero operand.
///
/// Ports `llvm::canIgnoreSignBitOfZero`.
///
/// The `ret` arm of upstream's sibling reads the enclosing function's
/// `nofpclass` return attribute, which llvmkit does not model; this predicate
/// has no such arm, so nothing is lost here.
pub fn can_ignore_sign_bit_of_zero<'ctx, B: ModuleBrand + 'ctx>(use_edge: Use<'ctx, B>) -> bool {
    let user = use_edge.user();
    let Some(kind) = instruction_kind(user) else {
        return false;
    };
    // `nsz` on the user settles it outright.
    if fast_math_flags(kind).is_some_and(|flags| flags.contains(FastMathFlags::NO_SIGNED_ZEROS)) {
        return true;
    }

    match kind {
        // A conversion to integer, and `fcmp`, treat both zeros as equal.
        InstructionKindData::Cast(data) => {
            matches!(data.kind, CastOpcode::FpToSi | CastOpcode::FpToUi)
        }
        InstructionKindData::Fcmp(_) => true,
        InstructionKindData::Call(_) => {
            sign_indifferent_intrinsic(user, kind, use_edge.index(), SignOf::Zero)
        }
        _ => false,
    }
}

/// Whether the user reached through `use_edge` is indifferent to the sign of a
/// NaN operand.
///
/// Ports `llvm::canIgnoreSignBitOfNaN`.
///
/// Upstream's `ret` arm reads the enclosing function's `nofpclass` return
/// attribute; llvmkit models no `nofpclass` payload, so that arm answers
/// `false` — the conservative direction, since the predicate licenses dropping
/// a sign.
pub fn can_ignore_sign_bit_of_nan<'ctx, B: ModuleBrand + 'ctx>(use_edge: Use<'ctx, B>) -> bool {
    let user = use_edge.user();
    let Some(kind) = instruction_kind(user) else {
        return false;
    };
    // `nnan` on the user settles it outright.
    if fast_math_flags(kind).is_some_and(|flags| flags.contains(FastMathFlags::NO_NANS)) {
        return true;
    }

    match kind {
        InstructionKindData::Cast(data) => matches!(
            data.kind,
            // A conversion to integer discards the sign with everything else,
            // and the two float conversions are proper FP math.
            CastOpcode::FpToSi | CastOpcode::FpToUi | CastOpcode::FpTrunc | CastOpcode::FpExt
        ),
        // Proper FP math ignores the sign bit of a NaN.
        InstructionKindData::Fadd(_)
        | InstructionKindData::Fsub(_)
        | InstructionKindData::Fmul(_)
        | InstructionKindData::Fdiv(_)
        | InstructionKindData::Frem(_)
        | InstructionKindData::Fcmp(_) => true,
        // Bitwise FP operations preserve it.
        InstructionKindData::Fneg(_)
        | InstructionKindData::Select(_)
        | InstructionKindData::Phi(_) => false,
        InstructionKindData::Call(_) | InstructionKindData::Invoke(_) => {
            sign_indifferent_intrinsic(user, kind, use_edge.index(), SignOf::Nan)
        }
        _ => false,
    }
}

/// Which of the two sign questions [`sign_indifferent_intrinsic`] is answering.
/// The two upstream predicates share a shape but not an intrinsic list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SignOf {
    Zero,
    Nan,
}

/// The `call` arm shared by [`can_ignore_sign_bit_of_zero`] and
/// [`can_ignore_sign_bit_of_nan`].
fn sign_indifferent_intrinsic<'ctx, B: ModuleBrand + 'ctx>(
    user: Value<'ctx, B>,
    kind: &'ctx InstructionKindData,
    operand_index: u32,
    sign_of: SignOf,
) -> bool {
    let callee_slot = match kind {
        InstructionKindData::Call(data) => data.callee.get(),
        InstructionKindData::Invoke(data) => data.callee.get(),
        _ => return false,
    };
    let callee = value_from_slot(user, callee_slot);
    let Some(descriptor) = descriptor_for_callee(callee) else {
        return false;
    };

    match descriptor.id().base_name() {
        // `fabs` overwrites the sign entirely.
        "llvm.fabs" => true,
        // `copysign` takes the magnitude from operand 0 and the sign from
        // operand 1, so only operand 0's sign is free.
        "llvm.copysign" => operand_index == 0,
        // `is.fpclass` is indifferent to a zero's sign only when its test mask
        // treats both zeros alike.
        "llvm.is.fpclass" if sign_of == SignOf::Zero => is_fpclass_mask_zero_agnostic(user),
        "llvm.is.fpclass" if sign_of == SignOf::Nan => true,
        // The rest are proper FP math, which ignores a NaN's sign but not a
        // zero's.
        "llvm.minnum" | "llvm.maxnum" | "llvm.minimum" | "llvm.maximum" | "llvm.minimumnum"
        | "llvm.maximumnum" | "llvm.canonicalize" | "llvm.fma" | "llvm.fmuladd" | "llvm.sqrt"
        | "llvm.pow" | "llvm.powi" | "llvm.fptoui.sat" | "llvm.fptosi.sat" => {
            sign_of == SignOf::Nan
        }
        _ => false,
    }
}

/// Whether an `@llvm.is.fpclass` test mask treats `+0` and `-0` alike — either
/// both are in it or neither is.
///
/// Ports the `Test == fcZero || Test == fcNone` check in
/// `canIgnoreSignBitOfZero`.
fn is_fpclass_mask_zero_agnostic<'ctx, B: ModuleBrand + 'ctx>(user: Value<'ctx, B>) -> bool {
    let Some((_, mask)) = is_fpclass_call_parts(user) else {
        return false;
    };
    let zeros = mask.intersection(FpClassTest::ZERO);
    zeros == FpClassTest::ZERO || zeros.is_none()
}

/// The predicate and operands of an `fcmp`. Ports
/// `m_FCmp(Pred, m_Value(LHS), m_Value(RHS))`.
fn float_compare_parts<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(FloatPredicate, Value<'ctx, B>, Value<'ctx, B>)> {
    let InstructionKindData::Fcmp(data) = instruction_kind(value)? else {
        return None;
    };
    Some((
        data.predicate,
        value_from_slot(value, data.lhs.get()),
        value_from_slot(value, data.rhs.get()),
    ))
}

/// The predicate and operands of an `icmp`. Ports
/// `m_ICmp(Pred, m_Value(LHS), m_Value(RHS))`.
fn int_compare_parts<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(IntPredicate, Value<'ctx, B>, Value<'ctx, B>)> {
    let InstructionKindData::Icmp(data) = instruction_kind(value)? else {
        return None;
    };
    Some((
        data.predicate,
        value_from_slot(value, data.lhs.get()),
        value_from_slot(value, data.rhs.get()),
    ))
}

/// The tested value and mask of an `@llvm.is.fpclass` call. Ports
/// `m_Intrinsic<Intrinsic::is_fpclass>(m_Value(), m_ConstantInt(ClassVal))`.
fn is_fpclass_call_parts<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, FpClassTest)> {
    let InstructionKindData::Call(data) = instruction_kind(value)? else {
        return None;
    };
    let callee = value_from_slot(value, data.callee.get());
    if descriptor_for_callee(callee)?.id().base_name() != "llvm.is.fpclass" {
        return None;
    }
    let tested = value_from_slot(value, data.args.first()?.get());
    let mask = constant_int(value_from_slot(value, data.args.get(1)?.get()))?;
    let raw = u32::try_from(mask.try_zext_u64()?).ok()?;
    Some((tested, FpClassTest::from_bits(raw)?))
}

/// The source of a `bitcast` that changes neither scalar-vs-vector nor the
/// element count. Ports `m_ElementWiseBitCast`.
fn element_wise_bitcast_source<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Cast(data) = instruction_kind(value)? else {
        return None;
    };
    if data.kind != CastOpcode::BitCast {
        return None;
    }
    let source = value_from_slot(value, data.src.get());
    // A fixed and a scalable vector of the same count differ, which is the
    // `getElementCount()` comparison upstream makes.
    let shape = |value: Value<'ctx, B>| {
        value
            .ty()
            .data()
            .as_vector()
            .map(|(_, lanes, scalable)| (lanes, scalable))
    };
    (shape(source) == shape(value)).then_some(source)
}

/// The integer constant `value` is, if it is one.
fn constant_int<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApInt> {
    let TypeKind::Integer { bits } = value.ty().kind() else {
        return None;
    };
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Int(words)) => Some(ApInt::from_words(bits, words)),
        _ => None,
    }
}

/// Ports the static `getMinMaxKind`, keyed on the intrinsic's base name because
/// llvmkit mints per-intrinsic constants only where its analyses need them.
fn min_max_kind(name: &str) -> Option<MinMaxKind> {
    Some(match name {
        "llvm.minimum" => MinMaxKind::Minimum,
        "llvm.maximum" => MinMaxKind::Maximum,
        "llvm.minimumnum" => MinMaxKind::MinimumNum,
        "llvm.maximumnum" => MinMaxKind::MaximumNum,
        "llvm.minnum" => MinMaxKind::MinNum,
        "llvm.maxnum" => MinMaxKind::MaxNum,
        _ => return None,
    })
}

/// The constant and poison leaves of `computeKnownFPClass`, each of which
/// answers exactly.
/// The floating-point constant `value` is, if it is one.
///
/// Ports `m_APFloat`'s scalar case: the constant behind an operand, which the
/// `fmul` arm reads for its denormal-scaling refinement.
fn constant_ap_float<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApFloat> {
    let ValueKindData::Constant(ConstantData::Float(bits)) = &value.data().kind else {
        return None;
    };
    let semantics = scalar_semantics(value.ty())?;
    // The same decode `ConstantFloatValue::ap_float` performs: the stored
    // `u128` is the raw bit pattern, low word first.
    let low = u64::try_from(*bits & 0xffff_ffff_ffff_ffff).ok()?;
    let high = u64::try_from(*bits >> 64).ok()?;
    let pattern = ApInt::from_words(semantics.bit_width(), &[low, high]);
    ApFloat::from_bits(semantics, &pattern).ok()
}

/// The denormal mode for `value`'s scalar type, or `None` when it has no
/// enclosing function.
///
/// Upstream's arithmetic arms read `cast<Instruction>(Op)->getFunction()` and
/// guard every use of the mode on it being non-null; `None` is that guard.
/// Deliberately not [`denormal_mode_of`], which answers `dynamic()` for a value
/// with no function — sound, but able to prove things upstream declines to,
/// which would be a divergence rather than a port.
fn scalar_denormal_mode<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<DenormalMode> {
    let function = enclosing_function_of(value)?;
    let semantics = scalar_semantics(value.ty())?;
    Some(function.denormal_mode(semantics))
}

fn constant_fp_class<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<KnownFpClass> {
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Float(_)) => {
            constant_ap_float(value).map(|float| KnownFpClass::of(&float))
        }
        // Poison belongs to no class at all — upstream sets `fcNone`.
        ValueKindData::Constant(ConstantData::Poison) => {
            Some(KnownFpClass::new(FpClassTest::NONE, Some(false)))
        }
        _ => None,
    }
}

/// The fast-math flags an operator carries, where it carries any.
fn fast_math_flags(kind: &InstructionKindData) -> Option<FastMathFlags> {
    match kind {
        InstructionKindData::Fadd(data)
        | InstructionKindData::Fsub(data)
        | InstructionKindData::Fmul(data)
        | InstructionKindData::Fdiv(data)
        | InstructionKindData::Frem(data) => Some(data.fmf),
        InstructionKindData::Fneg(data) => Some(data.fmf),
        InstructionKindData::Fcmp(data) => Some(data.fmf),
        InstructionKindData::Call(data) => Some(data.attrs.fast_math_flags_value()),
        _ => None,
    }
}

/// Whether the type is one of the multi-unit floating-point formats — in
/// practice `ppc_fp128`. Ports `Type::isMultiUnitFPType`.
fn is_multi_unit_float_type<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> bool {
    matches!(scalar_kind(ty), TypeKind::PpcFp128)
}

/// The `ApFloat` semantics of a scalar or per-lane floating-point type.
fn scalar_semantics<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> Option<ApFloatSemantics> {
    Some(match scalar_kind(ty) {
        TypeKind::Half => ApFloatSemantics::IeeeHalf,
        TypeKind::Bfloat => ApFloatSemantics::Bfloat,
        TypeKind::Float => ApFloatSemantics::IeeeSingle,
        TypeKind::Double => ApFloatSemantics::IeeeDouble,
        TypeKind::Fp128 => ApFloatSemantics::IeeeQuad,
        TypeKind::X86Fp80 => ApFloatSemantics::X87DoubleExtended,
        TypeKind::PpcFp128 => ApFloatSemantics::PpcDoubleDouble,
        _ => return None,
    })
}

/// The kind of the type's scalar, peeling one vector layer.
fn scalar_kind<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> TypeKind {
    match ty.data().as_vector() {
        Some((element, _, _)) => Type::new(element, ty.module()).kind(),
        None => ty.kind(),
    }
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
    let module = module_ref(anchor);
    let data = module.value_data(slot);
    Value::from_parts(slot, module, data.ty)
}

/// The module `value` lives in.
fn module_ref<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> ModuleRef<'ctx, B> {
    ModuleRef::new(value.module().core_ref())
}
