//! Phase C-phi coverage: `build_int_phi` plus
//! `PhiInst::add_incoming` for the post-creation flow.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
//! CreateCondBr)` for the multi-block control-flow scaffolding plus
//! `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)` for the
//! phi-incoming-value structure under test.

use crate::{Dyn, IRBuilder, IrError, Linkage, Module};

/// Mirrors `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`
/// (multi-block scaffolding) plus
/// `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)` (phi
/// incoming-value iteration).
#[test]
fn build_int_phi_two_predecessors_emits_phi() -> Result<(), IrError> {
    Module::with_new("p", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("phi2", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let other_label = other.label();
        let join_label = join.label();

        // entry: br label %join
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.build_br(join_label)?;

        // other: br label %join
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(other);
        b.build_br(join_label)?;

        // join: phi i32 [ 1, %entry ], [ 2, %other ]; ret i32 %p
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(join);
        let phi = b
            .build_int_phi::<i32, _>("p")?
            .add_incoming(1_i32, entry_label)?
            .add_incoming(2_i32, other_label)?;
        b.build_ret(phi.as_int_value())?;

        let text = format!("{m}");
        assert!(
            text.contains("%p = phi i32 [ 1, %entry ], [ 2, %other ]"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Mirrors `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)`
/// (post-creation `addIncoming` mutation of an empty phi). The control-flow
/// scaffolding follows
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)`.
#[test]
fn phi_with_post_creation_add_incoming() -> Result<(), IrError> {
    // Build the phi empty; add edges interleaved with other code; finally
    // emit `ret`. Mirrors the factorial-loop flow.
    Module::with_new("p", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("late", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let other_label = other.label();
        let join_label = join.label();

        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.build_br(join_label)?;
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(other);
        b.build_br(join_label)?;

        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(join);
        let phi = b.build_int_phi::<i32, _>("p")?;
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
    })
}
