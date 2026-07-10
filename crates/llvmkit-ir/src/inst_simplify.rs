//! Conservative instruction simplification transform.
//!
//! Mirrors the first scalar-cleanup slice of
//! `llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp::runImpl`: fold an
//! instruction to an already-existing constant when `constant_fold_instruction`
//! can prove the replacement without materialising new IR.

use super::IrResult;
use super::constant_folding::constant_fold_instruction;
use super::instruction::InstructionView;
use super::module::ModuleBrand;
use super::pass_access::PatchBody;
use super::pass_context::{FnCx, FnPatch, FnReport};
use super::pass_manager::FunctionPass;
use super::pass_pipeline::INSTSIMPLIFY;

/// Function transform that folds instructions to constants already expressible
/// in the existing module, then erases the original instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct InstSimplifyPass;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for InstSimplifyPass {
    // Folding replaces uses and erases the folded instruction in place; the CFG
    // is untouched, so the `PatchBody` floor is exactly right.
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = INSTSIMPLIFY.as_str();

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport> {
        // As in `DcePass`: no read-only pre-scan. Enter the mutator and fold;
        // `FnPatch::done` reports everything-preserved if nothing changed (the
        // dirty flag witnesses it) and the CFG-preserved floor otherwise.
        let patch = cx.mutate();
        while inst_simplify_iteration(&patch)? {}
        Ok(patch.done())
    }
}

fn inst_simplify_iteration<'ctx, B: ModuleBrand + 'ctx>(
    patch: &FnPatch<'_, '_, 'ctx, B, ()>,
) -> IrResult<bool> {
    let data_layout = patch.function().module().data_layout().clone();
    let module_ref = patch.module_mut().module_ref();

    for block in patch.function_mut().basic_blocks() {
        let instruction_ids = block.instruction_ids();
        for id in instruction_ids {
            let view = InstructionView::from_parts(id, module_ref);
            // Upstream `InstSimplifyPass::runImpl` only simplifies instructions
            // with uses (`!I.use_empty()`) and never re-queues a
            // simplified-but-live instruction. This restart-scan loop would
            // otherwise re-fold a folded-but-not-erased instruction (e.g. an
            // ordered atomic load from a constant global, kept by the
            // trivially-dead gate below) forever. Skipping use-empty
            // instructions makes the loop terminate: a folded instruction
            // has its uses replaced, so on the next scan it is use-empty and
            // skipped here (dead-code removal is DCE's job, not this pass's).
            if !view.as_value().has_uses() {
                continue;
            }
            let Some(replacement) = constant_fold_instruction(&view, &data_layout, None)? else {
                continue;
            };
            // Route the RAUW + erase through the mutator so the dirty flag is set.
            patch.replace_all_uses(&view, replacement)?;
            // Upstream `InstSimplifyPass::runImpl` erases the simplified
            // instruction only when it is trivially dead ("a call can get
            // simplified, but it may not be trivially dead"). Everything the
            // folder simplifies here is side-effect-free, so after RAUW it is
            // always trivially dead — but gate on it to match upstream and
            // stay correct if the folder ever grows a call path.
            if crate::dce::is_trivially_dead(&view) {
                patch.erase(&view)?;
            }
            return Ok(true);
        }
    }

    Ok(false)
}
