//! `getelementptr` print form: array offset, inbounds, struct GEP,
//! and zero-index degenerate GEP.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` ports a case from
//! `unittests/IR/InstructionsTest.cpp` (`GEPIndices`, `ZeroIndexGEP`)
//! or mirrors a `test/Assembler/getelementptr*.ll` fixture.

use llvmkit_ir::{Dyn, GepNoWrapFlags, IrBuilder, IrError, Linkage, module_new};

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, GEPIndices)`
/// for the array-offset GEP case. Textual form mirrors
/// `test/Assembler/getelementptr.ll`.
#[test]
fn gep_array_offset() -> Result<(), IrError> {
    let m = module_new!("g")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.function_type(ptr_ty.as_type(), [ptr_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let n: llvmkit_ir::IntValue<'_, llvmkit_ir::IntDyn, _> = m.view(f).param(1)?.try_into()?;
    let r = b.gep(i32_ty, p, [n], "p2")?;
    b.ret(r)?;
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
    let m = module_new!("g")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.function_type(ptr_ty.as_type(), [ptr_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("gi", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let n: llvmkit_ir::IntValue<'_, llvmkit_ir::IntDyn, _> = m.view(f).param(1)?.try_into()?;
    let r = b.inbounds_gep(i32_ty, p, [n], "p2")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%p2 = getelementptr inbounds i32, ptr %0, i32 %1"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/getelementptr.ll`'s positive struct-GEP print
/// form (e.g. `%B = getelementptr {i32, i32}, ptr %t, i92 %n, i32 0`) for
/// the `getelementptr inbounds nuw %S, ptr %x, i32 0, i32 N` struct-field
/// access print form -- `getelementptr_struct.ll` is a NEGATIVE fixture
/// (`RUN: not llvm-as`, invalid indices) and is not an accurate print-form
/// anchor. The `nuw` flag matches `IRBuilder::CreateStructGEP`
/// (`IRBuilder.h`), which passes `GEPNoWrapFlags::inBounds() |
/// GEPNoWrapFlags::noUnsignedWrap()`; the combined printed form is locked
/// against `test/Assembler/flags.ll` (`gep_inbounds_nuw`, `inbounds nuw`
/// prints in that order per `GEPNoWrapFlags`'s canonical ordering).
#[test]
fn struct_gep() -> Result<(), IrError> {
    let m = module_new!("g")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let s_ty = m.get_or_insert_named_struct("S");
    m.set_struct_body_dyn(s_ty, [i32_ty.as_type(), i64_ty.as_type()], false)?;
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.function_type(ptr_ty.as_type(), [ptr_ty.as_type()]);
    let f = m.add_function_dyn("sg", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let r = b.struct_gep(s_ty, p, 1, "p2")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%p2 = getelementptr inbounds nuw %S, ptr %0, i32 0, i32 1"),
        "got:\n{text}"
    );
    Ok(())
}

/// Port of `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, ZeroIndexGEP)`.
/// Textual form mirrors `test/Assembler/2009-07-24-ZeroArgGEP.ll`.
#[test]
fn gep_zero_index() -> Result<(), IrError> {
    let m = module_new!("g")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.function_type(ptr_ty.as_type(), [ptr_ty.as_type()]);
    let f = m.add_function_dyn("gz", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    // Zero-index degenerate GEP: just `getelementptr i32, ptr %0` (no
    // indices). Mirrors `2009-07-24-ZeroArgGEP.ll`.
    let no_indices: [llvmkit_ir::ConstantIntValue<'_, llvmkit_ir::IntDyn, _>; 0] = [];
    let r = b.gep(i32_ty, p, no_indices, "p2")?;
    b.ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%p2 = getelementptr i32, ptr %0"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `GetElementPtrInst::getGEPReturnType` (`IR/Instructions.h`), all
/// three branches, through the public erased builder. The two vector shapes
/// are `test/Assembler/opaque-ptr.ll`'s `@gep_vec1` (scalar base, vector index
/// -- the index's `ElementCount` applied to the pointer type) and `@gep_vec2`
/// (vector base -- the base type unchanged, scalar index left as written);
/// the scalar branch is `test/Assembler/getelementptr.ll`'s shape, kept here
/// so all three arms are exercised through one door.
#[test]
fn gep_erased_ports_gep_return_type() -> Result<(), IrError> {
    let m = module_new!("gv")?;
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let vec_ptr_ty = m.vector_type(ptr_ty.as_type(), 2);
    let vec_i32_ty = m.vector_type(i32_ty.as_type(), 2);
    let void_ty = m.void_type();
    let fn_ty = m.function_type(
        void_ty.as_type(),
        [ptr_ty.as_type(), vec_ptr_ty.as_type(), vec_i32_ty.as_type()],
    );
    let f = m.add_function_dyn("gv", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);

    let scalar_base = m.view(f).param(0)?;
    let vector_base = m.view(f).param(1)?;
    let vector_index = m.view(f).param(2)?;
    let two = i32_ty.const_int(2_i32).as_erased();

    // Scalar GEP: `return Ty`.
    let scalar = b.gep_erased(i8_ty, scalar_base, [two], GepNoWrapFlags::empty(), "s")?;
    // Vector index on a scalar base: `return VectorType::get(Ty, EltCount)`.
    let by_index = b.gep_erased(
        i8_ty,
        scalar_base,
        [vector_index],
        GepNoWrapFlags::empty(),
        "i",
    )?;
    // Vector base: `if (Ty->isVectorTy()) return Ty`.
    let by_base = b.gep_erased(i8_ty, vector_base, [two], GepNoWrapFlags::empty(), "v")?;
    b.ret_void()?;

    assert_eq!(m.view(scalar).ty(), ptr_ty.as_type());
    assert_eq!(m.view(by_index).ty(), vec_ptr_ty.as_type());
    assert_eq!(m.view(by_base).ty(), vec_ptr_ty.as_type());

    let text = format!("{m}");
    assert!(
        text.contains("%s = getelementptr i8, ptr %0, i32 2\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%i = getelementptr i8, ptr %0, <2 x i32> %2\n"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%v = getelementptr i8, <2 x ptr> %1, i32 2\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// `GetElementPtrInst`'s constructor initialises
/// `ResultElementType(getIndexedType(PointeeType, IdxList))`, which is null
/// for an index sequence that does not walk into the source type; llvmkit has
/// no null type to store, so the erased builder rejects instead. Same shape as
/// `test/Assembler/getelementptr_vec_struct.ll`, whose vector struct index has
/// disagreeing lanes and so fails `StructType::indexValid`'s splat read.
#[test]
fn gep_erased_rejects_invalid_indices() -> Result<(), IrError> {
    let m = module_new!("gv")?;
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let vec_ptr_ty = m.vector_type(ptr_ty.as_type(), 2);
    let vec_i32_ty = m.vector_type(i32_ty.as_type(), 2);
    let void_ty = m.void_type();
    let s_ty = m.struct_type([i32_ty.as_type(), i32_ty.as_type()]);
    let fn_ty = m.function_type(void_ty.as_type(), [vec_ptr_ty.as_type()]);
    let f = m.add_function_dyn("gvbad", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);

    let vector_base = m.view(f).param(0)?;
    let five_nine = vec_i32_ty
        .const_vector([i32_ty.const_int(5_i32), i32_ty.const_int(9_i32)])?
        .as_erased();
    let zero_one = vec_i32_ty
        .const_vector([i32_ty.const_int(0_i32), i32_ty.const_int(1_i32)])?
        .as_erased();

    let built = b.gep_erased(
        s_ty,
        vector_base,
        [five_nine, zero_one],
        GepNoWrapFlags::empty(),
        "w",
    );
    assert!(
        matches!(built, Err(IrError::GepInvalidIndices)),
        "got: {built:?}"
    );
    Ok(())
}
