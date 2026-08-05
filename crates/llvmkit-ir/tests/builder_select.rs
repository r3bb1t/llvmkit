//! `select` instruction print form for int, fp, and pointer arms.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` mirrors the canonical `select` textual form pinned
//! by `test/Assembler/select.ll`. Closest upstream functional coverage:
//! `unittests/IR/IRBuilderTest.cpp` `Builder.CreateSelect` call sites
//! (used inside `TEST_F(IRBuilderTest, FastMathFlags)` and friends).

use llvmkit_ir::{Constant, ConstantIntValue, Dyn, IrBuilder, IrError, Linkage, module_new};

/// Mirrors `test/Assembler/select.ll` for `select i1, <int>, <int>`.
#[test]
fn select_int_arms() -> Result<(), IrError> {
    let m = module_new!("s")?;
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(
        i32_ty,
        [bool_ty.as_type(), i32_ty.as_type(), i32_ty.as_type()],
        false,
    );
    let f = m.add_function_dyn("test", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let cond: llvmkit_ir::IntValue<'_, bool, _> = m.view(f).param(0)?.try_into()?;
    let t: llvmkit_ir::IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;
    let fl: llvmkit_ir::IntValue<'_, i32, _> = m.view(f).param(2)?.try_into()?;
    let r = b.select(cond, t, fl, "v")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%v = select i1 %0, i32 %1, i32 %2"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/select.ll` for `select i1, <fp>, <fp>`.
#[test]
fn select_fp_arms() -> Result<(), IrError> {
    let m = module_new!("s")?;
    let f64_ty = m.f64_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(
        f64_ty,
        [bool_ty.as_type(), f64_ty.as_type(), f64_ty.as_type()],
        false,
    );
    let f = m.add_function_dyn("test", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let cond: llvmkit_ir::IntValue<'_, bool, _> = m.view(f).param(0)?.try_into()?;
    let t: llvmkit_ir::FloatValue<'_, f64, _> = m.view(f).param(1)?.try_into()?;
    let fl: llvmkit_ir::FloatValue<'_, f64, _> = m.view(f).param(2)?.try_into()?;
    let r = b.select(cond, t, fl, "v")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%v = select i1 %0, double %1, double %2"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/select.ll` for `select i1, ptr, ptr`.
#[test]
fn select_pointer_arms() -> Result<(), IrError> {
    let m = module_new!("s")?;
    let ptr_ty = m.ptr_type(0);
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(
        ptr_ty.as_type(),
        [bool_ty.as_type(), ptr_ty.as_type(), ptr_ty.as_type()],
        false,
    );
    let f = m.add_function_dyn("test", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let cond: llvmkit_ir::IntValue<'_, bool, _> = m.view(f).param(0)?.try_into()?;
    let t: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(1)?.try_into()?;
    let fl: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(2)?.try_into()?;
    let r = b.select(cond, t, fl, "v")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%v = select i1 %0, ptr %1, ptr %2"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific regression for
/// `ConstantFold.cpp::ConstantFoldSelectInstruction`: the default builder
/// folder must fold all-constant `select` operands to the chosen arm.
#[test]
fn default_constant_folder_folds_select_to_chosen_arm() -> Result<(), IrError> {
    let m = module_new!("select-fold")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new(), false);
    let f = m.add_function_dyn("pick", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let true_arm: llvmkit_ir::IntValue<'_, i32, _> =
        i32_ty.const_int(7_i32).as_erased().try_into()?;
    let false_arm: llvmkit_ir::IntValue<'_, i32, _> =
        i32_ty.const_int(9_i32).as_erased().try_into()?;
    let result = b.select(true, true_arm, false_arm, "v")?;
    let folded =
        ConstantIntValue::<i32, _>::try_from(Constant::try_from(b.view(result).as_erased())?)?;
    assert_eq!(folded.ap_int().try_zext_u64(), Some(7));
    Ok(())
}
