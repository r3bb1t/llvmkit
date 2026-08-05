//! Mirrors of upstream `.ll` fixtures that exercise specific `icmp`
//! predicates, exercised here through the new per-predicate
//! `icmp_*` convenience methods (mirror `IRBuilder::CreateICmp{EQ,
//! SLT, ...}` in `IRBuilder.h`). Each test cites the closest upstream
//! Assembler / unit fixture that emits the same IR shape.

use llvmkit_ir::{Dyn, IntValue, IrBuilder, IrError, Linkage, module_new};

/// Mirrors `test/Assembler/2007-03-18-InvalidNumberedVar.ll` (which
/// emits `icmp eq i32 %b, %a`).
#[test]
fn build_icmp_eq_emits_icmp_eq() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let bool_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(bool_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let bv: IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;
    let r = b.icmp_eq::<i32, _, _, _>(a, bv, "r")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = icmp eq i32 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Clang/CodeGen/.../auto_upgrade_nvvm_intrinsics.ll`
/// (`icmp ne i32 %z, 0`). We use the simpler shape in our smaller
/// fixture but the IR pattern is identical.
#[test]
fn build_icmp_ne_emits_icmp_ne() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let bool_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(bool_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let z: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let r = b.icmp_ne::<i32, _, _, _>(z, 0_i32, "r")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = icmp ne i32 %0, 0"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/2004-02-27-SelfUseAssertError.ll`
/// (`icmp slt i32 %inc.2, 0`).
#[test]
fn build_icmp_slt_emits_icmp_slt() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let bool_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(bool_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let r = b.icmp_slt::<i32, _, _, _>(n, 0_i32, "r")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = icmp slt i32 %0, 0"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/auto_upgrade_nvvm_intrinsics.ll`
/// (`icmp sge i32 %a, 0`).
#[test]
fn build_icmp_sge_emits_icmp_sge() -> Result<(), IrError> {
    let m = module_new!("c")?;
    let bool_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(bool_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let r = b.icmp_sge::<i32, _, _, _>(a, 0_i32, "r")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = icmp sge i32 %0, 0"), "got:\n{text}");
    Ok(())
}
