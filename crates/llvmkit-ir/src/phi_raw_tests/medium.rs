//! Phase C-phi coverage: `build_int_phi` plus
//! `PhiInst::add_incoming` for the post-creation flow.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
//! CreateCondBr)` for the multi-block control-flow scaffolding plus
//! `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)` for the
//! phi-incoming-value structure under test.

use crate::{Dyn, IrBuilder, IrError, Linkage};

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`
/// (multi-block scaffolding) plus
/// `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)` (phi
/// incoming-value iteration).
#[test]
fn build_int_phi_two_predecessors_emits_phi() -> Result<(), IrError> {
    let m = crate::module_new!("p")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("phi2", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let other = m.view(f).append_basic_block(&m, "other");
    let join = m.view(f).append_basic_block(&m, "join");
    let entry_label = entry.id();
    let other_label = other.id();
    let join_label = join.id();

    // entry: br label %join
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.build_br(join_label)?;

    // other: br label %join
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(other);
    b.build_br(join_label)?;

    // join: phi i32 [ 1, %entry ], [ 2, %other ]; ret i32 %p
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = b
        .view(b.build_int_phi::<i32, _>("p")?)
        .add_incoming(1_i32, entry_label)?
        .add_incoming(2_i32, other_label)?;
    b.build_ret(phi.as_int_value())?;

    let text = format!("{m}");
    assert!(
        text.contains("%p = phi i32 [ 1, %entry ], [ 2, %other ]"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)`
/// (post-creation `addIncoming` mutation of an empty phi). The control-flow
/// scaffolding follows
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn phi_with_post_creation_add_incoming() -> Result<(), IrError> {
    // Build the phi empty; add edges interleaved with other code; finally
    // emit `ret`. Mirrors the factorial-loop flow.
    let m = crate::module_new!("p")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("late", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let other = m.view(f).append_basic_block(&m, "other");
    let join = m.view(f).append_basic_block(&m, "join");
    let entry_label = entry.id();
    let other_label = other.id();
    let join_label = join.id();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.build_br(join_label)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(other);
    b.build_br(join_label)?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(join);
    let phi = b.view(b.build_int_phi::<i32, _>("p")?);
    let phi = phi
        .add_incoming(10_i32, entry_label)?
        .add_incoming(20_i32, other_label)?;
    b.build_ret(phi.as_int_value())?;

    let text = format!("{m}");
    assert!(
        text.contains("%p = phi i32 [ 10, %entry ], [ 20, %other ]"),
        "got:\n{text}"
    );
    Ok(())
}
