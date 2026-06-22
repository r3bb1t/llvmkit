//! EH-call coverage: `invoke`, `callbr`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    CallSiteConfig, CallingConv, IRBuilder, InlineAsmOptions, IrError, Linkage, Module,
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
    Module::with_new("a", |m| {
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let callee = m.add_function::<(), _>("f.fastcc", callee_ty, Linkage::External)?;
        callee.set_calling_conv(&m, CallingConv::FAST);
        let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller =
            m.add_function::<(), _>("instructions.terminators", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let normal = caller.append_basic_block(&m, "defaultdest");
        let unwind = caller.append_basic_block(&m, "exc");
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(normal);
            bb_b.build_ret_void();
        }
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(unwind);
            bb_b.build_ret_void();
        }
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let _ = b.build_invoke_with_config(
            callee,
            Vec::<llvmkit_ir::Value>::new(),
            normal,
            unwind,
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
    })
}

// --------------------------------------------------------------------------
// callbr
// --------------------------------------------------------------------------

/// Ports `test/Assembler/callbr.ll` (the `; CHECK-NEXT: callbr void
/// @llvm.amdgcn.kill(i1 [[C]])` fixture, lines 8-13). Locks the callee,
/// successor list, and block order from the upstream fixture.
#[test]
fn callbr_void_with_one_indirect_dest() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let bool_ty = m.bool_type();
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(void_ty.as_type(), [bool_ty.as_type()], false);
        let callee = m.add_function::<(), _>("llvm.amdgcn.kill", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(void_ty.as_type(), [bool_ty.as_type()], false);
        let caller = m.add_function::<(), _>("test_kill", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let kill = caller.append_basic_block(&m, "kill");
        let cont = caller.append_basic_block(&m, "cont");
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(kill);
            bb_b.build_unreachable();
        }
        {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(cont);
            bb_b.build_ret_void();
        }
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let c: llvmkit_ir::IntValue<bool> = caller.param(0)?.try_into()?;
        let _ = b.build_callbr(callee, [c.as_value()], cont, &[kill], "")?;
        let text = format!("{m}");
        assert!(
            text.contains(
                "callbr void @llvm.amdgcn.kill(i1 %0)\n          to label %cont [label %kill]\n"
            ),
            "got:\n{text}"
        );
        assert!(
            text.contains("kill:\n  unreachable\n\ncont:\n  ret void\n"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Ports `test/Assembler/inline-asm-constraint-error.ll` line 65:
/// `callbr void asm sideeffect "", "~{flags},!i"()
/// to label %1 [label %2]`. The callee is the same inline-asm value and
/// the successor list matches the upstream label-constraint fixture.
#[test]
fn callbr_two_indirect_dests_print_form() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let void_ty = m.void_type();
        let asm_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let asm = m.inline_asm(
            asm_ty,
            "",
            "~{flags},!i",
            InlineAsmOptions::new().side_effects(true),
        );
        let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function::<(), _>("foo", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let bb1 = caller.append_basic_block(&m, "1");
        let bb2 = caller.append_basic_block(&m, "2");
        for bb in [bb1, bb2] {
            let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
            bb_b.build_ret_void();
        }
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let _ = b.build_inline_asm_callbr::<(), _, _, _, _>(
            asm,
            Vec::<llvmkit_ir::Value>::new(),
            bb1,
            &[bb2],
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
    })
}
