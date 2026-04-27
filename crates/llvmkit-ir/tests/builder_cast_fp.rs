//! Floating-point casts: `fpext`, `fptrunc`, `fptoui`, `fptosi`,
//! `uitofp`, `sitofp`. The fp/fp width casts are pinned at compile
//! time by the `FloatWiderThan` trait (compile-error test cases
//! documented inline as comments).
//!
//! ## Upstream provenance
//!
//! Each `#[test]` ports a cast-opcode case from
//! `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`,
//! which builds a representative instance of every cast opcode and
//! checks shape / type correctness.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `FPExtInst` case).
#[test]
fn fpext_f32_to_f64() -> Result<(), IrError> {
    let m = Module::new("c");
    let f32_ty = m.f32_type();
    let f64_ty = m.f64_type();
    let fn_ty = m.fn_type(f64_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<f64>("ext", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f64>(&m).position_at_end(entry);
    let arg: llvmkit_ir::FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_fp_ext(arg, f64_ty, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%y = fpext float %0 to double"),
        "got:\n{text}"
    );
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `FPTruncInst` case).
#[test]
fn fptrunc_f64_to_f32() -> Result<(), IrError> {
    let m = Module::new("c");
    let f32_ty = m.f32_type();
    let f64_ty = m.f64_type();
    let fn_ty = m.fn_type(f32_ty, [f64_ty.as_type()], false);
    let f = m.add_function::<f32>("tr", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    let arg: llvmkit_ir::FloatValue<f64> = f.param(0)?.try_into()?;
    let r = b.build_fp_trunc(arg, f32_ty, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%y = fptrunc double %0 to float"),
        "got:\n{text}"
    );
    Ok(())
}

// `b.build_fp_ext::<f64, f32>(...)` is a compile error: `f32` is not
// `FloatWiderThan<f64>`.
// `b.build_fp_trunc::<f32, f64>(...)` is a compile error: `f32` is not
// `FloatWiderThan<f64>` (the source must be wider than the
// destination).

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `FPToSIInst` case).
#[test]
fn fptosi_f32_to_i32() -> Result<(), IrError> {
    let m = Module::new("c");
    let f32_ty = m.f32_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<i32>("toi", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let arg: llvmkit_ir::FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_fp_to_si(arg, i32_ty, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%y = fptosi float %0 to i32"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `FPToUIInst` case).
#[test]
fn fptoui_f32_to_i32() -> Result<(), IrError> {
    let m = Module::new("c");
    let f32_ty = m.f32_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [f32_ty.as_type()], false);
    let f = m.add_function::<i32>("tou", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let arg: llvmkit_ir::FloatValue<f32> = f.param(0)?.try_into()?;
    let r = b.build_fp_to_ui(arg, i32_ty, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%y = fptoui float %0 to i32"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `SIToFPInst` case).
#[test]
fn sitofp_i32_to_f32() -> Result<(), IrError> {
    let m = Module::new("c");
    let f32_ty = m.f32_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(f32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<f32>("sif", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    let arg: llvmkit_ir::IntValue<i32> = f.param(0)?.try_into()?;
    let r = b.build_si_to_fp(arg, f32_ty, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%y = sitofp i32 %0 to float"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `UIToFPInst` case).
#[test]
fn uitofp_i32_to_f32() -> Result<(), IrError> {
    let m = Module::new("c");
    let f32_ty = m.f32_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(f32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<f32>("uif", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f32>(&m).position_at_end(entry);
    let arg: llvmkit_ir::IntValue<i32> = f.param(0)?.try_into()?;
    let r = b.build_ui_to_fp(arg, f32_ty, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%y = uitofp i32 %0 to float"), "got:\n{text}");
    Ok(())
}
