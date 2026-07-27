//! Phase-A2 coverage: builder positioning (SetInsertPoint(Instruction*) /
//! SetInsertPointPastAllocas / save/restore), integer unary ops
//! (`build_int_neg`, `build_int_neg_nsw`, `build_int_not`), and the
//! pointer-cast / is-null / is-not-null convenience methods.
//!
//! Each `#[test]` cites its upstream source (Doctrine D11). Tests whose
//! upstream `TEST_F` lacks direct coverage of the wrapper are marked
//! `llvmkit-specific` and cite the closest upstream usage site (typically
//! `lib/Frontend/OpenMP/OMPIRBuilder.cpp` or a transform pass).

use llvmkit_ir::{
    Dyn, IRBuilder, IntValue, IrError, Linkage, Module, PointerValue, SubFlags, module_new,
};

// --- Positioning ------------------------------------------------------

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, DebugLoc)`
/// (lines 1155-1190). That test exercises `Builder.SetInsertPoint(Br)` and
/// `Builder.SetInsertPoint(Call1->getParent(), Call1->getIterator())` --
/// the same upstream construct our `position_before` ports.
#[test]
fn position_before_inserts_between_prev_and_anchor() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let a = b.build_int_add(n, 1_i32, "a")?;
    let (sealed_block, ret_inst) = b.build_ret(a)?;
    let _ = sealed_block;
    let b2 = IRBuilder::new_for::<i32>(&m).position_before(&ret_inst.as_view());
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
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type<'_, _>>::new(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let slot = b.build_alloca(i32_ty, "slot")?;
    let zero = i32_ty.const_int(0_i32);
    b.build_store(zero, slot)?;
    b.build_ret_void()?;
    let b2 = IRBuilder::new_for::<Dyn>(&m).position_past_allocas(m.view(f));
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
fn save_and_restore_insert_point_before_terminator() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let saved = b.save_insert_point();
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let a = b.build_int_add(n, 1_i32, "a")?;
    let b2 = IRBuilder::new_for::<Dyn>(&m).restore_insert_point(saved)?;
    let extra = b2.build_int_add(n, 2_i32, "extra")?;
    b2.build_ret(extra)?;
    let _ = a;
    Ok(())
}

/// Rust-side T2 regression for LLVM's `Verifier::visitBasicBlock`
/// terminator invariant: a saved end-of-block insert point must not reopen
/// a block after `IRBuilder::build_ret` sealed it.
#[test]
fn restore_insert_point_rejects_terminated_block() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let saved = b.save_insert_point();
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let a = b.build_int_add(n, 1_i32, "a")?;
    let _ = b.build_ret(a)?;
    let err = match IRBuilder::new_for::<Dyn>(&m).restore_insert_point(saved) {
        Ok(_) => panic!("terminated block cannot be reopened from a saved insert point"),
        Err(err) => err,
    };
    assert!(matches!(err, IrError::InvalidOperation { .. }));
    Ok(())
}

/// llvmkit-specific (0.0.4 id currency). Closest upstream construct is
/// `Builder.SetInsertPoint(BB)` in `IRBuilder.h`, which takes a raw
/// `BasicBlock*` recovered from a walk. Our linear
/// [`IRBuilder::position_at_end`] consumes an `Unterminated` block token, so a
/// pass that only kept the block's [`llvmkit_ir::BlockId`] reaches the same
/// insertion point through the checked `_dyn` form.
#[test]
fn position_at_end_dyn_reopens_an_unterminated_block_from_its_id() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let entry_id = entry.id();
    // The linear token is consumed here; only the id survives.
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let a = b.build_int_add(n, 1_i32, "a")?;
    // Give the block's linear token back to the builder and let it go:
    // from here only `entry_id` names the block.
    let _consumed = b.into_insert_block();

    let b2 = IRBuilder::new_for::<Dyn>(&m).position_at_end_dyn(entry_id)?;
    b2.build_ret(a)?;
    let text = format!("{m}");
    assert!(
        text.contains("%a = add i32 %0, 1\n  ret i32 %a\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Rust-side T2 regression mirroring
/// [`restore_insert_point_rejects_terminated_block`]: the `_dyn` reposition
/// carries no termination marker, so it enforces LLVM's
/// `Verifier::visitBasicBlock` terminator invariant at run time instead.
#[test]
fn position_at_end_dyn_rejects_a_terminated_block() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let entry_id = entry.id();
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let a = b.build_int_add(n, 1_i32, "a")?;
    let _ = b.build_ret(a)?;

    let err = match IRBuilder::new_for::<Dyn>(&m).position_at_end_dyn(entry_id) {
        Ok(_) => panic!("a terminated block must not be reopened from its id"),
        Err(err) => err,
    };
    assert!(matches!(err, IrError::InvalidOperation { .. }));
    Ok(())
}

/// The other `position_at_end_dyn` rejection: a [`llvmkit_ir::BlockId`] minted
/// in a different module is `IrError::ForeignValueId`, checked before the arena
/// is touched.
///
/// Two [`module_new!`](llvmkit_ir::module_new) modules cannot express this —
/// their distinct generated brand types make the cross-module call a compile
/// error, so the runtime check is unreachable. Two [`llvmkit_ir::DynBrand`] modules share one brand
/// type, which is precisely why the tag has to hold the line here.
#[test]
fn position_at_end_dyn_rejects_a_block_from_another_module() -> Result<(), IrError> {
    let a = Module::dynamic("block-a");
    let b = Module::dynamic("block-b");

    let a_i32 = a.i32_type();
    let a_fn_ty = a.fn_type(a_i32, [a_i32.as_type()], false);
    let a_f = a.add_function_dyn("f", a_fn_ty, Linkage::External)?;
    let foreign_block = a.view(a_f).append_basic_block(&a, "entry").id();

    let b_i32 = b.i32_type();
    let b_fn_ty = b.fn_type(b_i32, [b_i32.as_type()], false);
    let b_f = b.add_function_dyn("f", b_fn_ty, Linkage::External)?;
    let b_entry = b.view(b_f).append_basic_block(&b, "entry").id();

    let err = match IRBuilder::new_for::<Dyn>(&b).position_at_end_dyn(foreign_block) {
        Ok(_) => panic!("a block from another module must not be an insertion point"),
        Err(err) => err,
    };
    assert!(matches!(err, IrError::ForeignValueId), "got {err:?}");

    // B's own block is accepted at the same call, so only the tag differed.
    IRBuilder::new_for::<Dyn>(&b)
        .position_at_end_dyn(b_entry)
        .expect("an owned block id must position");
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
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
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
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
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
    let m = module_new!("a")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
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
    let m = module_new!("a")?;
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(ptr_ty, [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
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
    let m = module_new!("a")?;
    let i1_ty = m.bool_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i1_ty, [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
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
    let m = module_new!("a")?;
    let i1_ty = m.bool_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i1_ty, [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let r = b.build_is_not_null(p, "ok")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%ok = icmp ne ptr %0, null\n"),
        "got:\n{text}"
    );
    Ok(())
}
