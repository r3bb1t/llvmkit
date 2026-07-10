//! Conservative dead-code elimination transform.
//!
//! Mirrors the first scalar-cleanup slice of
//! `llvm/lib/Transforms/Scalar/DCE.cpp::eliminateDeadCode`: erase unused
//! instructions that are trivially side-effect-free, repeating until cascaded
//! dead operands are removed.

use super::IrResult;
use super::instruction::{InstructionKind, InstructionView};
use super::module::ModuleBrand;
use super::pass_access::PatchBody;
use super::pass_context::{FnCx, FnPatch, FnReport};
use super::pass_manager::FunctionPass;
use super::pass_pipeline::DCE;

/// Function transform that erases unused side-effect-free instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DcePass;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for DcePass {
    // In-block instruction erasure only; the CFG is untouched, so the
    // `PatchBody` floor's "CFG analyses preserved" is exactly right.
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = DCE.as_str();

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport> {
        // Enter the mutator and erase dead instructions. No read-only
        // pre-scan is needed: `FnPatch::done` reports everything-preserved if
        // nothing was erased (the mutator's dirty flag *witnesses* that), and
        // the rung's CFG-preserved floor otherwise.
        let patch = cx.mutate();
        while dce_iteration(&patch) {}
        Ok(patch.done())
    }
}

fn dce_iteration<'ctx, B: ModuleBrand + 'ctx>(patch: &FnPatch<'_, '_, 'ctx, B, ()>) -> bool {
    let module_ref = patch.module_mut().module_ref();

    for block in patch.function_mut().basic_blocks() {
        let instruction_ids = block.instruction_ids();
        for id in instruction_ids {
            let view = InstructionView::from_parts(id, module_ref);
            if !is_trivially_dead(&view) {
                continue;
            }
            // `is_trivially_dead` already excludes terminators, so the narrow
            // succeeds; erase through the mutator so the dirty flag is set.
            let dead = view
                .as_non_terminator()
                .expect("a trivially-dead instruction is not a terminator");
            patch.erase(&dead);
            return true;
        }
    }

    false
}

pub(crate) fn is_trivially_dead<'ctx, B: ModuleBrand + 'ctx>(
    view: &InstructionView<'ctx, B>,
) -> bool {
    if view.as_value().has_uses() || view.is_terminator() {
        return false;
    }

    match view.kind() {
        // An unordered (non-volatile, NotAtomic-or-Unordered) load has no
        // memory-ordering side effects, so it is trivially dead — matches
        // `wouldInstructionBeTriviallyDead` via `LoadInst::isUnordered`
        // (an ordered atomic or volatile load is kept).
        Some(InstructionKind::Load(load)) => load.is_unordered(),
        Some(
            InstructionKind::Store(_)
            | InstructionKind::Fence(_)
            | InstructionKind::AtomicCmpXchg(_)
            | InstructionKind::AtomicRMW(_)
            | InstructionKind::Call(_)
            | InstructionKind::VAArg(_)
            | InstructionKind::LandingPad(_)
            | InstructionKind::CleanupPad(_)
            | InstructionKind::CatchPad(_),
        ) => false,
        Some(_) => true,
        None => false,
    }
}
