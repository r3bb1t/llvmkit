//! Control-flow graph queries. Mirrors the small `IR/CFG.h` surface
//! used by verifier and analysis code: successor, predecessor, and edge
//! enumeration over `BasicBlock` / terminator instruction structure.

use std::collections::HashMap;

use crate::basic_block::BasicBlock;
use crate::block_state::{BlockSealState, Unsealed};
use crate::function::FunctionValue;
use crate::instruction::{Instruction, InstructionKindData, state};
use crate::marker::{Dyn, ReturnMarker};
use crate::value::{ValueId, ValueKindData};

/// A directed edge in a function CFG. Mirrors LLVM's `BasicBlockEdge`
/// without pointer identity: endpoints are ordinary basic-block handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlockEdge<'ctx> {
    start: BasicBlock<'ctx, Dyn>,
    end: BasicBlock<'ctx, Dyn>,
}

impl<'ctx> BasicBlockEdge<'ctx> {
    #[inline]
    pub(crate) fn new(start: BasicBlock<'ctx, Dyn>, end: BasicBlock<'ctx, Dyn>) -> Self {
        Self { start, end }
    }

    /// Edge start block.
    #[inline]
    pub fn start(self) -> BasicBlock<'ctx, Dyn> {
        self.start
    }

    /// Edge end block.
    #[inline]
    pub fn end(self) -> BasicBlock<'ctx, Dyn> {
        self.end
    }
}

/// Recomputed CFG view for one function. Successor/predecessor lists
/// preserve duplicate edges, matching LLVM's CFG iterators.
#[derive(Debug, Clone)]
pub struct FunctionCfg<'ctx> {
    function: FunctionValue<'ctx, Dyn>,
    successors: HashMap<ValueId, Vec<ValueId>>,
    predecessors: HashMap<ValueId, Vec<ValueId>>,
    edges: Vec<BasicBlockEdge<'ctx>>,
}

impl<'ctx> FunctionCfg<'ctx> {
    /// Build a fresh CFG snapshot from the function's current terminators.
    pub fn new(function: FunctionValue<'ctx, Dyn>) -> Self {
        let module = function.module();
        let label_ty = module.label_type().as_type().id();
        let mut successors = HashMap::new();
        let mut predecessors: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
        let mut edges = Vec::new();

        for block in function.basic_blocks() {
            let block = block.as_dyn();
            let succ_ids = successor_ids(block);
            for succ_id in &succ_ids {
                predecessors.entry(*succ_id).or_default().push(block.id);
                edges.push(BasicBlockEdge::new(
                    block,
                    BasicBlock::from_parts(*succ_id, module, label_ty),
                ));
            }
            successors.insert(block.id, succ_ids);
        }

        Self {
            function,
            successors,
            predecessors,
            edges,
        }
    }

    /// Function this CFG was computed from.
    #[inline]
    pub fn function(&self) -> FunctionValue<'ctx, Dyn> {
        self.function
    }

    /// Successors of `block`, preserving duplicate edges.
    pub fn successors<R, S>(&self, block: BasicBlock<'ctx, R, S>) -> Vec<BasicBlock<'ctx, Dyn>>
    where
        R: ReturnMarker,
        S: BlockSealState,
    {
        ids_to_blocks(block.module(), self.successors.get(&block.as_dyn().id))
    }

    /// Predecessors of `block`, preserving duplicate incoming edges.
    pub fn predecessors<R, S>(&self, block: BasicBlock<'ctx, R, S>) -> Vec<BasicBlock<'ctx, Dyn>>
    where
        R: ReturnMarker,
        S: BlockSealState,
    {
        ids_to_blocks(block.module(), self.predecessors.get(&block.as_dyn().id))
    }

    /// Directed edges in function block order and terminator successor order.
    pub fn edges(&self) -> impl ExactSizeIterator<Item = BasicBlockEdge<'ctx>> + '_ {
        self.edges.iter().copied()
    }
}

pub(crate) fn block_successors<'ctx, R, S>(
    block: BasicBlock<'ctx, R, S>,
) -> Vec<BasicBlock<'ctx, Dyn>>
where
    R: ReturnMarker,
    S: BlockSealState,
{
    let module = block.module();
    let ids = successor_ids(block.as_dyn());
    ids_to_blocks(module, Some(&ids))
}

fn ids_to_blocks<'ctx>(
    module: &'ctx crate::Module<'ctx>,
    ids: Option<&Vec<ValueId>>,
) -> Vec<BasicBlock<'ctx, Dyn>> {
    let label_ty = module.label_type().as_type().id();
    ids.into_iter()
        .flat_map(|ids| ids.iter().copied())
        .map(|id| BasicBlock::from_parts(id, module, label_ty))
        .collect()
}

pub(crate) fn successor_ids<'ctx>(block: BasicBlock<'ctx, Dyn, Unsealed>) -> Vec<ValueId> {
    let Some(term) = block.terminator() else {
        return Vec::new();
    };
    instruction_successor_ids(&term)
}

pub(crate) fn instruction_successor_ids<'ctx>(
    inst: &Instruction<'ctx, state::Attached>,
) -> Vec<ValueId> {
    match &inst.as_value().data().kind {
        ValueKindData::Instruction(data) => kind_successor_ids(&data.kind),
        _ => Vec::new(),
    }
}

pub(crate) fn kind_successor_ids(kind: &InstructionKindData) -> Vec<ValueId> {
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
        | InstructionKindData::UDiv(_)
        | InstructionKindData::SDiv(_)
        | InstructionKindData::URem(_)
        | InstructionKindData::SRem(_)
        | InstructionKindData::Shl(_)
        | InstructionKindData::LShr(_)
        | InstructionKindData::AShr(_)
        | InstructionKindData::And(_)
        | InstructionKindData::Or(_)
        | InstructionKindData::Xor(_)
        | InstructionKindData::FAdd(_)
        | InstructionKindData::FSub(_)
        | InstructionKindData::FMul(_)
        | InstructionKindData::FDiv(_)
        | InstructionKindData::FRem(_)
        | InstructionKindData::FCmp(_)
        | InstructionKindData::Alloca(_)
        | InstructionKindData::Load(_)
        | InstructionKindData::Store(_)
        | InstructionKindData::Gep(_)
        | InstructionKindData::Call(_)
        | InstructionKindData::Select(_)
        | InstructionKindData::Cast(_)
        | InstructionKindData::ICmp(_)
        | InstructionKindData::Phi(_)
        | InstructionKindData::FNeg(_)
        | InstructionKindData::Freeze(_)
        | InstructionKindData::VAArg(_)
        | InstructionKindData::ExtractValue(_)
        | InstructionKindData::InsertValue(_)
        | InstructionKindData::ExtractElement(_)
        | InstructionKindData::InsertElement(_)
        | InstructionKindData::ShuffleVector(_)
        | InstructionKindData::Fence(_)
        | InstructionKindData::AtomicCmpXchg(_)
        | InstructionKindData::AtomicRMW(_)
        | InstructionKindData::LandingPad(_)
        | InstructionKindData::CleanupPad(_)
        | InstructionKindData::CatchPad(_) => Vec::new(),
    }
}

fn branch_successor_ids(d: &crate::instr_types::BranchInstData) -> Vec<ValueId> {
    match &d.kind {
        crate::instr_types::BranchKind::Unconditional(target) => vec![*target],
        crate::instr_types::BranchKind::Conditional {
            then_bb, else_bb, ..
        } => vec![*then_bb, *else_bb],
    }
}
