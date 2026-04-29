//! DominatorTree query coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    BasicBlockEdge, DominatorTree, FunctionCfg, IRBuilder, IntPredicate, IntValue, IrError,
    Linkage, Module, User,
};

fn inst<'ctx>(v: llvmkit_ir::Value<'ctx>) -> Result<llvmkit_ir::Instruction<'ctx>, IrError> {
    v.try_into()
}

/// Ports the block-reachability and block-dominance assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)`.
#[test]
fn reachable_and_unreachable_block_dominance() -> Result<(), IrError> {
    let m = Module::new("dt_blocks");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let then_bb = f.append_basic_block("then");
    let else_bb = f.append_basic_block("else");
    let join = f.append_basic_block("join");
    let dead = f.append_basic_block("dead");
    let x: IntValue<i32> = f.param(0)?.try_into()?;

    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let cond = b.build_int_cmp(IntPredicate::Eq, x, 0_i32, "cond")?;
    b.build_cond_br(cond, then_bb, else_bb)?;
    IRBuilder::new_for::<i32>(&m)
        .position_at_end(then_bb)
        .build_br(join)?;
    IRBuilder::new_for::<i32>(&m)
        .position_at_end(else_bb)
        .build_br(join)?;
    IRBuilder::new_for::<i32>(&m)
        .position_at_end(join)
        .build_ret(x)?;
    IRBuilder::new_for::<i32>(&m)
        .position_at_end(dead)
        .build_ret(x)?;

    let dt = DominatorTree::new(f.as_dyn());
    assert!(dt.is_reachable_from_entry(entry));
    assert!(dt.is_reachable_from_entry(then_bb));
    assert!(dt.is_reachable_from_entry(else_bb));
    assert!(dt.is_reachable_from_entry(join));
    assert!(!dt.is_reachable_from_entry(dead));

    assert!(dt.dominates_block(entry, entry));
    assert!(dt.dominates_block(entry, then_bb));
    assert!(dt.dominates_block(entry, else_bb));
    assert!(dt.dominates_block(entry, join));
    assert!(dt.dominates_block(entry, dead));
    assert!(!dt.properly_dominates_block(entry, entry));
    assert!(dt.properly_dominates_block(entry, join));
    assert!(!dt.dominates_block(then_bb, join));
    assert!(!dt.dominates_block(else_bb, join));
    assert!(dt.dominates_block(dead, dead));
    assert!(!dt.dominates_block(dead, entry));
    Ok(())
}

/// Ports the same-block instruction assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)`:
/// reachable blocks obey instruction order, while unreachable uses are
/// dominated even by themselves.
#[test]
fn same_block_instruction_order_and_unreachable_use_semantics() -> Result<(), IrError> {
    let m = Module::new("dt_inst_order");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let dead = f.append_basic_block("dead");
    let x: IntValue<i32> = f.param(0)?.try_into()?;

    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let y1 = b.build_int_add(x, 1_i32, "y1")?;
    let y2 = b.build_int_add(y1, 1_i32, "y2")?;
    b.build_ret(y2)?;

    let bd = IRBuilder::new_for::<i32>(&m).position_at_end(dead);
    let z1 = bd.build_int_add(x, 1_i32, "z1")?;
    let z2 = bd.build_int_add(z1, 1_i32, "z2")?;
    bd.build_ret(z2)?;

    let y1i = inst(y1.as_value())?;
    let y2i = inst(y2.as_value())?;
    let z1i = inst(z1.as_value())?;
    let z2i = inst(z2.as_value())?;
    let dt = DominatorTree::new(f.as_dyn());

    assert!(!dt.dominates_instruction(&y1i, &y1i));
    assert!(dt.dominates_instruction(&y1i, &y2i));
    assert!(!dt.dominates_instruction(&y2i, &y1i));
    assert!(!dt.dominates_instruction(&y2i, &y2i));
    assert!(dt.dominates_instruction(&z1i, &z1i));
    assert!(dt.dominates_instruction(&z1i, &z2i));
    assert!(dt.dominates_instruction(&z2i, &z1i));
    assert!(dt.dominates_instruction(&z2i, &z2i));
    Ok(())
}

/// Ports `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, PHIs)`
/// and `Dominators.cpp::DominatorTree::dominates(const BasicBlock*, const Use&)`:
/// PHI operands are uses on incoming edges, not ordinary uses at the PHI's block start.
#[test]
fn phi_operands_are_dominated_on_incoming_edges() -> Result<(), IrError> {
    let m = Module::new("dt_phi_use");
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let then_bb = f.append_basic_block("then");
    let else_bb = f.append_basic_block("else");
    let join = f.append_basic_block("join");
    let x: IntValue<i32> = f.param(0)?.try_into()?;
    let cond: IntValue<bool> = f.param(1)?.try_into()?;

    IRBuilder::new_for::<i32>(&m)
        .position_at_end(entry)
        .build_cond_br(cond, then_bb, else_bb)?;
    let bt = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
    let y = bt.build_int_add(x, 1_i32, "y")?;
    bt.build_br(join)?;
    IRBuilder::new_for::<i32>(&m)
        .position_at_end(else_bb)
        .build_br(join)?;
    let bj = IRBuilder::new_for::<i32>(&m).position_at_end(join);
    let phi = bj
        .build_int_phi::<i32>("p")?
        .add_incoming(y, then_bb)?
        .add_incoming(x, else_bb)?;
    bj.build_ret(phi.as_int_value())?;

    let yi = inst(y.as_value())?;
    let phii = phi.as_instruction();
    let y_use = phi
        .as_instruction()
        .operand_use(0)
        .expect("phi has first incoming use");
    let dt = DominatorTree::new(f.as_dyn());

    assert!(!dt.dominates_instruction(&yi, &phii));
    assert!(dt.dominates_use(y.as_value(), y_use));
    Ok(())
}

/// Ports the invoke-result assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)` and
/// `Dominators.cpp::DominatorTree::dominates(const Instruction*, const BasicBlock*)`.
#[test]
fn invoke_result_dominates_normal_destination_but_not_unwind() -> Result<(), IrError> {
    let m = Module::new("dt_invoke");
    let i32_ty = m.i32_type();
    let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let callee = m.add_function::<i32>("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", caller_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let normal = f.append_basic_block("normal");
    let unwind = f.append_basic_block("unwind");
    let x: IntValue<i32> = f.param(0)?.try_into()?;

    let (_sealed, invoke) = IRBuilder::new_for::<i32>(&m)
        .position_at_end(entry)
        .build_invoke(
            callee,
            Vec::<llvmkit_ir::Value>::new(),
            normal,
            unwind,
            "iv",
        )?;
    let invoke_value: IntValue<i32> = invoke.as_instruction().as_value().try_into()?;

    let bn = IRBuilder::new_for::<i32>(&m).position_at_end(normal);
    let normal_use = bn.build_int_add(invoke_value, 1_i32, "normal_use")?;
    bn.build_ret(normal_use)?;
    let bu = IRBuilder::new_for::<i32>(&m).position_at_end(unwind);
    let unwind_use = bu.build_int_add(invoke_value, 1_i32, "unwind_use")?;
    bu.build_ret(x)?;

    let invoke_inst = invoke.as_instruction();
    let normal_use_inst = inst(normal_use.as_value())?;
    let unwind_use_inst = inst(unwind_use.as_value())?;
    let dt = DominatorTree::new(f.as_dyn());

    assert!(dt.dominates_instruction(&invoke_inst, &normal_use_inst));
    assert!(!dt.dominates_instruction(&invoke_inst, &unwind_use_inst));
    Ok(())
}

/// Ports `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, NonUniqueEdges)`
/// and `Dominators.cpp::BasicBlockEdge::isSingleEdge`: one duplicate edge from
/// a conditional branch must not dominate the shared successor.
#[test]
fn duplicate_edges_do_not_dominate_successor() -> Result<(), IrError> {
    let m = Module::new("dt_non_unique_edge");
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let join = f.append_basic_block("join");
    let x: IntValue<i32> = f.param(0)?.try_into()?;
    let cond: IntValue<bool> = f.param(1)?.try_into()?;

    IRBuilder::new_for::<i32>(&m)
        .position_at_end(entry)
        .build_cond_br(cond, join, join)?;
    let bj = IRBuilder::new_for::<i32>(&m).position_at_end(join);
    let phi = bj
        .build_int_phi::<i32>("p")?
        .add_incoming(x, entry)?
        .add_incoming(x, entry)?;
    bj.build_ret(phi.as_int_value())?;

    let cfg = FunctionCfg::new(f.as_dyn());
    let edge: BasicBlockEdge<'_> = cfg.edges().next().expect("conditional branch has an edge");
    let dt = DominatorTree::new(f.as_dyn());

    assert!(!dt.dominates_edge(edge, join));
    Ok(())
}
