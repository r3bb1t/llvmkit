//! `getelementptr` print form: array offset, inbounds, struct GEP,
//! and zero-index degenerate GEP.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` ports a case from
//! `unittests/IR/InstructionsTest.cpp` (`GEPIndices`, `ZeroIndexGEP`)
//! or mirrors a `test/Assembler/getelementptr*.ll` fixture.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module, Ptr};

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, GEPIndices)`
/// for the array-offset GEP case. Textual form mirrors
/// `test/Assembler/getelementptr.ll`.
#[test]
fn gep_array_offset() -> Result<(), IrError> {
    let m = Module::new("g");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(
        ptr_ty.as_type(),
        [ptr_ty.as_type(), i32_ty.as_type()],
        false,
    );
    let f = m.add_function::<Ptr>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<Ptr>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let n: llvmkit_ir::IntValue<llvmkit_ir::IntDyn> = f.param(1)?.try_into()?;
    let r = b.build_gep(i32_ty, p, [n], "p2")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%p2 = getelementptr i32, ptr %0, i32 %1"),
        "got:\n{text}"
    );
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, GEPIndices)`
/// for the `inbounds` variant. Textual form mirrors
/// `test/Assembler/getelementptr.ll`.
#[test]
fn gep_inbounds() -> Result<(), IrError> {
    let m = Module::new("g");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(
        ptr_ty.as_type(),
        [ptr_ty.as_type(), i32_ty.as_type()],
        false,
    );
    let f = m.add_function::<Ptr>("gi", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<Ptr>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let n: llvmkit_ir::IntValue<llvmkit_ir::IntDyn> = f.param(1)?.try_into()?;
    let r = b.build_inbounds_gep(i32_ty, p, [n], "p2")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%p2 = getelementptr inbounds i32, ptr %0, i32 %1"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/getelementptr_struct.ll` for the
/// `getelementptr inbounds %S, ptr %x, i32 0, i32 N` struct-field
/// access print form.
#[test]
fn struct_gep() -> Result<(), IrError> {
    let m = Module::new("g");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let s_ty = m.named_struct("S");
    m.set_struct_body(s_ty, [i32_ty.as_type(), i64_ty.as_type()], false)?;
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(ptr_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<Ptr>("sg", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<Ptr>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let r = b.build_struct_gep(s_ty, p, 1, "p2")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%p2 = getelementptr inbounds %S, ptr %0, i32 0, i32 1"),
        "got:\n{text}"
    );
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, ZeroIndexGEP)`.
/// Textual form mirrors `test/Assembler/2009-07-24-ZeroArgGEP.ll`.
#[test]
fn gep_zero_index() -> Result<(), IrError> {
    let m = Module::new("g");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(ptr_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<Ptr>("gz", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<Ptr>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    // Zero-index degenerate GEP: just `getelementptr i32, ptr %0` (no
    // indices). Mirrors `2009-07-24-ZeroArgGEP.ll`.
    let no_indices: [llvmkit_ir::ConstantIntValue<llvmkit_ir::IntDyn>; 0] = [];
    let r = b.build_gep(i32_ty, p, no_indices, "p2")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%p2 = getelementptr i32, ptr %0"),
        "got:\n{text}"
    );
    Ok(())
}
