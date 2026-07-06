//! Conservative instruction simplification transform.
//!
//! Mirrors the first scalar-cleanup slice of
//! `llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp::runImpl`: fold an
//! instruction to an already-existing constant when `constant_fold_instruction`
//! can prove the replacement without materialising new IR.

use super::IrResult;
use super::analysis::{CFGAnalyses, PreservedAnalyses};
use super::constant_folding::constant_fold_instruction;
use super::instruction::{Instruction, state};
use super::module::ModuleBrand;
use super::pass_context::FunctionPassContext;
use super::pass_manager::{FunctionPass, PassPipelineInfo};
use super::pass_pipeline::{FunctionPassScope, INSTSIMPLIFY, PassName};

/// Function transform that folds instructions to constants already expressible
/// in the existing module, then erases the original instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct InstSimplifyPass;

impl PassPipelineInfo for InstSimplifyPass {
    type Scope = FunctionPassScope;

    const PIPELINE_NAME: PassName<Self::Scope> = INSTSIMPLIFY;
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for InstSimplifyPass {
    fn run(&mut self, cx: &mut FunctionPassContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses> {
        let mut changed = false;
        loop {
            let iteration_changed = inst_simplify_iteration(cx)?;
            if !iteration_changed {
                break;
            }
            changed = true;
        }

        if changed {
            let mut preserved = PreservedAnalyses::none();
            preserved.preserve_set::<CFGAnalyses>();
            Ok(preserved)
        } else {
            Ok(PreservedAnalyses::all())
        }
    }
}

fn inst_simplify_iteration<'ctx, B: ModuleBrand + 'ctx>(
    cx: &mut FunctionPassContext<'_, 'ctx, B>,
) -> IrResult<bool> {
    let data_layout = cx.module().data_layout().clone();
    let module_token = cx.module_mut();

    for block in cx.function_mut().basic_blocks() {
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
