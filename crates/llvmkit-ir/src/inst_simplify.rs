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
            if !view.to_erased().has_uses() {
                continue;
            }
            if let Some(replacement) = constant_fold_instruction(&view, &data_layout, None)? {
                patch.replace_all_uses(&view, replacement)?; // auto-pushes users
                if crate::dce::is_trivially_dead(&view) {
                    patch.erase(&inst);
                }
            } else if let Some(v) = uniform_phi_value(&view) {
                // `simplifyPHINode`'s core: every incoming is the
                // same value (self-references allowed), so the phi IS that
                // value. Going through the same `replace_all_uses` path re-queues
                // the phi's former users so a dependent chain re-simplifies in the
                // one run; erasing the now-use-less phi cannot change the CFG, so
                // the `PatchBody` floor still holds.
                patch.replace_all_uses(&view, v)?; // auto-pushes users
                if crate::dce::is_trivially_dead(&view) {
                    patch.erase(&inst);
                }
            }
        }
        drop(scope);
        Ok(patch.done())
    }
}

/// If every incoming of `view` (a phi) is one same value — ignoring entries
/// that are the phi itself — return that value. `None` for non-phis, phis with
/// zero non-self incomings, and mixed phis. Mirrors the common-value core of
/// `llvm::simplifyPHINode` (self-reference tolerance). Undef blending (upstream
/// folds `[X, undef]` to `X`) is deliberately not mirrored here; it is
/// documented as out of scope.
fn uniform_phi_value<'ctx, B: ModuleBrand + 'ctx>(
    view: &crate::instruction::InstructionView<'ctx, B>,
) -> Option<crate::value::Value<'ctx, B>> {
    let crate::instruction::InstructionKind::Phi(kind) = view.kind()? else {
        return None;
    };
    let self_value = view.to_erased();
    let mut common: Option<crate::value::Value<'ctx, B>> = None;
    for (value, _block) in kind.incomings() {
        if value == self_value {
            continue; // self-reference: neutral
        }
        match common {
            None => common = Some(value),
            Some(c) if c == value => {}
            Some(_) => return None,
        }
    }
    common
}
