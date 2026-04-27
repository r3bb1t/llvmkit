//! `select` instruction print form for int, fp, and pointer arms.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` mirrors the canonical `select` textual form pinned
//! by `test/Assembler/select.ll`. Closest upstream functional coverage:
//! `unittests/IR/IRBuilderTest.cpp` `Builder.CreateSelect` call sites
//! (used inside `TEST_F(IRBuilderTest, FastMathFlags)` and friends).

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

/// Mirrors `test/Assembler/select.ll` for `select i1, <int>, <int>`.
#[test]
fn select_int_arms() -> Result<(), IrError> {
    let m = Module::new("s");
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(
        i32_ty,
        [bool_ty.as_type(), i32_ty.as_type(), i32_ty.as_type()],
        false,
    );
    let f = m.add_function::<i32>("test", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let cond: llvmkit_ir::IntValue<bool> = f.param(0)?.try_into()?;
    let t: llvmkit_ir::IntValue<i32> = f.param(1)?.try_into()?;
    let fl: llvmkit_ir::IntValue<i32> = f.param(2)?.try_into()?;
    let r = b.build_select(cond, t, fl, "v")?;
    b.build_ret(r)?;
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
    let m = Module::new("s");
    let f64_ty = m.f64_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(
        f64_ty,
        [bool_ty.as_type(), f64_ty.as_type(), f64_ty.as_type()],
        false,
    );
    let f = m.add_function::<f64>("test", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<f64>(&m).position_at_end(entry);
    let cond: llvmkit_ir::IntValue<bool> = f.param(0)?.try_into()?;
    let t: llvmkit_ir::FloatValue<f64> = f.param(1)?.try_into()?;
    let fl: llvmkit_ir::FloatValue<f64> = f.param(2)?.try_into()?;
    let r = b.build_select(cond, t, fl, "v")?;
    b.build_ret(r)?;
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
    let m = Module::new("s");
    let ptr_ty = m.ptr_type(0);
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(
        ptr_ty.as_type(),
        [bool_ty.as_type(), ptr_ty.as_type(), ptr_ty.as_type()],
        false,
    );
    let f = m.add_function::<llvmkit_ir::Ptr>("test", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<llvmkit_ir::Ptr>(&m).position_at_end(entry);
    let cond: llvmkit_ir::IntValue<bool> = f.param(0)?.try_into()?;
    let t: llvmkit_ir::PointerValue = f.param(1)?.try_into()?;
    let fl: llvmkit_ir::PointerValue = f.param(2)?.try_into()?;
    let r = b.build_select(cond, t, fl, "v")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%v = select i1 %0, ptr %1, ptr %2"),
        "got:\n{text}"
    );
    Ok(())
}
