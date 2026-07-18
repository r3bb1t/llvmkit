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
fn cond_br_terminator_terminates_block() -> Result<(), IrError> {
    Module::with_new("cb", |m| {
        let void_ty = m.void_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("cb", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");

        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let lhs: IntValue<i32> = f.param(0)?.try_into()?;
        let cond: IntValue<bool> =
            b.build_int_cmp(llvmkit_ir::IntPredicate::Eq, lhs, 0_i32, "cond")?;
        let (terminated_entry, term) = b.build_cond_br(cond, &then_bb, &else_bb)?;

        // Mirrors `EXPECT_EQ(BI, TI)` -- the returned terminator handle
        // matches the block's terminator.
        let term_in_block = terminated_entry
            .terminator()
            .expect("terminated block has a terminator");
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
    })
}

/// llvmkit-specific (Doctrine D11): the termination-state typestate carries
/// no runtime data; verifying the cosmetic IR text still matches the
/// pre-T2 baseline guards against accidental AsmWriter regressions
/// when `BasicBlock<'ctx, R, Term>` gained its phantom parameter.
/// Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn termination_typestate_does_not_change_asm_output() -> Result<(), IrError> {
    Module::with_new("termination_asm", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("g", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let exit = f.append_basic_block(&m, "exit");

        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        b.build_br(&exit)?;
        let b = IRBuilder::new_for::<()>(&m).position_at_end(exit);
        b.build_ret_void();

        let expected = "; ModuleID = 'termination_asm'\n\
                        define void @g() {\n\
                        entry:\n  br label %exit\n\n\
                        exit:\n  ret void\n}\n";
        assert_eq!(format!("{m}"), expected);
        Ok(())
    })
}
