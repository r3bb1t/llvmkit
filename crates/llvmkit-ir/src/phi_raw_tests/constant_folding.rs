//! Relocated raw-phi constant-folding tests: synthetic phis with no real CFG,
//! built to drive `constant_fold_instruction`'s PHI arm. Block-args cannot
//! express these (no real predecessor edges), so they stay on the raw
//! `build_int_phi`/`add_incoming` path. Ported verbatim from
//! `tests/constant_folding_analysis.rs`; dormant until wired into the crate's
//! `#[cfg(test)]` tree.

use crate::constant_folding::constant_fold_instruction;
use crate::{DataLayout, Dyn, IRBuilder, InstructionView, IntValue, IrError, Linkage, Module};

/// llvmkit-specific subset of `ConstantFolding.cpp::ConstantFoldInstOperands`:
/// a PHI whose incoming values are the same constant folds to that constant.
#[test]
fn phi_same_constant_folds() -> Result<(), IrError> {
    Module::with_new("analysis-phi", |m| {
        let dl = DataLayout::default();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let entry_label = entry.label();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(7_i32, entry_label)?
            .add_incoming(7_i32, entry_label)?;
        let instruction = InstructionView::try_from(phi.as_int_value().into_erased())?;

        let folded =
            constant_fold_instruction(&instruction, &dl, None)?.expect("same-constant phi folds");

        assert_eq!(folded, i32_ty.const_int(7_i32).as_constant());
        Ok(())
    })
}

/// Port of `ConstantFolding.cpp::ConstantFoldInstruction`'s PHI arm: undef-like
/// incomings are skipped — upstream tests `isa<UndefValue>`, and `PoisonValue`
/// is-a `UndefValue` there — so a PHI over poison and undef folds to undef.
/// Folding to poison instead would weaken a possibly-undef value to poison,
/// which is the illegal refinement direction.
#[test]
fn phi_poison_and_undef_incomings_fold_to_undef() -> Result<(), IrError> {
    Module::with_new("analysis-phi-poison-undef", |m| {
        let dl = DataLayout::default();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        let entry_label = entry.label();
        let other_label = other.label();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let poison = IntValue::try_from(i32_ty.as_type().get_poison().into_erased())?;
        let undef = IntValue::try_from(i32_ty.as_type().get_undef().into_erased())?;
        // Distinct predecessor blocks: a phi with two *different* values from
        // the *same* block is ill-formed (AmbiguousPhi); the folder arm under
        // test folds by value regardless of predecessor identity.
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(poison, entry_label)?
            .add_incoming(undef, other_label)?;
        let instruction = InstructionView::try_from(phi.as_int_value().into_erased())?;

        let folded =
            constant_fold_instruction(&instruction, &dl, None)?.expect("undef-like phi folds");

        assert_eq!(folded, i32_ty.as_type().get_undef().as_constant());
        Ok(())
    })
}

/// Same `ConstantFoldInstruction` PHI arm: a poison incoming is skipped like
/// undef, so the remaining concrete constant wins.
#[test]
fn phi_poison_beside_constant_folds_to_the_constant() -> Result<(), IrError> {
    Module::with_new("analysis-phi-poison-const", |m| {
        let dl = DataLayout::default();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        let entry_label = entry.label();
        let other_label = other.label();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let poison = IntValue::try_from(i32_ty.as_type().get_poison().into_erased())?;
        // Distinct predecessor blocks: two different values from one block is
        // ill-formed (AmbiguousPhi); the poison-skipping folder arm folds by
        // value regardless of predecessor identity.
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(poison, entry_label)?
            .add_incoming(7_i32, other_label)?;
        let instruction = InstructionView::try_from(phi.as_int_value().into_erased())?;

        let folded =
            constant_fold_instruction(&instruction, &dl, None)?.expect("poison-skipped phi folds");

        assert_eq!(folded, i32_ty.const_int(7_i32).as_constant());
        Ok(())
    })
}
