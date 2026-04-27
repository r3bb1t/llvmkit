//! Phase C-cf coverage: `build_br` / `build_cond_br` /
//! `build_unreachable` plus their AsmWriter output.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
//! CreateCondBr)` -- the canonical builder coverage for branch terminators.

use llvmkit_ir::{IRBuilder, IntPredicate, IntValue, IrError, Linkage, Module, TerminatorKind};

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`
/// (unconditional-branch arm: `Builder.CreateBr(...)` produces `br label %...`).
#[test]
fn build_br_emits_unconditional() -> Result<(), IrError> {
    let m = Module::new("br");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let exit = f.append_basic_block("exit");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    b.build_br(exit)?;
    let b = IRBuilder::new_for::<()>(&m).position_at_end(exit);
    b.build_ret_void();
    let text = format!("{m}");
    assert!(text.contains("br label %exit"), "got:\n{text}");
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`
/// (the `Builder.CreateCondBr(getTrue(), TBB, FBB)` construction with i1
/// condition and two successor blocks).
#[test]
fn build_cond_br_branches_on_i1() -> Result<(), IrError> {
    let m = Module::new("cb");
    let void = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(void.as_type(), [i32_ty.as_type()], false);
    let f = m.add_function::<()>("cb", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let then_bb = f.append_basic_block("then");
    let else_bb = f.append_basic_block("else");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let cond = b.build_int_cmp::<i32, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
    b.build_cond_br(cond, then_bb, else_bb)?;
    let b = IRBuilder::new_for::<()>(&m).position_at_end(then_bb);
    b.build_ret_void();
    let b = IRBuilder::new_for::<()>(&m).position_at_end(else_bb);
    b.build_ret_void();
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
    let m = Module::new("u");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("dead", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let inst = b.build_unreachable();
    assert!(matches!(
        inst.terminator_kind(),
        Some(TerminatorKind::Unreachable(_))
    ));
    let text = format!("{m}");
    assert!(text.contains("\n  unreachable\n"), "got:\n{text}");
    Ok(())
}
