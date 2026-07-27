//! Shared CFG query coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    BlockId, Dyn, FunctionCfg, IRBuilder, IntValue, IrError, Linkage, ModuleBrand, PointerValue,
    ReturnMarker, module_new,
};

fn assert_successors<'ctx, R, B: ModuleBrand>(
    cfg: &FunctionCfg<'ctx, B>,
    from: BlockId<R, B>,
    expected: &[BlockId<Dyn, B>],
) where
    R: ReturnMarker,
{
    let got: Vec<_> = cfg.successors(from);
    assert_eq!(got, expected);
}

fn assert_predecessors<'ctx, R, B: ModuleBrand>(
    cfg: &FunctionCfg<'ctx, B>,
    block: BlockId<R, B>,
    expected: &[BlockId<Dyn, B>],
) where
    R: ReturnMarker,
{
    let got: Vec<_> = cfg.predecessors(block);
    assert_eq!(got, expected);
}

/// Mirrors `IR/CFG.h` `successors` / `predecessors` over a `BranchInst`
/// and `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn unconditional_branch_cfg_edges() -> Result<(), IrError> {
    let m = module_new!("cfg_br")?;
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let exit = m.view(f).append_basic_block(&m, "exit");
    let entry_label = entry.id();
    let exit_label = exit.id();

    IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(exit)
        .build_ret_void()?;
    IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .build_br(exit_label)?;

    let cfg = FunctionCfg::new(m.view(f).as_dyn());
    assert_successors(&cfg, entry_label, &[exit_label]);
    assert_predecessors(&cfg, exit_label, &[entry_label]);
    assert_eq!(cfg.edges().collect::<Vec<_>>().len(), 1);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration preserving duplicate `br` edges
/// and `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn conditional_branch_preserves_duplicate_edges() -> Result<(), IrError> {
    let m = module_new!("cfg_condbr")?;
    let bool_ty = m.bool_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [bool_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let target = m.view(f).append_basic_block(&m, "target");
    let entry_label = entry.id();
    let target_label = target.id();

    IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(target)
        .build_ret_void()?;
    let cond: IntValue<'_, bool, _> = m.view(f).param(0)?.try_into()?;
    IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .build_cond_br(cond, target_label, target_label)?;

    let cfg = FunctionCfg::new(m.view(f).as_dyn());
    assert_successors(&cfg, entry_label, &[target_label, target_label]);
    assert_predecessors(&cfg, target_label, &[entry_label, entry_label]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `SwitchInst` and
/// `IR/Instructions.h` `SwitchInst::case_*` destination semantics.
#[test]
fn switch_cfg_edges_include_default_then_cases() -> Result<(), IrError> {
    let m = module_new!("cfg_switch")?;
    let i8_ty = m.i8_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [i8_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let default_bb = m.view(f).append_basic_block(&m, "default");
    let case0 = m.view(f).append_basic_block(&m, "case0");
    let case1 = m.view(f).append_basic_block(&m, "case1");
    let entry_label = entry.id();
    let default_label = default_bb.id();
    let case0_label = case0.id();
    let case1_label = case1.id();
    for bb in [default_bb, case0, case1] {
        IRBuilder::new_for::<Dyn>(&m)
            .position_at_end(bb)
            .build_ret_void()?;
    }

    let val: IntValue<'_, i8, _> = m.view(f).param(0)?.try_into()?;
    let (_sealed, switch) = IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .build_switch_dyn(val, default_label, "")?;
    let _closed = switch
        .add_case(i8_ty.const_int(0_i8), case0_label)?
        .add_case(i8_ty.const_int(1_i8), case1_label)?
        .finish();

    let cfg = FunctionCfg::new(m.view(f).as_dyn());
    assert_successors(
        &cfg,
        entry_label,
        &[default_label, case0_label, case1_label],
    );
    assert_predecessors(&cfg, default_label, &[entry_label]);
    assert_predecessors(&cfg, case0_label, &[entry_label]);
    assert_predecessors(&cfg, case1_label, &[entry_label]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `IndirectBrInst` and
/// `IR/Instructions.h` `IndirectBrInst::destinations` semantics.
#[test]
fn indirectbr_cfg_edges_are_listed_destinations() -> Result<(), IrError> {
    let m = module_new!("cfg_indirectbr")?;
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let bb1 = m.view(f).append_basic_block(&m, "bb1");
    let bb2 = m.view(f).append_basic_block(&m, "bb2");
    let entry_label = entry.id();
    let bb1_label = bb1.id();
    let bb2_label = bb2.id();
    for bb in [bb1, bb2] {
        IRBuilder::new_for::<Dyn>(&m)
            .position_at_end(bb)
            .build_ret_void()?;
    }

    let addr: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let (_sealed, ibr) = IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .build_indirectbr(addr, "")?;
    let _closed = ibr
        .add_destination(bb1_label)?
        .add_destination(bb2_label)?
        .finish();

    let cfg = FunctionCfg::new(m.view(f).as_dyn());
    assert_successors(&cfg, entry_label, &[bb1_label, bb2_label]);
    assert_predecessors(&cfg, bb1_label, &[entry_label]);
    assert_predecessors(&cfg, bb2_label, &[entry_label]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `InvokeInst` and
/// `llvm/lib/IR/Verifier.cpp` unwind-destination validation.
#[test]
fn invoke_cfg_edges_are_normal_then_unwind() -> Result<(), IrError> {
    let m = module_new!("cfg_invoke")?;
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let normal = m.view(caller).append_basic_block(&m, "normal");
    let unwind = m.view(caller).append_basic_block(&m, "unwind");
    let entry_label = entry.id();
    let normal_label = normal.id();
    let unwind_label = unwind.id();
    for bb in [normal, unwind] {
        IRBuilder::new_for::<Dyn>(&m)
            .position_at_end(bb)
            .build_ret_void()?;
    }

    IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .build_invoke_dyn(
            m.view(callee),
            Vec::<llvmkit_ir::Value<'_, _>>::new(),
            normal_label,
            unwind_label,
            "",
        )?;

    let cfg = FunctionCfg::new(m.view(caller).as_dyn());
    assert_successors(&cfg, entry_label, &[normal_label, unwind_label]);
    assert_predecessors(&cfg, normal_label, &[entry_label]);
    assert_predecessors(&cfg, unwind_label, &[entry_label]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `CallBrInst` and
/// `test/Assembler/callbr.ll` fallthrough-plus-indirect destination order.
#[test]
fn callbr_cfg_edges_are_default_then_indirect_dests() -> Result<(), IrError> {
    let m = module_new!("cfg_callbr")?;
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
    let entry = m.view(caller).append_basic_block(&m, "entry");
    let dflt = m.view(caller).append_basic_block(&m, "default");
    let indirect = m.view(caller).append_basic_block(&m, "indirect");
    let entry_label = entry.id();
    let dflt_label = dflt.id();
    let indirect_label = indirect.id();
    for bb in [dflt, indirect] {
        IRBuilder::new_for::<Dyn>(&m)
            .position_at_end(bb)
            .build_ret_void()?;
    }

    IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .build_callbr(
            callee,
            Vec::<llvmkit_ir::Value<'_, _>>::new(),
            dflt_label,
            [indirect_label],
            "",
        )?;

    let cfg = FunctionCfg::new(m.view(caller).as_dyn());
    assert_successors(&cfg, entry_label, &[dflt_label, indirect_label]);
    assert_predecessors(&cfg, dflt_label, &[entry_label]);
    assert_predecessors(&cfg, indirect_label, &[entry_label]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `CatchReturnInst` and
/// `IR/Instructions.h` `CatchReturnInst::getSuccessor` semantics.
#[test]
fn catchret_cfg_edge_is_target_block() -> Result<(), IrError> {
    let m = module_new!("cfg_catchret")?;
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let cs_block = m.view(f).append_basic_block(&m, "cs");
    let cp_block = m.view(f).append_basic_block(&m, "cp");
    let ret_block = m.view(f).append_basic_block(&m, "ret");
    let cs_label = cs_block.id();
    let cp_label = cp_block.id();
    let ret_label = ret_block.id();
    IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(ret_block)
        .build_ret_void()?;

    let (_sealed, cs) = IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(cs_block)
        .build_catch_switch_within_none_to_caller("cs")?;
    let cs_closed = cs.add_handler(cp_label)?.finish();
    let b_cp = IRBuilder::new_for::<Dyn>(&m).position_at_end(cp_block);
    let cp = b_cp.build_catch_pad(
        cs_closed.to_erased(),
        Vec::<llvmkit_ir::Value<'_, _>>::new(),
        "cp",
    )?;
    b_cp.build_catch_ret(cp.to_erased(), ret_label, "")?;

    let cfg = FunctionCfg::new(m.view(f).as_dyn());
    assert_successors(&cfg, cs_label, &[cp_label]);
    assert_successors(&cfg, cp_label, &[ret_label]);
    assert_predecessors(&cfg, ret_label, &[cp_label]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `CleanupReturnInst` and
/// `llvm/lib/IR/Verifier.cpp` cleanupret unwind-destination validation.
#[test]
fn cleanupret_cfg_edge_is_optional_unwind_dest() -> Result<(), IrError> {
    let m = module_new!("cfg_cleanupret")?;
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let unwind = m.view(f).append_basic_block(&m, "unwind");
    let entry_label = entry.id();
    let unwind_label = unwind.id();
    IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(unwind)
        .build_ret_void()?;

    let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let cp = b.build_cleanup_pad_within_none(Vec::<llvmkit_ir::Value<'_, _>>::new(), "cp")?;
    b.build_cleanup_ret(cp.to_erased(), unwind_label, "")?;

    let cfg = FunctionCfg::new(m.view(f).as_dyn());
    assert_successors(&cfg, entry_label, &[unwind_label]);
    assert_predecessors(&cfg, unwind_label, &[entry_label]);
    Ok(())
}

/// Mirrors `IR/CFG.h` successor iteration for `CatchSwitchInst` and
/// `IR/Instructions.h` handler-plus-unwind destination semantics.
#[test]
fn catchswitch_cfg_edges_are_handlers_then_unwind_dest() -> Result<(), IrError> {
    let m = module_new!("cfg_catchswitch")?;
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let handler0 = m.view(f).append_basic_block(&m, "handler0");
    let handler1 = m.view(f).append_basic_block(&m, "handler1");
    let unwind = m.view(f).append_basic_block(&m, "unwind");
    let entry_label = entry.id();
    let handler0_label = handler0.id();
    let handler1_label = handler1.id();
    let unwind_label = unwind.id();
    for bb in [handler0, handler1, unwind] {
        IRBuilder::new_for::<Dyn>(&m)
            .position_at_end(bb)
            .build_ret_void()?;
    }

    let (_sealed, cs) = IRBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .build_catch_switch_within_none(unwind_label, "cs")?;
    let _closed = cs
        .add_handler(handler0_label)?
        .add_handler(handler1_label)?
        .finish();

    let cfg = FunctionCfg::new(m.view(f).as_dyn());
    assert_successors(
        &cfg,
        entry_label,
        &[handler0_label, handler1_label, unwind_label],
    );
    assert_predecessors(&cfg, handler0_label, &[entry_label]);
    assert_predecessors(&cfg, handler1_label, &[entry_label]);
    assert_predecessors(&cfg, unwind_label, &[entry_label]);
    Ok(())
}
