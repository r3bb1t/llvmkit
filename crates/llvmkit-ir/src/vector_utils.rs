//! Vector and shuffle-mask utilities.
//!
//! Ports `llvm/lib/Analysis/VectorUtils.cpp`.
//!
//! # What is here
//!
//! The splat family — [`get_splat_value`], [`splat_index`], [`is_splat_value`]
//! and [`find_scalar_element`] — plus [`shuffle_demanded_elements`].
//!
//! The two splat *questions* are deliberately separate and neither subsumes
//! the other. [`get_splat_value`] must name the broadcast scalar, so it only
//! recognises two shapes and answers with a value. [`is_splat_value`] only has
//! to decide whether all lanes agree, so it sees through binary operators and
//! `select`s whose operands are independently splats — cases with no single
//! scalar to name — and answers a `bool`. Upstream's header says as much:
//! `isSplatValue` "may be more powerful … because it is not limited by finding
//! a scalar source value".
//!
//! **[`find_scalar_element`] has no upstream unit test.** `VectorUtilsTest.cpp`
//! covers the other three and stops, so that one port rests on the
//! implementation alone — worth knowing before trusting a subtle answer from
//! it. The rest are pinned by ported fixtures.
//!
//! # What is not modeled, and why
//!
//! Upstream defines 37 functions here — 38 counting both
//! `widenShuffleMaskElts` overloads. **16 are absent**, each blocked on
//! something named:
//!
//! - **Eight take or return `Intrinsic::ID`** — `isTriviallyVectorizable`,
//!   `isTriviallyScalarizable`, `isVectorIntrinsicWithScalarOpAtArg`,
//!   `isVectorIntrinsicWithOverloadTypeAtArg`,
//!   `isVectorIntrinsicWithStructReturnOverloadAtField`,
//!   `getVectorIntrinsicIDForCall`, `getInterleaveIntrinsicFactor` and
//!   `getDeinterleaveIntrinsicFactor`. llvmkit has no public intrinsic-id
//!   type; the same blocker keeps `getIntrinsicForCallSite` out of the
//!   ValueTracking ledger, so closing it would unblock nine functions at once.
//! - **Four need metadata modeling** — `uniteAccessGroups`,
//!   `intersectAccessGroups`, `getMetadataToPropagate` and
//!   `propagateMetadata`.
//! - **Three need construction machinery** — `concatenateVectors` and
//!   `createBitMaskForGaps` need an `IRBuilder`, and
//!   `getDeinterleavedVectorType` needs `IntrinsicInst`.
//! - **`computeMinimumValueSizes` needs `TargetTransformInfo`.** llvmkit
//!   models no target — code generation and target backends are out of scope,
//!   not merely unfinished — so unlike the rest this one is blocked
//!   permanently rather than pending.
//!
//! Deriving that list by hand is error-prone: a `grep` anchoring the return
//! type and the `llvm::` name to one line silently misses every definition
//! whose return type wraps. Re-derive with
//! `grep -oE "\bllvm::[a-zA-Z_][a-zA-Z0-9_]*\(" … | sort -u`, discounting
//! `bit_ceil` and `bit_width`, which are calls into `ADT/bit.h` rather than
//! definitions here.

use crate::ap_int::ApInt;
use crate::constant::{Constant, ConstantData};
use crate::instr_types::BinaryOpcode;
use crate::instr_types::ShuffleMaskElem;
use crate::instruction::InstructionKindData;
use crate::module::ModuleBrand;
use crate::r#type::Type;
use crate::value::{Value, ValueKindData};
use crate::value_tracking::{
    MAX_ANALYSIS_RECURSION_DEPTH, binary_operator_parts, instruction_kind, value_from_id,
};

/// The one source lane a shuffle mask selects, when every defined element
/// selects the same one.
///
/// Ports `llvm::getSplatIndex`. Upstream returns `-1` for two situations at
/// once — a mask carrying two different defined lanes, and a mask that is
/// entirely poison — and its own doc comment states both. `None` keeps them
/// merged, because upstream's callers cannot tell them apart either and the
/// answer to "which single lane does this select" is genuinely absent in both.
///
/// Poison elements are skipped rather than disqualifying, so `<poison, 42,
/// poison>` still answers lane 42.
pub fn splat_index(mask: &[ShuffleMaskElem]) -> Option<u32> {
    let mut splat = None;
    for element in mask {
        // Ignore invalid (undefined) mask elements.
        let ShuffleMaskElem::Lane(lane) = *element else {
            continue;
        };
        // There can be only 1 non-negative mask element value if this is a
        // splat.
        if splat.is_some_and(|seen| seen != lane) {
            return None;
        }
        // Initialize the splat index to the 1st non-negative mask element.
        splat = Some(lane);
    }
    splat
}

/// The single scalar a splat vector broadcasts, when `value` is one.
///
/// Ports `llvm::getSplatValue`, which upstream's own comment describes as "not
/// fully general": it checks exactly two shapes, a splat constant vector and
/// the canonical broadcast-lane-0 instruction sequence
///
/// ```text
/// shuf (inselt ?, Splat, 0), ?, <0, poison, 0, ...>
/// ```
///
/// The answer to that second shape is `Splat` — the value the `insertelement`
/// put in lane 0 — not the `insertelement` itself. That distinction is the
/// point: the `insertelement`'s own vector operand is typically `poison`, so a
/// caller walking operands instead would answer conservatively where the splat
/// is provably well defined.
///
/// [`is_splat_value`] answers a *weaker* question — "are all lanes equal" —
/// without needing to name the scalar, and so succeeds in cases this returns
/// `None` for. Neither subsumes the other; upstream ships both.
pub fn get_splat_value<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    // `isa<VectorType>(V->getType())` guarding `dyn_cast<Constant>(V)`. The
    // `return` is upstream's: a vector-typed constant answers here whatever
    // `Constant::getSplatValue` says, including `None`, and never falls
    // through to the shuffle match below.
    if value.ty().is_vector()
        && let ValueKindData::Constant(_) = &value.data().kind
    {
        // `C->getSplatValue()` — the `AllowPoison` default, which is `false`.
        return Constant::from_parts(value)
            .splat_value(false)
            .map(Constant::into_erased);
    }

    let InstructionKindData::ShuffleVector(shuffle) = instruction_kind(value)? else {
        return None;
    };
    // `m_ZeroMask`: every element zero, or poison standing in for one.
    if !shuffle
        .mask
        .iter()
        .all(|element| matches!(element, ShuffleMaskElem::Lane(0) | ShuffleMaskElem::Poison))
    {
        return None;
    }
    // `m_InsertElt(m_Value(), m_Value(Splat), m_ZeroInt())` on operand 0. The
    // shuffle's second operand is `m_Value()` — anything at all.
    let inserted = value_from_id(value, shuffle.lhs.get());
    let InstructionKindData::InsertElement(insert) = instruction_kind(inserted)? else {
        return None;
    };
    let index = value_from_id(inserted, insert.index.get());
    is_zero_constant(index).then(|| value_from_id(inserted, insert.value.get()))
}

/// Whether every lane of `value` is poison or equal to every other non-poison
/// lane.
///
/// Ports `llvm::isSplatValue`. `index` is upstream's `Index` parameter, whose
/// `-1` default means "any lane will do"; `None` spells that, and `Some(n)`
/// additionally demands that lane `n` be the defined one.
///
/// This is the more powerful of the two splat questions — it does not have to
/// name a scalar source, so it sees through binary operators and `select`s
/// whose operands are each independently splats. It is also, in two places,
/// *deliberately* less clever than it could be, and upstream marks both with a
/// `FIXME`: a constant vector is tested by [`Constant::splat_value`] without
/// consulting `index` at all, and a shuffle mask carrying a poison element is
/// rejected outright even though the lane in question may be defined. Both
/// behaviours are ported as-is — the upstream unit tests pin them, so
/// "improving" either would be a silent divergence.
pub fn is_splat_value<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    index: Option<u32>,
) -> bool {
    is_splat_value_at_depth(value, index, 0)
}

fn is_splat_value_at_depth<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    index: Option<u32>,
    depth: u32,
) -> bool {
    if value.ty().is_vector() {
        match &value.data().kind {
            // `isa<UndefValue>`. `PoisonValue` derives from `UndefValue`
            // upstream, so this arm catches both; llvmkit keeps them as
            // separate payloads and has to name each.
            ValueKindData::Constant(ConstantData::Undef | ConstantData::Poison) => return true,
            // FIXME (upstream's): undefs could be allowed, but when `index`
            // was specified the constant should be checked at that index.
            // `Constant::getSplatValue` is called with `AllowPoison`'s
            // `false` default, and `index` goes unread.
            ValueKindData::Constant(_) => {
                return Constant::from_parts(value).splat_value(false).is_some();
            }
            _ => {}
        }
    }

    if let Some(InstructionKindData::ShuffleVector(shuffle)) = instruction_kind(value) {
        // FIXME (upstream's): undefs would be safe here too. A single poison
        // element makes the mask unequal, so `<0, poison>` is rejected.
        let Some(&first) = shuffle.mask.first() else {
            // `all_equal` of an empty mask is true, and upstream then matches
            // any index. A shuffle cannot actually have an empty mask.
            return index.is_none();
        };
        if !shuffle.mask.iter().all(|element| *element == first) {
            return false;
        }
        // Match any index.
        let Some(index) = index else {
            return true;
        };
        // Match a specific element. The mask should be defined at and match
        // the specified index. Upstream spells this `getMaskValue(Index) ==
        // Index`, where a poison element reads back as `-1` and so can never
        // equal a non-negative `Index`.
        return usize::try_from(index)
            .ok()
            .and_then(|position| shuffle.mask.get(position))
            .is_some_and(|element| *element == ShuffleMaskElem::Lane(index));
    }

    // The remaining tests are all recursive, so bail out if we hit the limit.
    if depth == MAX_ANALYSIS_RECURSION_DEPTH {
        return false;
    }
    let depth = depth + 1;

    let Some(kind) = instruction_kind(value) else {
        return false;
    };

    // If both operands of a binop are splats, the result is a splat.
    if let Some((_, binary)) = binary_operator_parts(kind) {
        return is_splat_value_at_depth(value_from_id(value, binary.lhs.get()), index, depth)
            && is_splat_value_at_depth(value_from_id(value, binary.rhs.get()), index, depth);
    }

    // If all operands of a select are splats, the result is a splat.
    if let InstructionKindData::Select(select) = kind {
        return is_splat_value_at_depth(value_from_id(value, select.cond.get()), index, depth)
            && is_splat_value_at_depth(value_from_id(value, select.true_val.get()), index, depth)
            && is_splat_value_at_depth(value_from_id(value, select.false_val.get()), index, depth);
    }

    // TODO (upstream's): unary ops (fneg), casts, intrinsics (overflow ops).
    false
}

/// The scalar already sitting in lane `element` of `value`, when it can be
/// found without materialising anything.
///
/// Ports `llvm::findScalarElement` — "see if the scalar value is already
/// around as a register, for example if it were inserted then extracted from
/// the vector". `None` is upstream's `nullptr`, meaning *unknown*; a returned
/// `poison` is a definite answer, not a failure, and the two must not be
/// conflated.
///
/// Upstream asserts `V->getType()->isVectorTy()`. llvmkit answers `None` for a
/// non-vector instead of aborting, since the repo forbids panics on production
/// paths.
///
/// Termination rests on the use-def graph being acyclic, exactly as upstream's
/// does: every recursive call walks to an operand, and the one shape that can
/// close a cycle in malformed IR — an `insertelement` whose vector operand is
/// itself — is guarded explicitly below, again as upstream guards it.
pub fn find_scalar_element<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    element: u32,
) -> Option<Value<'ctx, B>> {
    let (element_ty, lanes, scalable) = value.ty().data().as_vector()?;
    let element_ty = Type::new(element_ty, value.module());

    // For fixed-length vector, return poison for out of range access.
    if !scalable && element >= lanes {
        return Some(element_ty.get_poison().as_constant().into_erased());
    }

    if let ValueKindData::Constant(_) = &value.data().kind {
        return Constant::from_parts(value)
            .aggregate_element(element)
            .map(Constant::into_erased);
    }

    if let Some(InstructionKindData::InsertElement(insert)) = instruction_kind(value) {
        // If this is an insert to a variable element, we don't know what it
        // is.
        let index = value_from_id(value, insert.index.get());
        let index = constant_lane(index)?;

        // If this is an insert to the element we are looking for, return the
        // inserted value.
        if element == index {
            return Some(value_from_id(value, insert.value.get()));
        }

        // Guard against infinite loop on malformed, unreachable IR.
        if insert.vector.get() == value.slot() {
            return None;
        }

        // Otherwise, the insertelement doesn't modify the value, recurse on
        // its vector input.
        return find_scalar_element(value_from_id(value, insert.vector.get()), element);
    }

    if let Some(InstructionKindData::ShuffleVector(shuffle)) = instruction_kind(value) {
        // Restrict the following transformation to fixed-length vector.
        if !scalable {
            let source = value_from_id(value, shuffle.lhs.get());
            let (_, source_lanes, _) = source.ty().data().as_vector()?;
            let position = usize::try_from(element).ok()?;
            let Some(&ShuffleMaskElem::Lane(selected)) = shuffle.mask.get(position) else {
                return Some(element_ty.get_poison().as_constant().into_erased());
            };
            return if selected < source_lanes {
                find_scalar_element(source, selected)
            } else {
                find_scalar_element(
                    value_from_id(value, shuffle.rhs.get()),
                    selected.checked_sub(source_lanes)?,
                )
            };
        }
    }

    // Extract a value from a vector add operation with a constant zero.
    // TODO (upstream's): use `getBinOpIdentity` to generalize this.
    if let Some((BinaryOpcode::Add, add)) = instruction_kind(value).and_then(binary_operator_parts)
    {
        let addend = value_from_id(value, add.rhs.get());
        if let ValueKindData::Constant(_) = &addend.data().kind
            && let Some(lane) = Constant::from_parts(addend).aggregate_element(element)
            && lane.is_null_value()
        {
            return find_scalar_element(value_from_id(value, add.lhs.get()), element);
        }
    }

    // If the vector is a splat then we can trivially find the scalar element.
    if scalable
        && let Some(splat) = get_splat_value(value)
        && element < lanes
    {
        return Some(splat);
    }

    // Otherwise, we don't know.
    None
}

/// Whether `value` is the constant integer zero, upstream's `m_ZeroInt()`.
fn is_zero_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    matches!(constant_lane(value), Some(0))
}

/// The lane index a constant integer names, upstream's `m_ConstantInt(IIElt)`.
///
/// `None` covers both a non-constant operand and one too wide to be a lane
/// index; upstream's `uint64_t` would take the latter and then fail the
/// comparison, so the answers agree.
fn constant_lane<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<u32> {
    let ValueKindData::Constant(ConstantData::Int(words)) = &value.data().kind else {
        return None;
    };
    match &**words {
        [] => Some(0),
        [single] => u32::try_from(*single).ok(),
        _ => None,
    }
}

/// The lanes a `shufflevector` mask demands from each of its two sources.
///
/// Ports `llvm::getShuffleDemandedElts`, whose two out-parameters become the
/// returned pair. `None` is upstream's `false` — a poison mask element among
/// the demanded lanes when `allow_undefined_elements` is not set, or a mask
/// index past the end of both sources. Callers answer "nothing known" for it.
///
/// `source_width` is the lane count of *one* source, not their sum; both
/// operands of a `shufflevector` have the same type, and mask indices at or
/// above it select from the right-hand side.
///
/// `allow_undefined_elements` distinguishes the two questions a caller can
/// ask. Known-bits and known-fp-class analyses pass `false`: a demanded poison
/// lane means the result has no common state to describe, so the whole query
/// fails. A caller that only wants to know which source lanes are reachable
/// passes `true`, and poison lanes are simply skipped.
pub fn shuffle_demanded_elements(
    source_width: u32,
    mask: &[ShuffleMaskElem],
    demanded: &ApInt,
    allow_undefined_elements: bool,
) -> Option<(ApInt, ApInt)> {
    let mut left = ApInt::zero(source_width);
    let mut right = ApInt::zero(source_width);

    // Nothing demanded, nothing to trace back.
    if demanded.is_zero() {
        return Some((left, right));
    }

    // A shuffle with `zeroinitializer` reads lane 0 of the left source and
    // nothing else, whatever the demanded set says.
    if mask
        .iter()
        .all(|element| *element == ShuffleMaskElem::Lane(0))
    {
        left.set_bit(0);
        return Some((left, right));
    }

    for (lane, element) in mask.iter().enumerate() {
        let lane = u32::try_from(lane).ok()?;
        if !demanded.bit(lane) {
            continue;
        }
        let ShuffleMaskElem::Lane(index) = *element else {
            // For a poison element the result lane has no common state — so
            // unless the caller said it does not care, nothing is known.
            if allow_undefined_elements {
                continue;
            }
            return None;
        };
        if index < source_width {
            left.set_bit(index);
        } else {
            let right_lane = index.checked_sub(source_width)?;
            if right_lane >= source_width {
                return None;
            }
            right.set_bit(right_lane);
        }
    }
    Some((left, right))
}
