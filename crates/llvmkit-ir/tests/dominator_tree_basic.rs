//! DominatorTree query coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    BasicBlockEdge, DominatorTree, FunctionCfg, IRBuilder, InstructionView, IntPredicate, IntValue,
    IrError, Linkage, Module, User,
};

fn inst<'ctx>(v: llvmkit_ir::Value<'ctx>) -> Result<InstructionView<'ctx>, IrError> {
    InstructionView::try_from(v)
}

/// Ports the block-reachability and block-dominance assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)`.
#[test]
fn reachable_and_unreachable_block_dominance() -> Result<(), IrError> {
    Module::with_new("dt_blocks", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");
        let join = f.append_basic_block(&m, "join");
        let dead = f.append_basic_block(&m, "dead");
        let entry_label = entry.label();
        let then_label = then_bb.label();
        let else_label = else_bb.label();
        let join_label = join.label();
        let dead_label = dead.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let cond = b.build_int_cmp(IntPredicate::Eq, x, 0_i32, "cond")?;
        b.build_cond_br(cond, then_label, else_label)?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(then_bb)
            .build_br(join_label)?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(else_bb)
            .build_br(join_label)?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(join)
            .build_ret(x)?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(dead)
            .build_ret(x)?;

        let dt = DominatorTree::new(f.as_dyn());
        assert!(dt.is_reachable_from_entry(entry_label));
        assert!(dt.is_reachable_from_entry(then_label));
        assert!(dt.is_reachable_from_entry(else_label));
        assert!(dt.is_reachable_from_entry(join_label));
        assert!(!dt.is_reachable_from_entry(dead_label));

        assert!(dt.dominates_block(entry_label, entry_label));
        assert!(dt.dominates_block(entry_label, then_label));
        assert!(dt.dominates_block(entry_label, else_label));
        assert!(dt.dominates_block(entry_label, join_label));
        assert!(dt.dominates_block(entry_label, dead_label));
        assert!(!dt.properly_dominates_block(entry_label, entry_label));
        assert!(dt.properly_dominates_block(entry_label, join_label));
        assert!(!dt.dominates_block(then_label, join_label));
        assert!(!dt.dominates_block(else_label, join_label));
        assert!(dt.dominates_block(dead_label, dead_label));
        assert!(!dt.dominates_block(dead_label, entry_label));
        Ok(())
    })
}

/// Ports the same-block instruction assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)`:
/// reachable blocks obey instruction order, while unreachable uses are
/// dominated even by themselves.
#[test]
fn same_block_instruction_order_and_unreachable_use_semantics() -> Result<(), IrError> {
    Module::with_new("dt_inst_order", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let dead = f.append_basic_block(&m, "dead");
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
    })
}

/// Ports `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, PHIs)`
/// and `Dominators.cpp::DominatorTree::dominates(const BasicBlock*, const Use&)`:
/// PHI operands are uses on incoming edges, not ordinary uses at the PHI's block start.
#[test]
fn phi_operands_are_dominated_on_incoming_edges() -> Result<(), IrError> {
    Module::with_new("dt_phi_use", |m| {
        let i32_ty = m.i32_type();
        let bool_ty = m.bool_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");
        let then_label = then_bb.label();
        let else_label = else_bb.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let cond: IntValue<bool> = f.param(1)?.try_into()?;

        // join(%p: i32): the merge head-phi. Its incomings arrive in branch
        // order — `then` carries `%y` first, then `else` carries `%x` — so the
        // head-phi records `[%y, then], [%x, else]` and `%p` (params[0]) is the
        // phi result, exactly the explicit phi this test used before.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (join, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "join")?;
        let join_label = join.label();

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_cond_br(cond, then_label, else_label)?;
        let bt = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        let y = bt.build_int_add(x, 1_i32, "y")?;
        bt.build_br_with_args(join_label, &[y.as_value()])?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(else_bb)
            .build_br_with_args(join_label, &[x.as_value()])?;
        let p: IntValue<i32> = params[0].try_into()?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(join)
            .build_ret(p)?;

        let yi = inst(y.as_value())?;
        // The phi is the join block's head param; recover its view from
        // `params[0]`. `operand_use` consumes the view, so recover it twice.
        let phii = inst(params[0])?;
        let y_use = inst(params[0])?
            .operand_use(0)
            .expect("phi has first incoming use");
        let dt = DominatorTree::new(f.as_dyn());

        assert!(!dt.dominates_instruction(&yi, &phii));
        assert!(dt.dominates_use(y.as_value(), y_use));
        Ok(())
    })
}

/// Ports the invoke-result assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)` and
/// `Dominators.cpp::DominatorTree::dominates(const Instruction*, const BasicBlock*)`.
#[test]
fn invoke_result_dominates_normal_destination_but_not_unwind() -> Result<(), IrError> {
    Module::with_new("dt_invoke", |m| {
        let i32_ty = m.i32_type();
        let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let callee = m.add_function::<i32, _>("callee", callee_ty, Linkage::External)?;
        let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", caller_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let normal = f.append_basic_block(&m, "normal");
        let unwind = f.append_basic_block(&m, "unwind");
        let normal_label = normal.label();
        let unwind_label = unwind.label();
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        let (_sealed, invoke) = IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_invoke_dyn(
                callee,
                Vec::<llvmkit_ir::Value>::new(),
                normal_label,
                unwind_label,
                "iv",
            )?;
        let invoke_value: IntValue<i32> = invoke.as_value().try_into()?;

        let bn = IRBuilder::new_for::<i32>(&m).position_at_end(normal);
        let normal_use = bn.build_int_add(invoke_value, 1_i32, "normal_use")?;
        bn.build_ret(normal_use)?;
        let bu = IRBuilder::new_for::<i32>(&m).position_at_end(unwind);
        let unwind_use = bu.build_int_add(invoke_value, 1_i32, "unwind_use")?;
        bu.build_ret(x)?;

        let invoke_inst = invoke.as_view();
        let normal_use_inst = inst(normal_use.as_value())?;
        let unwind_use_inst = inst(unwind_use.as_value())?;
        let dt = DominatorTree::new(f.as_dyn());

        assert!(dt.dominates_instruction(&invoke_inst, &normal_use_inst));
        assert!(!dt.dominates_instruction(&invoke_inst, &unwind_use_inst));
        Ok(())
    })
}

/// Ports `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, NonUniqueEdges)`
/// and `Dominators.cpp::BasicBlockEdge::isSingleEdge`: one duplicate edge from
/// a conditional branch must not dominate the shared successor.
#[test]
fn duplicate_edges_do_not_dominate_successor() -> Result<(), IrError> {
    Module::with_new("dt_non_unique_edge", |m| {
        let i32_ty = m.i32_type();
        let bool_ty = m.bool_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        let cond: IntValue<bool> = f.param(1)?.try_into()?;

        // join(%p: i32): both arms of the conditional branch target join (a
        // duplicate edge), each carrying the same `%x`. The head-phi therefore
        // records `[%x, entry], [%x, entry]` — the same-value duplicate for the
        // shared predecessor is accepted.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (join, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "join")?;
        let join_label = join.label();

        IRBuilder::new_for::<i32>(&m)
            .position_at_end(entry)
            .build_cond_br_with_args(
                cond,
                join_label,
                &[x.as_value()],
                join_label,
                &[x.as_value()],
            )?;
        let p: IntValue<i32> = params[0].try_into()?;
        IRBuilder::new_for::<i32>(&m)
            .position_at_end(join)
            .build_ret(p)?;

        let cfg = FunctionCfg::new(f.as_dyn());
        let edge: BasicBlockEdge<'_> = cfg.edges().next().expect("conditional branch has an edge");
        let dt = DominatorTree::new(f.as_dyn());

        assert!(!dt.dominates_edge(edge, join_label));
        Ok(())
    })
}
