//! Conservative instruction simplification transform.
//!
//! Mirrors the first scalar-cleanup slice of
//! `llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp::runImpl`: fold an
//! instruction to an already-existing constant when `constant_fold_instruction`
//! can prove the replacement without materialising new IR.

use super::IrResult;
use super::constant_folding::constant_fold_instruction;
use super::data_layout::DataLayout;
use super::instruction::{Instruction, state};
use super::module::ModuleBrand;
use super::pass_access::PatchBody;
use super::pass_context::{FnCx, FnPatch, FnReport, FunctionView};
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
        // As in `DcePass`: a run that folds nothing reports everything preserved,
        // a run that folds anything reports the CFG-preserved floor. Since the
        // report can only be downgraded before `mutate()`, a read-only pre-scan
        // decides the no-op case first. This nets to the same preservation (and
        // the same folded/erased instructions) as the retired path.
        let data_layout = cx.module().data_layout().clone();
        if !has_foldable_instruction(cx.function(), &data_layout)? {
            return Ok(cx.done());
        }
        let patch = cx.mutate();
        loop {
            let iteration_changed = inst_simplify_iteration(&patch)?;
            if !iteration_changed {
                break;
            }
        }
        Ok(patch.done())
    }
}

/// Read-only pre-scan: is there a use-having instruction that folds to a
/// constant? Mirrors the fold gate in [`inst_simplify_iteration`].
fn has_foldable_instruction<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionView<'ctx, B>,
    data_layout: &DataLayout,
) -> IrResult<bool> {
    for block in function.as_function().basic_blocks() {
        for view in block.instructions() {
            if !view.as_value().has_uses() {
                continue;
            }
            if constant_fold_instruction(&view, data_layout, None)?.is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn inst_simplify_iteration<'ctx, B: ModuleBrand + 'ctx>(
    patch: &FnPatch<'_, '_, 'ctx, B, ()>,
) -> IrResult<bool> {
    let data_layout = patch.function().module().data_layout().clone();
    let module_token = patch.module_mut();

    for block in patch.function_mut().basic_blocks() {
        let instruction_ids = block.instruction_ids();
        for id in instruction_ids {
            let inst = Instruction::<state::Attached, B>::from_parts(id, module_token.module_ref());
            let view = inst.as_view();
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
            let id = inst.as_value().id();
            inst.replace_all_uses_with(module_token, replacement)?;
            // Upstream `InstSimplifyPass::runImpl` erases the simplified
            // instruction only when it is trivially dead ("a call can get
            // simplified, but it may not be trivially dead"). Everything the
            // folder simplifies here is side-effect-free, so after RAUW it is
            // always trivially dead — but gate on it to match upstream and
            // stay correct if the folder ever grows a call path.
            let erased =
                Instruction::<state::Attached, B>::from_parts(id, module_token.module_ref());
            if crate::dce::is_trivially_dead(&erased.as_view()) {
                erased.erase_from_parent(module_token);
            }
            return Ok(true);
        }
    }

    Ok(false)
}
