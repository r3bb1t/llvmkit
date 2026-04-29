//! Shared CFG query coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{FunctionCfg, IRBuilder, IntValue, IrError, Linkage, Module, PointerValue};

fn assert_successors<'ctx>(
    from: llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn>,
    expected: &[llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn>],
) {
    assert_eq!(from.successors(), expected);
}

fn assert_predecessors<'ctx>(
    cfg: &FunctionCfg<'ctx>,
    block: llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn>,
    expected: &[llvmkit_ir::BasicBlock<'ctx, llvmkit_ir::Dyn>],
) {
    assert_eq!(cfg.predecessors(block), expected);
}

/// Mirrors `IR/CFG.h` `successors` / `predecessors` over a `BranchInst`
/// and `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn unconditional_branch_cfg_edges() -> Result<(), IrError> {
    let m = Module::new("cfg_br");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let exit = f.append_basic_block("exit");

    IRBuilder::new_for::<()>(&m)
        .position_at_end(exit)
        .build_ret_void();
    IRBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .build_br(exit)?;

    let cfg = FunctionCfg::new(f.as_dyn());
    assert_successors(entry.as_dyn(), &[exit.as_dyn()]);
    assert_predecessors(&cfg, exit.as_dyn(), &[entry.as_dyn()]);
    assert_eq!(cfg.edges().collect::<Vec<_>>().len(), 1);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration preserving duplicate `br` edges
/// and `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn conditional_branch_preserves_duplicate_edges() -> Result<(), IrError> {
    let m = Module::new("cfg_condbr");
    let bool_ty = m.bool_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [bool_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let target = f.append_basic_block("target");

    IRBuilder::new_for::<()>(&m)
        .position_at_end(target)
        .build_ret_void();
    let cond: IntValue<bool> = f.param(0)?.try_into()?;
    IRBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .build_cond_br(cond, target, target)?;

    let cfg = FunctionCfg::new(f.as_dyn());
    assert_successors(entry.as_dyn(), &[target.as_dyn(), target.as_dyn()]);
    assert_predecessors(&cfg, target.as_dyn(), &[entry.as_dyn(), entry.as_dyn()]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `SwitchInst` and
/// `IR/Instructions.h` `SwitchInst::case_*` destination semantics.
#[test]
fn switch_cfg_edges_include_default_then_cases() -> Result<(), IrError> {
    let m = Module::new("cfg_switch");
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [i8_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let default_bb = f.append_basic_block("default");
    let case0 = f.append_basic_block("case0");
    let case1 = f.append_basic_block("case1");
    for bb in [default_bb, case0, case1] {
        IRBuilder::new_for::<()>(&m)
            .position_at_end(bb)
            .build_ret_void();
    }

    let val: IntValue<i8> = f.param(0)?.try_into()?;
    let (_sealed, switch) = IRBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .build_switch(val, default_bb, "")?;
    let _closed = switch
        .add_case(i8_ty.const_int(0_i8), case0)?
        .add_case(i8_ty.const_int(1_i8), case1)?
        .finish();

    let cfg = FunctionCfg::new(f.as_dyn());
    assert_successors(
        entry.as_dyn(),
        &[default_bb.as_dyn(), case0.as_dyn(), case1.as_dyn()],
    );
    assert_predecessors(&cfg, default_bb.as_dyn(), &[entry.as_dyn()]);
    assert_predecessors(&cfg, case0.as_dyn(), &[entry.as_dyn()]);
    assert_predecessors(&cfg, case1.as_dyn(), &[entry.as_dyn()]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `IndirectBrInst` and
/// `IR/Instructions.h` `IndirectBrInst::destinations` semantics.
#[test]
fn indirectbr_cfg_edges_are_listed_destinations() -> Result<(), IrError> {
    let m = Module::new("cfg_indirectbr");
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let bb1 = f.append_basic_block("bb1");
    let bb2 = f.append_basic_block("bb2");
    for bb in [bb1, bb2] {
        IRBuilder::new_for::<()>(&m)
            .position_at_end(bb)
            .build_ret_void();
    }

    let addr: PointerValue = f.param(0)?.try_into()?;
    let (_sealed, ibr) = IRBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .build_indirectbr(addr, "")?;
    let _closed = ibr.add_destination(bb1)?.add_destination(bb2)?.finish();

    let cfg = FunctionCfg::new(f.as_dyn());
    assert_successors(entry.as_dyn(), &[bb1.as_dyn(), bb2.as_dyn()]);
    assert_predecessors(&cfg, bb1.as_dyn(), &[entry.as_dyn()]);
    assert_predecessors(&cfg, bb2.as_dyn(), &[entry.as_dyn()]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `InvokeInst` and
/// `llvm/lib/IR/Verifier.cpp` unwind-destination validation.
#[test]
fn invoke_cfg_edges_are_normal_then_unwind() -> Result<(), IrError> {
    let m = Module::new("cfg_invoke");
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let callee = m.add_function::<()>("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let caller = m.add_function::<()>("caller", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let normal = caller.append_basic_block("normal");
    let unwind = caller.append_basic_block("unwind");
    for bb in [normal, unwind] {
        IRBuilder::new_for::<()>(&m)
            .position_at_end(bb)
            .build_ret_void();
    }

    IRBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .build_invoke(callee, Vec::<llvmkit_ir::Value>::new(), normal, unwind, "")?;

    let cfg = FunctionCfg::new(caller.as_dyn());
    assert_successors(entry.as_dyn(), &[normal.as_dyn(), unwind.as_dyn()]);
    assert_predecessors(&cfg, normal.as_dyn(), &[entry.as_dyn()]);
    assert_predecessors(&cfg, unwind.as_dyn(), &[entry.as_dyn()]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `CallBrInst` and
/// `test/Assembler/callbr.ll` fallthrough-plus-indirect destination order.
#[test]
fn callbr_cfg_edges_are_default_then_indirect_dests() -> Result<(), IrError> {
    let m = Module::new("cfg_callbr");
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let callee = m.add_function::<()>("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let caller = m.add_function::<()>("caller", caller_ty, Linkage::External)?;
    let entry = caller.append_basic_block("entry");
    let dflt = caller.append_basic_block("default");
    let indirect = caller.append_basic_block("indirect");
    for bb in [dflt, indirect] {
        IRBuilder::new_for::<()>(&m)
            .position_at_end(bb)
            .build_ret_void();
    }

    IRBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .build_callbr(
            callee,
            Vec::<llvmkit_ir::Value>::new(),
            dflt,
            &[indirect],
            "",
        )?;

    let cfg = FunctionCfg::new(caller.as_dyn());
    assert_successors(entry.as_dyn(), &[dflt.as_dyn(), indirect.as_dyn()]);
    assert_predecessors(&cfg, dflt.as_dyn(), &[entry.as_dyn()]);
    assert_predecessors(&cfg, indirect.as_dyn(), &[entry.as_dyn()]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `CatchReturnInst` and
/// `IR/Instructions.h` `CatchReturnInst::getSuccessor` semantics.
#[test]
fn catchret_cfg_edge_is_target_block() -> Result<(), IrError> {
    let m = Module::new("cfg_catchret");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let cs_block = f.append_basic_block("cs");
    let cp_block = f.append_basic_block("cp");
    let ret_block = f.append_basic_block("ret");
    IRBuilder::new_for::<()>(&m)
        .position_at_end(ret_block)
        .build_ret_void();

    let (_sealed, cs) = IRBuilder::new_for::<()>(&m)
        .position_at_end(cs_block)
        .build_catch_switch::<llvmkit_ir::Unsealed>(None, None, "cs")?;
    let cs_closed = cs.add_handler(cp_block)?.finish();
    let cp = IRBuilder::new_for::<()>(&m)
        .position_at_end(cp_block)
        .build_catch_pad(
            cs_closed.as_instruction().as_value(),
            Vec::<llvmkit_ir::Value>::new(),
            "cp",
        )?;
    IRBuilder::new_for::<()>(&m)
        .position_at_end(cp_block)
        .build_catch_ret(cp.as_instruction().as_value(), ret_block, "")?;

    let cfg = FunctionCfg::new(f.as_dyn());
    assert_successors(cs_block.as_dyn(), &[cp_block.as_dyn()]);
    assert_successors(cp_block.as_dyn(), &[ret_block.as_dyn()]);
    assert_predecessors(&cfg, ret_block.as_dyn(), &[cp_block.as_dyn()]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `CleanupReturnInst` and
/// `llvm/lib/IR/Verifier.cpp` cleanupret unwind-destination validation.
#[test]
fn cleanupret_cfg_edge_is_optional_unwind_dest() -> Result<(), IrError> {
    let m = Module::new("cfg_cleanupret");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let unwind = f.append_basic_block("unwind");
    IRBuilder::new_for::<()>(&m)
        .position_at_end(unwind)
        .build_ret_void();

    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let cp = b.build_cleanup_pad(None, Vec::<llvmkit_ir::Value>::new(), "cp")?;
    b.build_cleanup_ret(cp.as_instruction().as_value(), Some(unwind), "")?;

    let cfg = FunctionCfg::new(f.as_dyn());
    assert_successors(entry.as_dyn(), &[unwind.as_dyn()]);
    assert_predecessors(&cfg, unwind.as_dyn(), &[entry.as_dyn()]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `CatchSwitchInst` and
/// `IR/Instructions.h` handler-plus-unwind destination semantics.
#[test]
fn catchswitch_cfg_edges_are_handlers_then_unwind_dest() -> Result<(), IrError> {
    let m = Module::new("cfg_catchswitch");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let handler0 = f.append_basic_block("handler0");
    let handler1 = f.append_basic_block("handler1");
    let unwind = f.append_basic_block("unwind");
    for bb in [handler0, handler1, unwind] {
        IRBuilder::new_for::<()>(&m)
            .position_at_end(bb)
            .build_ret_void();
    }

    let (_sealed, cs) = IRBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .build_catch_switch(None, Some(unwind), "cs")?;
    let _closed = cs.add_handler(handler0)?.add_handler(handler1)?.finish();

    let cfg = FunctionCfg::new(f.as_dyn());
    assert_successors(
        entry.as_dyn(),
        &[handler0.as_dyn(), handler1.as_dyn(), unwind.as_dyn()],
    );
    assert_predecessors(&cfg, handler0.as_dyn(), &[entry.as_dyn()]);
    assert_predecessors(&cfg, handler1.as_dyn(), &[entry.as_dyn()]);
    assert_predecessors(&cfg, unwind.as_dyn(), &[entry.as_dyn()]);
    Ok(())
}
