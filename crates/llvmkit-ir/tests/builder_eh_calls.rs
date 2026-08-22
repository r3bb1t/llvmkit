//! EH-call coverage: `invoke`, `callbr`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    CallSiteConfig, CallingConv, Dyn, InlineAsmOptions, IntValue, IrBuilder, IrError, Linkage,
    module_new,
};

// --------------------------------------------------------------------------
// invoke
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1325:
/// `invoke fastcc void @f.fastcc() to label %defaultdest unwind label %exc`.
/// Locks the fastcc call-site convention and the
/// `\n          to label %... unwind label %...` suffix that
/// `printInstruction` emits in `lib/IR/AsmWriter.cpp`.
#[test]
fn invoke_void_to_unwind() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let void_ty = m.void_type();
    let callee_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let callee = m.add_function_dyn("f.fastcc", callee_ty, Linkage::External)?;
    m.view(callee).set_calling_conv(&m, CallingConv::FAST);
    let caller_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let caller = m.add_function_dyn("instructions.terminators", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let normal = m.view(caller).append_basic_block(&m, "defaultdest");
    let unwind = m.view(caller).append_basic_block(&m, "exc");
    let normal_label = normal.id();
    let unwind_label = unwind.id();
    {
        let bb_b = IrBuilder::new_for::<Dyn>(&m).position_at_end(normal);
        bb_b.ret_void()?;
    }
    {
        let bb_b = IrBuilder::new_for::<Dyn>(&m).position_at_end(unwind);
        bb_b.ret_void()?;
    }
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let _ = b.invoke_dyn_with_config(
        m.view(callee),
        Vec::<llvmkit_ir::Value<'_, _>>::new(),
        normal_label,
        unwind_label,
        CallSiteConfig::new("").calling_conv(CallingConv::FAST),
    )?;
    let text = format!("{m}");
    assert!(
        text.contains(
            "invoke fastcc void @f.fastcc()\n          to label %defaultdest unwind label %exc\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

/// TYPED `invoke`: the callee's return marker is derived (not
/// caller-asserted) into the returned `InvokeInst<'_, Ret::Marker>` --
/// `invoke.to_erased()` narrows via `TryFrom` to `IntValue<i32>` without
/// error, proving the marker is really `i32` and not `Dyn`. Prints
/// identically to the dyn form for the same signature. Closest
/// upstream coverage: same `IRBuilder::CreateInvoke` shape as
/// `invoke_void_to_unwind`, exercised through the typed callee facade.
#[test]
fn typed_invoke_derives_return_marker_from_callee() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let callee = m.add_typed_function::<i32, (), _>("callee", Linkage::External)?;
    let caller = m.add_typed_function::<i32, (i32,), _>("caller", Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let normal = m.view(caller).append_basic_block(&m, "normal");
    let unwind = m.view(caller).append_basic_block(&m, "unwind");
    let normal_label = normal.id();
    let unwind_label = unwind.id();
    let (x,) = m.view(caller).params();
    {
        let bb_b = IrBuilder::new_for::<i32>(&m).position_at_end(unwind);
        bb_b.ret(x)?;
    }
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    let (_sealed, invoke) = b.invoke(m.view(callee), (), normal_label, unwind_label, "iv")?;
    // The invoke's marker is already `i32` (derived from the callee),
    // so this infallible-in-practice narrowing never errors.
    let result: IntValue<'_, i32, _> = invoke.to_erased().try_into()?;
    let bn = IrBuilder::new_for::<i32>(&m).position_at_end(normal);
    bn.ret(result)?;
    let text = format!("{m}");
    assert!(
        text.contains(
            "%iv = invoke i32 @callee()\n          to label %normal unwind label %unwind\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// callbr
// --------------------------------------------------------------------------

/// Ports `test/Assembler/callbr.ll` (the `; CHECK-NEXT: callbr void
/// @llvm.amdgcn.kill(i1 [[C]])` fixture, lines 8-13). Locks the callee,
/// successor list, and block order from the upstream fixture.
#[test]
fn callbr_void_with_one_indirect_dest() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let bool_ty = m.bool_type();
    let void_ty = m.void_type();
    let callee = m.get_or_insert_intrinsic_declaration_by_name("llvm.amdgcn.kill")?;
    let caller_ty = m.function_type(void_ty.as_type(), [bool_ty.as_type()]);
    let caller = m.add_function_dyn("test_kill", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let kill = m.view(caller).append_basic_block(&m, "kill");
    let cont = m.view(caller).append_basic_block(&m, "cont");
    let kill_label = kill.id();
    let cont_label = cont.id();
    {
        let bb_b = IrBuilder::new_for::<Dyn>(&m).position_at_end(kill);
        bb_b.unreachable();
    }
    {
        let bb_b = IrBuilder::new_for::<Dyn>(&m).position_at_end(cont);
        bb_b.ret_void()?;
    }
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let c: llvmkit_ir::IntValue<'_, bool, _> = m.view(caller).param(0)?.try_into()?;
    let _ = b.callbr(callee, [c.as_erased()], cont_label, [kill_label], "")?;
    let text = format!("{m}");
    assert!(
        text.contains(
            "callbr void @llvm.amdgcn.kill(i1 %0)\n          to label %cont [label %kill]\n"
        ),
        "got:\n{text}"
    );
    assert!(
        text.contains(
            "kill:                                             ; preds = %entry\n  unreachable\n\ncont:                                             ; preds = %entry\n  ret void\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Assembler/inline-asm-constraint-error.ll` line 65:
/// `callbr void asm sideeffect "", "~{flags},!i"()
/// to label %1 [label %2]`. The callee is the same inline-asm value and
/// the successor list matches the upstream label-constraint fixture.
#[test]
fn callbr_two_indirect_dests_print_form() -> Result<(), IrError> {
    let m = module_new!("a")?;
    let void_ty = m.void_type();
    let asm_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let asm = m.inline_asm(
        asm_ty,
        "",
        "~{flags},!i",
        InlineAsmOptions::new().side_effects(),
    );
    let caller_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let caller = m.add_function_dyn("foo", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let bb1 = m.view(caller).append_basic_block(&m, "1");
    let bb2 = m.view(caller).append_basic_block(&m, "2");
    let bb1_label = bb1.id();
    let bb2_label = bb2.id();
    for bb in [bb1, bb2] {
        let bb_b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
        bb_b.ret_void()?;
    }
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let _ = b.inline_asm_callbr::<(), _, _, _, _, _, _>(
        asm,
        Vec::<llvmkit_ir::Value<'_, _>>::new(),
        bb1_label,
        [bb2_label],
        "",
    )?;
    let text = format!("{m}");
    assert!(
        text.contains(
            "callbr void asm sideeffect \"\", \"~{flags},!i\"()\n          to label %\"1\" [label %\"2\"]\n"
        ),
        "got:\n{text}"
    );
    Ok(())
}
