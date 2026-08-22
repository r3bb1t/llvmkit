//! Recompute-on-demand dominance queries. Mirrors the observable
//! `llvm::DominatorTree` behavior needed by the verifier and the
//! analysis/pass-manager substrate. It implements the [`CfgIncremental`]
//! preservation hook by rebuilding from scratch (correct-by-recompute);
//! a genuinely sub-linear incremental update algorithm is deferred.
//!
//! [`CfgIncremental`]: crate::analysis::CfgIncremental

use std::collections::{HashMap, HashSet, VecDeque};

use super::basic_block::{BasicBlock, BasicBlockLabel};
use super::block_state::BlockTerminationState;
use super::cfg::{BasicBlockEdge, FunctionCfg};
use super::function::FunctionValue;
use super::instruction::{InstructionKindData, InstructionView};
use super::marker::{Dyn, ReturnMarker};
use super::module::ModuleBrand;
use super::pass_context::BasicBlockView;
use super::r#use::Use;
use super::value::{IsValue, Value, ValueKindData, ValueSlot};
use super::value_id::BlockId;

/// Analysis marker for caching a [`DominatorTree`] in the new-pass-manager
/// substrate. Its invalidation rule is wired in `analysis.rs`: preserved by
/// itself, `AllAnalysesOnFunction`, or `CfgAnalyses`, matching LLVM's
/// `DominatorTree::invalidate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DominatorTreeAnalysis;

/// Forward dominator tree for one function. The tree stores only value IDs, so
/// cached analysis results do not borrow the module; query methods accept typed
/// handles and compare their stable IDs.
#[derive(Debug, Clone)]
pub struct DominatorTree {
    reachable: HashSet<ValueSlot>,
    dominators: HashMap<ValueSlot, HashSet<ValueSlot>>,
    predecessors: HashMap<ValueSlot, Vec<ValueSlot>>,
    normal_dest: HashMap<ValueSlot, ValueSlot>,
    phi_incoming_blocks: HashMap<ValueSlot, Vec<ValueSlot>>,
    instruction_parent: HashMap<ValueSlot, ValueSlot>,
    instruction_order: HashMap<ValueSlot, (ValueSlot, usize)>,
}

mod dominator_block_sealed {
    pub trait Sealed {}
}

/// Basic-block identity accepted by dominator-tree block queries.
pub trait DominatorTreeBlock<'ctx>: dominator_block_sealed::Sealed {
    fn dominator_block_id(self) -> ValueSlot;
}

impl<'ctx, R, S, B> dominator_block_sealed::Sealed for BasicBlock<'ctx, R, S, B>
where
    R: ReturnMarker,
    S: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
}

impl<'ctx, R, S, B> DominatorTreeBlock<'ctx> for BasicBlock<'ctx, R, S, B>
where
    R: ReturnMarker,
    S: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
    #[inline]
    fn dominator_block_id(self) -> ValueSlot {
        self.slot()
    }
}

impl<'ctx, R, S, B> dominator_block_sealed::Sealed for &BasicBlock<'ctx, R, S, B>
where
    R: ReturnMarker,
    S: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
}

impl<'ctx, R, S, B> DominatorTreeBlock<'ctx> for &BasicBlock<'ctx, R, S, B>
where
    R: ReturnMarker,
    S: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
    #[inline]
    fn dominator_block_id(self) -> ValueSlot {
        self.slot()
    }
}

impl<'ctx, R, B> dominator_block_sealed::Sealed for BasicBlockLabel<'ctx, R, B>
where
    R: ReturnMarker,
    B: ModuleBrand + 'ctx,
{
}

impl<R, B> dominator_block_sealed::Sealed for BlockId<R, B>
where
    R: ReturnMarker,
    B: ModuleBrand,
{
}

impl<'ctx, R, B> DominatorTreeBlock<'ctx> for BlockId<R, B>
where
    R: ReturnMarker,
    B: ModuleBrand,
{
    #[inline]
    fn dominator_block_id(self) -> ValueSlot {
        self.slot()
    }
}

impl<R, B> dominator_block_sealed::Sealed for &BlockId<R, B>
where
    R: ReturnMarker,
    B: ModuleBrand,
{
}

impl<'ctx, R, B> DominatorTreeBlock<'ctx> for &BlockId<R, B>
where
    R: ReturnMarker,
    B: ModuleBrand,
{
    #[inline]
    fn dominator_block_id(self) -> ValueSlot {
        (*self).slot()
    }
}

impl<'ctx, R, B> DominatorTreeBlock<'ctx> for BasicBlockLabel<'ctx, R, B>
where
    R: ReturnMarker,
    B: ModuleBrand + 'ctx,
{
    #[inline]
    fn dominator_block_id(self) -> ValueSlot {
        self.slot()
    }
}

impl<'ctx, R, B> dominator_block_sealed::Sealed for &BasicBlockLabel<'ctx, R, B>
where
    R: ReturnMarker,
    B: ModuleBrand + 'ctx,
{
}

impl<'ctx, R, B> DominatorTreeBlock<'ctx> for &BasicBlockLabel<'ctx, R, B>
where
    R: ReturnMarker,
    B: ModuleBrand + 'ctx,
{
    #[inline]
    fn dominator_block_id(self) -> ValueSlot {
        self.slot()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> dominator_block_sealed::Sealed for BasicBlockView<'ctx, B> {}

impl<'ctx, B: ModuleBrand + 'ctx> DominatorTreeBlock<'ctx> for BasicBlockView<'ctx, B> {
    #[inline]
    fn dominator_block_id(self) -> ValueSlot {
        self.as_basic_block().slot()
    }
}

impl DominatorTree {
    /// Recompute dominance for `function`.
    pub fn new<'ctx, B: ModuleBrand + 'ctx>(function: FunctionValue<'ctx, Dyn, B>) -> Self {
        compute(function)
    }

    /// Recalculate this tree for a function. Mirrors LLVM's `recalculate`.
    pub fn recalculate<'ctx, B: ModuleBrand + 'ctx>(
        &mut self,
        function: FunctionValue<'ctx, Dyn, B>,
    ) {
        *self = compute(function);
    }

    /// Whether `block` is statically reachable from the entry block.
    pub fn is_reachable_from_entry<'ctx, B>(&self, block: B) -> bool
    where
        B: DominatorTreeBlock<'ctx>,
    {
        self.reachable.contains(&block.dominator_block_id())
    }

    /// Inclusive block dominance. For an unreachable use block, LLVM answers
    /// as if every reachable block dominates it; an unreachable block only
    /// dominates itself.
    pub fn dominates_block<'ctx, A, B>(&self, a: A, b: B) -> bool
    where
        A: DominatorTreeBlock<'ctx>,
        B: DominatorTreeBlock<'ctx>,
    {
        let a_id = a.dominator_block_id();
        let b_id = b.dominator_block_id();
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
    pub fn properly_dominates_block<'ctx, A, B>(&self, a: A, b: B) -> bool
    where
        A: DominatorTreeBlock<'ctx>,
        B: DominatorTreeBlock<'ctx>,
    {
        let a_id = a.dominator_block_id();
        let b_id = b.dominator_block_id();
        a_id != b_id && self.dominates_block_ids(a_id, b_id)
    }

    /// Whether instruction `def` dominates all ordinary uses in `user`.
    pub fn dominates_instruction<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        def: &InstructionView<'ctx, B>,
        user: &InstructionView<'ctx, B>,
    ) -> bool {
        let use_bb = user.parent();
        let def_bb = def.parent();
        let def_id = def.slot();
        let user_id = user.slot();

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
        if def_bb.slot() != use_bb.slot() {
            return self.dominates_block(def_bb, use_bb);
        }
        self.instruction_comes_before(def_id, user_id)
    }

    /// Whether instruction `def` dominates every possible use in `block`.
    pub fn dominates_instruction_block<'ctx, B, Block>(
        &self,
        def: &InstructionView<'ctx, B>,
        block: Block,
    ) -> bool
    where
        B: ModuleBrand + 'ctx,
        Block: DominatorTreeBlock<'ctx>,
    {
        let use_bb_id = block.dominator_block_id();
        let def_bb = def.parent();
        let def_id = def.slot();
        if !self.reachable.contains(&use_bb_id) {
            return true;
        }
        if !self.is_reachable_from_entry(def_bb) {
            return false;
        }
        if def_bb.slot() == use_bb_id {
            return false;
        }
        if let Some(normal_dest) = self.normal_dest.get(&def_id).copied() {
            return self.dominates_edge_slots(def_bb.slot(), normal_dest, use_bb_id);
        }
        self.dominates_block_ids(def_bb.slot(), use_bb_id)
    }

    /// Whether `def` dominates this specific operand use. Non-instruction
    /// values (arguments, constants, globals, functions) dominate all uses.
    pub fn dominates_use<'ctx, B: ModuleBrand + 'ctx>(
        &self,
        def: Value<'ctx, B>,
        use_edge: Use<'ctx, B>,
    ) -> bool {
        let Ok(def_inst) = InstructionView::try_from(def) else {
            return true;
        };
        let Ok(user_inst) = InstructionView::try_from(use_edge.user()) else {
            return true;
        };
        let def_id = def_inst.slot();
        let user_id = user_inst.slot();
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
    pub fn dominates_edge<'ctx, EB, B>(&self, edge: BasicBlockEdge<EB>, block: B) -> bool
    where
        EB: ModuleBrand + 'ctx,
        B: DominatorTreeBlock<'ctx>,
    {
        self.dominates_edge_slots(
            edge.start().slot(),
            edge.end().slot(),
            block.dominator_block_id(),
        )
    }

    /// Whether edge `edge` dominates this specific use.
    pub fn dominates_edge_use<'ctx, EB: ModuleBrand + 'ctx, B: ModuleBrand + 'ctx>(
        &self,
        edge: BasicBlockEdge<EB>,
        use_edge: Use<'ctx, B>,
    ) -> bool {
        let Ok(user_inst) = InstructionView::try_from(use_edge.user()) else {
            return true;
        };
        self.dominates_edge_use_ids(
            edge.start().slot(),
            edge.end().slot(),
            user_inst.slot(),
            use_edge.index(),
        )
    }

    fn dominates_edge_use_ids(
        &self,
        start_id: ValueSlot,
        end_id: ValueSlot,
        user_id: ValueSlot,
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
        self.dominates_edge_slots(start_id, end_id, use_bb_id)
    }

    /// Edge dominance over raw slots. Ports the `BasicBlockEdge` overload of
    /// `DominatorTree::dominates` once the edge is already in hand.
    pub(crate) fn dominates_edge_slots(
        &self,
        start_id: ValueSlot,
        end_id: ValueSlot,
        use_bb_id: ValueSlot,
    ) -> bool {
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

    /// Every block that *strictly* dominates `block`.
    ///
    /// Upstream reaches the same set one step at a time, walking
    /// `DomTreeNode::getIDom` from `block` to the root — see the dominating-
    /// condition loop in `isGuaranteedNotToBeUndefOrPoison`
    /// (`ValueTracking.cpp`). llvmkit stores the dominator *sets* rather than
    /// an idom tree, so the walk is spelled as the set it enumerates.
    ///
    /// Order is therefore unspecified where upstream's is nearest-first. Every
    /// caller so far is a pure existential over the set — "does any dominating
    /// terminator branch on this value?" — for which the two agree; a caller
    /// that wants the *nearest* such block must not use this.
    ///
    /// Empty when `block` is unreachable, which is upstream's `if (!DNode)`.
    pub(crate) fn strictly_dominating_blocks(
        &self,
        block: ValueSlot,
    ) -> impl Iterator<Item = ValueSlot> + '_ {
        let reachable = self.reachable.contains(&block);
        self.dominators
            .get(&block)
            .filter(|_| reachable)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |dominator| *dominator != block)
    }

    fn dominates_block_ids(&self, a_id: ValueSlot, b_id: ValueSlot) -> bool {
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

    fn use_block_id(&self, user_id: ValueSlot, use_index: u32) -> ValueSlot {
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

    fn instruction_comes_before(&self, def: ValueSlot, user: ValueSlot) -> bool {
        let Some((def_bb, def_index)) = self.instruction_order.get(&def) else {
            return false;
        };
        let Some((user_bb, user_index)) = self.instruction_order.get(&user) else {
            return false;
        };
        def_bb == user_bb && def_index < user_index
    }
}

fn compute<'ctx, B: ModuleBrand + 'ctx>(function: FunctionValue<'ctx, Dyn, B>) -> DominatorTree {
    let cfg = FunctionCfg::new(function);
    let reachable = compute_reachable(function, &cfg);
    let dominators = compute_dominators(function, &cfg, &reachable);
    let predecessors = compute_predecessors(function, &cfg);
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

fn compute_reachable<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, Dyn, B>,
    cfg: &FunctionCfg<'ctx, B>,
) -> HashSet<ValueSlot> {
    let mut reachable = HashSet::new();
    let Some(entry) = function.entry_block().map(|bb| bb.id()) else {
        return reachable;
    };
    let mut worklist = VecDeque::from([entry]);
    while let Some(block) = worklist.pop_front() {
        let block_id = block.slot();
        if !reachable.insert(block_id) {
            continue;
        }
        for succ in cfg.successors(block) {
            if !reachable.contains(&succ.slot()) {
                worklist.push_back(succ);
            }
        }
    }
    reachable
}

fn compute_dominators<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, Dyn, B>,
    cfg: &FunctionCfg<'ctx, B>,
    reachable: &HashSet<ValueSlot>,
) -> HashMap<ValueSlot, HashSet<ValueSlot>> {
    let Some(entry) = function.entry_block().map(|bb| bb.as_dyn()) else {
        return HashMap::new();
    };
    let all_reachable = reachable.clone();
    let mut doms: HashMap<ValueSlot, HashSet<ValueSlot>> = HashMap::new();
    for block in function.basic_blocks().map(|bb| bb.as_dyn()) {
        let id = block.slot();
        if !reachable.contains(&id) {
            continue;
        }
        if id == entry.slot() {
            doms.insert(id, HashSet::from([id]));
        } else {
            doms.insert(id, all_reachable.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for block in function.basic_blocks().map(|bb| bb.as_dyn()) {
            let block_id = block.slot();
            if block_id == entry.slot() || !reachable.contains(&block_id) {
                continue;
            }
            let mut pred_sets = cfg
                .predecessors(&block)
                .filter(|pred| reachable.contains(&pred.slot()))
                .filter_map(|pred| doms.get(&pred.slot()).cloned());
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

/// The predecessor map, read off [`FunctionCfg`] rather than re-derived by
/// transposing its edge list — `pred_iterator` is a use-list view and the
/// edge list is in block order, so the two answer in different orders.
fn compute_predecessors<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, Dyn, B>,
    cfg: &FunctionCfg<'ctx, B>,
) -> HashMap<ValueSlot, Vec<ValueSlot>> {
    function
        .basic_blocks()
        .map(|bb| {
            (
                bb.slot(),
                cfg.predecessors(&bb.as_dyn())
                    .map(|pred| pred.slot())
                    .collect(),
            )
        })
        .collect()
}

type InstructionMaps = (
    HashMap<ValueSlot, ValueSlot>,
    HashMap<ValueSlot, (ValueSlot, usize)>,
    HashMap<ValueSlot, ValueSlot>,
    HashMap<ValueSlot, Vec<ValueSlot>>,
);

fn compute_instruction_maps<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, Dyn, B>,
) -> InstructionMaps {
    let mut parent = HashMap::new();
    let mut order = HashMap::new();
    let mut normal_dest = HashMap::new();
    let mut phi_incoming_blocks = HashMap::new();
    for block in function.basic_blocks() {
        let block_id = block.slot();
        for (index, inst) in block.instructions().enumerate() {
            let inst_id = inst.slot();
            parent.insert(inst_id, block_id);
            order.insert(inst_id, (block_id, index));
            if let ValueKindData::Instruction(data) = &inst.as_erased().data().kind {
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

fn is_phi<B: ModuleBrand>(inst: &InstructionView<'_, B>) -> bool {
    matches!(
        &inst.as_erased().data().kind,
        ValueKindData::Instruction(data) if matches!(data.kind, InstructionKindData::Phi(_))
    )
}

fn is_invoke<B: ModuleBrand>(inst: &InstructionView<'_, B>) -> bool {
    matches!(
        &inst.as_erased().data().kind,
        ValueKindData::Instruction(data) if matches!(data.kind, InstructionKindData::Invoke(_))
    )
}

fn is_callbr<B: ModuleBrand>(inst: &InstructionView<'_, B>) -> bool {
    matches!(
        &inst.as_erased().data().kind,
        ValueKindData::Instruction(data) if matches!(data.kind, InstructionKindData::CallBr(_))
    )
}
