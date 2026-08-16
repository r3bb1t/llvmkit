//! Control-flow graph queries. Mirrors the small `IR/CFG.h` surface
//! used by verifier and analysis code: successor, predecessor, and edge
//! enumeration over `BasicBlock` / terminator instruction structure.

use crate::Branded;
use core::iter::FusedIterator;
use std::collections::HashMap;

use super::basic_block::{BasicBlock, IntoBasicBlockLabel};
use super::block_state::{BlockTerminationState, Unterminated};
use super::function::FunctionValue;
use super::instr_types::{BranchInstData, BranchKind};
use super::instruction::{InstructionKindData, InstructionView};
use super::marker::{Dyn, ReturnMarker};
use super::module::{ModuleBrand, ModuleRef};
use super::value::{ValueKindData, ValueSlot};
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
        let label_ty = module.label_type().as_type().id();
        let mut successors = HashMap::new();
        let mut predecessors: HashMap<ValueSlot, Vec<ValueSlot>> = HashMap::new();
        let mut edges = Vec::new();

        for block in function.basic_blocks() {
            let block = block.as_dyn();
            let succ_ids = successor_ids(&block);
            let block_id = block.slot();
            for succ_id in &succ_ids {
                predecessors.entry(*succ_id).or_default().push(block_id);
                edges.push(BasicBlockEdge::new(
                    block.id(),
                    BasicBlock::<Dyn, Unterminated, B>::from_parts(*succ_id, module_ref, label_ty)
                        .id(),
                ));
            }
            successors.insert(block_id, succ_ids);
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
            .and_then(|block| table.get(&block.slot()))
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
        InstructionKindData::CleanupReturn(d) => d.unwind_dest.into_iter().collect(),
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
        InstructionKindData::CatchReturn(d) => vec![d.target_bb],
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
