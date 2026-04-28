//! Phase-A4 coverage: vector splat and `ptr_add` convenience wrappers.
//! Each `#[test]` cites its upstream source (Doctrine D11). All three
//! tests are direct ports of upstream usage sites.

use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module, PointerValue};

/// Mirrors `unittests/Analysis/VectorUtilsTest.cpp::TEST_F(BasicTest, ...)`
/// (line 92): `IRB.CreateVectorSplat(5, ScalarC)`. The upstream call splats
/// an `i8` constant across 5 lanes; we exercise the same shape through the
/// typed `build_vector_splat` wrapper. The expected AsmWriter form mirrors
/// `lib/IR/IRBuilder.cpp::IRBuilderBase::CreateVectorSplat` lines 1141-1158
/// (insertelement-into-poison + zero-mask shufflevector).
#[test]
fn build_vector_splat_expands_to_insertelement_plus_shuffle() -> Result<(), IrError> {
    let m = Module::new("a");
    let i8_ty = m.i8_type();
    let v_ty = m.vector_type(i8_ty, 5, false);
    let fn_ty = m.fn_type(v_ty, [i8_ty.as_type()], false);
    let f = m.add_function::<llvmkit_ir::marker::Dyn>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);
    let scalar: IntValue<i8> = f.param(0)?.try_into()?;
    let splat = b.build_vector_splat(5, scalar, "v")?;
    b.build_ret(splat.as_value())?;
    let text = format!("{m}");
    // The two-step expansion that upstream emits, mirrored byte-for-byte:
    // %v.splatinsert = insertelement <5 x i8> poison, i8 %0, i64 0
    // %v.splat = shufflevector <5 x i8> %v.splatinsert, <5 x i8> poison, <5 x i32> zeroinitializer
    assert!(
        text.contains("%v.splatinsert = insertelement <5 x i8> poison, i8 %0, i64 0\n"),
        "splatinsert missing; got:\n{text}"
    );
    assert!(
        text.contains("%v.splat = shufflevector"),
        "splat shufflevector missing; got:\n{text}"
    );
    Ok(())
}

/// Mirrors `unittests/Analysis/MemorySSATest.cpp` line 1117-1118:
/// `B.CreatePtrAdd(Foo, B.getInt64(1), "bar")`. The expected AsmWriter form
/// is locked against `test/Assembler/opaque-ptr.ll` line 62
/// (`%res = getelementptr i8, ptr %a, i32 2`) -- the canonical upstream
/// `; CHECK:` directive for `getelementptr i8, ptr ..., <offset>`.
#[test]
fn build_ptr_add_emits_gep_i8() -> Result<(), IrError> {
    let m = Module::new("a");
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(ptr_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<llvmkit_ir::marker::Ptr>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<llvmkit_ir::marker::Ptr>(&m).position_at_end(entry);
    let p: PointerValue = f.param(0)?.try_into()?;
    let i64_ty = m.i64_type();
    let one = i64_ty.const_int(1_i64);
    let q = b.build_ptr_add::<_, _, i64>(p, one, "bar")?;
    b.build_ret(q)?;
    let text = format!("{m}");
    assert!(
        text.contains("%bar = getelementptr i8, ptr %0, i64 1\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` line 322:
/// `%gep = getelementptr inbounds i8, ptr %p, i64 %idx`. The upstream
/// `; CHECK:` directive is the canonical print form for
/// `IRBuilder::CreateInBoundsPtrAdd`.
#[test]
fn build_inbounds_ptr_add_emits_gep_inbounds_i8() -> Result<(), IrError> {
    let m = Module::new("a");
    let ptr_ty = m.ptr_type(0);
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(ptr_ty, [ptr_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<llvmkit_ir::marker::Ptr>("gep_inbounds", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<llvmkit_ir::marker::Ptr>(&m).position_at_end(entry);
    let p: PointerValue = f.param(0)?.try_into()?;
    let idx: IntValue<i64> = f.param(1)?.try_into()?;
    let q = b.build_inbounds_ptr_add::<_, _, i64>(p, idx, "gep")?;
    b.build_ret(q)?;
    let text = format!("{m}");
    assert!(
        text.contains("%gep = getelementptr inbounds i8, ptr %0, i64 %1\n"),
        "got:\n{text}"
    );
    Ok(())
}
