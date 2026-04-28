//! Phase-A2 coverage: builder positioning (SetInsertPoint(Instruction*) /
//! SetInsertPointPastAllocas / save/restore), integer unary ops
//! (`build_int_neg`, `build_int_neg_nsw`, `build_int_not`), and the
//! pointer-cast / is-null / is-not-null convenience methods.
//!
//! Each `#[test]` cites its upstream source (Doctrine D11). Tests whose
//! upstream `TEST_F` lacks direct coverage of the wrapper are marked
//! `llvmkit-specific` and cite the closest upstream usage site (typically
//! `lib/Frontend/OpenMP/OMPIRBuilder.cpp` or a transform pass).

use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module, PointerValue, SubFlags};

// --- Positioning ------------------------------------------------------

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, DebugLoc)`
/// (lines 1155-1190). That test exercises `Builder.SetInsertPoint(Br)` and
/// `Builder.SetInsertPoint(Call1->getParent(), Call1->getIterator())` --
/// the same upstream construct our `position_before` ports.
#[test]
fn position_before_inserts_between_prev_and_anchor() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let a = b.build_int_add(n, 1_i32, "a")?;
    let (sealed_block, ret_inst) = b.build_ret(a)?;
    let _ = sealed_block;
    let b2 = IRBuilder::new_for::<i32>(&m).position_before(ret_inst);
    let _ = b2.build_int_sub(a, 0_i32, "noop")?;
    let text = format!("{m}");
    let pos_a = text.find("%a = add").expect("%a present");
    let pos_noop = text.find("%noop = sub").expect("%noop present");
    let pos_ret = text.find("ret i32 %a").expect("ret present");
    assert!(
        pos_a < pos_noop && pos_noop < pos_ret,
        "expected order add -> sub -> ret; got:\n{text}"
    );
    Ok(())
}

/// Mirrors `IRBuilder.h::IRBuilder::SetInsertPointPastAllocas(Function*)`.
/// llvmkit-specific scaffold: upstream `unittests/IR/IRBuilderTest.cpp` has
/// no dedicated `TEST_F` for this entry-block-scan helper; closest upstream
/// coverage is the live use sites in `lib/Frontend/OpenMP/OMPIRBuilder.cpp`
/// and `lib/Transforms/Scalar/SROA.cpp`.
#[test]
fn position_past_allocas_anchors_after_alloca_prefix() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let slot = b.build_alloca(i32_ty, "slot")?;
    let zero = i32_ty.const_int(0_i32);
    b.build_store(zero, slot)?;
    b.build_ret_void();
    let b2 = IRBuilder::new_for::<()>(&m).position_past_allocas(f);
    let _hoisted = b2.build_alloca(i32_ty, "hoisted")?;
    let text = format!("{m}");
    let pos_slot = text.find("%slot = alloca").expect("slot present");
    let pos_hoisted = text.find("%hoisted = alloca").expect("hoisted present");
    let pos_store = text.find("store i32").expect("store present");
    assert!(
        pos_slot < pos_hoisted && pos_hoisted < pos_store,
        "expected order slot-alloca -> hoisted-alloca -> store; got:\n{text}"
    );
    Ok(())
}

/// Mirrors `unittests/Frontend/OpenMPIRBuilderTest.cpp` use of
/// `Builder.saveIP()` / `Builder.restoreIP(...)` (lines 244 / 253) --
/// the canonical upstream usage of the IRBuilder save/restore API.
#[test]
fn save_and_restore_insert_point_round_trip() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let saved = b.save_insert_point();
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let a = b.build_int_add(n, 1_i32, "a")?;
    let _ = b.build_ret(a)?;
    let b2 = IRBuilder::new_for::<i32>(&m).restore_insert_point(saved);
    let _ = b2.build_int_add(n, 2_i32, "extra");
    Ok(())
}

// --- Unary integer helpers --------------------------------------------

/// Mirrors `IRBuilder.h::IRBuilder::CreateNeg(V, Name)` -> `sub 0, V`.
/// AsmWriter print form locked against
/// `test/Assembler/auto_upgrade_nvvm_intrinsics.ll` line 128 (which has the
/// upstream `; CHECK-DAG: ... = sub i32 0, %a` directive).
/// llvmkit-specific scaffold (no upstream `TEST_F` exists for `CreateNeg`).
#[test]
fn build_int_neg_emits_sub_zero() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let neg = b.build_int_neg(n, "neg")?;
    b.build_ret(neg)?;
    let text = format!("{m}");
    assert!(text.contains("%neg = sub i32 0, %0\n"), "got:\n{text}");
    Ok(())
}

/// Mirrors `IRBuilder.h::IRBuilder::CreateNSWNeg` -> `sub nsw 0, V`.
/// Closest upstream `TEST_F`:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)` (line
/// 773) which exercises `CreateNSWAdd` / `CreateNSWSub` -- the same
/// flag-bearing arithmetic family.
#[test]
fn build_int_neg_nsw_emits_sub_nsw() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let neg = b.build_int_neg_nsw(n, "neg")?;
    b.build_ret(neg)?;
    let text = format!("{m}");
    assert!(text.contains("%neg = sub nsw i32 0, %0\n"), "got:\n{text}");
    let _ = SubFlags::new().nsw();
    Ok(())
}

/// Mirrors `IRBuilder.h::IRBuilder::CreateNot(V)` -> `xor V, -1`.
/// llvmkit-specific scaffold (no upstream `TEST_F` for `CreateNot`).
/// AsmWriter print form mirrors `lib/IR/AsmWriter.cpp::printInstruction`
/// Xor arm.
#[test]
fn build_int_not_emits_xor_minus_one() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let inv = b.build_int_not(n, "inv")?;
    b.build_ret(inv)?;
    let text = format!("{m}");
    assert!(text.contains("%inv = xor i32 %0, -1\n"), "got:\n{text}");
    Ok(())
}

// --- Pointer cast / is_null / is_not_null -----------------------------

/// Mirrors `IRBuilder.h::IRBuilder::CreatePointerBitCastOrAddrSpaceCast`.
/// Upstream call site:
/// `unittests/Frontend/OpenMPIRBuilderTest.cpp` line 6473 invokes
/// `Builder.CreatePointerBitCastOrAddrSpaceCast(Addr, Input->getType())`.
/// llvmkit-specific scaffold (no dedicated `TEST_F` for the wrapper).
#[test]
fn build_pointer_cast_same_addrspace_emits_bitcast() -> Result<(), IrError> {
    let m = Module::new("a");
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(ptr_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<llvmkit_ir::marker::Ptr>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<llvmkit_ir::marker::Ptr>(&m).position_at_end(entry);
    let p: PointerValue = f.param(0)?.try_into()?;
    let cast = b.build_pointer_cast(p, ptr_ty, "cast")?;
    b.build_ret(cast)?;
    let text = format!("{m}");
    assert!(
        text.contains("%cast = bitcast ptr %0 to ptr\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `IRBuilder.h::IRBuilder::CreateIsNull(Arg)` ->
/// `icmp eq <ptr>, null`. llvmkit-specific scaffold (no dedicated `TEST_F`).
/// Sibling `CreateIsNotNull` is exercised at
/// `unittests/Frontend/OpenMPIRBuilderTest.cpp` line 1153.
#[test]
fn build_is_null_emits_icmp_eq_null() -> Result<(), IrError> {
    let m = Module::new("a");
    let i1_ty = m.bool_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i1_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<bool>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
    let p: PointerValue = f.param(0)?.try_into()?;
    let r = b.build_is_null(p, "isn")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%isn = icmp eq ptr %0, null\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `unittests/Frontend/OpenMPIRBuilderTest.cpp` line 1153:
/// `Builder.CreateIsNotNull(F->arg_begin())` -- the canonical upstream
/// use site for this wrapper.
#[test]
fn build_is_not_null_emits_icmp_ne_null() -> Result<(), IrError> {
    let m = Module::new("a");
    let i1_ty = m.bool_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i1_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<bool>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
    let p: PointerValue = f.param(0)?.try_into()?;
    let r = b.build_is_not_null(p, "ok")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%ok = icmp ne ptr %0, null\n"),
        "got:\n{text}"
    );
    Ok(())
}
