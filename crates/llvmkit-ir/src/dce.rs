//! Conservative dead-code elimination transform.
//!
//! Mirrors the first scalar-cleanup slice of
//! `llvm/lib/Transforms/Scalar/DCE.cpp::eliminateDeadCode`: erase unused
//! instructions that are trivially side-effect-free, repeating until cascaded
//! dead operands are removed.

use super::IrResult;
use super::instruction::{Instruction, InstructionKind, InstructionView, state};
use super::module::ModuleBrand;
use super::pass_access::PatchBody;
use super::pass_context::{FnCx, FnPatch, FnReport, FunctionView};
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
        // A run that erases nothing reports everything preserved; a run that
        // erases even one instruction reports the rung's CFG-preserved floor via
        // `FnPatch::done`. This nets to the same preservation (and the same
        // erased instructions) as the retired `none()` + `MinPreserves` path —
        // but the report can only be downgraded *before* `mutate()`, so the
        // all-live case is decided by a read-only pre-scan first.
        if !has_trivially_dead_instruction(cx.function()) {
            return Ok(cx.done());
        }
        let patch = cx.mutate();
        while dce_iteration(&patch) {}
        Ok(patch.done())
    }
}

/// Read-only pre-scan: does the function contain any trivially-dead instruction?
fn has_trivially_dead_instruction<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionView<'ctx, B>,
) -> bool {
    function
        .as_function()
        .basic_blocks()
        .any(|block| block.instructions().any(|inst| is_trivially_dead(&inst)))
}

fn dce_iteration<'ctx, B: ModuleBrand + 'ctx>(patch: &FnPatch<'_, '_, 'ctx, B, ()>) -> bool {
    let module_token = patch.module_mut();

    for block in patch.function_mut().basic_blocks() {
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
