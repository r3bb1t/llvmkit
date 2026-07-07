//! Conservative dead-code elimination transform.
//!
//! Mirrors the first scalar-cleanup slice of
//! `llvm/lib/Transforms/Scalar/DCE.cpp::eliminateDeadCode`: erase unused
//! instructions that are trivially side-effect-free, repeating until cascaded
//! dead operands are removed.

use super::IrResult;
use super::analysis::{CFGAnalyses, PreserveSet, PreservedAnalyses};
use super::instruction::{Instruction, InstructionKind, InstructionView, state};
use super::module::ModuleBrand;
use super::pass_context::TypedFunctionPassContext;
use super::pass_manager::{MutatesIr, PassPipelineInfo, TypedFunctionPass};
use super::pass_pipeline::{DCE, FunctionPassScope, PassName};

/// Function transform that erases unused side-effect-free instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DcePass;

impl PassPipelineInfo for DcePass {
    type Scope = FunctionPassScope;

    const PIPELINE_NAME: PassName<Self::Scope> = DCE;
}

impl<'ctx, B: ModuleBrand + 'ctx> TypedFunctionPass<'ctx, B> for DcePass {
    type Effect = MutatesIr;
    type Requires = ();
    // DCE never touches the CFG; declaring it makes under-reporting impossible
    // and lets the returned PA drop the manual preserve_set (D8).
    type MinPreserves = (PreserveSet<CFGAnalyses>,);
    const NAME: &'static str = DCE.as_str();

    fn run(
        &mut self,
        cx: &mut TypedFunctionPassContext<'_, '_, 'ctx, B, (), MutatesIr>,
    ) -> IrResult<PreservedAnalyses> {
        let mut changed = false;
        loop {
            let iteration_changed = dce_iteration(cx);
            if !iteration_changed {
                break;
            }
            changed = true;
        }

        if changed {
            Ok(PreservedAnalyses::none())
        } else {
            Ok(PreservedAnalyses::all())
        }
    }
}

fn dce_iteration<'ctx, B: ModuleBrand + 'ctx>(
    cx: &mut TypedFunctionPassContext<'_, '_, 'ctx, B, (), MutatesIr>,
) -> bool {
    let module_token = cx.module_mut();

    for block in cx.function_mut().basic_blocks() {
        let instruction_ids = block.instruction_ids();
        for id in instruction_ids {
            let inst = Instruction::<state::Attached, B>::from_parts(id, module_token.module_ref());
            if !is_trivially_dead(&inst.as_view()) {
                continue;
            }
            inst.erase_from_parent(module_token);
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
