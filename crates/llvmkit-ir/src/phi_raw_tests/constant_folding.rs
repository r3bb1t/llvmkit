//! Relocated raw-phi constant-folding tests: synthetic phis with no real CFG,
//! built to drive `constant_fold_instruction`'s PHI arm. Block-args cannot
//! express these (no real predecessor edges), so they stay on the raw
//! `int_phi`/`add_incoming` path. Ported verbatim from
//! `tests/constant_folding_analysis.rs`; dormant until wired into the crate's
//! `#[cfg(test)]` tree.

use crate::constant_folding::constant_fold_instruction;
use crate::{DataLayout, Dyn, InstructionView, IntValue, IrBuilder, IrError, Linkage};

/// llvmkit-specific subset of `ConstantFolding.cpp::ConstantFoldInstOperands`:
/// a PHI whose incoming values are the same constant folds to that constant.
#[test]
fn phi_same_constant_folds() -> Result<(), IrError> {
    let m = crate::module_new!("analysis-phi")?;
    let dl = DataLayout::default();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let entry_label = entry.id();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(7_i32, entry_label)?
        .add_incoming(7_i32, entry_label)?;
    let instruction = InstructionView::try_from(phi.as_int_value().as_erased())?;

    let folded =
        constant_fold_instruction(&instruction, &dl, None)?.expect("same-constant phi folds");

    assert_eq!(folded, i32_ty.const_int(7_i32).as_constant());
    Ok(())
}

/// Port of `ConstantFolding.cpp::ConstantFoldInstruction`'s PHI arm: undef-like
/// incomings are skipped — upstream tests `isa<UndefValue>`, and `PoisonValue`
/// is-a `UndefValue` there — so a PHI over poison and undef folds to undef.
/// Folding to poison instead would weaken a possibly-undef value to poison,
/// which is the illegal refinement direction.
#[test]
fn phi_poison_and_undef_incomings_fold_to_undef() -> Result<(), IrError> {
    let m = crate::module_new!("analysis-phi-poison-undef")?;
    let dl = DataLayout::default();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let other = m.view(f).append_basic_block(&m, "other");
    let entry_label = entry.id();
    let other_label = other.id();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let poison = IntValue::try_from(i32_ty.as_type().poison().as_erased())?;
    let undef = IntValue::try_from(i32_ty.as_type().undef().as_erased())?;
    // Distinct predecessor blocks: a phi with two *different* values from
    // the *same* block is ill-formed (AmbiguousPhi); the folder arm under
    // test folds by value regardless of predecessor identity.
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(poison, entry_label)?
        .add_incoming(undef, other_label)?;
    let instruction = InstructionView::try_from(phi.as_int_value().as_erased())?;

    let folded = constant_fold_instruction(&instruction, &dl, None)?.expect("undef-like phi folds");

    assert_eq!(folded, i32_ty.as_type().undef().as_constant());
    Ok(())
}

/// Same `ConstantFoldInstruction` PHI arm: a poison incoming is skipped like
/// undef, so the remaining concrete constant wins.
#[test]
fn phi_poison_beside_constant_folds_to_the_constant() -> Result<(), IrError> {
    let m = crate::module_new!("analysis-phi-poison-const")?;
    let dl = DataLayout::default();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let other = m.view(f).append_basic_block(&m, "other");
    let entry_label = entry.id();
    let other_label = other.id();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let poison = IntValue::try_from(i32_ty.as_type().poison().as_erased())?;
    // Distinct predecessor blocks: two different values from one block is
    // ill-formed (AmbiguousPhi); the poison-skipping folder arm folds by
    // value regardless of predecessor identity.
    let phi = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(poison, entry_label)?
        .add_incoming(7_i32, other_label)?;
    let instruction = InstructionView::try_from(phi.as_int_value().as_erased())?;

    let folded =
        constant_fold_instruction(&instruction, &dl, None)?.expect("poison-skipped phi folds");

    assert_eq!(folded, i32_ty.const_int(7_i32).as_constant());
    Ok(())
}
