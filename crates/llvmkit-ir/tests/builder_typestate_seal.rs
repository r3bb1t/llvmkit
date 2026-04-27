//! Construction-lifecycle typestate coverage (session T2). The
//! compile-time guarantees themselves are exercised by the trybuild
//! fixtures in `tests/compile_fail/`; this file ports the runtime-shape
//! upstream tests and confirms the typestate-aware API still produces
//! the same IR text.

use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module};

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`
/// (the cond-br arm: `Builder.CreateCondBr(cond, TBB, FBB)` produces a
/// terminator with two successors, then iterates both successor blocks
/// from the resulting `Instruction`). Upstream uses
/// `TI->getNumSuccessors()` / `TI->getSuccessor(i)`; we do not yet ship
/// a `Successors` API, so the structural assertion translates to
/// inspecting the AsmWriter output.
#[test]
fn cond_br_terminator_seals_block() -> Result<(), IrError> {
    let m = Module::new("cb");
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<()>("cb", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let then_bb = f.append_basic_block("then");
    let else_bb = f.append_basic_block("else");

    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let cond: IntValue<bool> =
        b.build_int_cmp::<i32, _, _>(llvmkit_ir::IntPredicate::Eq, f.param(0)?, 0_i32, "cond")?;
    let (sealed_entry, term) = b.build_cond_br(cond, then_bb, else_bb)?;

    // Mirrors `EXPECT_EQ(BI, TI)` -- the returned terminator handle
    // matches the block's terminator.
    let term_in_block = sealed_entry
        .terminator()
        .expect("sealed block has a terminator");
    assert_eq!(term.as_value(), term_in_block.as_value());

    // `br i1 ..., label %then, label %else` is the canonical form
    // (matches upstream `EXPECT_EQ(TBB, TI->getSuccessor(0))` /
    // `EXPECT_EQ(FBB, TI->getSuccessor(1))`).
    let b = IRBuilder::new_for::<()>(&m).position_at_end(then_bb);
    b.build_ret_void();
    let b = IRBuilder::new_for::<()>(&m).position_at_end(else_bb);
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("br i1 %cond, label %then, label %else"),
        "got:\n{text}"
    );
    Ok(())
}

/// Port of `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)`
/// (the phi-iteration assertion `EXPECT_EQ(std::distance(Phis.begin(),
/// Phis.end()), 3)`). Upstream's filter-iterator over `BB`'s
/// instructions is mirrored here by collecting and counting the
/// phi-handle subset.
#[test]
fn phi_range_iterates_three_phis() -> Result<(), IrError> {
    let m = Module::new("p");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<i32>("p", fn_ty, Linkage::External)?;
    let bb = f.append_basic_block("bb");

    let b = IRBuilder::new_for::<i32>(&m).position_at_end(bb);
    let p1 = b.build_int_phi::<i32>("phi.1")?;
    let p2 = b.build_int_phi::<i32>("phi.2")?;
    let p3 = b.build_int_phi::<i32>("phi.3")?;
    // Upstream wires `P1->addIncoming(P2, BB)` etc. via the same `BB`
    // (cycle). We add poisons referencing self -- the structural shape
    // matches the upstream phi count assertion regardless of operand
    // identities.
    p1.add_incoming(0_i32, bb)?.finish();
    p2.add_incoming(0_i32, bb)?.finish();
    p3.add_incoming(0_i32, bb)?.finish();
    let _sum = b.build_int_add(p1.as_int_value(), p2.as_int_value(), "sum")?;
    b.build_ret(p3.as_int_value())?;

    // Upstream `EXPECT_EQ(std::distance(Phis.begin(), Phis.end()), 3)`.
    let phi_count = bb
        .instructions()
        .filter(|inst| matches!(inst.kind(), Some(llvmkit_ir::InstructionKind::Phi(_))))
        .count();
    assert_eq!(phi_count, 3);
    Ok(())
}

/// llvmkit-specific (Doctrine D11): the seal-state typestate carries
/// no runtime data; verifying the cosmetic IR text still matches the
/// pre-T2 baseline guards against accidental AsmWriter regressions
/// when `BasicBlock<'ctx, R, Seal>` gained its phantom parameter.
/// Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn seal_typestate_does_not_change_asm_output() -> Result<(), IrError> {
    let m = Module::new("seal_asm");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let exit = f.append_basic_block("exit");

    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    b.build_br(exit)?;
    let b = IRBuilder::new_for::<()>(&m).position_at_end(exit);
    b.build_ret_void();

    let expected = "; ModuleID = 'seal_asm'\n\
                    define void @g() {\n\
                    entry:\n  br label %exit\n\n\
                    exit:\n  ret void\n}\n";
    assert_eq!(format!("{m}"), expected);
    Ok(())
}
