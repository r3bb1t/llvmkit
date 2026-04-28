//! EH-call coverage: `invoke`, `callbr`.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

// --------------------------------------------------------------------------
// invoke
// --------------------------------------------------------------------------

/// Ports `test/Bitcode/compatibility.ll` line 1325:
/// `invoke fastcc void @f.fastcc() to label %defaultdest unwind label %exc`.
/// Locks the default-cc / void-return print form and the
/// `\n          to label %... unwind label %...` suffix that
/// `printInstruction` emits in `lib/IR/AsmWriter.cpp`.
#[test]
fn invoke_void_to_unwind() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let callee = m.add_function::<()>("f.callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let caller = m.add_function::<()>("instructions.terminators", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let normal = caller.append_basic_block("defaultdest");
    let unwind = caller.append_basic_block("exc");
    {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(normal);
        bb_b.build_ret_void();
    }
    {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(unwind);
        bb_b.build_ret_void();
    }
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let _ = b.build_invoke(
        callee,
        Vec::<llvmkit_ir::value::Value>::new(),
        normal,
        unwind,
        "",
    )?;
    let text = format!("{m}");
    assert!(text.contains("invoke void @f.callee()"), "got:\n{text}");
    assert!(
        text.contains("to label %defaultdest unwind label %exc"),
        "got:\n{text}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// callbr
// --------------------------------------------------------------------------

/// Ports `test/Assembler/callbr.ll` (the `; CHECK-NEXT: callbr void
/// @llvm.amdgcn.kill(i1 [[C]])` fixture, lines 8-9). Locks the print
/// form `callbr void @callee(<args>)\n          to label %fallthrough
/// [label %indirect]`.
#[test]
fn callbr_void_with_one_indirect_dest() -> Result<(), IrError> {
    let m = Module::new("a");
    let bool_ty = m.bool_type();
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(void_ty.as_type(), [bool_ty.as_type()], false);
    let callee = m.add_function::<()>("kill", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(void_ty.as_type(), [bool_ty.as_type()], false);
    let caller = m.add_function::<()>("h", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let cont = caller.append_basic_block("cont");
    let kill = caller.append_basic_block("kill");
    {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(cont);
        bb_b.build_ret_void();
    }
    {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(kill);
        bb_b.build_unreachable();
    }
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let c: llvmkit_ir::IntValue<bool> = caller.param(0)?.try_into()?;
    let _ = b.build_callbr(callee, [c.as_value()], cont, &[kill], "")?;
    let text = format!("{m}");
    assert!(text.contains("callbr void @kill(i1 %0)"), "got:\n{text}");
    assert!(
        text.contains("to label %cont [label %kill]"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports `test/Assembler/inline-asm-constraint-error.ll` line 65:
/// `callbr void asm sideeffect "", "~{flags},!i"()
/// to label %1 [label %2]`. Inline-asm callees aren't built in this
/// test (InlineAsm is Session 2 territory) — we assert the
/// `[label %a, label %b]` two-destination print shape using a regular
/// function callee.
#[test]
fn callbr_two_indirect_dests_print_form() -> Result<(), IrError> {
    let m = Module::new("a");
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let callee = m.add_function::<()>("g", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let caller = m.add_function::<()>("h", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let dflt = caller.append_basic_block("dflt");
    let bb1 = caller.append_basic_block("bb1");
    let bb2 = caller.append_basic_block("bb2");
    for bb in [dflt, bb1, bb2] {
        let bb_b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
        bb_b.build_ret_void();
    }
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let _ = b.build_callbr(
        callee,
        Vec::<llvmkit_ir::value::Value>::new(),
        dflt,
        &[bb1, bb2],
        "",
    )?;
    let text = format!("{m}");
    assert!(
        text.contains("to label %dflt [label %bb1, label %bb2]"),
        "got:\n{text}"
    );
    Ok(())
}
