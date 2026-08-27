//! Speculation safety and UB reachability.
//!
//! Mirrors the `isSafeToSpeculativelyExecute` /
//! `isGuaranteedToTransferExecutionToSuccessor` / `programUndefinedIfPoison`
//! slice of `llvm/lib/Analysis/ValueTracking.cpp`, together with the
//! `Instruction.cpp` predicates those functions lean on (`mayThrow`,
//! `willReturn`, `mayReadFromMemory`, `mayWriteToMemory`).
//!
//! Split out of [`value_tracking`](crate::value_tracking) rather than added to
//! it because the questions are different in kind: everything here is about
//! *control flow and effects* — what may trap, what is guaranteed to run —
//! where `value_tracking` reasons about the bits a value can hold. The two meet
//! at one place, [`program_undefined_if_undef_or_poison`], which
//! `is_known_not_undef_or_poison` calls.
//!
//! # What is not modeled, and why
//!
//! - **The `load` arm of [`is_safe_to_speculatively_execute`] answers `false`
//!   unconditionally.** Upstream continues into
//!   `isDereferenceableAndAlignedPointer` (`llvm/lib/Analysis/Loads.cpp`),
//!   which llvmkit does not port. `false` is the conservative direction — a
//!   caller is told not to hoist a load it might in fact be able to hoist.
//! - **Consequently these entry points take no context instruction, assumption
//!   cache, dominator tree or `TargetLibraryInfo`.** Upstream threads all four
//!   through *only* to reach that one call. Taking parameters that provably
//!   cannot change the answer would be dishonest; when the `Loads.cpp` port
//!   lands, the signature grows.
//! - `isGuaranteedToTransferExecutionToSuccessor`'s `catchpad` arm consults
//!   `classifyEHPersonality`. llvmkit models the personality function as a
//!   value but not its *classification*, so that arm takes upstream's
//!   conservative `default`: a `catchpad` is assumed not to transfer.

use crate::atomic_ordering::AtomicOrdering;
use crate::attributes::{AttrIndex, AttrKind, AttributeStorage, AttributeStored, MemoryEffects};
use crate::cfg::kind_successor_ids;
use crate::constant::ConstantData;
use crate::dominator_tree::DominatorTree;
use crate::instr_types::{
    BranchKind, CallAttributeData, CastOpcode, LandingPadClauseKind, LandingPadInstData, Opcode,
    ShuffleMaskElem, ShuffleVectorInstData,
};
use crate::instruction::{InstructionKindData, InstructionView};
use crate::intrinsics::{IntrinsicId, descriptor_for_callee};
use crate::module::{ModuleBrand, ModuleRef};
use crate::pass_context::BasicBlockView;
use crate::r#type::TypeKind;
use crate::value::{Value, ValueKindData, ValueSlot};
use crate::value_tracking::propagates_poison;
use crate::{ApInt, IsValue};
use core::cell::Cell;
use std::collections::{HashSet, VecDeque};

/// How many instructions [`program_undefined_if_poison`] will scan.
///
/// Ports the `unsigned ScanLimit = 32` in upstream's static
/// `programUndefinedIfUndefOrPoison`, whose comment reads "chosen arbitrarily".
const PROGRAM_UNDEFINED_SCAN_LIMIT: u32 = 32;

/// Default scan limit for
/// [`instructions_transfer_execution_to_successor`].
///
/// Ports the `unsigned ScanLimit = 32` default on
/// `isGuaranteedToTransferExecutionToSuccessor(Begin, End, ScanLimit)`.
pub const DEFAULT_TRANSFER_SCAN_LIMIT: u32 = 32;

// --------------------------------------------------------------------------
// Speculation safety
// --------------------------------------------------------------------------

/// The two toggles `isSafeToSpeculativelyExecute` actually reads.
///
/// Upstream spells them as trailing defaulted `bool` parameters
/// (`UseVariableInfo = true`, `IgnoreUBImplyingAttrs = true`); Rust has no
/// default arguments, and two bare `bool`s at a call site say nothing about
/// which is which. [`Default`] reproduces upstream's defaults, so
/// `SpeculationOptions::default()` is upstream's no-argument call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpeculationOptions {
    use_variable_info: bool,
    ignore_ub_implying_attrs: bool,
}

impl Default for SpeculationOptions {
    fn default() -> Self {
        Self {
            use_variable_info: true,
            ignore_ub_implying_attrs: true,
        }
    }
}

impl SpeculationOptions {
    /// Upstream's defaults: variable information used, UB-implying attributes
    /// ignored.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether facts derived from the operands' *values* may be used.
    ///
    /// Cleared by [`is_safe_to_speculatively_execute_with_variable_replaced`],
    /// whose caller is about to swap an operand out, so anything learned from
    /// the current one is about to stop being true.
    #[must_use]
    pub fn without_variable_info(mut self) -> Self {
        self.use_variable_info = false;
        self
    }

    /// Whether attributes that make *particular* operand values UB (`noundef`,
    /// `dereferenceable`, `dereferenceable_or_null`) may be disregarded.
    #[must_use]
    pub fn ignoring_ub_implying_attrs(mut self) -> Self {
        self.ignore_ub_implying_attrs = true;
        self
    }

    /// See [`Self::without_variable_info`].
    pub fn uses_variable_info(self) -> bool {
        self.use_variable_info
    }

    /// See [`Self::ignoring_ub_implying_attrs`].
    pub fn ignores_ub_implying_attrs(self) -> bool {
        self.ignore_ub_implying_attrs
    }
}

/// Whether `instruction` can be moved to a point where it might not have run,
/// without introducing undefined behaviour.
///
/// Ports `llvm::isSafeToSpeculativelyExecute`.
pub fn is_safe_to_speculatively_execute<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
    options: SpeculationOptions,
) -> bool {
    is_safe_to_speculatively_execute_with_opcode(instruction.opcode(), instruction, options)
}

/// [`is_safe_to_speculatively_execute`] with `instruction`'s opcode overridden.
///
/// Ports `llvm::isSafeToSpeculativelyExecuteWithOpcode`. When `opcode` is the
/// instruction's own, the two agree; when it differs, the switch below runs for
/// `opcode` while the operands come from `instruction` — which is how a caller
/// asks "if I rewrote this as a `udiv`, could I still hoist it?".
///
/// Upstream guards the mismatched case with `#ifndef NDEBUG` assertions that
/// the operand count and types fit the override. llvmkit has no assertion
/// channel and no runtime panics in production paths, so the arms that read
/// operands answer conservatively when the operand is missing instead.
pub fn is_safe_to_speculatively_execute_with_opcode<'ctx, B: ModuleBrand + 'ctx>(
    opcode: Opcode,
    instruction: &InstructionView<'ctx, B>,
    options: SpeculationOptions,
) -> bool {
    let anchor = instruction.to_erased();
    let kind = view_kind(instruction);
    let operand = |index: usize| -> Option<Value<'ctx, B>> {
        let slot = kind.operand_ids().get(index).copied()?;
        Some(value_from_slot(anchor, slot))
    };

    match opcode {
        // x / y and x % y are undefined when y == 0.
        Opcode::Udiv | Opcode::Urem => {
            let Some(denominator) = operand(1).and_then(int_constant) else {
                return false;
            };
            !denominator.is_zero()
        }
        // Signed division adds INT_MIN / -1.
        Opcode::Sdiv | Opcode::Srem => {
            let Some(denominator) = operand(1).and_then(int_constant) else {
                return false;
            };
            if denominator.is_zero() {
                return false;
            }
            if !denominator.is_all_ones() {
                return true;
            }
            // The denominator is -1, so this is safe exactly when the numerator
            // is known not to be the signed minimum. A non-constant numerator
            // *might* be, so it answers false.
            match operand(0).and_then(int_constant) {
                Some(numerator) => !numerator.is_min_signed_value(),
                None => false,
            }
        }
        Opcode::Load => {
            if !options.uses_variable_info() {
                return false;
            }
            let InstructionKindData::Load(data) = kind else {
                return false;
            };
            if !is_unordered(data.ordering, data.volatile) {
                // Ports `mustSuppressSpeculation` (`Loads.cpp`) less its
                // sanitizer half: llvmkit models no sanitizer function
                // attributes, so only the ordering test applies.
                return false;
            }
            // Upstream continues into `isDereferenceableAndAlignedPointer`
            // (`Loads.cpp`), which llvmkit does not model. See the module
            // docs: `false` is the conservative direction.
            false
        }
        Opcode::Call => {
            let InstructionKindData::Call(data) = kind else {
                return false;
            };
            let callee = value_from_slot(anchor, data.callee.get());
            if !callee_is_speculatable(callee) {
                return false;
            }
            // Hoisting may change which values the operands hold, so the
            // attributes that make particular operand values UB matter again.
            options.ignores_ub_implying_attrs() || !has_ub_implying_attrs(&data.attrs)
        }
        // Upstream's `default: return true`.
        Opcode::Fneg
        | Opcode::Add
        | Opcode::Fadd
        | Opcode::Sub
        | Opcode::Fsub
        | Opcode::Mul
        | Opcode::Fmul
        | Opcode::Fdiv
        | Opcode::Frem
        | Opcode::Shl
        | Opcode::Lshr
        | Opcode::Ashr
        | Opcode::And
        | Opcode::Or
        | Opcode::Xor
        | Opcode::GetElementPtr
        | Opcode::Trunc
        | Opcode::Zext
        | Opcode::Sext
        | Opcode::FpToUi
        | Opcode::FpToSi
        | Opcode::UiToFp
        | Opcode::SiToFp
        | Opcode::FpTrunc
        | Opcode::FpExt
        | Opcode::PtrToInt
        | Opcode::PtrToAddr
        | Opcode::IntToPtr
        | Opcode::BitCast
        | Opcode::AddrSpaceCast
        | Opcode::Icmp
        | Opcode::Fcmp
        | Opcode::Select
        | Opcode::ExtractElement
        | Opcode::InsertElement
        | Opcode::ShuffleVector
        | Opcode::ExtractValue
        | Opcode::InsertValue
        | Opcode::Freeze => true,
        // Upstream's explicit "misc instructions which have effects" list.
        Opcode::VaArg
        | Opcode::Alloca
        | Opcode::Invoke
        | Opcode::CallBr
        | Opcode::Phi
        | Opcode::Store
        | Opcode::Ret
        | Opcode::Br
        | Opcode::IndirectBr
        | Opcode::Switch
        | Opcode::Unreachable
        | Opcode::Fence
        | Opcode::AtomicRmw
        | Opcode::AtomicCmpXchg
        | Opcode::LandingPad
        | Opcode::Resume
        | Opcode::CatchSwitch
        | Opcode::CatchPad
        | Opcode::CatchReturn
        | Opcode::CleanupPad
        | Opcode::CleanupReturn => false,
    }
}

/// [`is_safe_to_speculatively_execute`] for a caller that is about to replace
/// one of the operands.
///
/// Ports the inline `llvm::isSafeToSpeculativelyExecuteWithVariableReplaced`,
/// which is exactly the base call with `UseVariableInfo = false`.
pub fn is_safe_to_speculatively_execute_with_variable_replaced<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
    options: SpeculationOptions,
) -> bool {
    is_safe_to_speculatively_execute(instruction, options.without_variable_info())
}

/// Whether moving `instruction` relative to another one could change behaviour
/// for a reason other than an SSA def-use edge.
///
/// Ports `llvm::mayHaveNonDefUseDependency`.
pub fn may_have_non_def_use_dependency<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
) -> bool {
    if may_read_or_write_memory(instruction) {
        // A memory dependency is possible.
        return true;
    }
    if !is_safe_to_speculatively_execute(instruction, SpeculationOptions::new()) {
        // Cannot move above a may-throw call or an infinite loop.
        return true;
    }
    // Cannot reorder two infinite-loop calls even when read-only, nor move one
    // below an instruction that is unsafe to speculate.
    !is_guaranteed_to_transfer_execution_to_successor(instruction)
}

// --------------------------------------------------------------------------
// Guaranteed execution
// --------------------------------------------------------------------------

/// Whether control reaching `instruction` is guaranteed to reach the next one.
///
/// Ports the `const Instruction *` overload of
/// `llvm::isGuaranteedToTransferExecutionToSuccessor`.
///
/// An atomic operation qualifies: another thread can interfere with it for an
/// arbitrary length of time, but programs are not allowed to rely on that.
pub fn is_guaranteed_to_transfer_execution_to_successor<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
) -> bool {
    let anchor = instruction.to_erased();
    let kind = view_kind(instruction);

    // With no successor, execution cannot transfer to one.
    if matches!(
        kind,
        InstructionKindData::Ret(_) | InstructionKindData::Unreachable(_)
    ) {
        return false;
    }

    // Upstream branches on `classifyEHPersonality` here and answers `true` only
    // for CoreCLR, where a `catchpad` is just a type test. llvmkit does not
    // classify personalities, so this takes upstream's `default` arm.
    if matches!(kind, InstructionKindData::CatchPad(_)) {
        return false;
    }

    // An instruction that returns without throwing must transfer control to a
    // successor.
    !may_throw(anchor, kind) && will_return(anchor, kind)
}

/// Whether every instruction in `block` transfers execution to its successor.
///
/// Ports the `const BasicBlock *` overload of
/// `llvm::isGuaranteedToTransferExecutionToSuccessor`. Note it is *not* the
/// same question as "the block runs to completion": upstream's own comment
/// records that leaving via an `invoke`'s unwind edge is normal control flow
/// that this call still reports as a non-transfer.
pub fn block_transfers_execution_to_successor<'ctx, B: ModuleBrand + 'ctx>(
    block: BasicBlockView<'ctx, B>,
) -> bool {
    block
        .instructions()
        .all(|instruction| is_guaranteed_to_transfer_execution_to_successor(&instruction))
}

/// Whether every instruction in `instructions` transfers execution to its
/// successor, giving up after `scan_limit` of them.
///
/// Ports the two iterator overloads of
/// `llvm::isGuaranteedToTransferExecutionToSuccessor`, which differ only in how
/// the range is spelled — one takes `Begin`/`End`, the other the range they
/// make. [`DEFAULT_TRANSFER_SCAN_LIMIT`] is upstream's default.
///
/// A `scan_limit` of zero answers `false`. Upstream asserts it is non-zero;
/// "gave up before looking" is the answer its own `--ScanLimit == 0` test
/// reaches one instruction later, so declining is the faithful reading rather
/// than a panic.
pub fn instructions_transfer_execution_to_successor<'ctx, B, I>(
    instructions: I,
    scan_limit: u32,
) -> bool
where
    B: ModuleBrand + 'ctx,
    I: IntoIterator<Item = InstructionView<'ctx, B>>,
{
    let mut remaining = scan_limit;
    for instruction in instructions {
        // Upstream decrements first and bails at zero, so the limit counts the
        // instructions it *declines* to look at as well as those it inspects.
        remaining = remaining.saturating_sub(1);
        if remaining == 0 || !is_guaranteed_to_transfer_execution_to_successor(&instruction) {
            return false;
        }
    }
    true
}

/// Whether `instruction` runs on every iteration of the loop whose header is
/// `loop_header`.
///
/// Ports `llvm::isGuaranteedToExecuteForEveryIteration`. Upstream takes the
/// whole `const Loop *` and reads exactly one thing out of it,
/// `L->getHeader()`; llvmkit has no `LoopInfo`, so the header is passed
/// directly rather than blocking the port on an analysis this function never
/// consults.
///
/// Upstream's `FIXME` — that this is stricter than it needs to be, covering
/// only the header rather than every block guaranteed to execute each
/// iteration — is inherited along with the behaviour.
///
/// `false` when `instruction` is not in `loop_header`. That is upstream's first
/// test; its trailing `llvm_unreachable` ("Instruction not contained in its own
/// parent basic block") is unreachable here for the same reason, so it has no
/// counterpart.
pub fn is_guaranteed_to_execute_for_every_iteration<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
    loop_header: BasicBlockView<'ctx, B>,
) -> bool {
    if instruction.parent() != loop_header.id() {
        return false;
    }
    for candidate in loop_header.instructions() {
        if candidate.slot() == instruction.slot() {
            return true;
        }
        if !is_guaranteed_to_transfer_execution_to_successor(&candidate) {
            return false;
        }
    }
    false
}

// --------------------------------------------------------------------------
// Poison reachability
// --------------------------------------------------------------------------

/// Whether `instruction` is guaranteed to trigger undefined behaviour when the
/// values in `known_poison` are poison.
///
/// Ports `llvm::mustTriggerUB`.
pub fn must_trigger_ub<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
    known_poison: &HashSet<ValueSlot>,
) -> bool {
    guaranteed_non_poison_operands(instruction.to_erased(), |operand| {
        known_poison.contains(&operand)
    })
}

/// Whether the program has undefined behaviour if `instruction` yields poison.
///
/// Ports `llvm::programUndefinedIfPoison`.
pub fn program_undefined_if_poison<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
) -> bool {
    program_undefined_for_value(instruction.to_erased(), true)
}

/// Whether the program has undefined behaviour if `instruction` yields undef or
/// poison.
///
/// Ports `llvm::programUndefinedIfUndefOrPoison`.
pub fn program_undefined_if_undef_or_poison<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
) -> bool {
    program_undefined_for_value(instruction.to_erased(), false)
}

/// Ports the static `programUndefinedIfUndefOrPoison(const Value *V, bool PoisonOnly)`.
///
/// Crate-visible rather than public because the surface `ValueTracking.h`
/// declares takes an `Instruction`; the `Argument` arm exists only to serve
/// `is_known_not_undef_or_poison`, which asks about arbitrary values.
pub(crate) fn program_undefined_for_value<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    poison_only: bool,
) -> bool {
    // Only uses within one basic block are considered, so that "the use runs if
    // `value` runs" is guaranteed rather than merely likely. Upstream's FIXME
    // about looking further — which needs strong post-dominance, not
    // post-dominance — is inherited.
    let Some((block, start)) = scan_origin(value) else {
        return false;
    };

    let mut scan_limit = PROGRAM_UNDEFINED_SCAN_LIMIT;

    if !poison_only {
        // Undef does not propagate eagerly, so be conservative and look only
        // for a *direct* use as an operand that must be well defined.
        for instruction in block_instructions_from(value, block, start) {
            scan_limit = scan_limit.saturating_sub(1);
            if scan_limit == 0 {
                break;
            }
            if guaranteed_well_defined_operands(instruction, |operand| operand == value.slot()) {
                return true;
            }
            if !transfers_execution(instruction) {
                break;
            }
        }
        return false;
    }

    // The set of instructions proven to yield poison if `value` does.
    let mut yields_poison: HashSet<ValueSlot> = HashSet::new();
    let mut visited: HashSet<ValueSlot> = HashSet::new();
    yields_poison.insert(value.slot());
    visited.insert(block);

    let mut block = block;
    let mut start = start;
    loop {
        for instruction in block_instructions_from(value, block, start) {
            scan_limit = scan_limit.saturating_sub(1);
            if scan_limit == 0 {
                return false;
            }
            if guaranteed_non_poison_operands(instruction, |operand| {
                yields_poison.contains(&operand)
            }) {
                return true;
            }
            if !transfers_execution(instruction) {
                return false;
            }

            let Some(kind) = instruction_kind(instruction) else {
                continue;
            };

            // A poison operand in a position that propagates makes the result
            // poison too.
            let propagates = kind
                .operand_ids()
                .iter()
                .enumerate()
                .any(|(index, operand)| {
                    yields_poison.contains(operand) && propagates_poison(instruction, index)
                });
            if propagates {
                yields_poison.insert(instruction.slot());
                continue;
            }

            // `select` needs one extra case the loop above cannot see: operand
            // 0 is covered by `propagates_poison`, but the result is also
            // poison when *both* arms are.
            if let InstructionKindData::Select(data) = kind
                && yields_poison.contains(&data.true_val.get())
                && yields_poison.contains(&data.false_val.get())
            {
                yields_poison.insert(instruction.slot());
            }
        }

        let Some(successor) = single_successor(value, block) else {
            break;
        };
        if !visited.insert(successor) {
            break;
        }
        block = successor;
        start = ScanStart::AfterLeadingPhis;
    }
    false
}

/// Whether undefined behaviour provably executes on the way to `on_path_to`
/// when `root` yields poison.
///
/// Ports `llvm::mustExecuteUBIfPoisonOnPathTo`. This says nothing about whether
/// `on_path_to` actually executes or whether `root` is actually poison; it is
/// how a caller decides whether adding a new use of `root` control-equivalent
/// with `on_path_to` would introduce UB that did not previously exist. As
/// upstream's comment records, a `false` answer conveys no information.
pub fn must_execute_ub_if_poison_on_path_to<'ctx, B: ModuleBrand + 'ctx>(
    root: &InstructionView<'ctx, B>,
    on_path_to: &InstructionView<'ctx, B>,
    dominator_tree: &DominatorTree,
) -> bool {
    // Assume `root` is poison, propagate that forward through every user whose
    // propagation is tractable, then ask whether any of them is provable UB
    // that must run before `on_path_to`.
    let root_slot = root.slot();
    let mut known_poison: HashSet<ValueSlot> = HashSet::new();
    let mut worklist: VecDeque<InstructionView<'ctx, B>> = VecDeque::new();
    worklist.push_back(*root);

    while let Some(view) = worklist.pop_back() {
        if must_trigger_ub(&view, &known_poison)
            && dominator_tree.dominates_instruction(&view, on_path_to)
        {
            return true;
        }

        // Where propagation cannot be tracked, skip the instruction and its
        // transitive users. Safe, because `false` is the conservative answer.
        let instruction = view.to_erased();
        if view.slot() != root_slot {
            let kind = view_kind(&view);
            let carries_poison = kind
                .operand_ids()
                .iter()
                .enumerate()
                .any(|(index, operand)| {
                    known_poison.contains(operand) && propagates_poison(instruction, index)
                });
            if !carries_poison {
                continue;
            }
        }

        if known_poison.insert(view.slot()) {
            worklist.extend(instruction.users());
        }
    }

    false
}

// --------------------------------------------------------------------------
// Intrinsic classification
// --------------------------------------------------------------------------

/// Whether `instruction` is an intrinsic that cannot be speculated but also
/// cannot trap.
///
/// Ports `llvm::isAssumeLikeIntrinsic`, whose body is
/// `IntrinsicInst::isAssumeLikeIntrinsic` (`IntrinsicInst.h`).
pub fn is_assume_like_intrinsic<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
) -> bool {
    let Some(id) = called_intrinsic(instruction.to_erased()) else {
        return false;
    };
    matches!(
        id.base_name(),
        "llvm.assume"
            | "llvm.sideeffect"
            | "llvm.pseudoprobe"
            | "llvm.dbg.assign"
            | "llvm.dbg.declare"
            | "llvm.dbg.value"
            | "llvm.dbg.label"
            | "llvm.invariant.start"
            | "llvm.invariant.end"
            | "llvm.lifetime.start"
            | "llvm.lifetime.end"
            | "llvm.experimental.noalias.scope.decl"
            | "llvm.objectsize"
            | "llvm.ptr.annotation"
            | "llvm.var.annotation"
    )
}

/// Whether poison in any operand of `intrinsic` makes its result poison.
///
/// Ports `llvm::intrinsicPropagatesPoison`. Upstream's own `TODO: Add more
/// intrinsics` is inherited: this is what LLVM 22.1.4 lists, not the full set
/// that could qualify.
///
/// Matched on [`IntrinsicId::base_name`] rather than on an id constant because
/// llvmkit mints per-intrinsic constants only for the ones its own analyses
/// need; the base names come from the same generated table and are exact.
pub fn intrinsic_propagates_poison(intrinsic: IntrinsicId) -> bool {
    matches!(
        intrinsic.base_name(),
        // For the with-overflow family, a poison lane in an input poisons the
        // corresponding lane of both the result and the overflow vector.
        "llvm.sadd.with.overflow"
            | "llvm.ssub.with.overflow"
            | "llvm.smul.with.overflow"
            | "llvm.uadd.with.overflow"
            | "llvm.usub.with.overflow"
            | "llvm.umul.with.overflow"
            | "llvm.ctpop"
            | "llvm.ctlz"
            | "llvm.cttz"
            | "llvm.abs"
            | "llvm.smax"
            | "llvm.smin"
            | "llvm.umax"
            | "llvm.umin"
            | "llvm.scmp"
            | "llvm.is.fpclass"
            | "llvm.ptrmask"
            | "llvm.ucmp"
            | "llvm.bitreverse"
            | "llvm.bswap"
            | "llvm.sadd.sat"
            | "llvm.ssub.sat"
            | "llvm.sshl.sat"
            | "llvm.uadd.sat"
            | "llvm.usub.sat"
            | "llvm.ushl.sat"
            | "llvm.smul.fix"
            | "llvm.smul.fix.sat"
            | "llvm.umul.fix"
            | "llvm.umul.fix.sat"
            | "llvm.pow"
            | "llvm.powi"
            | "llvm.sin"
            | "llvm.sinh"
            | "llvm.cos"
            | "llvm.cosh"
            | "llvm.sincos"
            | "llvm.sincospi"
            | "llvm.tan"
            | "llvm.tanh"
            | "llvm.asin"
            | "llvm.acos"
            | "llvm.atan"
            | "llvm.atan2"
            | "llvm.canonicalize"
            | "llvm.sqrt"
            | "llvm.exp"
            | "llvm.exp2"
            | "llvm.exp10"
            | "llvm.log"
            | "llvm.log2"
            | "llvm.log10"
            | "llvm.modf"
            | "llvm.floor"
            | "llvm.ceil"
            | "llvm.trunc"
            | "llvm.rint"
            | "llvm.nearbyint"
            | "llvm.round"
            | "llvm.roundeven"
            | "llvm.lround"
            | "llvm.llround"
            | "llvm.lrint"
            | "llvm.llrint"
            | "llvm.fptosi.sat"
            | "llvm.fptoui.sat"
            | "llvm.fshl"
            | "llvm.fshr"
            | "llvm.fabs"
            | "llvm.minnum"
            | "llvm.maxnum"
            | "llvm.minimum"
            | "llvm.maximum"
            | "llvm.minimumnum"
            | "llvm.maximumnum"
            | "llvm.copysign"
            | "llvm.ldexp"
            | "llvm.frexp"
            | "llvm.fma"
            | "llvm.fmuladd"
    )
}

/// Whether `instruction` computes each result lane from the same input lane.
///
/// Ports `llvm::isNotCrossLaneOperation`.
pub fn is_not_cross_lane_operation<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
) -> bool {
    let anchor = instruction.to_erased();
    if let Some(id) = called_intrinsic(anchor) {
        return is_trivially_vectorizable(id);
    }
    let kind = view_kind(instruction);
    // A shuffle stays lane-local only in its select form. Upstream's remaining
    // conjunct — not a call, bitcast or extractelement — is vacuously true for
    // a shuffle, so this arm needs only the select test.
    if let InstructionKindData::ShuffleVector(data) = kind {
        return shuffle_is_select(anchor, data);
    }
    !matches!(
        kind,
        InstructionKindData::Call(_)
            | InstructionKindData::Invoke(_)
            | InstructionKindData::CallBr(_)
            | InstructionKindData::ExtractElement(_)
    ) && !is_bitcast(kind)
}

/// Ports `llvm::isTriviallyVectorizable` (`llvm/lib/Analysis/VectorUtils.cpp`).
///
/// Not public: it belongs to `VectorUtils.h`, a surface the ValueTracking
/// parity ledger does not track, and it exists here only because
/// [`is_not_cross_lane_operation`] calls it.
fn is_trivially_vectorizable(intrinsic: IntrinsicId) -> bool {
    matches!(
        intrinsic.base_name(),
        "llvm.abs"
            | "llvm.bswap"
            | "llvm.bitreverse"
            | "llvm.ctpop"
            | "llvm.ctlz"
            | "llvm.cttz"
            | "llvm.fshl"
            | "llvm.fshr"
            | "llvm.smax"
            | "llvm.smin"
            | "llvm.umax"
            | "llvm.umin"
            | "llvm.sadd.sat"
            | "llvm.ssub.sat"
            | "llvm.uadd.sat"
            | "llvm.usub.sat"
            | "llvm.smul.fix"
            | "llvm.smul.fix.sat"
            | "llvm.umul.fix"
            | "llvm.umul.fix.sat"
            | "llvm.sqrt"
            | "llvm.asin"
            | "llvm.acos"
            | "llvm.atan"
            | "llvm.atan2"
            | "llvm.sin"
            | "llvm.cos"
            | "llvm.sincos"
            | "llvm.sincospi"
            | "llvm.tan"
            | "llvm.sinh"
            | "llvm.cosh"
            | "llvm.tanh"
            | "llvm.exp"
            | "llvm.exp10"
            | "llvm.exp2"
            | "llvm.frexp"
            | "llvm.ldexp"
            | "llvm.log"
            | "llvm.log10"
            | "llvm.log2"
            | "llvm.fabs"
            | "llvm.minnum"
            | "llvm.maxnum"
            | "llvm.minimum"
            | "llvm.maximum"
            | "llvm.minimumnum"
            | "llvm.maximumnum"
            | "llvm.modf"
            | "llvm.copysign"
            | "llvm.floor"
            | "llvm.ceil"
            | "llvm.trunc"
            | "llvm.rint"
            | "llvm.nearbyint"
            | "llvm.round"
            | "llvm.roundeven"
            | "llvm.pow"
            | "llvm.fma"
            | "llvm.fmuladd"
            | "llvm.is.fpclass"
            | "llvm.powi"
            | "llvm.canonicalize"
            | "llvm.fptosi.sat"
            | "llvm.fptoui.sat"
            | "llvm.lround"
            | "llvm.llround"
            | "llvm.lrint"
            | "llvm.llrint"
            | "llvm.ucmp"
            | "llvm.scmp"
    )
}

// --------------------------------------------------------------------------
// Operand enumeration shared by the UB predicates
// --------------------------------------------------------------------------

/// Ports the template `handleGuaranteedWellDefinedOps`: visit every operand of
/// `instruction` that must be well defined, stopping at the first `handle` that
/// answers true.
fn guaranteed_well_defined_operands<'ctx, B, F>(instruction: Value<'ctx, B>, mut handle: F) -> bool
where
    B: ModuleBrand + 'ctx,
    F: FnMut(ValueSlot) -> bool,
{
    let Some(kind) = instruction_kind(instruction) else {
        return false;
    };
    match kind {
        InstructionKindData::Store(data) => handle(data.ptr.get()),
        InstructionKindData::Load(data) => handle(data.ptr.get()),
        // `dereferenceable` implies `noundef`, so an atomic operation's pointer
        // is implicitly `noundef` too.
        InstructionKindData::AtomicCmpXchg(data) => handle(data.ptr.get()),
        InstructionKindData::AtomicRmw(data) => handle(data.ptr.get()),
        InstructionKindData::Call(_) | InstructionKindData::Invoke(_) => {
            let Some(call) = call_parts(kind) else {
                return false;
            };
            // An indirect call's callee operand must be well defined.
            if is_indirect_callee(value_from_slot(instruction, call.callee.get()))
                && handle(call.callee.get())
            {
                return true;
            }
            for (index, arg) in call.args.iter().enumerate() {
                let Ok(index) = u32::try_from(index) else {
                    continue;
                };
                if param_is_well_defined(call.attrs.arg_attrs(), index) && handle(arg.get()) {
                    return true;
                }
            }
            false
        }
        InstructionKindData::Ret(data) => {
            let Some(returned) = data.value.get() else {
                return false;
            };
            function_returns_noundef(instruction) && handle(returned)
        }
        InstructionKindData::Switch(data) => handle(data.cond.get()),
        InstructionKindData::Br(data) => match &*data.kind.borrow() {
            BranchKind::Unconditional(_) => false,
            BranchKind::Conditional { cond, .. } => handle(cond.get()),
        },
        _ => false,
    }
}

/// Ports the template `handleGuaranteedNonPoisonOps`: the well-defined operands
/// plus the divisors, which may be *partially* undef but never poison.
fn guaranteed_non_poison_operands<'ctx, B, F>(instruction: Value<'ctx, B>, mut handle: F) -> bool
where
    B: ModuleBrand + 'ctx,
    F: FnMut(ValueSlot) -> bool,
{
    if guaranteed_well_defined_operands(instruction, &mut handle) {
        return true;
    }
    match instruction_kind(instruction) {
        Some(
            InstructionKindData::Udiv(data)
            | InstructionKindData::Sdiv(data)
            | InstructionKindData::Urem(data)
            | InstructionKindData::Srem(data),
        ) => handle(data.rhs.get()),
        _ => false,
    }
}

// --------------------------------------------------------------------------
// The `Instruction.cpp` predicates the above lean on
// --------------------------------------------------------------------------

/// Ports `Instruction::mayThrow` at its default `IncludePhaseOneUnwind = false`.
fn may_throw<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    kind: &InstructionKindData,
) -> bool {
    match kind {
        InstructionKindData::Call(data) => {
            !call_site_has_fn_attr(anchor, data.callee.get(), &data.attrs, AttrKind::NoUnwind)
        }
        // `unwindsToCaller()` is "no unwind destination".
        InstructionKindData::CleanupReturn(data) => data.unwind_dest.is_none(),
        InstructionKindData::CatchSwitch(data) => data.unwind_dest.get().is_none(),
        InstructionKindData::Resume(_) => true,
        InstructionKindData::Invoke(data) => {
            // A landingpad does not itself unwind, but an invoke of a *skipped*
            // landingpad keeps unwinding.
            let unwind_dest = data.unwind_dest.get();
            match first_non_phi(anchor, unwind_dest) {
                Some(pad) => match instruction_kind(pad) {
                    Some(InstructionKindData::LandingPad(landing_pad)) => {
                        can_unwind_past_landing_pad(pad, landing_pad)
                    }
                    _ => false,
                },
                None => false,
            }
        }
        // Treated the same as a cleanup landingpad: only phase-one unwinding
        // passes it, which the default `IncludePhaseOneUnwind = false` excludes.
        InstructionKindData::CleanupPad(_) => false,
        _ => false,
    }
}

/// Ports the static `canUnwindPastLandingPad(LP, /*IncludePhaseOneUnwind=*/false)`.
fn can_unwind_past_landing_pad<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    landing_pad: &LandingPadInstData,
) -> bool {
    if landing_pad.cleanup.get() {
        // Phase-one unwinding skips cleanup landingpads, effectively unwinding
        // past this frame — but only in phase one, which the default excludes.
        return false;
    }
    for (clause_kind, clause) in landing_pad.clauses.borrow().iter() {
        let clause = value_from_slot(anchor, clause.get());
        match clause_kind {
            // `catch ptr null` catches every exception.
            LandingPadClauseKind::Catch => {
                if matches!(
                    &clause.data().kind,
                    ValueKindData::Constant(ConstantData::PointerNull)
                ) {
                    return false;
                }
            }
            // `filter [0 x ptr]` catches every exception.
            LandingPadClauseKind::Filter => {
                if clause.ty().kind() == TypeKind::Array
                    && clause.ty().data().as_array().is_some_and(|(_, n)| n == 0)
                {
                    return false;
                }
            }
        }
    }
    // Only some subset of exceptions is caught; the rest keep unwinding.
    true
}

/// Ports `Instruction::willReturn`.
fn will_return<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    kind: &InstructionKindData,
) -> bool {
    match kind {
        // A volatile store is not guaranteed to return; see LangRef.
        InstructionKindData::Store(data) => !data.volatile,
        InstructionKindData::Call(_)
        | InstructionKindData::Invoke(_)
        | InstructionKindData::CallBr(_) => {
            let Some(call) = call_parts(kind) else {
                return true;
            };
            call_site_has_fn_attr(anchor, call.callee.get(), call.attrs, AttrKind::WillReturn)
        }
        _ => true,
    }
}

/// Ports `Instruction::mayReadFromMemory() || Instruction::mayWriteToMemory()`.
///
/// The two upstream switches are folded into one: `load` is an unconditional
/// read and `store` an unconditional write, so their `isUnordered` tests — the
/// only place the two lists disagree — cannot change the union.
fn may_read_or_write_memory<'ctx, B: ModuleBrand + 'ctx>(
    instruction: &InstructionView<'ctx, B>,
) -> bool {
    let anchor = instruction.to_erased();
    let kind = view_kind(instruction);
    match kind {
        InstructionKindData::VaArg(_)
        | InstructionKindData::Fence(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRmw(_)
        | InstructionKindData::CatchPad(_)
        | InstructionKindData::CatchReturn(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Store(_) => true,
        InstructionKindData::Call(_)
        | InstructionKindData::Invoke(_)
        | InstructionKindData::CallBr(_) => {
            let Some(call) = call_parts(kind) else {
                return true;
            };
            let effects = call_site_memory_effects(anchor, call.callee.get(), call.attrs);
            // Reads unless write-only; writes unless read-only.
            !effects.only_writes_memory() || !effects.only_reads_memory()
        }
        _ => false,
    }
}

/// Ports `Instruction::mayWriteToMemory`.
fn may_write_to_memory<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    kind: &InstructionKindData,
) -> bool {
    match kind {
        // Upstream's `FIXME: refine definition of mayWriteToMemory` sits on the
        // `fence` arm; the conservative answer is inherited with it.
        InstructionKindData::Fence(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::VaArg(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRmw(_)
        | InstructionKindData::CatchPad(_)
        | InstructionKindData::CatchReturn(_) => true,
        InstructionKindData::Call(_)
        | InstructionKindData::Invoke(_)
        | InstructionKindData::CallBr(_) => {
            let Some(call) = call_parts(kind) else {
                return true;
            };
            !call_site_memory_effects(anchor, call.callee.get(), call.attrs).only_reads_memory()
        }
        InstructionKindData::Load(data) => !is_unordered(data.ordering, data.volatile),
        _ => false,
    }
}

/// Ports `Instruction::mayHaveSideEffects`, which is `mayWriteToMemory() ||
/// mayThrow() || !willReturn()`.
pub(crate) fn instruction_may_have_side_effects<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    kind: &InstructionKindData,
) -> bool {
    may_write_to_memory(anchor, kind) || may_throw(anchor, kind) || !will_return(anchor, kind)
}

// --------------------------------------------------------------------------
// Small shared helpers
// --------------------------------------------------------------------------

/// Where the scan for uses of a value starts.
#[derive(Clone, Copy)]
enum ScanStart {
    /// Just past the instruction that defines the value.
    AfterInstruction(ValueSlot),
    /// At the first non-phi instruction; used for an argument's entry block and
    /// for each block the scan walks into.
    AfterLeadingPhis,
}

/// Ports the `dyn_cast<Instruction>` / `dyn_cast<Argument>` prologue of the
/// static `programUndefinedIfUndefOrPoison`: which block to scan, and where in
/// it to start.
fn scan_origin<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
) -> Option<(ValueSlot, ScanStart)> {
    match &value.data().kind {
        ValueKindData::Instruction(instruction) => Some((
            instruction.parent.get(),
            ScanStart::AfterInstruction(value.slot()),
        )),
        ValueKindData::Argument { parent_fn, .. } => {
            let function = value_from_slot(value, *parent_fn);
            let ValueKindData::Function(data) = &function.data().kind else {
                return None;
            };
            // A declaration has no entry block to scan.
            let entry = *data.basic_blocks.borrow().first()?;
            Some((entry, ScanStart::AfterLeadingPhis))
        }
        _ => None,
    }
}

/// The instructions of `block` from `start` to the end, as values.
fn block_instructions_from<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
    start: ScanStart,
) -> Vec<Value<'ctx, B>> {
    let module = module_ref(anchor);
    let ValueKindData::BasicBlock(data) = &module.value_data(block).kind else {
        return Vec::new();
    };
    let instructions = data.instructions.borrow();
    let from = match start {
        ScanStart::AfterInstruction(slot) => match instructions.iter().position(|id| *id == slot) {
            Some(position) => position + 1,
            None => return Vec::new(),
        },
        ScanStart::AfterLeadingPhis => instructions
            .iter()
            .position(|id| {
                !matches!(
                    instruction_kind(value_from_slot(anchor, *id)),
                    Some(InstructionKindData::Phi(_))
                )
            })
            .unwrap_or(instructions.len()),
    };
    instructions[from..]
        .iter()
        .map(|id| value_from_slot(anchor, *id))
        .collect()
}

/// The first non-phi instruction of `block`. Ports `BasicBlock::getFirstNonPHIIt`.
fn first_non_phi<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
) -> Option<Value<'ctx, B>> {
    block_instructions_from(anchor, block, ScanStart::AfterLeadingPhis)
        .into_iter()
        .next()
}

/// Ports `BasicBlock::getSingleSuccessor`: the unique successor, or `None` when
/// there are none or more than one distinct one.
fn single_successor<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    block: ValueSlot,
) -> Option<ValueSlot> {
    let module = module_ref(anchor);
    let ValueKindData::BasicBlock(data) = &module.value_data(block).kind else {
        return None;
    };
    let terminator = *data.instructions.borrow().last()?;
    let kind = instruction_kind(value_from_slot(anchor, terminator))?;
    let successors = kind_successor_ids(kind);
    let first = *successors.first()?;
    successors
        .iter()
        .all(|successor| *successor == first)
        .then_some(first)
}

/// [`is_guaranteed_to_transfer_execution_to_successor`] on a bare value.
fn transfers_execution<'ctx, B: ModuleBrand + 'ctx>(instruction: Value<'ctx, B>) -> bool {
    match InstructionView::try_from(instruction) {
        Ok(view) => is_guaranteed_to_transfer_execution_to_successor(&view),
        Err(_) => false,
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

/// The payload behind an [`InstructionView`], which is one by construction.
fn view_kind<'ctx, B: ModuleBrand + 'ctx>(
    view: &InstructionView<'ctx, B>,
) -> &'ctx InstructionKindData {
    match instruction_kind(view.to_erased()) {
        Some(kind) => kind,
        None => unreachable!("InstructionView invariant: kind is Instruction"),
    }
}

/// The three fields shared by `call`, `invoke` and `callbr` — the slice of
/// `CallBase` these predicates read.
struct CallParts<'a> {
    callee: &'a Cell<ValueSlot>,
    args: &'a [Cell<ValueSlot>],
    attrs: &'a CallAttributeData,
}

/// Ports the `cast<CallBase>` that upstream reaches all three call forms
/// through.
fn call_parts(kind: &InstructionKindData) -> Option<CallParts<'_>> {
    match kind {
        InstructionKindData::Call(data) => Some(CallParts {
            callee: &data.callee,
            args: &data.args,
            attrs: &data.attrs,
        }),
        InstructionKindData::Invoke(data) => Some(CallParts {
            callee: &data.callee,
            args: &data.args,
            attrs: &data.attrs,
        }),
        InstructionKindData::CallBr(data) => Some(CallParts {
            callee: &data.callee,
            args: &data.args,
            attrs: &data.attrs,
        }),
        _ => None,
    }
}

/// Whether the call site or its callee carries `attribute` as a function
/// attribute. Ports `CallBase::hasFnAttr`, which checks the call site first and
/// falls back to the called function.
pub(crate) fn call_site_has_fn_attr<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    callee: ValueSlot,
    attrs: &CallAttributeData,
    attribute: AttrKind,
) -> bool {
    if storage_has_enum_attr(attrs.function_attrs(), AttrIndex::Function, attribute) {
        return true;
    }
    // `LLParser::parseCall` folds a `#N` attribute-group reference into the
    // call's `AttributeList` before `CallBase::hasFnAttr` ever reads it, so
    // upstream has no separate group lookup. llvmkit keeps the group numbers
    // beside the call and resolves them here; without this, `call void @f() #0`
    // with `attributes #0 = { noreturn }` reports no `noreturn` at all.
    let module = module_ref(anchor).module();
    for group in attrs.function_attr_groups_slice() {
        if let Some(group_attrs) = module.attribute_group(*group)
            && storage_has_enum_attr(&group_attrs, AttrIndex::Function, attribute)
        {
            return true;
        }
    }
    let callee = value_from_slot(anchor, callee);
    let ValueKindData::Function(data) = &callee.data().kind else {
        return false;
    };
    if storage_has_enum_attr(&data.attributes.borrow(), AttrIndex::Function, attribute) {
        return true;
    }
    // An intrinsic declaration carries its TableGen properties whether or not
    // they were spelled out in the `.ll`: upstream materialises them in the
    // `Function` constructor, llvmkit reads them back off the record.
    match descriptor_for_callee(callee).map(|descriptor| descriptor.id()) {
        Some(id) => match attribute {
            AttrKind::NoUnwind => !id.may_throw(),
            AttrKind::WillReturn => id.will_return(),
            AttrKind::Speculatable => id.is_speculatable(),
            AttrKind::NoFree => id.no_free(),
            AttrKind::NoReturn => id.no_return(),
            _ => false,
        },
        None => false,
    }
}

/// The memory effects of a call site: the `memory(...)` attribute if present,
/// otherwise the callee's, otherwise unknown. Ports `CallBase::getMemoryEffects`.
fn call_site_memory_effects<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    callee: ValueSlot,
    attrs: &CallAttributeData,
) -> MemoryEffects {
    if let Some(effects) = memory_effects_in(attrs.function_attrs()) {
        return effects;
    }
    let callee = value_from_slot(anchor, callee);
    if let ValueKindData::Function(data) = &callee.data().kind
        && let Some(effects) = memory_effects_in(&data.attributes.borrow())
    {
        return effects;
    }
    match descriptor_for_callee(callee) {
        Some(descriptor) => descriptor.id().memory_effects(),
        None => MemoryEffects::unknown(),
    }
}

/// The `memory(...)` attribute in `storage`'s function-attribute slot.
fn memory_effects_in(storage: &AttributeStorage) -> Option<MemoryEffects> {
    storage
        .get(AttrIndex::Function)?
        .iter()
        .find_map(|stored| match stored {
            AttributeStored::Memory(effects) => Some(*effects),
            _ => None,
        })
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
            .any(|attr| matches!(attr, AttributeStored::Enum(kind) if *kind == attribute))
    })
}

/// Whether parameter `index` of a call site carries one of the three attributes
/// that make an undef argument UB. Ports `CallBase::isPassingUndefUB`.
fn param_is_well_defined(arg_attrs: &[AttributeStorage], index: u32) -> bool {
    let Ok(position) = usize::try_from(index) else {
        return false;
    };
    let Some(storage) = arg_attrs.get(position) else {
        return false;
    };
    storage
        .get(AttrIndex::Param(index))
        .into_iter()
        .flatten()
        .any(is_ub_implying_attribute)
}

/// Ports `Instruction::hasUBImplyingAttrs` for a call site: any argument or the
/// return carrying `noundef`, `dereferenceable` or `dereferenceable_or_null`.
fn has_ub_implying_attrs(attrs: &CallAttributeData) -> bool {
    let arg_attrs = attrs.arg_attrs();
    for index in 0..arg_attrs.len() {
        let Ok(index) = u32::try_from(index) else {
            continue;
        };
        if param_is_well_defined(arg_attrs, index) {
            return true;
        }
    }
    attrs
        .return_attrs()
        .get(AttrIndex::Return)
        .into_iter()
        .flatten()
        .any(is_ub_implying_attribute)
}

/// The three attributes that make an undef value at this position UB.
/// `dereferenceable` and `dereferenceable_or_null` both imply `noundef`.
fn is_ub_implying_attribute(attribute: &AttributeStored) -> bool {
    matches!(
        attribute,
        AttributeStored::Enum(AttrKind::NoUndef)
            | AttributeStored::Int(AttrKind::Dereferenceable, _)
            | AttributeStored::Int(AttrKind::DereferenceableOrNull, _)
    )
}

/// Whether the enclosing function's return carries `noundef`. Ports
/// `I->getFunction()->hasRetAttribute(Attribute::NoUndef)`.
fn function_returns_noundef<'ctx, B: ModuleBrand + 'ctx>(instruction: Value<'ctx, B>) -> bool {
    let Some(function) = enclosing_function(instruction) else {
        return false;
    };
    let ValueKindData::Function(data) = &function.data().kind else {
        return false;
    };
    storage_has_enum_attr(
        &data.attributes.borrow(),
        AttrIndex::Return,
        AttrKind::NoUndef,
    )
}

/// The function `instruction` belongs to. Ports `Instruction::getFunction`.
fn enclosing_function<'ctx, B: ModuleBrand + 'ctx>(
    instruction: Value<'ctx, B>,
) -> Option<Value<'ctx, B>> {
    let ValueKindData::Instruction(data) = &instruction.data().kind else {
        return None;
    };
    let block = value_from_slot(instruction, data.parent.get());
    let ValueKindData::BasicBlock(block_data) = &block.data().kind else {
        return None;
    };
    let parent = (*block_data.parent.borrow())?;
    Some(value_from_slot(instruction, parent))
}

/// Whether `callee` is anything other than a direct reference to a function.
/// Ports `CallBase::isIndirectCall`.
fn is_indirect_callee<'ctx, B: ModuleBrand + 'ctx>(callee: Value<'ctx, B>) -> bool {
    !matches!(&callee.data().kind, ValueKindData::Function(_))
}

/// The intrinsic `instruction` calls, when it is a direct call to one.
fn called_intrinsic<'ctx, B: ModuleBrand + 'ctx>(
    instruction: Value<'ctx, B>,
) -> Option<IntrinsicId> {
    let kind = instruction_kind(instruction)?;
    let call = call_parts(kind)?;
    let callee = value_from_slot(instruction, call.callee.get());
    Some(descriptor_for_callee(callee)?.id())
}

/// Whether a callee is annotated `speculatable`. Ports
/// `Function::isSpeculatable`.
fn callee_is_speculatable<'ctx, B: ModuleBrand + 'ctx>(callee: Value<'ctx, B>) -> bool {
    let ValueKindData::Function(data) = &callee.data().kind else {
        // Upstream's `if (!Callee)`: an indirect call could do anything.
        return false;
    };
    if storage_has_enum_attr(
        &data.attributes.borrow(),
        AttrIndex::Function,
        AttrKind::Speculatable,
    ) {
        return true;
    }
    match descriptor_for_callee(callee) {
        Some(descriptor) => descriptor.id().is_speculatable(),
        None => false,
    }
}

/// Ports `ShuffleVectorInst::isSelect`: the mask does not change the length,
/// and every element picks its own lane from one input or the other.
fn shuffle_is_select<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    data: &ShuffleVectorInstData,
) -> bool {
    let lhs = value_from_slot(anchor, data.lhs.get());
    let Some((_, lanes, _)) = lhs.ty().data().as_vector() else {
        return false;
    };
    let Ok(lane_count) = usize::try_from(lanes) else {
        return false;
    };
    // `!changesLength()`: the mask is exactly as wide as a source vector.
    if data.mask.len() != lane_count {
        return false;
    }
    data.mask.iter().enumerate().all(|(index, element)| {
        // A poison mask element is compatible with either choice.
        let ShuffleMaskElem::Lane(lane) = *element else {
            return true;
        };
        let Ok(index) = u32::try_from(index) else {
            return false;
        };
        // The second disjunct is `lane == index + lanes`, written as a
        // subtraction so an oversized mask cannot overflow.
        lane == index || lane.checked_sub(lanes) == Some(index)
    })
}

/// Whether `kind` is a `bitcast`.
fn is_bitcast(kind: &InstructionKindData) -> bool {
    matches!(kind, InstructionKindData::Cast(data) if data.kind == CastOpcode::BitCast)
}

/// Ports `LoadInst::isUnordered` / `StoreInst::isUnordered`: not volatile, and
/// at most `unordered`.
fn is_unordered(ordering: AtomicOrdering, volatile: bool) -> bool {
    !volatile
        && matches!(
            ordering,
            AtomicOrdering::NotAtomic | AtomicOrdering::Unordered
        )
}

/// The `ApInt` behind a scalar integer constant.
fn int_constant<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> Option<ApInt> {
    let TypeKind::Integer { bits } = value.ty().kind() else {
        return None;
    };
    match &value.data().kind {
        ValueKindData::Constant(ConstantData::Int(words)) => Some(ApInt::from_words(bits, words)),
        _ => None,
    }
}

fn value_from_slot<'ctx, B: ModuleBrand + 'ctx>(
    anchor: Value<'ctx, B>,
    slot: ValueSlot,
) -> Value<'ctx, B> {
    let module = module_ref(anchor);
    let data = module.value_data(slot);
    Value::from_parts(slot, module, data.ty)
}

fn module_ref<'ctx, B: ModuleBrand + 'ctx>(value: Value<'ctx, B>) -> ModuleRef<'ctx, B> {
    ModuleRef::new(value.module().core_ref())
}
