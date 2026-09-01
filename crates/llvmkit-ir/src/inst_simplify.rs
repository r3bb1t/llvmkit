//! Conservative instruction simplification transform.
//!
//! Mirrors the first scalar-cleanup slice of
//! `llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp::runImpl`: fold an
//! instruction to an already-existing constant when `constant_fold_instruction`
//! can prove the replacement without materialising new IR.

use super::IrResult;
use super::constant::{Constant, is_poison, is_undef};
use super::constant_folding::constant_fold_instruction;
use super::data_layout::DataLayout;
use super::dominator_tree::{DominatorTree, DominatorTreeAnalysis};
use super::instruction::{InstructionKind, InstructionView};
use super::module::ModuleBrand;
use super::pass_access::PatchBody;
use super::pass_context::{FnCx, FnReport};
use super::pass_manager::FunctionPass;
use super::pass_pipeline::INSTSIMPLIFY;
use super::value::Value;
use super::value_tracking::{ValueTrackingQuery, is_known_not_poison};

/// Function transform that folds instructions to constants already expressible
/// in the existing module, then erases the original instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct InstSimplifyPass;

impl<B: ModuleBrand> FunctionPass<B> for InstSimplifyPass {
    // Folding replaces uses and erases the folded instruction in place; the CFG
    // is untouched, so the `PatchBody` floor is exactly right.
    type Access = PatchBody;
    // `runImpl`'s `SimplifyQuery` carries a real `DominatorTree` at both entry
    // points, and the block loop's first statement reads it. Prefetching it is
    // what makes that statement portable; a `PatchBody` pass cannot change the
    // CFG, so the tree stays valid for the whole run.
    type Requires = (DominatorTreeAnalysis,);
    const NAME: &'static str = INSTSIMPLIFY.as_str();

    fn run<'m, 'ctx>(
        &mut self,
        cx: FnCx<'m, '_, 'ctx, B, PatchBody, (DominatorTreeAnalysis,)>,
    ) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        // As in `DcePass`: no read-only pre-scan. Enter the mutator and fold;
        // `FnPatch::done` reports everything-preserved if nothing changed (the
        // dirty flag witnesses it) and the CFG-preserved floor otherwise.
        let patch = cx.mutate();
        let dominators = patch.analysis::<DominatorTreeAnalysis, _>();
        let data_layout = patch.function().module().data_layout().clone();
        let scope = patch.worklist();
        while let Some(inst) = scope.step() {
            let view = inst.as_view();
            // `for (BasicBlock &BB : F) { if (!SQ.DT->isReachableFromEntry(&BB))
            // continue; ... }` — runImpl's first statement, with its own
            // comment: "Unreachable code can take on strange forms that we are
            // not prepared to handle. For example, an instruction may have
            // itself as an operand." llvmkit walks a flat instruction worklist
            // rather than upstream's block loop, so the block gate is asked per
            // instruction, of the instruction's parent.
            if !dominators.is_reachable_from_entry(view.parent()) {
                continue;
            }
            // Upstream runImpl only simplifies instructions with uses (!use_empty);
            // this also makes the ordered-atomic-load-from-constant-global case
            // terminate (folded once, kept, then use-empty on any re-visit).
            if !view.to_erased().has_uses() {
                continue;
            }
            // `if (Value *V = simplifyInstruction(&I, SQ))`. A phi is answered
            // by `simplifyPHINode` and never reaches `ConstantFoldInstruction`
            // upstream -- `runImpl` does not call that routine at all -- so the
            // phi arm is asked first and constant folding stands in for
            // `simplifyInstruction`'s remaining arms. Asking in the other order
            // is observable: `fold_phi` (the port of `ConstantFoldInstruction`'s
            // own `PHINode` arm) answers `undef` for an all-`poison` phi, where
            // `simplifyPHINode` answers `poison`.
            let replacement = match simplify_phi_node(&view, dominators, &data_layout)? {
                Some(value) => Some(value),
                None => {
                    constant_fold_instruction(&view, &data_layout, None)?.map(Constant::as_erased)
                }
            };
            if let Some(replacement) = replacement {
                // Going through one `replace_all_uses` path re-queues the
                // instruction's former users, so a dependent chain
                // re-simplifies within the one run; erasing the now-use-less
                // instruction cannot change the CFG, so the `PatchBody` floor
                // still holds.
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

/// Ports `llvm::simplifyPHINode` (`InstructionSimplify.cpp`) for the shape
/// this pass reaches it in: `IncomingValues` is the phi's own operand list, so
/// the `ArrayRef` parameter is read off `view` rather than passed in.
///
/// Self-referencing, `poison` and `undef` incomings are skipped; a phi whose
/// remaining incomings are one common value simplifies to that value, and one
/// with no remaining incoming simplifies to `undef` (if any incoming was
/// `undef`) or `poison`.
///
/// `constant_fold_instruction`'s `fold_phi` ports `ConstantFoldInstruction`'s own
/// `PHINode` arm, and upstream has both routines — but `InstSimplifyPass.cpp`'s
/// `runImpl` never calls `ConstantFoldInstruction`, so a phi reaches this one
/// and only this one. The pass asks in that order. The two disagree on an
/// all-`poison` phi: `fold_phi` answers `undef` (its arm skips
/// `isa<UndefValue>`, which catches poison, and falls back to
/// `UndefValue::get`) where `simplifyPHINode` answers `poison`.
///
/// Upstream's two guards on the blended return are ported with it:
/// `valueDominatesPHI`, and `isGuaranteedNotToBePoison` when an `undef`
/// incoming is present (do not replace an `undef` with a `poison`).
fn simplify_phi_node<'ctx, B: ModuleBrand + 'ctx>(
    view: &InstructionView<'ctx, B>,
    dominators: &DominatorTree,
    data_layout: &DataLayout,
) -> IrResult<Option<Value<'ctx, B>>> {
    let Some(InstructionKind::Phi(kind)) = view.kind() else {
        return Ok(None);
    };
    let self_value = view.to_erased();
    // WARNING carried over from upstream: no PHI CSE here — the PHI this may
    // simplify to need not be def-reachable from the original PHI.
    let mut common_value: Option<Value<'ctx, B>> = None;
    let mut has_poison_input = false;
    let mut has_undef_input = false;
    for (incoming, _block) in kind.incomings() {
        // If the incoming value is the phi node itself, it can safely be skipped.
        if incoming == self_value {
            continue;
        }
        let constant = Constant::try_from(incoming).ok();
        if constant.is_some_and(is_poison) {
            has_poison_input = true;
            continue;
        }
        // Remember that we saw an undef value, but otherwise ignore them.
        if constant.is_some_and(is_undef) {
            has_undef_input = true;
            continue;
        }
        if common_value.is_some_and(|common| incoming != common) {
            return Ok(None); // Not the same, bail out.
        }
        common_value = Some(incoming);
    }

    // If `common_value` is `None` then all of the incoming values were either
    // undef, poison or equal to the phi node itself.
    let Some(common_value) = common_value else {
        let ty = self_value.ty();
        return Ok(Some(if has_undef_input {
            ty.undef().as_constant().as_erased()
        } else {
            ty.poison().as_constant().as_erased()
        }));
    };

    if has_poison_input || has_undef_input {
        // If we have a PHI node like phi(X, undef, X), where X is defined by
        // some instruction, we cannot return X as the result of the PHI node
        // unless it dominates the PHI block.
        if !value_dominates_phi(common_value, view, dominators) {
            return Ok(None);
        }
        // Make sure we do not replace an undef value with poison.
        if has_undef_input {
            let query = ValueTrackingQuery::new(data_layout)
                .with_dominator_tree(dominators)
                .with_context_instruction(view);
            if !is_known_not_poison(common_value, &query)? {
                return Ok(None);
            }
        }
        return Ok(Some(common_value));
    }

    Ok(Some(common_value))
}

/// Ports `valueDominatesPHI` (`InstructionSimplify.cpp`).
///
/// Upstream's null-`DominatorTree` fallback (entry block, not `invoke` and not
/// `callbr`) has no counterpart here: `InstSimplifyPass` names
/// `DominatorTreeAnalysis` in its `Requires`, so the tree is never absent.
fn value_dominates_phi<'ctx, B: ModuleBrand + 'ctx>(
    value: Value<'ctx, B>,
    phi: &InstructionView<'ctx, B>,
    dominators: &DominatorTree,
) -> bool {
    let Ok(instruction) = InstructionView::try_from(value) else {
        // Arguments and constants dominate all instructions.
        return true;
    };
    dominators.dominates_instruction(&instruction, phi)
}
