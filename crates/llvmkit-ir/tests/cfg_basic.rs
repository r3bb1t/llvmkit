//! Shared CFG query coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    BasicBlockLabel, FunctionCfg, IRBuilder, IntValue, IrError, Linkage, Module, PointerValue,
    ReturnMarker, Value,
};

fn assert_successors<'ctx, R>(
    cfg: &FunctionCfg<'ctx>,
    from: BasicBlockLabel<'ctx, R>,
    expected: &[Value<'ctx>],
) where
    R: ReturnMarker,
{
    let got: Vec<_> = cfg
        .successors(from)
        .into_iter()
        .map(|bb| bb.as_value())
        .collect();
    assert_eq!(got, expected);
}

fn assert_predecessors<'ctx, R>(
    cfg: &FunctionCfg<'ctx>,
    block: BasicBlockLabel<'ctx, R>,
    expected: &[Value<'ctx>],
) where
    R: ReturnMarker,
{
    let got: Vec<_> = cfg
        .predecessors(block)
        .into_iter()
        .map(|bb| bb.as_value())
        .collect();
    assert_eq!(got, expected);
}

/// Mirrors `IR/CFG.h` `successors` / `predecessors` over a `BranchInst`
/// and `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn unconditional_branch_cfg_edges() -> Result<(), IrError> {
    Module::with_new("cfg_br", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let exit = f.append_basic_block(&m, "exit");
        let entry_label = entry.label();
        let entry_value = entry.as_value();
        let exit_label = exit.label();
        let exit_value = exit.as_value();

        IRBuilder::new_for::<()>(&m)
            .position_at_end(exit)
            .build_ret_void();
        IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_br(exit_label)?;

        let cfg = FunctionCfg::new(f.as_dyn());
        assert_successors(&cfg, entry_label, &[exit_value]);
        assert_predecessors(&cfg, exit_label, &[entry_value]);
        assert_eq!(cfg.edges().collect::<Vec<_>>().len(), 1);
        Ok(())
    })
}

/// Mirrors `IR/CFG.h` successor iteration preserving duplicate `br` edges
/// and `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn conditional_branch_preserves_duplicate_edges() -> Result<(), IrError> {
    Module::with_new("cfg_condbr", |m| {
        let bool_ty = m.bool_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [bool_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let target = f.append_basic_block(&m, "target");
        let entry_label = entry.label();
        let entry_value = entry.as_value();
        let target_label = target.label();
        let target_value = target.as_value();

        IRBuilder::new_for::<()>(&m)
            .position_at_end(target)
            .build_ret_void();
        let cond: IntValue<bool> = f.param(0)?.try_into()?;
        IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_cond_br(cond, target_label, target_label)?;

        let cfg = FunctionCfg::new(f.as_dyn());
        assert_successors(&cfg, entry_label, &[target_value, target_value]);
        assert_predecessors(&cfg, target_label, &[entry_value, entry_value]);
        Ok(())
    })
}

/// Mirrors `IR/CFG.h` successor iteration for `SwitchInst` and
/// `IR/Instructions.h` `SwitchInst::case_*` destination semantics.
#[test]
fn switch_cfg_edges_include_default_then_cases() -> Result<(), IrError> {
    Module::with_new("cfg_switch", |m| {
        let i8_ty = m.i8_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [i8_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let default_bb = f.append_basic_block(&m, "default");
        let case0 = f.append_basic_block(&m, "case0");
        let case1 = f.append_basic_block(&m, "case1");
        let entry_label = entry.label();
        let entry_value = entry.as_value();
        let default_label = default_bb.label();
        let default_value = default_bb.as_value();
        let case0_label = case0.label();
        let case0_value = case0.as_value();
        let case1_label = case1.label();
        let case1_value = case1.as_value();
        for bb in [default_bb, case0, case1] {
            IRBuilder::new_for::<()>(&m)
                .position_at_end(bb)
                .build_ret_void();
        }

        let val: IntValue<i8> = f.param(0)?.try_into()?;
        let (_sealed, switch) = IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_switch(val, default_label, "")?;
        let _closed = switch
            .add_case(i8_ty.const_int(0_i8), case0_label)?
            .add_case(i8_ty.const_int(1_i8), case1_label)?
            .finish();

        let cfg = FunctionCfg::new(f.as_dyn());
        assert_successors(
            &cfg,
            entry_label,
            &[default_value, case0_value, case1_value],
        );
        assert_predecessors(&cfg, default_label, &[entry_value]);
        assert_predecessors(&cfg, case0_label, &[entry_value]);
        assert_predecessors(&cfg, case1_label, &[entry_value]);
        Ok(())
    })
}

/// Mirrors `IR/CFG.h` successor iteration for `IndirectBrInst` and
/// `IR/Instructions.h` `IndirectBrInst::destinations` semantics.
#[test]
fn indirectbr_cfg_edges_are_listed_destinations() -> Result<(), IrError> {
    Module::with_new("cfg_indirectbr", |m| {
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let bb1 = f.append_basic_block(&m, "bb1");
        let bb2 = f.append_basic_block(&m, "bb2");
        let entry_label = entry.label();
        let entry_value = entry.as_value();
        let bb1_label = bb1.label();
        let bb1_value = bb1.as_value();
        let bb2_label = bb2.label();
        let bb2_value = bb2.as_value();
        for bb in [bb1, bb2] {
            IRBuilder::new_for::<()>(&m)
                .position_at_end(bb)
                .build_ret_void();
        }

        let addr: PointerValue = f.param(0)?.try_into()?;
        let (_sealed, ibr) = IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_indirectbr(addr, "")?;
        let _closed = ibr
            .add_destination(bb1_label)?
            .add_destination(bb2_label)?
            .finish();

        let cfg = FunctionCfg::new(f.as_dyn());
        assert_successors(&cfg, entry_label, &[bb1_value, bb2_value]);
        assert_predecessors(&cfg, bb1_label, &[entry_value]);
        assert_predecessors(&cfg, bb2_label, &[entry_value]);
        Ok(())
    })
}

/// Mirrors `IR/CFG.h` successor iteration for `InvokeInst` and
/// `llvm/lib/IR/Verifier.cpp` unwind-destination validation.
#[test]
fn invoke_cfg_edges_are_normal_then_unwind() -> Result<(), IrError> {
    Module::with_new("cfg_invoke", |m| {
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let callee = m.add_function::<(), _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let normal = caller.append_basic_block(&m, "normal");
        let unwind = caller.append_basic_block(&m, "unwind");
        let entry_label = entry.label();
        let entry_value = entry.as_value();
        let normal_label = normal.label();
        let normal_value = normal.as_value();
        let unwind_label = unwind.label();
        let unwind_value = unwind.as_value();
        for bb in [normal, unwind] {
            IRBuilder::new_for::<()>(&m)
                .position_at_end(bb)
                .build_ret_void();
        }

        IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_invoke(
                callee,
                Vec::<llvmkit_ir::Value>::new(),
                normal_label,
                unwind_label,
                "",
            )?;

        let cfg = FunctionCfg::new(caller.as_dyn());
        assert_successors(&cfg, entry_label, &[normal_value, unwind_value]);
        assert_predecessors(&cfg, normal_label, &[entry_value]);
        assert_predecessors(&cfg, unwind_label, &[entry_value]);
        Ok(())
    })
}

/// Mirrors `IR/CFG.h` successor iteration for `CallBrInst` and
/// `test/Assembler/callbr.ll` fallthrough-plus-indirect destination order.
#[test]
fn callbr_cfg_edges_are_default_then_indirect_dests() -> Result<(), IrError> {
    Module::with_new("cfg_callbr", |m| {
        let void_ty = m.void_type();
        let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let callee = m.add_function::<(), _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let dflt = caller.append_basic_block(&m, "default");
        let indirect = caller.append_basic_block(&m, "indirect");
        let entry_label = entry.label();
        let entry_value = entry.as_value();
        let dflt_label = dflt.label();
        let dflt_value = dflt.as_value();
        let indirect_label = indirect.label();
        let indirect_value = indirect.as_value();
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
                dflt_label,
                [indirect_label],
                "",
            )?;

        let cfg = FunctionCfg::new(caller.as_dyn());
        assert_successors(&cfg, entry_label, &[dflt_value, indirect_value]);
        assert_predecessors(&cfg, dflt_label, &[entry_value]);
        assert_predecessors(&cfg, indirect_label, &[entry_value]);
        Ok(())
    })
}

/// Mirrors `IR/CFG.h` successor iteration for `CatchReturnInst` and
/// `IR/Instructions.h` `CatchReturnInst::getSuccessor` semantics.
#[test]
fn catchret_cfg_edge_is_target_block() -> Result<(), IrError> {
    Module::with_new("cfg_catchret", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let cs_block = f.append_basic_block(&m, "cs");
        let cp_block = f.append_basic_block(&m, "cp");
        let ret_block = f.append_basic_block(&m, "ret");
        let cs_label = cs_block.label();
        let cp_label = cp_block.label();
        let cp_value = cp_block.as_value();
        let ret_label = ret_block.label();
        let ret_value = ret_block.as_value();
        IRBuilder::new_for::<()>(&m)
            .position_at_end(ret_block)
            .build_ret_void();

        let (_sealed, cs) = IRBuilder::new_for::<()>(&m)
            .position_at_end(cs_block)
            .build_catch_switch_within_none_to_caller("cs")?;
        let cs_closed = cs.add_handler(cp_label)?.finish();
        let b_cp = IRBuilder::new_for::<()>(&m).position_at_end(cp_block);
        let cp =
            b_cp.build_catch_pad(cs_closed.as_value(), Vec::<llvmkit_ir::Value>::new(), "cp")?;
        b_cp.build_catch_ret(cp.as_value(), ret_label, "")?;

        let cfg = FunctionCfg::new(f.as_dyn());
        assert_successors(&cfg, cs_label, &[cp_value]);
        assert_successors(&cfg, cp_label, &[ret_value]);
        assert_predecessors(&cfg, ret_label, &[cp_value]);
        Ok(())
    })
}

/// Mirrors `IR/CFG.h` successor iteration for `CleanupReturnInst` and
/// `llvm/lib/IR/Verifier.cpp` cleanupret unwind-destination validation.
#[test]
fn cleanupret_cfg_edge_is_optional_unwind_dest() -> Result<(), IrError> {
    Module::with_new("cfg_cleanupret", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let unwind = f.append_basic_block(&m, "unwind");
        let entry_label = entry.label();
        let entry_value = entry.as_value();
        let unwind_label = unwind.label();
        let unwind_value = unwind.as_value();
        IRBuilder::new_for::<()>(&m)
            .position_at_end(unwind)
            .build_ret_void();

        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let cp = b.build_cleanup_pad_within_none(Vec::<llvmkit_ir::Value>::new(), "cp")?;
        b.build_cleanup_ret(cp.as_value(), unwind_label, "")?;

        let cfg = FunctionCfg::new(f.as_dyn());
        assert_successors(&cfg, entry_label, &[unwind_value]);
        assert_predecessors(&cfg, unwind_label, &[entry_value]);
        Ok(())
    })
}

/// Mirrors `IR/CFG.h` successor iteration for `CatchSwitchInst` and
/// `IR/Instructions.h` handler-plus-unwind destination semantics.
#[test]
fn catchswitch_cfg_edges_are_handlers_then_unwind_dest() -> Result<(), IrError> {
    Module::with_new("cfg_catchswitch", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let handler0 = f.append_basic_block(&m, "handler0");
        let handler1 = f.append_basic_block(&m, "handler1");
        let unwind = f.append_basic_block(&m, "unwind");
        let entry_label = entry.label();
        let entry_value = entry.as_value();
        let handler0_label = handler0.label();
        let handler0_value = handler0.as_value();
        let handler1_label = handler1.label();
        let handler1_value = handler1.as_value();
        let unwind_label = unwind.label();
        let unwind_value = unwind.as_value();
        for bb in [handler0, handler1, unwind] {
            IRBuilder::new_for::<()>(&m)
                .position_at_end(bb)
                .build_ret_void();
        }

        let (_sealed, cs) = IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_catch_switch_within_none(unwind_label, "cs")?;
        let _closed = cs
            .add_handler(handler0_label)?
            .add_handler(handler1_label)?
            .finish();

        let cfg = FunctionCfg::new(f.as_dyn());
        assert_successors(
            &cfg,
            entry_label,
            &[handler0_value, handler1_value, unwind_value],
        );
        assert_predecessors(&cfg, handler0_label, &[entry_value]);
        assert_predecessors(&cfg, handler1_label, &[entry_value]);
        assert_predecessors(&cfg, unwind_label, &[entry_value]);
        Ok(())
    })
}
