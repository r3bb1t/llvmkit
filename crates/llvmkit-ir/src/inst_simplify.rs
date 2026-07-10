//! Conservative instruction simplification transform.
//!
//! Mirrors the first scalar-cleanup slice of
//! `llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp::runImpl`: fold an
//! instruction to an already-existing constant when `constant_fold_instruction`
//! can prove the replacement without materialising new IR.

use super::IrResult;
use super::constant_folding::constant_fold_instruction;
use super::module::ModuleBrand;
use super::pass_access::PatchBody;
use super::pass_context::{FnCx, FnReport};
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
        let data_layout = patch.function().module().data_layout().clone();
        let scope = patch.worklist();
        while let Some(inst) = scope.next() {
            let view = inst.as_view();
            // Upstream runImpl only simplifies instructions with uses (!use_empty);
            // this also makes the ordered-atomic-load-from-constant-global case
            // terminate (folded once, kept, then use-empty on any re-visit).
            if !view.as_value().has_uses() {
                continue;
            }
            if let Some(replacement) = constant_fold_instruction(&view, &data_layout, None)? {
                patch.replace_all_uses(&view, replacement)?; // auto-pushes users
                if crate::dce::is_trivially_dead(&view) {
                    patch.erase(&inst);
                }
            }
        }
        drop(scope);
        Ok(patch.done())
    }
}
