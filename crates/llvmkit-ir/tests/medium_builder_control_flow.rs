//! Phase C-cf coverage: `br` / `cond_br` /
//! `unreachable` plus their AsmWriter output.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
//! CreateCondBr)` -- the canonical builder coverage for branch terminators.

use llvmkit_ir::{
    Dyn, IntPredicate, IntValue, IrBuilder, IrError, Linkage, TerminatorKind, module_new,
};

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`
/// (unconditional-branch arm: `Builder.CreateBr(...)` produces `br label %...`).
#[test]
fn build_br_emits_unconditional() -> Result<(), IrError> {
    let m = module_new!("br")?;
    let void = m.void_type();
    let fn_ty = m.function_type(void.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let exit = m.view(f).append_basic_block(&m, "exit");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.br(&exit)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(exit);
    b.ret_void()?;
    let text = format!("{m}");
    assert!(text.contains("br label %exit"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`
/// (the `Builder.CreateCondBr(getTrue(), TBB, FBB)` construction with i1
/// condition and two successor blocks).
#[test]
fn build_cond_br_branches_on_i1() -> Result<(), IrError> {
    let m = module_new!("cb")?;
    let void = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(void.as_type(), [i32_ty.as_type()]);
    let f = m.add_function_dyn("cb", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_bb = m.view(f).append_basic_block(&m, "then");
    let else_bb = m.view(f).append_basic_block(&m, "else");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let cond = b.int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
    b.cond_br(cond, &then_bb, &else_bb)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(then_bb);
    b.ret_void()?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(else_bb);
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("br i1 %is_zero, label %then, label %else"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific: dedicated coverage of the `unreachable` terminator and its
/// `TerminatorKind::Unreachable` discriminant. Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` (the
/// builder-terminator family).
#[test]
fn build_unreachable_terminator() -> Result<(), IrError> {
    let m = module_new!("u")?;
    let void = m.void_type();
    let fn_ty = m.function_type(void.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("dead", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let (_sealed, inst) = b.unreachable();
    assert!(matches!(
        inst.terminator_kind(),
        Some(TerminatorKind::Unreachable(_))
    ));
    let text = format!("{m}");
    assert!(text.contains("\n  unreachable\n"), "got:\n{text}");
    Ok(())
}
