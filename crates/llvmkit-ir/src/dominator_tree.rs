//! Recompute-on-demand dominance queries. Mirrors the observable
//! `llvm::DominatorTree` behavior needed by the verifier and the first
//! analysis/pass-manager substrate, while deliberately deferring
//! incremental update APIs.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::basic_block::BasicBlock;
use crate::block_state::BlockSealState;
use crate::cfg::{BasicBlockEdge, FunctionCfg};
use crate::function::FunctionValue;
use crate::instruction::{Instruction, InstructionKindData, state};
use crate::marker::{Dyn, ReturnMarker};
use crate::r#use::Use;
use crate::value::{Value, ValueId, ValueKindData};

/// Analysis marker for caching a [`DominatorTree`] in the new-pass-manager
/// substrate. Its invalidation rule is wired in `analysis.rs`: preserved by
/// itself, `AllAnalysesOnFunction`, or `CFGAnalyses`, matching LLVM's
/// `DominatorTree::invalidate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DominatorTreeAnalysis;

/// Forward dominator tree for one function. The tree stores only value IDs, so
/// cached analysis results do not borrow the module; query methods accept typed
/// handles and compare their stable IDs.
#[derive(Debug, Clone)]
pub struct DominatorTree {
    reachable: HashSet<ValueId>,
    dominators: HashMap<ValueId, HashSet<ValueId>>,
    predecessors: HashMap<ValueId, Vec<ValueId>>,
    normal_dest: HashMap<ValueId, ValueId>,
    phi_incoming_blocks: HashMap<ValueId, Vec<ValueId>>,
    instruction_parent: HashMap<ValueId, ValueId>,
    instruction_order: HashMap<ValueId, (ValueId, usize)>,
}

impl DominatorTree {
    /// Recompute dominance for `function`.
    pub fn new<'ctx>(function: FunctionValue<'ctx, Dyn>) -> Self {
        compute(function)
    }

    /// Recalculate this tree for a function. Mirrors LLVM's `recalculate`.
    pub fn recalculate<'ctx>(&mut self, function: FunctionValue<'ctx, Dyn>) {
        *self = compute(function);
    }

    /// Whether `block` is statically reachable from the entry block.
    pub fn is_reachable_from_entry<'ctx, R, S>(&self, block: BasicBlock<'ctx, R, S>) -> bool
    where
        R: ReturnMarker,
        S: BlockSealState,
    {
        self.reachable.contains(&block.as_dyn().as_value().id)
    }

    /// Inclusive block dominance. For an unreachable use block, LLVM answers
    /// as if every reachable block dominates it; an unreachable block only
    /// dominates itself.
    pub fn dominates_block<'ctx, RA, SA, RB, SB>(
        &self,
        a: BasicBlock<'ctx, RA, SA>,
        b: BasicBlock<'ctx, RB, SB>,
    ) -> bool
    where
        RA: ReturnMarker,
        SA: BlockSealState,
        RB: ReturnMarker,
        SB: BlockSealState,
    {
        let a_id = a.as_dyn().as_value().id;
        let b_id = b.as_dyn().as_value().id;
        if a_id == b_id {
            return true;
        }
        let a_reachable = self.reachable.contains(&a_id);
        let b_reachable = self.reachable.contains(&b_id);
        if !b_reachable {
            return a_reachable;
        }
        if !a_reachable {
            return false;
        }
        self.dominators
            .get(&b_id)
            .is_some_and(|doms| doms.contains(&a_id))
    }

    /// Strict block dominance.
    pub fn properly_dominates_block<'ctx, RA, SA, RB, SB>(
        &self,
        a: BasicBlock<'ctx, RA, SA>,
        b: BasicBlock<'ctx, RB, SB>,
    ) -> bool
    where
        RA: ReturnMarker,
        SA: BlockSealState,
        RB: ReturnMarker,
        SB: BlockSealState,
    {
        a.as_dyn().as_value().id != b.as_dyn().as_value().id && self.dominates_block(a, b)
    }

    /// Whether instruction `def` dominates all ordinary uses in `user`.
    pub fn dominates_instruction<'ctx>(
        &self,
        def: &Instruction<'ctx, state::Attached>,
        user: &Instruction<'ctx, state::Attached>,
    ) -> bool {
        let use_bb = user.parent();
        let def_bb = def.parent();
        let def_id = def.as_value().id;
        let user_id = user.as_value().id;

        if !self.is_reachable_from_entry(use_bb) {
            return true;
        }
        if !self.is_reachable_from_entry(def_bb) {
            return false;
        }
        if def_id == user_id {
            return false;
        }
        if is_invoke(def) || is_callbr(def) || is_phi(user) {
            return self.dominates_instruction_block(def, use_bb);
        }
        if def_bb.as_value().id != use_bb.as_value().id {
            return self.dominates_block(def_bb, use_bb);
        }
        self.instruction_comes_before(def_id, user_id)
    }

    /// Whether instruction `def` dominates every possible use in `block`.
    pub fn dominates_instruction_block<'ctx, R, S>(
        &self,
        def: &Instruction<'ctx, state::Attached>,
        block: BasicBlock<'ctx, R, S>,
    ) -> bool
    where
        R: ReturnMarker,
        S: BlockSealState,
    {
        let use_bb = block.as_dyn();
        let def_bb = def.parent();
        let def_id = def.as_value().id;
        if !self.is_reachable_from_entry(use_bb) {
            return true;
        }
        if !self.is_reachable_from_entry(def_bb) {
            return false;
        }
        if def_bb.as_value().id == use_bb.as_value().id {
            return false;
        }
        if let Some(normal_dest) = self.normal_dest.get(&def_id).copied() {
            return self.dominates_edge_ids(
                def_bb.as_value().id,
                normal_dest,
                use_bb.as_value().id,
            );
        }
        self.dominates_block(def_bb, use_bb)
    }

    /// Whether `def` dominates this specific operand use. Non-instruction
    /// values (arguments, constants, globals, functions) dominate all uses.
    pub fn dominates_use<'ctx>(&self, def: Value<'ctx>, use_edge: Use<'ctx>) -> bool {
        let Ok(def_inst) = Instruction::try_from(def) else {
            return true;
        };
        let Ok(user_inst) = Instruction::try_from(use_edge.user()) else {
            return true;
        };
        let def_id = def_inst.as_value().id;
        let user_id = user_inst.as_value().id;
        let Some(def_bb_id) = self.instruction_parent.get(&def_id).copied() else {
            return false;
        };
        let use_bb_id = self.use_block_id(user_id, use_edge.index());

        if !self.reachable.contains(&use_bb_id) {
            return true;
        }
        if !self.reachable.contains(&def_bb_id) {
            return false;
        }
        if let Some(normal_dest) = self.normal_dest.get(&def_id).copied() {
            return self.dominates_edge_use_ids(def_bb_id, normal_dest, user_id, use_edge.index());
        }
        if def_bb_id != use_bb_id {
            return self.dominates_block_ids(def_bb_id, use_bb_id);
        }
        if self.phi_incoming_blocks.contains_key(&user_id) {
            return true;
        }
        self.instruction_comes_before(def_id, user_id)
    }

    /// Whether edge `edge` dominates all uses in `block`.
    pub fn dominates_edge<'ctx, R, S>(
        &self,
        edge: BasicBlockEdge<'ctx>,
        block: BasicBlock<'ctx, R, S>,
    ) -> bool
    where
        R: ReturnMarker,
        S: BlockSealState,
    {
        self.dominates_edge_ids(
            edge.start().as_value().id,
            edge.end().as_value().id,
            block.as_dyn().as_value().id,
        )
    }

    /// Whether edge `edge` dominates this specific use.
    pub fn dominates_edge_use<'ctx>(
        &self,
        edge: BasicBlockEdge<'ctx>,
        use_edge: Use<'ctx>,
    ) -> bool {
        let Ok(user_inst) = Instruction::try_from(use_edge.user()) else {
            return true;
        };
        self.dominates_edge_use_ids(
            edge.start().as_value().id,
            edge.end().as_value().id,
            user_inst.as_value().id,
            use_edge.index(),
        )
    }

    fn dominates_edge_use_ids(
        &self,
        start_id: ValueId,
        end_id: ValueId,
        user_id: ValueId,
        use_index: u32,
    ) -> bool {
        if self
            .phi_incoming_blocks
            .get(&user_id)
            .is_some_and(|blocks| {
                let Some(index) = usize::try_from(use_index).ok() else {
                    return false;
                };
                blocks
                    .get(index)
                    .is_some_and(|incoming| *incoming == start_id)
                    && self.instruction_parent.get(&user_id).copied() == Some(end_id)
            })
        {
            return true;
        }
        let use_bb_id = self.use_block_id(user_id, use_index);
        self.dominates_edge_ids(start_id, end_id, use_bb_id)
    }

    fn dominates_edge_ids(&self, start_id: ValueId, end_id: ValueId, use_bb_id: ValueId) -> bool {
        if !self.dominates_block_ids(end_id, use_bb_id) {
            return false;
        }
        let Some(preds) = self.predecessors.get(&end_id) else {
            return false;
        };
        let mut start_edge_seen = false;
        for pred in preds {
            if *pred == start_id {
                if start_edge_seen {
                    return false;
                }
                start_edge_seen = true;
                continue;
            }
            if !self.dominates_block_ids(end_id, *pred) {
                return false;
            }
        }
        start_edge_seen
    }

    fn dominates_block_ids(&self, a_id: ValueId, b_id: ValueId) -> bool {
        if a_id == b_id {
            return true;
        }
        let a_reachable = self.reachable.contains(&a_id);
        let b_reachable = self.reachable.contains(&b_id);
        if !b_reachable {
            return a_reachable;
        }
        if !a_reachable {
            return false;
        }
        self.dominators
            .get(&b_id)
            .is_some_and(|doms| doms.contains(&a_id))
    }

    fn use_block_id(&self, user_id: ValueId, use_index: u32) -> ValueId {
        if let Some(blocks) = self.phi_incoming_blocks.get(&user_id)
            && let Some(index) = usize::try_from(use_index).ok()
            && let Some(block_id) = blocks.get(index)
        {
            return *block_id;
        }
        self.instruction_parent
            .get(&user_id)
            .copied()
            .unwrap_or(user_id)
    }

    fn instruction_comes_before(&self, def: ValueId, user: ValueId) -> bool {
        let Some((def_bb, def_index)) = self.instruction_order.get(&def) else {
            return false;
        };
        let Some((user_bb, user_index)) = self.instruction_order.get(&user) else {
            return false;
        };
        def_bb == user_bb && def_index < user_index
    }
}

fn compute<'ctx>(function: FunctionValue<'ctx, Dyn>) -> DominatorTree {
    let cfg = FunctionCfg::new(function);
    let reachable = compute_reachable(function, &cfg);
    let dominators = compute_dominators(function, &cfg, &reachable);
    let predecessors = compute_predecessors(&cfg);
    let (instruction_parent, instruction_order, normal_dest, phi_incoming_blocks) =
        compute_instruction_maps(function);
    DominatorTree {
        reachable,
        dominators,
        predecessors,
        normal_dest,
        phi_incoming_blocks,
        instruction_parent,
        instruction_order,
    }
}

fn compute_reachable<'ctx>(
    function: FunctionValue<'ctx, Dyn>,
    cfg: &FunctionCfg<'ctx>,
) -> HashSet<ValueId> {
    let mut reachable = HashSet::new();
    let Some(entry) = function.entry_block() else {
        return reachable;
    };
    let mut worklist = VecDeque::from([entry.as_dyn()]);
    while let Some(block) = worklist.pop_front() {
        let block_id = block.as_value().id;
        if !reachable.insert(block_id) {
            continue;
        }
        for succ in cfg.successors(block) {
            if !reachable.contains(&succ.as_value().id) {
                worklist.push_back(succ);
            }
        }
    }
    reachable
}

fn compute_dominators<'ctx>(
    function: FunctionValue<'ctx, Dyn>,
    cfg: &FunctionCfg<'ctx>,
    reachable: &HashSet<ValueId>,
) -> HashMap<ValueId, HashSet<ValueId>> {
    let Some(entry) = function.entry_block().map(|bb| bb.as_dyn()) else {
        return HashMap::new();
    };
    let all_reachable = reachable.clone();
    let mut doms: HashMap<ValueId, HashSet<ValueId>> = HashMap::new();
    for block in function.basic_blocks().map(|bb| bb.as_dyn()) {
        let id = block.as_value().id;
        if !reachable.contains(&id) {
            continue;
        }
        if id == entry.as_value().id {
            doms.insert(id, HashSet::from([id]));
        } else {
            doms.insert(id, all_reachable.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for block in function.basic_blocks().map(|bb| bb.as_dyn()) {
            let block_id = block.as_value().id;
            if block_id == entry.as_value().id || !reachable.contains(&block_id) {
                continue;
            }
            let mut pred_sets = cfg
                .predecessors(block)
                .into_iter()
                .filter(|pred| reachable.contains(&pred.as_value().id))
                .filter_map(|pred| doms.get(&pred.as_value().id).cloned());
            let mut new_set = pred_sets.next().unwrap_or_default();
            for pred_set in pred_sets {
                new_set = new_set.intersection(&pred_set).copied().collect();
            }
            new_set.insert(block_id);
            if doms.get(&block_id) != Some(&new_set) {
                doms.insert(block_id, new_set);
                changed = true;
            }
        }
    }
    doms
}

fn compute_predecessors<'ctx>(cfg: &FunctionCfg<'ctx>) -> HashMap<ValueId, Vec<ValueId>> {
    let mut predecessors: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    for edge in cfg.edges() {
        predecessors
            .entry(edge.end().as_value().id)
            .or_default()
            .push(edge.start().as_value().id);
    }
    predecessors
}

type InstructionMaps = (
    HashMap<ValueId, ValueId>,
    HashMap<ValueId, (ValueId, usize)>,
    HashMap<ValueId, ValueId>,
    HashMap<ValueId, Vec<ValueId>>,
);

fn compute_instruction_maps<'ctx>(function: FunctionValue<'ctx, Dyn>) -> InstructionMaps {
    let mut parent = HashMap::new();
    let mut order = HashMap::new();
    let mut normal_dest = HashMap::new();
    let mut phi_incoming_blocks = HashMap::new();
    for block in function.basic_blocks() {
        let block_id = block.as_value().id;
        for (index, inst) in block.instructions().enumerate() {
            let inst_id = inst.as_value().id;
            parent.insert(inst_id, block_id);
            order.insert(inst_id, (block_id, index));
            if let ValueKindData::Instruction(data) = &inst.as_value().data().kind {
                match &data.kind {
                    InstructionKindData::Invoke(invoke) => {
                        normal_dest.insert(inst_id, invoke.normal_dest.get());
                    }
                    InstructionKindData::Phi(phi) => {
                        phi_incoming_blocks.insert(
                            inst_id,
                            phi.incoming.borrow().iter().map(|(_, b)| *b).collect(),
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    (parent, order, normal_dest, phi_incoming_blocks)
}

fn is_phi(inst: &Instruction<'_, state::Attached>) -> bool {
    matches!(
        &inst.as_value().data().kind,
        ValueKindData::Instruction(data) if matches!(data.kind, InstructionKindData::Phi(_))
    )
}

fn is_invoke(inst: &Instruction<'_, state::Attached>) -> bool {
    matches!(
        &inst.as_value().data().kind,
        ValueKindData::Instruction(data) if matches!(data.kind, InstructionKindData::Invoke(_))
    )
}

fn is_callbr(inst: &Instruction<'_, state::Attached>) -> bool {
    matches!(
        &inst.as_value().data().kind,
        ValueKindData::Instruction(data) if matches!(data.kind, InstructionKindData::CallBr(_))
    )
}
