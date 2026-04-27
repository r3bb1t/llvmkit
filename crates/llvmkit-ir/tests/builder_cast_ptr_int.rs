//! Pointer / integer cast forms: `ptrtoint`, `inttoptr`,
//! `addrspacecast`.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` ports a cast-opcode case from
//! `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
//! and / or mirrors the assembler fixture pinning the textual form.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `PtrToIntInst` case). Textual form mirrors
/// `test/Assembler/2002-08-15-CastAmbiguity.ll`.
#[test]
fn ptrtoint_emits_canonical_form() -> Result<(), IrError> {
    let m = Module::new("c");
    let ptr_ty = m.ptr_type(0);
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i64>("p2i", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let arg: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let r = b.build_ptr_to_int(arg, i64_ty, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%y = ptrtoint ptr %0 to i64"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `IntToPtrInst` case).
#[test]
fn inttoptr_emits_canonical_form() -> Result<(), IrError> {
    let m = Module::new("c");
    let ptr_ty = m.ptr_type(0);
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(ptr_ty.as_type(), [i64_ty.as_type()], false);
    let f = m.add_function::<llvmkit_ir::Ptr>("i2p", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<llvmkit_ir::Ptr>(&m).position_at_end(entry);
    let arg: llvmkit_ir::IntValue<i64> = f.param(0)?.try_into()?;
    let r = b.build_int_to_ptr(arg, ptr_ty, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%y = inttoptr i64 %0 to ptr"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/addrspacecast-alias.ll` for the
/// `addrspacecast ptr %x to ptr addrspace(N)` print form. Closest
/// upstream functional coverage:
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (the `AddrSpaceCastInst` case).
#[test]
fn addrspacecast_emits_canonical_form() -> Result<(), IrError> {
    let m = Module::new("c");
    let ptr0 = m.ptr_type(0);
    let ptr1 = m.ptr_type(1);
    let fn_ty = m.fn_type(ptr1.as_type(), [ptr0.as_type()], false);
    let f = m.add_function::<llvmkit_ir::Ptr>("ac", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<llvmkit_ir::Ptr>(&m).position_at_end(entry);
    let arg: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let r = b.build_addrspace_cast(arg, ptr1, "y")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%y = addrspacecast ptr %0 to ptr addrspace(1)"),
        "got:\n{text}"
    );
    Ok(())
}
