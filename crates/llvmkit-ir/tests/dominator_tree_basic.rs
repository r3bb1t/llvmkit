//! DominatorTree query coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    BasicBlockEdge, DominatorTree, Dyn, FunctionCfg, InstructionView, IntPredicate, IntValue,
    IrBuilder, IrError, Linkage, ModuleBrand, User, module_new,
};

fn inst<'ctx, B: ModuleBrand + 'ctx>(
    v: llvmkit_ir::Value<'ctx, B>,
) -> Result<InstructionView<'ctx, B>, IrError> {
    InstructionView::try_from(v)
}

/// Ports the block-reachability and block-dominance assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)`.
#[test]
fn reachable_and_unreachable_block_dominance() -> Result<(), IrError> {
    let m = module_new!("dt_blocks")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_bb = m.view(f).append_basic_block(&m, "then");
    let else_bb = m.view(f).append_basic_block(&m, "else");
    let join = m.view(f).append_basic_block(&m, "join");
    let dead = m.view(f).append_basic_block(&m, "dead");
    let entry_label = entry.id();
    let then_label = then_bb.id();
    let else_label = else_bb.id();
    let join_label = join.id();
    let dead_label = dead.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let cond = b.int_cmp(IntPredicate::Eq, x, 0_i32, "cond")?;
    b.cond_br(cond, then_label, else_label)?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(then_bb)
        .br(join_label)?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(else_bb)
        .br(join_label)?;
    IrBuilder::new_for::<Dyn>(&m).position_at_end(join).ret(x)?;
    IrBuilder::new_for::<Dyn>(&m).position_at_end(dead).ret(x)?;

    let dt = DominatorTree::new(m.view(f).as_dyn());
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
}

/// Ports the same-block instruction assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)`:
/// reachable blocks obey instruction order, while unreachable uses are
/// dominated even by themselves.
#[test]
fn same_block_instruction_order_and_unreachable_use_semantics() -> Result<(), IrError> {
    let m = module_new!("dt_inst_order")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let dead = m.view(f).append_basic_block(&m, "dead");
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let y1 = b.int_add(x, 1_i32, "y1")?;
    let y2 = b.int_add(y1, 1_i32, "y2")?;
    b.ret(y2)?;

    let bd = IrBuilder::new_for::<Dyn>(&m).position_at_end(dead);
    let z1 = bd.int_add(x, 1_i32, "z1")?;
    let z2 = bd.int_add(z1, 1_i32, "z2")?;
    bd.ret(z2)?;

    let y1i = inst(m.view(y1).into_erased())?;
    let y2i = inst(m.view(y2).into_erased())?;
    let z1i = inst(m.view(z1).into_erased())?;
    let z2i = inst(m.view(z2).into_erased())?;
    let dt = DominatorTree::new(m.view(f).as_dyn());

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
    let m = module_new!("dt_phi_use")?;
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_bb = m.view(f).append_basic_block(&m, "then");
    let else_bb = m.view(f).append_basic_block(&m, "else");
    let then_label = then_bb.id();
    let else_label = else_bb.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let cond: IntValue<'_, bool, _> = m.view(f).param(1)?.try_into()?;

    // join(%p: i32): the merge head-phi. Its incomings arrive in branch
    // order — `then` carries `%y` first, then `else` carries `%x` — so the
    // head-phi records `[%y, then], [%x, else]` and `%p` (params[0]) is the
    // phi result, exactly the explicit phi this test used before.
    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (join, params) = bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "join")?;
    let join_label = join.id();

    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .cond_br(cond, then_label, else_label)?;
    let bt = IrBuilder::new_for::<Dyn>(&m).position_at_end(then_bb);
    let y = bt.int_add(x, 1_i32, "y")?;
    bt.br_with_args(join_label, &[m.view(y).into_erased()])?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(else_bb)
        .br_with_args(join_label, &[x.into_erased()])?;
    let p: IntValue<'_, i32, _> = params[0].try_into()?;
    IrBuilder::new_for::<Dyn>(&m).position_at_end(join).ret(p)?;

    let yi = inst(m.view(y).into_erased())?;
    // The phi is the join block's head param; recover its view from
    // `params[0]`. `operand_use` consumes the view, so recover it twice.
    let phii = inst(params[0])?;
    let y_use = inst(params[0])?
        .operand_use(0)
        .expect("phi has first incoming use");
    let dt = DominatorTree::new(m.view(f).as_dyn());

    assert!(!dt.dominates_instruction(&yi, &phii));
    assert!(dt.dominates_use(m.view(y).into_erased(), y_use));
    Ok(())
}

/// Ports the invoke-result assertions from
/// `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, Unreachable)` and
/// `Dominators.cpp::DominatorTree::dominates(const Instruction*, const BasicBlock*)`.
#[test]
fn invoke_result_dominates_normal_destination_but_not_unwind() -> Result<(), IrError> {
    let m = module_new!("dt_invoke")?;
    let i32_ty = m.i32_type();
    let callee_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new(), false);
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", caller_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let normal = m.view(f).append_basic_block(&m, "normal");
    let unwind = m.view(f).append_basic_block(&m, "unwind");
    let normal_label = normal.id();
    let unwind_label = unwind.id();
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let (_sealed, invoke) = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .invoke_dyn(
            m.view(callee),
            Vec::<llvmkit_ir::Value<'_, _>>::new(),
            normal_label,
            unwind_label,
            "iv",
        )?;
    let invoke_value: IntValue<'_, i32, _> = invoke.to_erased().try_into()?;

    let bn = IrBuilder::new_for::<Dyn>(&m).position_at_end(normal);
    let normal_use = bn.int_add(invoke_value, 1_i32, "normal_use")?;
    bn.ret(normal_use)?;
    let bu = IrBuilder::new_for::<Dyn>(&m).position_at_end(unwind);
    let unwind_use = bu.int_add(invoke_value, 1_i32, "unwind_use")?;
    bu.ret(x)?;

    let invoke_inst = invoke.as_view();
    let normal_use_inst = inst(m.view(normal_use).into_erased())?;
    let unwind_use_inst = inst(m.view(unwind_use).into_erased())?;
    let dt = DominatorTree::new(m.view(f).as_dyn());

    assert!(dt.dominates_instruction(&invoke_inst, &normal_use_inst));
    assert!(!dt.dominates_instruction(&invoke_inst, &unwind_use_inst));
    Ok(())
}

/// Ports `unittests/IR/DominatorTreeTest.cpp::TEST(DominatorTree, NonUniqueEdges)`
/// and `Dominators.cpp::BasicBlockEdge::isSingleEdge`: one duplicate edge from
/// a conditional branch must not dominate the shared successor.
#[test]
fn duplicate_edges_do_not_dominate_successor() -> Result<(), IrError> {
    let m = module_new!("dt_non_unique_edge")?;
    let i32_ty = m.i32_type();
    let bool_ty = m.bool_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), bool_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let cond: IntValue<'_, bool, _> = m.view(f).param(1)?.try_into()?;

    // join(%p: i32): both arms of the conditional branch target join (a
    // duplicate edge), each carrying the same `%x`. The head-phi therefore
    // records `[%x, entry], [%x, entry]` — the same-value duplicate for the
    // shared predecessor is accepted.
    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (join, params) = bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "join")?;
    let join_label = join.id();

    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .cond_br_with_args(
            cond,
            join_label,
            &[x.into_erased()],
            join_label,
            &[x.into_erased()],
        )?;
    let p: IntValue<'_, i32, _> = params[0].try_into()?;
    IrBuilder::new_for::<Dyn>(&m).position_at_end(join).ret(p)?;

    let cfg = FunctionCfg::new(m.view(f).as_dyn());
    let edge: BasicBlockEdge<_> = cfg.edges().next().expect("conditional branch has an edge");
    let dt = DominatorTree::new(m.view(f).as_dyn());

    assert!(!dt.dominates_edge(edge, join_label));
    Ok(())
}
