//! Vector and shuffle-mask utilities.
//!
//! Ports `llvm/lib/Analysis/VectorUtils.cpp`.
//!
//! # What is here
//!
//! **Everything llvmkit's scope admits.** Upstream defines 37 functions; 20 of
//! those names are ported (21 functions, since `widenShuffleMaskElts` has two
//! overloads), and the remaining 17 are blocked on something named below.
//!
//! - **The splat family** — [`get_splat_value`], [`splat_index`],
//!   [`is_splat_value`], [`find_scalar_element`].
//! - **The mask transforms** — [`narrow_shuffle_mask_elements`],
//!   [`widen_shuffle_mask_elements`],
//!   [`widen_shuffle_mask_elements_in_pairs`],
//!   [`scale_shuffle_mask_elements`], [`shuffle_mask_with_widest_elements`].
//! - **The mask constructors** — [`create_replicated_mask`],
//!   [`create_interleave_mask`], [`create_stride_mask`],
//!   [`create_sequential_mask`], [`create_unary_mask`].
//! - **The demanded-lane queries** — [`shuffle_demanded_elements`],
//!   [`possibly_demanded_elements_in_mask`],
//!   [`horizontal_demanded_elements_for_first_operand`].
//! - **The `<N x i1>` mask predicates** — [`mask_is_all_zero_or_undefined`],
//!   [`mask_is_all_one_or_undefined`], [`mask_contains_all_one_or_undefined`].
//! - **Slide recognition** — [`masked_slide_pair`].
//!
//! [`possibly_demanded_elements_in_mask`] is **stronger** than upstream's, in
//! a sound direction, and says so at its site.
//!
//! **The mask transforms are narrower than upstream's, permanently.** Upstream
//! reads mask elements as raw `int`s, so the same functions serve both the IR
//! alphabet `{lane, poison}` and the wider one SelectionDAG and the X86 backend
//! use (`SM_SentinelZero` is `-2`). llvmkit takes `&[ShuffleMaskElem]`, which
//! models the IR alphabet only — code generation and target backends are out of
//! scope — so the rule "negatives must be *equal* across a widened group"
//! collapses to "all poison". The difference is unobservable on any mask
//! llvmkit can hold; `widen_shuffle_mask_elements` says so at its site, and
//! `tests/vector_utils_masks.rs` records the three upstream assertions that
//! therefore have no llvmkit spelling.
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
//! **17 of the 37 are absent**, each blocked on something named:
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
//! - **Two are permanently out of scope, not pending.** llvmkit models no
//!   target: code generation and target backends are excluded by charter, not
//!   merely unfinished. `computeMinimumValueSizes` takes a
//!   `TargetTransformInfo`, and `processShuffleMasks` splits a mask across
//!   *physical registers* — every one of its callers lives in
//!   `lib/CodeGen/SelectionDAG` or `lib/Target/{RISCV,X86}`. Both would need a
//!   target model to mean anything.
//!
//! The shuffle-mask *alphabet* is narrowed for the same charter reason, which
//! is a separate matter from a function being absent — see the note on the
//! mask transforms above.
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
    MAX_ANALYSIS_RECURSION_DEPTH, binary_operator_parts, instruction_kind, value_from_slot,
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
    let inserted = value_from_slot(value, shuffle.lhs.get());
    let InstructionKindData::InsertElement(insert) = instruction_kind(inserted)? else {
        return None;
    };
    let index = value_from_slot(inserted, insert.index.get());
    is_zero_constant(index).then(|| value_from_slot(inserted, insert.value.get()))
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
        return is_splat_value_at_depth(value_from_slot(value, binary.lhs.get()), index, depth)
            && is_splat_value_at_depth(value_from_slot(value, binary.rhs.get()), index, depth);
    }

    // If all operands of a select are splats, the result is a splat.
    if let InstructionKindData::Select(select) = kind {
        return is_splat_value_at_depth(value_from_slot(value, select.cond.get()), index, depth)
            && is_splat_value_at_depth(
                value_from_slot(value, select.true_val.get()),
                index,
                depth,
            )
            && is_splat_value_at_depth(
                value_from_slot(value, select.false_val.get()),
                index,
                depth,
            );
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
        let index = value_from_slot(value, insert.index.get());
        let index = constant_lane(index)?;

        // If this is an insert to the element we are looking for, return the
        // inserted value.
        if element == index {
            return Some(value_from_slot(value, insert.value.get()));
        }

        // Guard against infinite loop on malformed, unreachable IR.
        if insert.vector.get() == value.slot() {
            return None;
        }

        // Otherwise, the insertelement doesn't modify the value, recurse on
        // its vector input.
        return find_scalar_element(value_from_slot(value, insert.vector.get()), element);
    }

    if let Some(InstructionKindData::ShuffleVector(shuffle)) = instruction_kind(value) {
        // Restrict the following transformation to fixed-length vector.
        if !scalable {
            let source = value_from_slot(value, shuffle.lhs.get());
            let (_, source_lanes, _) = source.ty().data().as_vector()?;
            let position = usize::try_from(element).ok()?;
            let Some(&ShuffleMaskElem::Lane(selected)) = shuffle.mask.get(position) else {
                return Some(element_ty.get_poison().as_constant().into_erased());
            };
            return if selected < source_lanes {
                find_scalar_element(source, selected)
            } else {
                find_scalar_element(
                    value_from_slot(value, shuffle.rhs.get()),
                    selected.checked_sub(source_lanes)?,
                )
            };
        }
    }

    // Extract a value from a vector add operation with a constant zero.
    // TODO (upstream's): use `getBinOpIdentity` to generalize this.
    if let Some((BinaryOpcode::Add, add)) = instruction_kind(value).and_then(binary_operator_parts)
    {
        let addend = value_from_slot(value, add.rhs.get());
        if let ValueKindData::Constant(_) = &addend.data().kind
            && let Some(lane) = Constant::from_parts(addend).aggregate_element(element)
            && lane.is_null_value()
        {
            return find_scalar_element(value_from_slot(value, add.lhs.get()), element);
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

/// Which of a `shufflevector`'s two operands a mask element reads.
///
/// Upstream spells this as the `int Src = M >= NumElts` of `isMaskedSlidePair`
/// — `0` for the left operand, `1` for the right — alongside a third state,
/// `-1`, meaning "no operand recorded yet". That third state is
/// [`MaskedSlide`]'s absence here rather than a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShuffleSource {
    /// The first operand — mask elements below the source lane count.
    Left,
    /// The second operand — mask elements at or above it.
    Right,
}

/// One of the two constant-offset slides a mask can decompose into.
///
/// `offset` is upstream's `Diff`: the result lane minus the source lane the
/// mask reads for it, so a positive offset slides the source toward higher
/// lanes. It is signed because the slide can go either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaskedSlide {
    /// Which operand this slide reads.
    pub source: ShuffleSource,
    /// Result lane minus source lane, constant across the slide.
    pub offset: i64,
}

/// Decompose `mask` into at most two constant-offset slides, when it is one.
///
/// Ports `llvm::isMaskedSlidePair`, whose `bool` plus filled-in
/// `std::array<std::pair<int, int>, 2>` becomes the returned pair: the first
/// slide is always present when the answer is `Some`, and the second is `None`
/// when one slide covered every defined lane.
///
/// Upstream's "unused slot" marker — a `Src` of `-1` paired with a `Diff` of
/// `NumElts * 2`, which exists only to be asserted against — has no
/// counterpart, because `Option` already spells an unfilled slot. That is the
/// whole of the difference: an all-poison mask still answers `None`, matching
/// upstream's closing `SrcInfo[0].first != -1`.
///
/// `lane_count` is one operand's lane count, not the sum.
pub fn masked_slide_pair(
    mask: &[ShuffleMaskElem],
    lane_count: u32,
) -> Option<(MaskedSlide, Option<MaskedSlide>)> {
    if lane_count == 0 {
        return None;
    }
    let mut slides: [Option<MaskedSlide>; 2] = [None, None];
    for (lane, element) in mask.iter().enumerate() {
        let ShuffleMaskElem::Lane(selected) = *element else {
            continue;
        };
        let candidate = MaskedSlide {
            source: if selected >= lane_count {
                ShuffleSource::Right
            } else {
                ShuffleSource::Left
            },
            offset: i64::try_from(lane).ok()? - i64::from(selected % lane_count),
        };
        // Claim the first free slot, then compare — upstream fills an unused
        // entry and falls straight through to the equality test.
        let matched = slides
            .iter_mut()
            .any(|slot| *slot.get_or_insert(candidate) == candidate);
        if !matched {
            return None;
        }
    }
    // Avoid all undef masks.
    slides[0].map(|first| (first, slides[1]))
}

/// The lanes a horizontal binary operation's first operand demands, split
/// across the two vectors it reads.
///
/// Ports `llvm::getHorizDemandedEltsForFirstOperand`. A horizontal add or
/// subtract pairs *adjacent* lanes within each 128-bit lane group, so result
/// lane `i` of the first half reads source lanes `2i` and `2i + 1`; this
/// reports the first of each such pair for the left and right vectors.
///
/// `None` covers upstream's assert — a vector narrower than 128 bits — plus
/// the degenerate case it does not name, a demanded mask with fewer bits than
/// there are 128-bit lane groups.
pub fn horizontal_demanded_elements_for_first_operand(
    vector_bit_width: u32,
    demanded: &ApInt,
) -> Option<(ApInt, ApInt)> {
    // "Vectors smaller than 128 bit not supported".
    if vector_bit_width < 128 {
        return None;
    }
    let lane_groups = vector_bit_width / 128;
    let lanes = demanded.bit_width();
    if lane_groups == 0 || lanes < lane_groups {
        return None;
    }
    let lanes_per_group = lanes / lane_groups;
    if lanes_per_group == 0 {
        return None;
    }
    let half_per_group = lanes_per_group / 2;

    let mut left = ApInt::zero(lanes);
    let mut right = ApInt::zero(lanes);

    // Map the demanded lanes to the horizontal operands.
    for lane in 0..lanes {
        if !demanded.bit(lane) {
            continue;
        }
        let group_base = (lane / lanes_per_group) * lanes_per_group;
        let local = lane % lanes_per_group;
        if local < half_per_group {
            left.set_bit(group_base + 2 * local);
        } else {
            right.set_bit(group_base + 2 * (local - half_per_group));
        }
    }
    Some((left, right))
}

/// Whether `mask` is a constant `<N x i1>` whose every lane is zero or
/// undefined.
///
/// Ports `llvm::maskIsAllZeroOrUndef`, the masked-load/store predicate.
/// Upstream asserts the operand is a vector of `i1`; llvmkit answers `false`
/// instead, since "not known to be all-zero" is the conservative reading and
/// the repo forbids panics on production paths. A non-constant mask, and any
/// scalable one that is not wholly null or undefined, likewise answer `false`.
pub fn mask_is_all_zero_or_undefined<'ctx, B: ModuleBrand + 'ctx>(mask: Value<'ctx, B>) -> bool {
    constant_mask_lanes_satisfy(mask, MaskLaneTest::Zero, MaskQuantifier::Every)
}

/// Whether `mask` is a constant `<N x i1>` whose every lane is one or
/// undefined.
///
/// Ports `llvm::maskIsAllOneOrUndef`. Same shape and same conservative `false`
/// as [`mask_is_all_zero_or_undefined`].
pub fn mask_is_all_one_or_undefined<'ctx, B: ModuleBrand + 'ctx>(mask: Value<'ctx, B>) -> bool {
    constant_mask_lanes_satisfy(mask, MaskLaneTest::One, MaskQuantifier::Every)
}

/// Whether `mask` is a constant `<N x i1>` with *at least one* lane that is
/// one or undefined.
///
/// Ports `llvm::maskContainsAllOneOrUndef`. Despite sharing a name shape with
/// [`mask_is_all_one_or_undefined`], the quantifier is the opposite one:
/// upstream's loop `return`s `true` on the first qualifying lane rather than
/// `continue`ing past it.
pub fn mask_contains_all_one_or_undefined<'ctx, B: ModuleBrand + 'ctx>(
    mask: Value<'ctx, B>,
) -> bool {
    constant_mask_lanes_satisfy(mask, MaskLaneTest::One, MaskQuantifier::Any)
}

/// Which lane value the mask predicates accept alongside `undef`/`poison`.
#[derive(Clone, Copy)]
enum MaskLaneTest {
    Zero,
    One,
}

/// Whether every lane must satisfy the test or merely one of them.
#[derive(Clone, Copy)]
enum MaskQuantifier {
    Every,
    Any,
}

/// The body the three `maskIs…`/`maskContains…` predicates share verbatim
/// upstream, differing only in the lane test and the quantifier.
fn constant_mask_lanes_satisfy<'ctx, B: ModuleBrand + 'ctx>(
    mask: Value<'ctx, B>,
    test: MaskLaneTest,
    quantifier: MaskQuantifier,
) -> bool {
    let ValueKindData::Constant(_) = &mask.data().kind else {
        return false;
    };
    let constant = Constant::from_parts(mask);

    let whole_vector = match test {
        MaskLaneTest::Zero => constant.is_null_value(),
        MaskLaneTest::One => constant.is_all_ones_value(),
    };
    // `isa<UndefValue>` catches `poison`, which derives from it upstream.
    let undefined = matches!(
        &mask.data().kind,
        ValueKindData::Constant(ConstantData::Undef | ConstantData::Poison)
    );
    if whole_vector || undefined {
        return true;
    }

    let Some((_, lanes, scalable)) = mask.ty().data().as_vector() else {
        return false;
    };
    if scalable {
        return false;
    }

    let lane_qualifies = |lane: u32| {
        constant.aggregate_element(lane).is_some_and(|element| {
            let element_undefined = matches!(
                &element.into_erased().data().kind,
                ValueKindData::Constant(ConstantData::Undef | ConstantData::Poison)
            );
            element_undefined
                || match test {
                    MaskLaneTest::Zero => element.is_null_value(),
                    MaskLaneTest::One => element.is_all_ones_value(),
                }
        })
    };
    match quantifier {
        MaskQuantifier::Every => (0..lanes).all(lane_qualifies),
        MaskQuantifier::Any => (0..lanes).any(lane_qualifies),
    }
}

/// The lanes a masked operation might touch, given its `<N x i1>` mask.
///
/// Ports `llvm::possiblyDemandedEltsInMask` — an over-approximation, so every
/// lane is demanded unless the mask is a constant that proves otherwise.
/// `None` covers upstream's assert: the mask is not a fixed-width vector, so
/// there is no lane count to answer at.
///
/// **Stronger than upstream, in a sound direction.** Upstream reaches the
/// element loop only through `dyn_cast<ConstantVector>`, which a
/// `zeroinitializer` is not — it is a `ConstantAggregateZero` — so upstream
/// answers "every lane demanded" for an all-zero mask that demands none.
/// llvmkit stores both spellings as one element list, so the loop runs and the
/// answer is the exact zero. Over-approximating fewer lanes is always safe for
/// this query; the divergence can only make a caller more precise.
pub fn possibly_demanded_elements_in_mask<'ctx, B: ModuleBrand + 'ctx>(
    mask: Value<'ctx, B>,
) -> Option<ApInt> {
    let (_, lanes, scalable) = mask.ty().data().as_vector()?;
    if scalable {
        return None;
    }
    let mut demanded = ApInt::all_ones(lanes);
    if let ValueKindData::Constant(_) = &mask.data().kind {
        let constant = Constant::from_parts(mask);
        for lane in 0..lanes {
            // Upstream dereferences `getAggregateElement` without a null
            // check, which cannot fail for the `ConstantVector` its `dyn_cast`
            // admitted. An absent element here leaves the lane demanded, which
            // is the over-approximating direction.
            if constant
                .aggregate_element(lane)
                .is_some_and(Constant::is_null_value)
            {
                demanded.clear_bit(lane);
            }
        }
    }
    Some(demanded)
}

/// `<0, …, 0, 1, …, 1, …>` — each of `vectorization_factor` lanes repeated
/// `replication_factor` times.
///
/// Ports `llvm::createReplicatedMask`.
pub fn create_replicated_mask(
    replication_factor: u32,
    vectorization_factor: u32,
) -> Vec<ShuffleMaskElem> {
    let mut mask = Vec::with_capacity(
        (vectorization_factor as usize).saturating_mul(replication_factor as usize),
    );
    for lane in 0..vectorization_factor {
        for _ in 0..replication_factor {
            mask.push(ShuffleMaskElem::Lane(lane));
        }
    }
    mask
}

/// The mask that interleaves `vector_count` vectors of `vectorization_factor`
/// lanes: `<0, VF, 2VF, …, 1, VF+1, …>`.
///
/// Ports `llvm::createInterleaveMask`. `None` when a lane index would not fit
/// in a `u32`; upstream's `unsigned` arithmetic wraps there instead.
pub fn create_interleave_mask(
    vectorization_factor: u32,
    vector_count: u32,
) -> Option<Vec<ShuffleMaskElem>> {
    let mut mask =
        Vec::with_capacity((vectorization_factor as usize).saturating_mul(vector_count as usize));
    for lane in 0..vectorization_factor {
        for vector in 0..vector_count {
            mask.push(ShuffleMaskElem::Lane(
                vector
                    .checked_mul(vectorization_factor)?
                    .checked_add(lane)?,
            ));
        }
    }
    Some(mask)
}

/// `<start, start + stride, start + 2·stride, …>`, `vectorization_factor`
/// lanes long.
///
/// Ports `llvm::createStrideMask`. `None` when a lane index would not fit in a
/// `u32`.
pub fn create_stride_mask(
    start: u32,
    stride: u32,
    vectorization_factor: u32,
) -> Option<Vec<ShuffleMaskElem>> {
    let mut mask = Vec::with_capacity(vectorization_factor as usize);
    for step in 0..vectorization_factor {
        mask.push(ShuffleMaskElem::Lane(
            start.checked_add(step.checked_mul(stride)?)?,
        ));
    }
    Some(mask)
}

/// `defined_count` consecutive lanes from `start`, followed by `poison_count`
/// poison elements.
///
/// Ports `llvm::createSequentialMask`, whose `NumUndefs` tail is `-1` in
/// upstream's alphabet. `None` when a lane index would not fit in a `u32`.
pub fn create_sequential_mask(
    start: u32,
    defined_count: u32,
    poison_count: u32,
) -> Option<Vec<ShuffleMaskElem>> {
    let mut mask =
        Vec::with_capacity((defined_count as usize).saturating_add(poison_count as usize));
    for step in 0..defined_count {
        mask.push(ShuffleMaskElem::Lane(start.checked_add(step)?));
    }
    mask.extend(std::iter::repeat_n(
        ShuffleMaskElem::Poison,
        poison_count as usize,
    ));
    Some(mask)
}

/// Rewrite `mask` so it reads only the first operand, folding second-operand
/// lanes onto their counterparts.
///
/// Ports `llvm::createUnaryMask`: a mask element selecting lane `i` of the
/// second operand becomes lane `i` of the first. Poison elements are
/// unchanged. Upstream asserts the element count is non-zero and that every
/// mask element is in range for two operands; neither can make this answer
/// wrong, so an out-of-range element simply keeps its excess here.
pub fn create_unary_mask(mask: &[ShuffleMaskElem], lane_count: u32) -> Vec<ShuffleMaskElem> {
    mask.iter()
        .map(|element| match *element {
            ShuffleMaskElem::Lane(lane) if lane >= lane_count => {
                ShuffleMaskElem::Lane(lane - lane_count)
            }
            other => other,
        })
        .collect()
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

/// Rewrite `mask` for an element type `scale` times *narrower*, so the same
/// shuffle is expressed over `scale` times as many lanes.
///
/// Ports `llvm::narrowShuffleMaskElts`. Each source lane becomes `scale`
/// consecutive ones and each poison element becomes `scale` poison elements,
/// so the result is always `scale * mask.len()` long.
///
/// `None` covers upstream's two asserts, which llvmkit cannot spell as asserts
/// because the repo forbids panics on production paths: a `scale` of zero, and
/// a rewritten lane index that would not fit in the `int` upstream stores mask
/// elements in.
///
/// See [`widen_shuffle_mask_elements`] for the inverse, and
/// [`scale_shuffle_mask_elements`] for the wrapper that picks between them.
pub fn narrow_shuffle_mask_elements(
    scale: u32,
    mask: &[ShuffleMaskElem],
) -> Option<Vec<ShuffleMaskElem>> {
    if scale == 0 {
        return None;
    }
    // Fast-path: if no scaling, then it is just a copy.
    if scale == 1 {
        return Some(mask.to_vec());
    }

    let mut scaled = Vec::with_capacity(mask.len().checked_mul(usize::try_from(scale).ok()?)?);
    for element in mask {
        match *element {
            ShuffleMaskElem::Poison => {
                scaled.extend(std::iter::repeat_n(ShuffleMaskElem::Poison, scale as usize));
            }
            ShuffleMaskElem::Lane(lane) => {
                let base = u64::from(scale).checked_mul(u64::from(lane))?;
                // Upstream's `Scale * MaskElt + (Scale - 1) <= INT32_MAX`
                // assert: its mask elements are `int`, so a wider index has no
                // representation to be written into.
                let highest = base.checked_add(u64::from(scale) - 1)?;
                if highest > i32::MAX.unsigned_abs().into() {
                    return None;
                }
                for slice_element in 0..scale {
                    scaled.push(ShuffleMaskElem::Lane(
                        u32::try_from(base + u64::from(slice_element)).ok()?,
                    ));
                }
            }
        }
    }
    Some(scaled)
}

/// Rewrite `mask` for an element type `scale` times *wider*, so the same
/// shuffle is expressed over `scale` times fewer lanes.
///
/// Ports the `(int Scale, ArrayRef<int> Mask, …)` overload of
/// `llvm::widenShuffleMaskElts`, whose `false` becomes `None`: the mask does
/// not divide evenly by `scale`, or some group of `scale` lanes is not one
/// consecutive run starting at a multiple of `scale`, or is a mix of poison
/// and defined lanes. A group that is entirely poison widens to one poison.
///
/// **One upstream behaviour is unreachable here, permanently.** Upstream reads
/// mask elements as raw `int`s and requires negative ones to be *equal* across
/// a group — not merely all negative — because SelectionDAG and the X86
/// backend extend the alphabet past `-1` (`SM_SentinelZero` is `-2`). llvmkit
/// models the IR mask alphabet only, where the sole negative is `-1`, and its
/// own `shufflevector` validation rejects anything else. Code generation and
/// target backends are out of scope, so this is not a gap awaiting work: on the
/// alphabet llvmkit can represent, "all poison" and "all equal" are the same
/// predicate. Upstream's `{-1,-2,-1,-1}` and `{-2,-2,-3,-3}` fixtures have no
/// llvmkit spelling for that reason.
pub fn widen_shuffle_mask_elements(
    scale: u32,
    mask: &[ShuffleMaskElem],
) -> Option<Vec<ShuffleMaskElem>> {
    if scale == 0 {
        return None;
    }
    // Fast-path: if no scaling, then it is just a copy.
    if scale == 1 {
        return Some(mask.to_vec());
    }

    // We must map the original elements down evenly to a type with less
    // elements.
    let group = usize::try_from(scale).ok()?;
    if !mask.len().is_multiple_of(group) {
        return None;
    }

    let mut scaled = Vec::with_capacity(mask.len() / group);
    // Step through the input mask by splitting into `scale`-sized slices.
    // `chunks_exact` yields nothing for an empty mask, where upstream's
    // do-while would read past the end of one; a `shufflevector` cannot have
    // an empty mask, so the two agree everywhere the question arises.
    for slice in mask.chunks_exact(group) {
        // The first element of the slice determines how we evaluate this
        // slice.
        match slice[0] {
            // Poison must hold across the entire slice.
            ShuffleMaskElem::Poison => {
                if !slice.iter().all(|e| *e == ShuffleMaskElem::Poison) {
                    return None;
                }
                scaled.push(ShuffleMaskElem::Poison);
            }
            ShuffleMaskElem::Lane(front) => {
                // A defined mask element must be cleanly divisible.
                if !front.is_multiple_of(scale) {
                    return None;
                }
                // Elements of the slice must be consecutive.
                for (offset, element) in slice.iter().enumerate().skip(1) {
                    let expected = front.checked_add(u32::try_from(offset).ok()?)?;
                    if *element != ShuffleMaskElem::Lane(expected) {
                        return None;
                    }
                }
                scaled.push(ShuffleMaskElem::Lane(front / scale));
            }
        }
    }
    Some(scaled)
}

/// Halve `mask`'s lane count, tolerating one poison element in a pair.
///
/// Ports the `(ArrayRef<int> M, SmallVectorImpl<int> &NewMask)` overload of
/// `llvm::widenShuffleMaskElts` — a *different function* from
/// [`widen_shuffle_mask_elements`] with `scale` of 2, not a specialisation of
/// it. Where that one demands a whole group be poison or a whole group be one
/// consecutive run, this one accepts a pair that is half poison and takes the
/// defined half's answer: `(poison, odd)` and `(even, poison)` both widen.
///
/// `None` is upstream's `false`: an odd lane count, or a pair that is neither
/// wholly poison nor anchored on an even lane.
pub fn widen_shuffle_mask_elements_in_pairs(
    mask: &[ShuffleMaskElem],
) -> Option<Vec<ShuffleMaskElem>> {
    if !mask.len().is_multiple_of(2) {
        return None;
    }
    let mut widened = Vec::with_capacity(mask.len() / 2);
    for pair in mask.chunks_exact(2) {
        match (pair[0], pair[1]) {
            // If both elements are poison, the new element is poison too.
            (ShuffleMaskElem::Poison, ShuffleMaskElem::Poison) => {
                widened.push(ShuffleMaskElem::Poison);
            }
            // A poison low half is covered by an odd high half.
            (ShuffleMaskElem::Poison, ShuffleMaskElem::Lane(high)) if !high.is_multiple_of(2) => {
                widened.push(ShuffleMaskElem::Lane(high / 2));
            }
            // An even low half covers its successor, or a poison high half.
            (ShuffleMaskElem::Lane(low), high) if low.is_multiple_of(2) => {
                let follows =
                    matches!(high, ShuffleMaskElem::Lane(h) if low.checked_add(1) == Some(h));
                if !follows && high != ShuffleMaskElem::Poison {
                    return None;
                }
                widened.push(ShuffleMaskElem::Lane(low / 2));
            }
            _ => return None,
        }
    }
    Some(widened)
}

/// Rewrite `mask` to run over `destination_lanes` lanes instead of its own,
/// widening or narrowing as needed.
///
/// Ports `llvm::scaleShuffleMaskElts`, the wrapper that picks between
/// [`widen_shuffle_mask_elements`] and [`narrow_shuffle_mask_elements`].
/// `None` is upstream's `false` — a mask that cannot be widened — and also
/// covers its two asserts: a zero lane count on either side, and lane counts
/// that are not whole multiples of one another.
pub fn scale_shuffle_mask_elements(
    destination_lanes: u32,
    mask: &[ShuffleMaskElem],
) -> Option<Vec<ShuffleMaskElem>> {
    let source_lanes = u32::try_from(mask.len()).ok()?;
    if source_lanes == 0 || destination_lanes == 0 {
        return None;
    }

    // Fast-path: if no scaling, then it is just a copy.
    if source_lanes == destination_lanes {
        return Some(mask.to_vec());
    }

    // Ensure we can find a whole scale factor.
    if source_lanes > destination_lanes {
        if !source_lanes.is_multiple_of(destination_lanes) {
            return None;
        }
        return widen_shuffle_mask_elements(source_lanes / destination_lanes, mask);
    }
    if !destination_lanes.is_multiple_of(source_lanes) {
        return None;
    }
    narrow_shuffle_mask_elements(destination_lanes / source_lanes, mask)
}

/// The same shuffle written over the fewest lanes it can be.
///
/// Ports `llvm::getShuffleMaskWithWidestElts`: widen repeatedly, at every
/// factor from 2 upward, and hand back whatever survives. A mask that cannot
/// be widened at all comes back unchanged, so this is total where
/// [`widen_shuffle_mask_elements`] is not.
pub fn shuffle_mask_with_widest_elements(mask: &[ShuffleMaskElem]) -> Vec<ShuffleMaskElem> {
    let mut input = mask.to_vec();
    let mut scale = 2u32;
    while u32::try_from(input.len()).is_ok_and(|lanes| scale <= lanes) {
        while let Some(widened) = widen_shuffle_mask_elements(scale, &input) {
            input = widened;
        }
        scale += 1;
    }
    input
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
