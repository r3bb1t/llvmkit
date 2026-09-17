//! Control-flow graph queries. Mirrors the small `IR/CFG.h` surface
//! used by verifier and analysis code: successor, predecessor, and edge
//! enumeration over `BasicBlock` / terminator instruction structure.

use crate::Branded;
use core::cell::Cell;
use core::iter::FusedIterator;
use std::collections::HashMap;

use super::basic_block::{BasicBlock, IntoBasicBlockLabel};
use super::block_state::{BlockTerminationState, Unterminated};
use super::function::FunctionValue;
use super::instr_types::{BranchInstData, BranchKind};
use super::instruction::{InstructionKindData, InstructionView};
use super::marker::{Dyn, ReturnMarker};
use super::module::{ModuleBrand, ModuleRef};
use super::r#type::TypeSlotAccess;
use super::value::{Value, ValueKindData, ValueSlot, ValueSlotAccess, ValueUse};
use super::value_id::{BlockId, FunctionId};

/// A directed edge in a function CFG. Mirrors LLVM's `BasicBlockEdge`
/// without pointer identity: endpoints are storable [`BlockId`]s, not
/// insertion-capability handles.
///
/// Lifetime-free like the ids it holds — resolve an endpoint with
/// [`Module::view`](crate::Module::view) when you need to read the block.
#[derive(Branded)]
pub struct BasicBlockEdge<B: ModuleBrand> {
    start: BlockId<Dyn, B>,
    end: BlockId<Dyn, B>,
}

impl<B: ModuleBrand> BasicBlockEdge<B> {
    #[inline]
    pub(super) fn new(start: BlockId<Dyn, B>, end: BlockId<Dyn, B>) -> Self {
        Self { start, end }
    }

    /// Edge start block id.
    #[inline]
    pub fn start(&self) -> BlockId<Dyn, B> {
        self.start
    }

    /// Edge end block id.
    #[inline]
    pub fn end(&self) -> BlockId<Dyn, B> {
        self.end
    }
}

/// Recomputed CFG view for one function. Successor/predecessor lists
/// preserve duplicate edges, matching LLVM's CFG iterators.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct FunctionCfg<'ctx, B: ModuleBrand + 'ctx> {
    function: FunctionValue<'ctx, Dyn, B>,
    successors: HashMap<ValueSlot, Vec<ValueSlot>>,
    predecessors: HashMap<ValueSlot, Vec<ValueSlot>>,
    edges: Vec<BasicBlockEdge<B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionCfg<'ctx, B> {
    /// Build a fresh CFG snapshot from the function's current terminators.
    pub fn new(function: FunctionValue<'ctx, Dyn, B>) -> Self {
        let module = function.module();
        let module_ref: ModuleRef<'ctx, B> = module.into();
        let label_ty = module.label_type().as_type().slot_trusting_same_module();
        let mut successors = HashMap::new();
        let mut predecessors: HashMap<ValueSlot, Vec<ValueSlot>> = HashMap::new();
        let mut edges = Vec::new();

        for block in function.basic_blocks() {
            let block = block.as_dyn();
            let succ_ids = successor_ids(&block);
            let block_id = block.to_erased().slot_trusting_same_module();
            for succ_id in &succ_ids {
                edges.push(BasicBlockEdge::new(
                    block.id(),
                    BasicBlock::<Dyn, Unterminated, B>::from_parts(*succ_id, module_ref, label_ty)
                        .id(),
                ));
            }
            successors.insert(block_id, succ_ids);
            // `predecessors(BB)` is a *use-list* view, not the transpose of
            // the successor walk: `PredIterator` reads `BB->user_begin()`,
            // which `Use::addToList` head-inserts into, so it answers
            // newest-first. Building it by pushing each block onto its
            // successors' lists gives block order instead, and the two
            // disagree wherever a block has more than one distinct
            // predecessor.
            predecessors.insert(block_id, block_predecessors(block.to_erased()));
        }

        Self {
            function,
            successors,
            predecessors,
            edges,
        }
    }

    /// Id of the function this CFG was computed from.
    ///
    /// A storable [`FunctionId`], not the borrowing handle: like the
    /// [`BlockId`]s [`Self::successors`] / [`Self::predecessors`] hand back,
    /// what leaves a CFG snapshot is id currency. View it with
    /// [`Module::view`](crate::Module::view) to read the function.
    #[inline]
    pub fn function(&self) -> FunctionId<Dyn, B> {
        self.function.id()
    }

    /// Successors of `block`, preserving duplicate edges.
    ///
    /// A block that does not resolve in this CFG's module — a foreign
    /// [`BlockId`] — has no successors here, exactly as a block absent from the
    /// snapshot does.
    ///
    /// The iterator *borrows* the snapshot rather than copying out of it: a
    /// [`FunctionCfg`] owns its adjacency lists outright (no interior
    /// mutability), so the successor slots can be read straight off the stored
    /// slice. The absent-block and foreign-block cases resolve to the empty
    /// slice, which is why one expression covers all three.
    pub fn successors<R, Block>(
        &self,
        block: Block,
    ) -> impl ExactSizeIterator<Item = BlockId<Dyn, B>> + DoubleEndedIterator + FusedIterator + '_
    where
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.adjacent(&self.successors, block)
    }

    /// Predecessors of `block`, preserving duplicate incoming edges.
    ///
    /// A foreign [`BlockId`] has no predecessors here, and the result borrows
    /// the snapshot; see [`successors`](Self::successors).
    pub fn predecessors<R, Block>(
        &self,
        block: Block,
    ) -> impl ExactSizeIterator<Item = BlockId<Dyn, B>> + DoubleEndedIterator + FusedIterator + '_
    where
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.adjacent(&self.predecessors, block)
    }

    /// Shared body of [`successors`](Self::successors) and
    /// [`predecessors`](Self::predecessors): resolve `block` against this
    /// CFG's module, then hand back the stored adjacency slice retagged as
    /// ids. An unresolvable or absent block yields the empty slice.
    fn adjacent<'cfg, R, Block>(
        &'cfg self,
        table: &'cfg HashMap<ValueSlot, Vec<ValueSlot>>,
        block: Block,
    ) -> impl ExactSizeIterator<Item = BlockId<Dyn, B>> + DoubleEndedIterator + FusedIterator + 'cfg
    where
        R: ReturnMarker,
        Block: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module: ModuleRef<'ctx, B> = self.function.module().into();
        let tag = module.id();
        let slots: &'cfg [ValueSlot] = block
            .into_basic_block_label(module)
            .ok()
            .and_then(|block| table.get(&block.to_erased().slot_trusting_same_module()))
            .map_or(&[], Vec::as_slice);
        slots
            .iter()
            .map(move |slot| BlockId::<Dyn, B>::from_raw(tag, *slot))
    }

    /// Directed edges in function block order and terminator successor order.
    pub fn edges(
        &self,
    ) -> impl ExactSizeIterator<Item = BasicBlockEdge<B>> + DoubleEndedIterator + FusedIterator + '_
    {
        self.edges.iter().cloned()
    }
}

/// `predecessors(BB)` (`llvm/IR/CFG.h`): `PredIterator` walks
/// `BB->user_begin()` and its `advancePastNonTerminators` skips every user
/// that is not an `Instruction` — "Loop to ignore non-terminator uses (for
/// example BlockAddresses)" — then `assert`s `Inst->isTerminator()` and stops.
/// It yields `cast<Instruction>(*It)->getParent()`.
///
/// So upstream filters on *being an instruction* and only asserts the
/// terminator half; the `is_terminator()` test below is llvmkit's spelling of
/// that assertion, and it is sound for the same reason upstream's assert
/// holds: the only non-terminator that could name a block is a `PHINode`, and
/// `InstructionKindData::block_operand_ids`'s `Phi` arm yields nothing —
/// mirroring `PHINode`'s hung-off block array, which is reached by
/// `block_begin` and is not a use list. A phi therefore never registers a
/// block use to filter out. The repo bans runtime panics, so the dead branch
/// is a `filter` rather than an assert; it can never change the result.
///
/// Not sorted and not deduplicated — a terminator naming the same successor
/// twice yields it twice, exactly as upstream. The use list is the ordering
/// authority: `ValueData::add_use` head-inserts, mirroring `Use::addToList`,
/// so this reads newest-first the way `Value::uses()` does — which is what
/// every upstream consumer of `predecessors(BB)` sees, `AsmWriter`'s
/// `; preds = …` comment and the dominator-tree builder alike. It lives here
/// rather than in `asm_writer.rs` because [`FunctionCfg`] answers from it too.
pub(super) fn block_predecessors<'ctx, B: ModuleBrand + 'ctx>(
    block: Value<'ctx, B>,
) -> Vec<ValueSlot> {
    let context = block.module().context();
    block
        .data()
        .use_list
        .borrow()
        .iter()
        .filter_map(|edge| match edge {
            ValueUse::Instruction(user) => Some(*user),
            _ => None,
        })
        .filter_map(|user| {
            let ValueKindData::Instruction(instruction) = &context.value_data(user).kind else {
                return None;
            };
            if !instruction.kind.is_terminator() {
                return None;
            }
            Some(instruction.parent.get())
        })
        .collect()
}

/// Ports `BasicBlock::getSinglePredecessor` (`lib/IR/BasicBlock.cpp`): the
/// predecessor when `block` has exactly one predecessor edge. It counts edges,
/// as upstream's `pred_begin`/`pred_end` walk does, so a block reached twice
/// from one predecessor — a `switch` with two cases into it, or
/// `br i1 %c, label %b, label %b` — has no single predecessor. Counting
/// distinct predecessor blocks instead is `BasicBlock::getUniquePredecessor`.
pub(super) fn single_predecessor<'ctx, B: ModuleBrand + 'ctx>(
    block: Value<'ctx, B>,
) -> Option<ValueSlot> {
    match block_predecessors(block).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

pub(super) fn block_successors<'ctx, R, S, B>(
    block: &BasicBlock<'ctx, R, S, B>,
) -> Vec<BlockId<Dyn, B>>
where
    R: ReturnMarker,
    S: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
    let tag = block.module_ref().id();
    successor_ids(&block.as_dyn())
        .into_iter()
        .map(|slot| BlockId::<Dyn, B>::from_raw(tag, slot))
        .collect()
}

pub(super) fn successor_ids<'ctx, R, S, B>(block: &BasicBlock<'ctx, R, S, B>) -> Vec<ValueSlot>
where
    R: ReturnMarker,
    S: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
    let Some(term) = block.terminator() else {
        return Vec::new();
    };
    instruction_successor_ids(&term)
}

pub(super) fn instruction_successor_ids<'ctx, B: ModuleBrand + 'ctx>(
    inst: &InstructionView<'ctx, B>,
) -> Vec<ValueSlot> {
    match &inst.to_erased().data().kind {
        ValueKindData::Instruction(data) => kind_successor_ids(&data.kind),
        _ => Vec::new(),
    }
}

pub(super) fn kind_successor_ids(kind: &InstructionKindData) -> Vec<ValueSlot> {
    match kind {
        InstructionKindData::Ret(_)
        | InstructionKindData::Resume(_)
        | InstructionKindData::Unreachable(_) => Vec::new(),
        InstructionKindData::CleanupReturn(d) => d.unwind_dest.get().into_iter().collect(),
        InstructionKindData::Br(d) => branch_successor_ids(d),
        InstructionKindData::Switch(d) => {
            let mut ids = Vec::with_capacity(d.cases.borrow().len() + 1);
            ids.push(d.default_bb.get());
            ids.extend(d.cases.borrow().iter().map(|(_, target)| *target));
            ids
        }
        InstructionKindData::IndirectBr(d) => d.destinations.borrow().clone(),
        InstructionKindData::Invoke(d) => vec![d.normal_dest.get(), d.unwind_dest.get()],
        InstructionKindData::CallBr(d) => {
            let mut ids = Vec::with_capacity(d.indirect_dests.len() + 1);
            ids.push(d.default_dest.get());
            ids.extend(d.indirect_dests.iter().map(|target| target.get()));
            ids
        }
        InstructionKindData::CatchReturn(d) => vec![d.target_bb.get()],
        InstructionKindData::CatchSwitch(d) => {
            let handlers = d.handlers.borrow();
            let mut ids =
                Vec::with_capacity(handlers.len() + usize::from(d.unwind_dest.get().is_some()));
            ids.extend(handlers.iter().copied());
            ids.extend(d.unwind_dest.get());
            ids
        }
        InstructionKindData::Add(_)
        | InstructionKindData::Sub(_)
        | InstructionKindData::Mul(_)
        | InstructionKindData::Udiv(_)
        | InstructionKindData::Sdiv(_)
        | InstructionKindData::Urem(_)
        | InstructionKindData::Srem(_)
        | InstructionKindData::Shl(_)
        | InstructionKindData::Lshr(_)
        | InstructionKindData::Ashr(_)
        | InstructionKindData::And(_)
        | InstructionKindData::Or(_)
        | InstructionKindData::Xor(_)
        | InstructionKindData::Fadd(_)
        | InstructionKindData::Fsub(_)
        | InstructionKindData::Fmul(_)
        | InstructionKindData::Fdiv(_)
        | InstructionKindData::Frem(_)
        | InstructionKindData::Fcmp(_)
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Gep(_)
        | InstructionKindData::Call(_)
        | InstructionKindData::Select(_)
        | InstructionKindData::Cast(_)
        | InstructionKindData::Icmp(_)
        | InstructionKindData::Phi(_)
        | InstructionKindData::Fneg(_)
        | InstructionKindData::Freeze(_)
        | InstructionKindData::VaArg(_)
        | InstructionKindData::ExtractValue(_)
        | InstructionKindData::InsertValue(_)
        | InstructionKindData::ExtractElement(_)
        | InstructionKindData::InsertElement(_)
        | InstructionKindData::ShuffleVector(_)
        | InstructionKindData::Fence(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRmw(_)
        | InstructionKindData::LandingPad(_)
        | InstructionKindData::CleanupPad(_)
        | InstructionKindData::CatchPad(_) => Vec::new(),
    }
}

fn branch_successor_ids(d: &BranchInstData) -> Vec<ValueSlot> {
    match &*d.kind.borrow() {
        BranchKind::Unconditional(target) => vec![*target],
        BranchKind::Conditional {
            then_bb, else_bb, ..
        } => vec![*then_bb, *else_bb],
    }
}

/// Port of `Instruction::replaceSuccessorWith` (`lib/IR/Instruction.cpp`): every
/// successor slot of `kind` naming `old` names `new` instead. Upstream walks
/// `getSuccessor(Idx)` over every index and calls `setSuccessor(Idx, NewBB)` on
/// a match; the arms cover the same successor slots as [`kind_successor_ids`].
///
/// Upstream `setSuccessor` assigns through `Use::set`, which keeps both blocks'
/// use lists current. llvmkit stores successors as plain slots, so the caller
/// re-establishes both blocks' edges with [`sync_block_uses`].
pub(super) fn replace_successor_with(kind: &InstructionKindData, old: ValueSlot, new: ValueSlot) {
    let replace = |slot: &mut ValueSlot| {
        if *slot == old {
            *slot = new;
        }
    };
    let replace_cell = |cell: &Cell<ValueSlot>| {
        if cell.get() == old {
            cell.set(new);
        }
    };
    let replace_optional_cell = |cell: &Cell<Option<ValueSlot>>| {
        if cell.get() == Some(old) {
            cell.set(Some(new));
        }
    };
    match kind {
        InstructionKindData::Ret(_)
        | InstructionKindData::Resume(_)
        | InstructionKindData::Unreachable(_) => {}
        InstructionKindData::CleanupReturn(d) => replace_optional_cell(&d.unwind_dest),
        InstructionKindData::Br(d) => match &mut *d.kind.borrow_mut() {
            BranchKind::Unconditional(target) => replace(target),
            BranchKind::Conditional {
                then_bb, else_bb, ..
            } => {
                replace(then_bb);
                replace(else_bb);
            }
        },
        InstructionKindData::Switch(d) => {
            replace_cell(&d.default_bb);
            for (_, target) in d.cases.borrow_mut().iter_mut() {
                replace(target);
            }
        }
        InstructionKindData::IndirectBr(d) => {
            for target in d.destinations.borrow_mut().iter_mut() {
                replace(target);
            }
        }
        InstructionKindData::Invoke(d) => {
            replace_cell(&d.normal_dest);
            replace_cell(&d.unwind_dest);
        }
        InstructionKindData::CallBr(d) => {
            replace_cell(&d.default_dest);
            for target in &d.indirect_dests {
                replace_cell(target);
            }
        }
        InstructionKindData::CatchReturn(d) => replace_cell(&d.target_bb),
        InstructionKindData::CatchSwitch(d) => {
            for handler in d.handlers.borrow_mut().iter_mut() {
                replace(handler);
            }
            replace_optional_cell(&d.unwind_dest);
        }
        InstructionKindData::Add(_)
        | InstructionKindData::Sub(_)
        | InstructionKindData::Mul(_)
        | InstructionKindData::Udiv(_)
        | InstructionKindData::Sdiv(_)
        | InstructionKindData::Urem(_)
        | InstructionKindData::Srem(_)
        | InstructionKindData::Shl(_)
        | InstructionKindData::Lshr(_)
        | InstructionKindData::Ashr(_)
        | InstructionKindData::And(_)
        | InstructionKindData::Or(_)
        | InstructionKindData::Xor(_)
        | InstructionKindData::Fadd(_)
        | InstructionKindData::Fsub(_)
        | InstructionKindData::Fmul(_)
        | InstructionKindData::Fdiv(_)
        | InstructionKindData::Frem(_)
        | InstructionKindData::Fcmp(_)
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Gep(_)
        | InstructionKindData::Call(_)
        | InstructionKindData::Select(_)
        | InstructionKindData::Cast(_)
        | InstructionKindData::Icmp(_)
        | InstructionKindData::Phi(_)
        | InstructionKindData::Fneg(_)
        | InstructionKindData::Freeze(_)
        | InstructionKindData::VaArg(_)
        | InstructionKindData::ExtractValue(_)
        | InstructionKindData::InsertValue(_)
        | InstructionKindData::ExtractElement(_)
        | InstructionKindData::InsertElement(_)
        | InstructionKindData::ShuffleVector(_)
        | InstructionKindData::Fence(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRmw(_)
        | InstructionKindData::LandingPad(_)
        | InstructionKindData::CleanupPad(_)
        | InstructionKindData::CatchPad(_) => {}
    }
}

/// Restore `block`'s use-list edges from the terminator `terminator` after
/// that terminator's successor slots have been edited.
///
/// Upstream needs no such routine: a terminator's successors *are* `Use`s,
/// and `Use::set` unlinks from the old value's list and links into the new
/// one's as part of the assignment, so `predecessors(BB)` — which reads
/// `BB->user_begin()` — is never stale. llvmkit stores successors as plain
/// slots and registers the use once at construction
/// (`IrBuilder::append_instruction` extends its operand walk with
/// `block_operand_ids`), so an edit has to re-establish the same invariant:
/// one `ValueUse::Instruction(terminator)` entry per occurrence of the block
/// among the terminator's successors.
///
/// Reconciling against the post-edit successor list rather than counting
/// edits is what makes it correct for the many-edge slots — a `switch`
/// redirect retargets *every* case naming the old block, and a `cond_br %c,
/// X, X` registers two entries on `X` of which collapsing one arm must
/// leave one. New entries go to the **head**, as `Use::set` does; which of
/// several identical entries is dropped is unobservable.
pub(super) fn sync_block_uses<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleRef<'ctx, B>,
    terminator: ValueSlot,
    block: ValueSlot,
) {
    let wanted = match &module.value_data(terminator).kind {
        ValueKindData::Instruction(data) => kind_successor_ids(&data.kind)
            .iter()
            .filter(|successor| **successor == block)
            .count(),
        _ => 0,
    };
    let mut uses = module.value_data(block).use_list.borrow_mut();
    let edge = ValueUse::Instruction(terminator);
    let have = uses.iter().filter(|entry| **entry == edge).count();
    if have > wanted {
        let mut surplus = have - wanted;
        uses.retain(|entry| {
            if surplus > 0 && *entry == edge {
                surplus -= 1;
                false
            } else {
                true
            }
        });
    } else {
        for _ in have..wanted {
            uses.insert(0, edge);
        }
    }
}
