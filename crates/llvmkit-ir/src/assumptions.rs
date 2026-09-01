//! `@llvm.assume` reasoning: which values a condition constrains, and where an
//! assumption may be used.
//!
//! Three of the four entry points are `ValueTracking.h`'s
//! (`findValuesAffectedByCondition`, `isValidAssumeForContext`,
//! `willNotFreeBetween`); the two caches port `llvm/Analysis/AssumptionCache.h`
//! and `llvm/Analysis/DomConditionCache.h`, which exist only to answer "which
//! assumes / branches mention this value" and are built by calling
//! [`find_values_affected_by_condition`]. They live here rather than in their
//! own modules because that is the whole of their content once LLVM's
//! `CallbackVH` invalidation machinery — which llvmkit has no counterpart for,
//! since a cache is rebuilt rather than repaired — is out of the picture.
//!
//! # What is not modeled, and why
//!
//! - **`AssumptionCache`'s operand-bundle half is partial.** Upstream records a
//!   `(assume, bundle index)` pair for every bundle whose tag is not `"ignore"`
//!   and decodes it through `getKnowledgeFromBundle`
//!   (`llvm/Analysis/AssumeBundleQueries.h`), which llvmkit does not port.
//!   [`AssumptionCache`] records the pairs — the indices are what
//!   `computeKnownBitsFromContext` keys on — but nothing here decodes one, so
//!   an alignment bundle contributes no known bits. The direction is
//!   conservative: a caller learns less, never something false.
//! - **`AssumptionCache`'s `TargetTransformInfo` arm** (`getPredicatedAddrSpace`)
//!   has no counterpart; llvmkit models no target.

use core::iter::FusedIterator;

use crate::attributes::{AttrIndex, AttrKind, AttributeStorage, AttributeStored};
use crate::basic_block::BasicBlockData;
use crate::cfg::kind_successor_ids;
use crate::constant::ConstantData;
use crate::dominator_tree::DominatorTree;
use crate::instr_types::{BinaryOpData, BranchKind, CallAttributeData, CastOpcode, SelectInstData};
use crate::instruction::{InstructionKindData, InstructionView};
use crate::intrinsics::descriptor_for_callee;
use crate::module::{ModuleBrand, ModuleRef};
use crate::pass_context::FunctionView;
use crate::speculation::{
    instruction_may_have_side_effects, instructions_transfer_execution_to_successor,
};
use crate::r#type::TypeKind;
use crate::value::{Value, ValueKindData, ValueSlot};
use crate::{ApInt, IntPredicate, IsValue};
use std::collections::{HashMap, HashSet};

/// How many instructions [`is_valid_assume_for_context`] will scan between a
/// context instruction and a later assume in the same block.
///
/// Ports the literal `15` upstream passes to
/// `isGuaranteedToTransferExecutionToSuccessor`, whose comment records that the
/// limit "is chosen arbitrarily, so it can be adjusted if needed".
const ASSUME_SCAN_LIMIT: u32 = 15;

/// How many instructions [`will_not_free_between`] inspects on each side of a
/// block boundary.
///
/// Ports `static constexpr unsigned MaxInstrsToCheckForFree = 16`
/// (`ValueTracking.cpp`).
const MAX_INSTRUCTIONS_TO_CHECK_FOR_FREE: usize = 16;

// --------------------------------------------------------------------------
// Affected values
// --------------------------------------------------------------------------

/// Call `insert_affected` on every value whose known bits — or value — may be
/// constrained by `condition`.
///
/// Ports `llvm::findValuesAffectedByCondition`. `is_assume` selects the caller:
/// `true` for an `@llvm.assume` argument, where both operands of a compare are
/// affected and the walk does not descend through `and`/`or` (an
/// `assume(A && B)` is split into two assumes before this runs); `false` for a
/// branch condition, where only the non-constant side of a compare is affected
/// and the walk does descend.
///
/// The upstream callback is `function_ref<void(Value *)>`, which may be called
/// more than once for the same value; de-duplication is the caller's job, as it
/// is upstream — [`AssumptionCache`] and [`DomConditionCache`] each do their
/// own.
pub fn find_values_affected_by_condition<'ctx, B, F>(
    condition: Value<'ctx, B>,
    is_assume: bool,
    mut insert_affected: F,
) where
    B: ModuleBrand + 'ctx,
    F: FnMut(Value<'ctx, B>),
{
    let mut worklist = vec![condition];
    let mut visited: HashSet<ValueSlot> = HashSet::new();

    while let Some(value) = worklist.pop() {
        if !visited.insert(value.slot()) {
            continue;
        }

        if is_assume {
            add_value_affected_by_condition(value, &mut insert_affected);
            if let Some(inner) = not_operand(value) {
                add_value_affected_by_condition(inner, &mut insert_affected);
            }
        }

        if let Some((lhs, rhs)) = logical_op_operands(value) {
            // `assume(A && B)` is split into `assume(A); assume(B)` before this
            // point, and `assume(A || B)` intersects rather than unions, so
            // neither is worth descending into. A branch condition has no such
            // split, so it is.
            if !is_assume {
                worklist.push(lhs);
                worklist.push(rhs);
            }
            continue;
        }

        if let Some(cmp) = int_compare_parts(value) {
            add_int_compare_affected(&cmp, is_assume, &mut insert_affected);
            continue;
        }

        if let Some((lhs, rhs)) = float_compare_operands(value) {
            add_compare_operands(lhs, rhs, is_assume, &mut insert_affected);

            // `fcmp fneg(x), y` / `fcmp fabs(x), y` / the two composed.
            let mut peeled = lhs;
            if let Some(inner) = float_negation_source(peeled) {
                peeled = inner;
                add_value_affected_by_condition(peeled, &mut insert_affected);
            }
            if let Some(inner) = float_absolute_source(peeled) {
                add_value_affected_by_condition(inner, &mut insert_affected);
            }
            continue;
        }

        if let Some(tested) = is_fpclass_operand(value) {
            add_value_affected_by_condition(tested, &mut insert_affected);
            continue;
        }

        if is_assume {
            continue;
        }

        // `trunc` and `not` are reached only for a branch condition: for an
        // assume both are already covered above, the first by
        // `add_value_affected_by_condition` peeking through unary operators and
        // the second by the `m_Not` arm, which upstream keeps off the worklist
        // to avoid ephemeral values.
        if let Some(source) = trunc_source(value) {
            add_value_affected_by_condition(source, &mut insert_affected);
        } else if let Some(inner) = not_operand(value) {
            worklist.push(inner);
        }
    }
}

/// The operands and predicate of an `icmp`, resolved to values.
struct IntCompareParts<'ctx, B: ModuleBrand> {
    predicate: IntPredicate,
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
}

/// The `m_ICmp` arm of [`find_values_affected_by_condition`].
fn add_int_compare_affected<'ctx, B, F>(
    cmp: &IntCompareParts<'ctx, B>,
    is_assume: bool,
    insert_affected: &mut F,
) where
    B: ModuleBrand + 'ctx,
    F: FnMut(Value<'ctx, B>),
{
    let has_rhs_constant = is_constant_int(cmp.rhs);
    let lhs = cmp.lhs;

    if cmp.predicate.is_equality() {
        add_value_affected_by_condition(lhs, insert_affected);
        if is_assume {
            add_value_affected_by_condition(cmp.rhs, insert_affected);
        }
        if has_rhs_constant {
            if let Some((source, _)) = shift_by_constant(lhs) {
                // `(X << C)`, `(X >>s C)` or `(X >>u C)`.
                add_value_affected_by_condition(source, insert_affected);
            } else if let Some((x, y)) = and_or_sub_operands(lhs) {
                // `(X & C)`, `(X | C)` or `X - Y`.
                add_value_affected_by_condition(x, insert_affected);
                add_value_affected_by_condition(y, insert_affected);
            }
        }
    } else {
        add_compare_operands(lhs, cmp.rhs, is_assume, insert_affected);
        if has_rhs_constant {
            // `(A + C1) u< C2` is the canonical form of `A > C3 && A < C4`.
            if let Some((x, _)) = add_like_by_constant(lhs) {
                add_value_affected_by_condition(x, insert_affected);
            }

            if !cmp.predicate.is_signed() && !cmp.predicate.is_equality() {
                // `X & Y u> C -> X u> C && Y u> C`,
                // `X | Y u< C -> X u< C && Y u< C`,
                // `X nuw+ Y u< C -> X u< C && Y u< C`.
                if let Some((x, y)) = and_or_nuw_add_operands(lhs) {
                    add_value_affected_by_condition(x, insert_affected);
                    add_value_affected_by_condition(y, insert_affected);
                }
                // `X nuw- Y u> C -> X u> C`.
                if let Some((x, _)) = nuw_sub_operands(lhs) {
                    add_value_affected_by_condition(x, insert_affected);
                }
            }
        }

        // `icmp slt/sgt (bitcast X to int), 0/-1`, which `computeKnownFPClass`
        // supports. Upstream calls `InsertAffected` directly here rather than
        // going through `addValueAffectedByCondition`, so the peek-through does
        // not apply; that difference is deliberate and is reproduced.
        if let Some(source) = element_wise_bitcast_source(lhs) {
            let matches = match cmp.predicate {
                IntPredicate::Slt => constant_int(cmp.rhs).is_some_and(|c| c.is_zero()),
                IntPredicate::Sgt => constant_int(cmp.rhs).is_some_and(|c| c.is_all_ones()),
                _ => false,
            };
            if matches {
                insert_affected(source);
            }
        }
    }

    if has_rhs_constant && let Some(source) = ctpop_operand(lhs) {
        add_value_affected_by_condition(source, insert_affected);
    }
}

/// Ports `addValueAffectedByCondition`: record `value` itself, then peek
/// through the two unary operators that do not change what a condition says
/// about the source.
fn add_value_affected_by_condition<'ctx, B, F>(value: Value<'ctx, B>, insert_affected: &mut F)
where
    B: ModuleBrand + 'ctx,
    F: FnMut(Value<'ctx, B>),
{
    match &value.data().kind {
        ValueKindData::Argument { .. }
        | ValueKindData::Function(_)
        | ValueKindData::GlobalAlias(_)
        | ValueKindData::GlobalIfunc(_)
        | ValueKindData::GlobalVariable(_) => insert_affected(value),
        ValueKindData::Instruction(_) => {
            insert_affected(value);
            let source = ptr_to_int_or_addr_source(value).or_else(|| trunc_source(value));
            if let Some(source) = source
                && matches!(
                    source.data().kind,
                    ValueKindData::Instruction(_) | ValueKindData::Argument { .. }
                )
            {
                insert_affected(source);
            }
        }
        _ => {}
    }
}

/// Ports the `AddCmpOperands` lambda: for an assume both operands are affected,
/// for a branch only the left one and only when the right is constant.
fn add_compare_operands<'ctx, B, F>(
    lhs: Value<'ctx, B>,
    rhs: Value<'ctx, B>,
    is_assume: bool,
    insert_affected: &mut F,
) where
    B: ModuleBrand + 'ctx,
    F: FnMut(Value<'ctx, B>),
{
    if is_assume {
        add_value_affected_by_condition(lhs, insert_affected);
        add_value_affected_by_condition(rhs, insert_affected);
    } else if is_constant(rhs) {
        add_value_affected_by_condition(lhs, insert_affected);
    }
}

// --------------------------------------------------------------------------
// Assumption validity
// --------------------------------------------------------------------------

/// Whether the assumption `assume` may be used at `context`.
///
/// Ports `llvm::isValidAssumeForContext`. There are two restrictions, and
/// upstream's comment names both: the assume must dominate the context — or
/// control flow must reach the assume whenever it reaches the context — and the
/// context must not be one of the assume's *ephemeral* values, since using the
/// assume to prove its own condition would delete the assume.
///
/// `allow_ephemerals` lifts the second restriction. A caller that can rule out
/// the circularity itself gets more valid assumptions; upstream's own use is
/// the alignment-bundle arm of `computeKnownBitsFromContext`, where the context
/// may legitimately be the instruction that produced the bundled pointer.
///
/// Without a `dominator_tree` the cross-block case falls back to the two shapes
/// that dominate trivially: the assume is in `context`'s single predecessor, or
/// in the entry block.
pub fn is_valid_assume_for_context<'ctx, B: ModuleBrand + 'ctx>(
    assume: &InstructionView<'ctx, B>,
    context: &InstructionView<'ctx, B>,
    dominator_tree: Option<&DominatorTree>,
    allow_ephemerals: bool,
) -> bool {
    let assume_block = assume.parent().slot();
    let context_block = context.parent().slot();
    let anchor = assume.to_erased();

    if assume_block == context_block {
        // The assume runs first: nothing in between can matter.
        if instruction_comes_before(anchor, assume_block, assume.slot(), context.slot()) {
            return true;
        }

        // Don't let an assume affect itself — that is exactly the circularity
        // the ephemeral-value test exists to prevent, and it would also make
        // the scan below run off the end of the block.
        if !allow_ephemerals && assume.slot() == context.slot() {
            return false;
        }

        // The context comes first. Everything from it up to (but not
        // including) the assume must transfer execution to its successor, or
        // the assume might not be reached.
        if !instructions_between_transfer(anchor, assume_block, context.slot(), assume.slot()) {
            return false;
        }

        return allow_ephemerals || !is_ephemeral_value_of(anchor, assume.slot(), context.slot());
    }

    if let Some(dominator_tree) = dominator_tree {
        return dominator_tree.dominates_instruction(assume, context);
    }

    // No dominator tree, but these two shapes dominate trivially.
    single_predecessor(anchor, context_block) == Some(assume_block)
        || is_entry_block(anchor, assume_block)
}

/// Whether nothing between `assume` and `context` may free memory.
///
/// Ports `llvm::willNotFreeBetween`. The enclosing function must be `nosync` —
/// upstream's comment: that "ensures the current function cannot arrange for
/// another thread to free on its behalf" — and every call in the range must be
/// `nofree`.
///
/// Two block layouts are accepted, matching upstream: `context` after `assume`
/// in the same block, or `context` in a block whose single predecessor is the
/// assume's, in which case both halves of the split range are scanned.
pub fn will_not_free_between<'ctx, B: ModuleBrand + 'ctx>(
    assume: &InstructionView<'ctx, B>,
    context: &InstructionView<'ctx, B>,
) -> bool {
    let anchor = assume.to_erased();
    let assume_block = assume.parent().slot();
    let context_block = context.parent().slot();

    if !enclosing_function_has_attribute(anchor, context_block, AttrKind::NoSync) {
        return false;
    }

    if assume_block != context_block {
        if single_predecessor(anchor, context_block) != Some(assume_block) {
            return false;
        }
        // The context block's leading half: everything before `context`.
        let leading = block_instructions(anchor, context_block);
        let Some(context_index) = leading.iter().position(|slot| *slot == context.slot()) else {
            return false;
        };
        if !has_no_free_calls(anchor, &leading[..context_index]) {
            return false;
        }
        // ... and then the assume's block from the assume to its end.
        let assume_range = block_instructions(anchor, assume_block);
        let Some(assume_index) = assume_range.iter().position(|slot| *slot == assume.slot()) else {
            return false;
        };
        return has_no_free_calls(anchor, &assume_range[assume_index..]);
    }

    let instructions = block_instructions(anchor, assume_block);
    let (Some(assume_index), Some(context_index)) = (
        instructions.iter().position(|slot| *slot == assume.slot()),
        instructions.iter().position(|slot| *slot == context.slot()),
    ) else {
        return false;
    };
    if assume_index >= context_index {
        return false;
    }
    has_no_free_calls(anchor, &instructions[assume_index..context_index])
}

/// Ports the `hasNoFreeCalls` lambda. Upstream's `Idx > MaxInstrsToCheckForFree`
/// bails *after* looking at instruction 17, so the limit is inclusive of it.
fn has_no_free_calls<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    instructions: &[ValueSlot],
) -> bool {
    for (index, slot) in instructions.iter().enumerate() {
        if index > MAX_INSTRUCTIONS_TO_CHECK_FOR_FREE {
            return false;
        }
        let instruction = value_from_slot(anchor, *slot);
        let Some(kind) = instruction_kind(instruction) else {
            continue;
        };
        let Some((callee, attrs)) = call_parts(kind) else {
            continue;
        };
        if !call_site_has_fn_attr(anchor, callee, attrs, AttrKind::NoFree) {
            return false;
        }
    }
    true
}

/// Ports `isEphemeralValueOf`: whether `context` exists only to feed `assume`.
///
/// The worklist walks backwards from the assume through operands, admitting a
/// value once every one of its users is already known ephemeral. Upstream's
/// comment on the first test — "The instruction defining an assumption's
/// condition itself is always considered ephemeral to that assumption (even if
/// it has other non-ephemeral users)" — is load-bearing and is reproduced.
fn is_ephemeral_value_of<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    assume: ValueSlot,
    context: ValueSlot,
) -> bool {
    let assume_value = value_from_slot(anchor, assume);
    if let Some(kind) = instruction_kind(assume_value)
        && kind.operand_ids().contains(&context)
    {
        return true;
    }

    let mut worklist = vec![assume];
    let mut visited: HashSet<ValueSlot> = HashSet::new();
    let mut ephemeral: HashSet<ValueSlot> = HashSet::new();

    while let Some(slot) = worklist.pop() {
        if !visited.insert(slot) {
            continue;
        }
        let value = value_from_slot(anchor, slot);

        // Ephemeral only when every user already is.
        if !value
            .users()
            .all(|user| ephemeral.contains(&user.to_erased().slot()))
        {
            continue;
        }

        if slot == context {
            return true;
        }

        let Some(kind) = instruction_kind(value) else {
            continue;
        };
        if slot != assume && (may_have_side_effects(value, kind) || kind.is_terminator()) {
            continue;
        }
        ephemeral.insert(slot);
        for operand in kind.operand_ids() {
            let operand_value = value_from_slot(anchor, operand);
            if matches!(operand_value.data().kind, ValueKindData::Instruction(_)) {
                worklist.push(operand);
            }
        }
    }

    false
}

// --------------------------------------------------------------------------
// Caches
// --------------------------------------------------------------------------

/// Which operand bundle of an assume produced an entry.
///
/// Ports the `Index` half of `AssumptionCache::ResultElem`, whose sentinel
/// `ExprResultIdx = -1` distinguishes "the assume's condition mentions this
/// value" from "operand bundle N is about this value". A sentinel index becomes
/// a variant, per the standing translation rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssumptionSource {
    /// The assume's condition operand mentions the value.
    ///
    /// Ports `AssumptionCache::ExprResultIdx`.
    Condition,
    /// The assume's operand bundle at this index is about the value.
    Bundle(usize),
}

/// One assumption that mentions a value.
///
/// Ports `AssumptionCache::ResultElem`. `assume` is the `@llvm.assume` call
/// itself; upstream stores a `WeakVH` that may have been nulled out, which the
/// consumer skips — llvmkit has no dangling-handle state, so there is no
/// counterpart to that check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Assumption {
    assume: ValueSlot,
    source: AssumptionSource,
}

impl Assumption {
    /// The `@llvm.assume` call, as a view onto `module`.
    pub fn assume<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        module: ModuleRef<'ctx, B>,
    ) -> Option<InstructionView<'ctx, B>> {
        let data = module.value_data(self.assume);
        InstructionView::try_from(Value::from_parts(self.assume, module, data.ty)).ok()
    }

    /// Which part of the assume mentions the value this was looked up by.
    pub fn source(&self) -> AssumptionSource {
        self.source
    }
}

/// The `@llvm.assume` calls in a function, indexed by the values they mention.
///
/// Ports `llvm::AssumptionCache`. Built once by scanning a function
/// ([`AssumptionCache::new`], upstream's `scanFunction`); llvmkit has no
/// counterpart to upstream's incremental `registerAssumption` /
/// `unregisterAssumption` pair, because a `CallbackVH`-driven cache that
/// repairs itself as the IR changes has no place in a crate where an analysis
/// result is invalidated and recomputed.
#[derive(Debug, Default, Clone)]
pub struct AssumptionCache {
    affected: HashMap<ValueSlot, Vec<Assumption>>,
    assumes: Vec<ValueSlot>,
}

impl AssumptionCache {
    /// Scan `function` for `@llvm.assume` calls and index what they mention.
    ///
    /// Ports `AssumptionCache::scanFunction` together with the
    /// `updateAffectedValues` it ends with.
    pub fn new<'ctx, B: ModuleBrand + 'ctx>(function: FunctionView<'ctx, B>) -> Self {
        let mut cache = Self::default();
        for block in function.basic_blocks() {
            for instruction in block.instructions() {
                let value = instruction.to_erased();
                if !is_assume_call(value) {
                    continue;
                }
                cache.assumes.push(instruction.slot());
                cache.update_affected_values(value);
            }
        }
        cache
    }

    /// Every `@llvm.assume` in the scanned function, in program order.
    ///
    /// Ports `AssumptionCache::assumptions`. Upstream's `WeakVH` list can hold
    /// records whose instruction has since gone away; here the equivalent is a
    /// slot that no longer resolves as an instruction, and such entries are
    /// skipped — which is why this is **not** an [`ExactSizeIterator`]: the
    /// count is only known after the walk.
    ///
    /// The slot list is snapshotted (a `Vec<ValueSlot>` clone, no IR touched)
    /// so the receiver stays out of the returned opaque type and the iterator
    /// chains off a borrowed cache.
    ///
    /// [`ExactSizeIterator`]: core::iter::ExactSizeIterator
    pub fn assumptions<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        module: ModuleRef<'ctx, B>,
    ) -> impl DoubleEndedIterator<Item = InstructionView<'ctx, B>> + FusedIterator + use<'ctx, B>
    {
        let assumes = self.assumes.clone();
        assumes.into_iter().filter_map(move |slot| {
            let data = module.value_data(slot);
            InstructionView::try_from(Value::from_parts(slot, module, data.ty)).ok()
        })
    }

    /// The assumptions that mention `value`.
    ///
    /// Ports `AssumptionCache::assumptionsFor`.
    pub fn assumptions_for<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        value: Value<'ctx, B>,
    ) -> &[Assumption] {
        self.affected
            .get(&value.slot())
            .map_or(&[], |entries| entries.as_slice())
    }

    /// Ports `AssumptionCache::updateAffectedValues`, whose own `findAffectedValues`
    /// helper is inlined here — it is one loop over the bundles plus one call to
    /// [`find_values_affected_by_condition`].
    fn update_affected_values<'ctx, B: ModuleBrand + 'ctx>(&mut self, assume: Value<'ctx, B>) {
        let mut affected: Vec<(ValueSlot, AssumptionSource)> = Vec::new();

        for (index, bundle) in assume_bundle_operands(assume).into_iter().enumerate() {
            for operand in bundle {
                affected.push((operand.slot(), AssumptionSource::Bundle(index)));
            }
        }

        if let Some(condition) = assume_condition(assume) {
            find_values_affected_by_condition(condition, true, |value| {
                affected.push((value.slot(), AssumptionSource::Condition));
            });
        }

        let assume_slot = assume.slot();
        for (slot, source) in affected {
            let entries = self.affected.entry(slot).or_default();
            let record = Assumption {
                assume: assume_slot,
                source,
            };
            if !entries.contains(&record) {
                entries.push(record);
            }
        }
    }
}

/// The branch conditions that constrain a value, keyed by the value.
///
/// Ports `llvm::DomConditionCache`. Upstream's own header records that this,
/// unlike [`AssumptionCache`], "does not perform any automatic analysis or
/// invalidation" — the caller registers the branches it cares about. That is
/// reproduced: there is no `new(function)` constructor, only
/// [`register_branch`](Self::register_branch).
///
/// `removeValue` has no counterpart. It exists upstream so a caller can drop an
/// entry for a value it is about to erase from a cache it keeps across edits;
/// dropping and rebuilding is the llvmkit shape.
#[derive(Debug, Default, Clone)]
pub struct DomConditionCache {
    affected: HashMap<ValueSlot, Vec<ValueSlot>>,
}

impl DomConditionCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Index `branch`'s condition under every value it constrains.
    ///
    /// Ports `DomConditionCache::registerBranch`. Upstream asserts the branch
    /// is conditional; an unconditional one has no condition to index, so it is
    /// accepted and ignored rather than made a panic in a production path.
    pub fn register_branch<'ctx, B: ModuleBrand + 'ctx>(&mut self, branch: Value<'ctx, B>) {
        let Some(condition) = conditional_branch_condition(branch) else {
            return;
        };
        let branch_slot = branch.slot();
        find_values_affected_by_condition(condition, false, |value| {
            let entries = self.affected.entry(value.slot()).or_default();
            if !entries.contains(&branch_slot) {
                entries.push(branch_slot);
            }
        });
    }

    /// The registered branches whose conditions constrain `value`.
    ///
    /// Ports `DomConditionCache::conditionsFor`. A value with no registered
    /// branch yields nothing, exactly as upstream's empty `ArrayRef` does.
    ///
    /// The slot list is snapshotted (a `Vec<ValueSlot>` clone) rather than
    /// borrowed, so the receiver stays out of the returned opaque type; every
    /// slot maps to a value, so the count is exact.
    pub fn conditions_for<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        value: Value<'ctx, B>,
    ) -> impl ExactSizeIterator<Item = Value<'ctx, B>> + DoubleEndedIterator + FusedIterator + use<'ctx, B>
    {
        let slots = self
            .affected
            .get(&value.slot())
            .cloned()
            .unwrap_or_default();
        slots
            .into_iter()
            .map(move |slot| value_from_slot(value, slot))
    }
}

// --------------------------------------------------------------------------
// IR navigation
// --------------------------------------------------------------------------

/// Whether `earlier` precedes `later` in `block`. Ports `Instruction::comesBefore`,
/// whose order cache llvmkit has no counterpart for — the instruction list is
/// the order.
fn instruction_comes_before<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
    earlier: ValueSlot,
    later: ValueSlot,
) -> bool {
    let instructions = block_instructions(anchor, block);
    let (Some(earlier_index), Some(later_index)) = (
        instructions.iter().position(|slot| *slot == earlier),
        instructions.iter().position(|slot| *slot == later),
    ) else {
        return false;
    };
    earlier_index < later_index
}

/// Whether every instruction from `from` (inclusive) up to `to` (exclusive)
/// transfers execution to its successor.
///
/// Ports the `make_range(CxtI->getIterator(), Inv->getIterator())` argument
/// upstream hands `isGuaranteedToTransferExecutionToSuccessor` together with
/// its [`ASSUME_SCAN_LIMIT`].
fn instructions_between_transfer<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
    from: ValueSlot,
    to: ValueSlot,
) -> bool {
    let module: ModuleRef<B> = ModuleRef::new(anchor.module().core_ref());
    let instructions = block_instructions(anchor, block);
    let (Some(from_index), Some(to_index)) = (
        instructions.iter().position(|slot| *slot == from),
        instructions.iter().position(|slot| *slot == to),
    ) else {
        return false;
    };
    if from_index > to_index {
        return false;
    }
    let range = instructions[from_index..to_index]
        .iter()
        .filter_map(|slot| {
            let data = module.value_data(*slot);
            InstructionView::try_from(Value::from_parts(*slot, module, data.ty)).ok()
        });
    instructions_transfer_execution_to_successor(range, ASSUME_SCAN_LIMIT)
}

/// `block`'s instruction list, copied out so the `RefCell` borrow does not
/// outlive the call.
fn block_instructions<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
) -> Vec<ValueSlot> {
    with_block_data(anchor, block, |data| data.instructions.borrow().clone()).unwrap_or_default()
}

/// The block's single predecessor, or `None` when it has zero or several.
/// Ports `BasicBlock::getSinglePredecessor`.
pub(crate) fn single_predecessor<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
) -> Option<ValueSlot> {
    let parent = with_block_data(anchor, block, |data| *data.parent.borrow())??;
    let function = value_from_slot(anchor, parent);
    let ValueKindData::Function(data) = &function.data().kind else {
        return None;
    };
    let blocks = data.basic_blocks.borrow().clone();
    let mut found = None;
    for candidate in blocks {
        let Some(terminator) = terminator_of_block(anchor, candidate) else {
            continue;
        };
        let Some(kind) = instruction_kind(terminator) else {
            continue;
        };
        if !kind_successor_ids(kind).contains(&block) {
            continue;
        }
        if found.is_some_and(|previous| previous != candidate) {
            return None;
        }
        found = Some(candidate);
    }
    found
}

/// Whether `block` is its function's entry block. Ports `BasicBlock::isEntryBlock`.
fn is_entry_block<'ctx, B: ModuleBrand + 'ctx>(anchor: Value<'ctx, B>, block: ValueSlot) -> bool {
    let Some(Some(parent)) = with_block_data(anchor, block, |data| *data.parent.borrow()) else {
        return false;
    };
    let function = value_from_slot(anchor, parent);
    let ValueKindData::Function(data) = &function.data().kind else {
        return false;
    };
    data.basic_blocks.borrow().first() == Some(&block)
}

/// Whether the function containing `block` carries `attribute`.
fn enclosing_function_has_attribute<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
    attribute: AttrKind,
) -> bool {
    let Some(Some(parent)) = with_block_data(anchor, block, |data| *data.parent.borrow()) else {
        return false;
    };
    let function = value_from_slot(anchor, parent);
    let ValueKindData::Function(data) = &function.data().kind else {
        return false;
    };
    storage_has_enum_attr(&data.attributes.borrow(), AttrIndex::Function, attribute)
}

/// Run `f` over `block`'s [`BasicBlockData`], or answer `None` when the slot is
/// not a block.
fn with_block_data<'ctx, B, T, F>(anchor: Value<'ctx, B>, block: ValueSlot, f: F) -> Option<T>
where
    B: ModuleBrand + 'ctx,
    F: FnOnce(&BasicBlockData) -> T,
{
    let module: ModuleRef<B> = ModuleRef::new(anchor.module().core_ref());
    match &module.value_data(block).kind {
        ValueKindData::BasicBlock(data) => Some(f(data)),
        _ => None,
    }
}

/// The terminator of `block`, as a value.
pub(crate) fn terminator_of_block<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
) -> Option<Value<'ctx, B>> {
    let terminator = *block_instructions(anchor, block).last()?;
    Some(value_from_slot(anchor, terminator))
}

// --------------------------------------------------------------------------
// Pattern helpers
// --------------------------------------------------------------------------

/// The condition of a conditional `br`. Ports the `m_Br` half of
/// `DomConditionCache::registerBranch`'s precondition.
fn conditional_branch_condition<'ctx, B: ModuleBrand + 'ctx>(
    branch: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Br(data) = instruction_kind(branch)? else {
        return None;
    };
    let condition = match &*data.kind.borrow() {
        BranchKind::Unconditional(_) => return None,
        BranchKind::Conditional { cond, .. } => cond.get(),
    };
    Some(value_from_slot(branch, condition))
}

/// Whether `value` is a call to `@llvm.assume`.
fn is_assume_call<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    let Some(InstructionKindData::Call(data)) = instruction_kind(value) else {
        return false;
    };
    let callee = value_from_slot(value, data.callee.get());
    descriptor_for_callee(callee)
        .is_some_and(|descriptor| descriptor.id().base_name() == "llvm.assume")
}

/// The condition operand of an `@llvm.assume`.
fn assume_condition<'ctx, B: ModuleBrand + 'ctx>(assume: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Call(data) = instruction_kind(assume)? else {
        return None;
    };
    Some(value_from_slot(assume, data.args.first()?.get()))
}

/// The operand bundles of an `@llvm.assume`, as values, one inner `Vec` per
/// bundle in declaration order.
///
/// Ports the shape `AssumptionCache::findValuesAffectedByOperandBundle` reads
/// but not its filtering: upstream keeps only `Bundle.Inputs[ABA_WasOn]` (and
/// the two underlying objects of a `separate_storage` bundle), and skips the
/// `"ignore"` tag. Those choices belong with `getKnowledgeFromBundle`, which is
/// not ported — see the module header — so every input is recorded and the
/// index is what a consumer keys on.
fn assume_bundle_operands<'ctx, B: ModuleBrand + 'ctx>(
    assume: Value<'ctx, B>,
) -> Vec<Vec<Value<'ctx, B>>> {
    let Some(InstructionKindData::Call(data)) = instruction_kind(assume) else {
        return Vec::new();
    };
    data.attrs
        .operand_bundles_slice()
        .iter()
        .map(|bundle| {
            bundle
                .inputs()
                .map(|input| value_from_slot(assume, input))
                .collect()
        })
        .collect()
}

/// The two operands of a logical `and`/`or`, in either the bitwise or the
/// `select` spelling. Ports `m_LogicalOp`, whose `LogicalOp_match` requires an
/// `i1` result and, for the `select` forms, a condition of the same type.
fn logical_op_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    if !value.ty().is_int_or_int_vector_of_width(1) {
        return None;
    }
    match instruction_kind(value)? {
        InstructionKindData::And(data) | InstructionKindData::Or(data) => Some((
            value_from_slot(value, data.lhs.get()),
            value_from_slot(value, data.rhs.get()),
        )),
        InstructionKindData::Select(data) => logical_select_operands(value, data),
        _ => None,
    }
}

/// The `select` spellings of a logical operator: `L ? R : false` is `L && R`,
/// `L ? true : R` is `L || R`.
fn logical_select_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    data: &SelectInstData,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    let condition = value_from_slot(value, data.cond.get());
    if condition.ty().id() != value.ty().id() {
        return None;
    }
    let true_value = value_from_slot(value, data.true_val.get());
    let false_value = value_from_slot(value, data.false_val.get());
    if constant_int(false_value).is_some_and(|c| c.is_zero()) {
        return Some((condition, true_value));
    }
    if constant_int(true_value).is_some_and(|c| c.is_all_ones()) {
        return Some((condition, false_value));
    }
    None
}

/// The predicate and operands of an `icmp`.
fn int_compare_parts<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<IntCompareParts<'ctx, B>> {
    let InstructionKindData::Icmp(data) = instruction_kind(value)? else {
        return None;
    };
    Some(IntCompareParts {
        predicate: data.predicate,
        lhs: value_from_slot(value, data.lhs.get()),
        rhs: value_from_slot(value, data.rhs.get()),
    })
}

/// The operands of an `fcmp`.
fn float_compare_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    let InstructionKindData::Fcmp(data) = instruction_kind(value)? else {
        return None;
    };
    Some((
        value_from_slot(value, data.lhs.get()),
        value_from_slot(value, data.rhs.get()),
    ))
}

/// The operand of `xor X, -1`. Ports `m_Not`.
fn not_operand<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Xor(data) = instruction_kind(value)? else {
        return None;
    };
    let lhs = value_from_slot(value, data.lhs.get());
    let rhs = value_from_slot(value, data.rhs.get());
    if constant_int(rhs).is_some_and(|c| c.is_all_ones()) {
        return Some(lhs);
    }
    if constant_int(lhs).is_some_and(|c| c.is_all_ones()) {
        return Some(rhs);
    }
    None
}

/// The source of a `trunc`. Ports `m_Trunc`.
fn trunc_source<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    cast_source_with_opcode(value, |kind| kind == CastOpcode::Trunc)
}

/// The source of a `ptrtoint` or `ptrtoaddr`. Ports `m_PtrToIntOrAddr`.
fn ptr_to_int_or_addr_source<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    cast_source_with_opcode(value, |kind| {
        matches!(kind, CastOpcode::PtrToInt | CastOpcode::PtrToAddr)
    })
}

/// The source of a `bitcast` that changes neither scalar-vs-vector nor the
/// element count. Ports `m_ElementWiseBitCast`.
fn element_wise_bitcast_source<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let source = cast_source_with_opcode(value, |kind| kind == CastOpcode::BitCast)?;
    if vector_shape(source) != vector_shape(value) {
        return None;
    }
    Some(source)
}

/// The lane count and scalability of a vector-typed value, or `None` for a
/// scalar. A fixed and a scalable vector of the same count differ, which is the
/// `getElementCount()` comparison upstream makes.
fn vector_shape<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<(u32, bool)> {
    value
        .ty()
        .data()
        .as_vector()
        .map(|(_, lanes, scalable)| (lanes, scalable))
}

/// The source of a cast whose opcode `accepts`.
fn cast_source_with_opcode<'ctx, B, F>(value: Value<'ctx, B>, accepts: F) -> Option<Value<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
    F: FnOnce(CastOpcode) -> bool,
{
    let InstructionKindData::Cast(data) = instruction_kind(value)? else {
        return None;
    };
    accepts(data.kind).then(|| value_from_slot(value, data.src.get()))
}

/// The source and shift amount of `shl`/`lshr`/`ashr` by a constant. Ports
/// `m_Shift(m_Value(X), m_ConstantInt())`.
fn shift_by_constant<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, ApInt)> {
    let data = match instruction_kind(value)? {
        InstructionKindData::Shl(data)
        | InstructionKindData::Lshr(data)
        | InstructionKindData::Ashr(data) => data,
        _ => return None,
    };
    let amount = constant_int(value_from_slot(value, data.rhs.get()))?;
    Some((value_from_slot(value, data.lhs.get()), amount))
}

/// The operands of `and`, `or` or `sub`. Ports the three-way alternation in the
/// equality arm of `findValuesAffectedByCondition`.
fn and_or_sub_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    match instruction_kind(value)? {
        InstructionKindData::And(data)
        | InstructionKindData::Or(data)
        | InstructionKindData::Sub(data) => Some(binary_operands(value, data)),
        _ => None,
    }
}

/// The operands of `and`, `or` or `add nuw`. Ports the alternation in the
/// unsigned arm.
fn and_or_nuw_add_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    match instruction_kind(value)? {
        InstructionKindData::And(data) | InstructionKindData::Or(data) => {
            Some(binary_operands(value, data))
        }
        InstructionKindData::Add(data) => {
            data.no_unsigned_wrap.then(|| binary_operands(value, data))
        }
        _ => None,
    }
}

/// The operands of `sub nuw`. Ports `m_NUWSub`.
fn nuw_sub_operands<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, Value<'ctx, B>)> {
    let InstructionKindData::Sub(data) = instruction_kind(value)? else {
        return None;
    };
    data.no_unsigned_wrap.then(|| binary_operands(value, data))
}

/// The non-constant operand of `add X, C` or `or disjoint X, C`. Ports
/// `m_AddLike(m_Value(X), m_ConstantInt())`.
fn add_like_by_constant<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(Value<'ctx, B>, ApInt)> {
    let data = match instruction_kind(value)? {
        InstructionKindData::Add(data) => data,
        InstructionKindData::Or(data) if data.disjoint => data,
        _ => return None,
    };
    let (lhs, rhs) = binary_operands(value, data);
    let constant = constant_int(rhs)?;
    Some((lhs, constant))
}

/// The operand of `fneg`, or of an `fsub` against negative zero.
fn float_negation_source<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Fneg(data) = instruction_kind(value)? else {
        return None;
    };
    Some(value_from_slot(value, data.src.get()))
}

/// The operand of `@llvm.fabs`.
fn float_absolute_source<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    intrinsic_first_argument(value, "llvm.fabs")
}

/// The operand of `@llvm.ctpop`.
fn ctpop_operand<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<Value<'ctx, B>> {
    intrinsic_first_argument(value, "llvm.ctpop")
}

/// The tested operand of `@llvm.is.fpclass`.
fn is_fpclass_operand<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    intrinsic_first_argument(value, "llvm.is.fpclass")
}

/// The first argument of a call to the intrinsic named `base_name`.
fn intrinsic_first_argument<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    base_name: &str,
) -> Option<Value<'ctx, B>> {
    let InstructionKindData::Call(data) = instruction_kind(value)? else {
        return None;
    };
    let callee = value_from_slot(value, data.callee.get());
    let descriptor = descriptor_for_callee(callee)?;
    if descriptor.id().base_name() != base_name {
        return None;
    }
    Some(value_from_slot(value, data.args.first()?.get()))
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

/// Whether `value` is any constant. Ports `m_Constant`.
fn is_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    matches!(value.data().kind, ValueKindData::Constant(_))
}

/// Whether `value` is a constant integer, scalar or splat. Ports
/// `m_ConstantInt()` in its no-capture form.
fn is_constant_int<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> bool {
    constant_int(value).is_some()
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

/// Whether the instruction may have side effects. Ports
/// `Instruction::mayHaveSideEffects`, which is `mayWriteToMemory() ||
/// mayThrow() || !willReturn()`; [`crate::speculation`] owns those three.
fn may_have_side_effects<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    kind: &InstructionKindData,
) -> bool {
    instruction_may_have_side_effects(value, kind)
}

/// Whether the call site or its callee carries `attribute`. Ports
/// `CallBase::hasFnAttr`.
fn call_site_has_fn_attr<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    callee: ValueSlot,
    attrs: &CallAttributeData,
    attribute: AttrKind,
) -> bool {
    if storage_has_enum_attr(attrs.function_attrs(), AttrIndex::Function, attribute) {
        return true;
    }
    let callee = value_from_slot(anchor, callee);
    let ValueKindData::Function(data) = &callee.data().kind else {
        return false;
    };
    if storage_has_enum_attr(&data.attributes.borrow(), AttrIndex::Function, attribute) {
        return true;
    }
    match descriptor_for_callee(callee).map(|descriptor| descriptor.id()) {
        Some(id) if attribute == AttrKind::NoFree => id.no_free(),
        _ => false,
    }
}

/// The callee and call-site attributes of a call-like instruction.
fn call_parts(kind: &InstructionKindData) -> Option<(ValueSlot, &CallAttributeData)> {
    match kind {
        InstructionKindData::Call(data) => Some((data.callee.get(), &data.attrs)),
        InstructionKindData::Invoke(data) => Some((data.callee.get(), &data.attrs)),
        InstructionKindData::CallBr(data) => Some((data.callee.get(), &data.attrs)),
        _ => None,
    }
}

/// Whether `storage` carries the enum attribute `attribute` at `index`.
fn storage_has_enum_attr(
    storage: &AttributeStorage,
    index: AttrIndex,
    attribute: AttrKind,
) -> bool {
    storage.get(index).is_some_and(|stored| {
        stored
            .iter()
            .any(|entry| matches!(entry, AttributeStored::Enum(kind) if *kind == attribute))
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
