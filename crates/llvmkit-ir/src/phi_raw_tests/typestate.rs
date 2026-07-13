//! Relocated raw-phi typestate mechanics: the `Open -> Closed -> finish`
//! finalisation lifecycle, `add_incoming` linearity, `PhiKind` rediscovery,
//! phi-head placement, and a deliberately self-referential phi-iteration
//! shape. These cases drive the raw `build_*_phi`/`add_incoming`/`finish`
//! typestate that block-argument authoring cannot express. Ported verbatim
//! from `tests/builder_typestate_phi.rs` and the phi test extracted from
//! `tests/builder_typestate_termination.rs` (only the `llvmkit_ir::` paths
//! are rewritten to `crate::`); dormant until wired into the crate's
//! `#[cfg(test)]` tree.

use crate::{
    FloatDyn, FloatValue, IRBuilder, InstructionKind, IntValue, IrError, Linkage, Module, PhiKind,
    Type,
};

/// The `Open -> Closed` finalisation applies to every phi family, not just the
/// integer one: finishing an `fp` and a `pointer` phi consumes the open handle
/// and yields a `Closed` view that still reads back its incoming count. Covers
/// `FpPhiInst::finish` / `PointerPhiInst::finish` (the int case is
/// `phi_finishes_after_all_incomings`).
#[test]
fn fp_and_pointer_phi_finish_to_closed() -> Result<(), IrError> {
    Module::with_new("phi_finish_fp_ptr", |m| {
        let f64_ty = m.f64_type();
        let fn_ty = m.fn_type(f64_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<f64, _>("f", fn_ty, Linkage::External)?;
        let bb = f.append_basic_block(&m, "bb");
        let b = IRBuilder::new_for::<f64>(&m).position_at_end(bb);

        // `finish()` consumes the Open handle for the fp and pointer families
        // exactly as it does for the int family. No incomings are added, so the
        // closed handles read back a count of zero.
        let fp_closed = b.build_fp_phi::<f64, _>("fp")?.finish();
        let ptr_closed = b.build_pointer_phi("pp")?.finish();
        assert_eq!(fp_closed.incoming_count(), 0);
        assert_eq!(ptr_closed.incoming_count(), 0);
        Ok(())
    })
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
/// CreateCondBr)` constructive shape, extended to exercise the phi
/// `Open -> Closed` typestate. The structural assertions
/// (`incoming_count`, two distinct incoming blocks) mirror upstream's
/// `EXPECT_EQ(P->getNumIncomingValues(), 2)` style.
#[test]
fn phi_finishes_after_all_incomings() -> Result<(), IrError> {
    Module::with_new("phi_finish", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let other_label = other.label();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_br(&join)?;
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(other);
        b.build_br(&join)?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi_open = b.build_int_phi::<i32, _>("p")?;
        let phi_closed = phi_open
            .add_incoming(1_i32, entry_label)?
            .add_incoming(2_i32, other_label)?
            .finish();

        // Closed handles still expose read accessors. Mirrors upstream
        // `P->getNumIncomingValues()`.
        assert_eq!(phi_closed.incoming_count(), 2);
        let (_, incoming0) = phi_closed.incoming(0)?;
        let (_, incoming1) = phi_closed.incoming(1)?;
        assert_ne!(incoming0.as_value(), incoming1.as_value());

        // The phi result is still usable after finish().
        b.build_ret(phi_closed.as_int_value())?;
        let text = format!("{m}");
        assert!(
            text.contains("%p = phi i32 [ 1, %entry ], [ 2, %other ]"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Rediscovering a phi through `InstructionKind::Phi` narrows to a variant
/// chosen from the phi's *result type* — an `f64` phi is `PhiKind::Fp` (with
/// `as_float_value`), not the old integer-flavored handle whose
/// `as_int_value()` lied. Guards the core of the `PhiKind` split.
#[test]
fn rediscovered_phi_narrows_to_result_type() -> Result<(), IrError> {
    Module::with_new("phi_kind_rediscovery", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let bb = f.append_basic_block(&m, "bb");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(bb);

        let int_phi = b.build_int_phi::<i32, _>("ip")?;
        let fp_phi = b.build_fp_phi::<f64, _>("fp")?;
        let ptr_phi = b.build_pointer_phi("pp")?;

        assert!(matches!(
            int_phi.as_view().kind(),
            Some(InstructionKind::Phi(PhiKind::Int(_)))
        ));

        // The f64 phi comes back as `Fp`, never a lying `Int`.
        let Some(InstructionKind::Phi(PhiKind::Fp(fp))) = fp_phi.as_view().kind() else {
            panic!("expected float phi to rediscover as PhiKind::Fp");
        };
        let _fp_result: FloatValue<FloatDyn> = fp.as_float_value();

        assert!(matches!(
            ptr_phi.as_view().kind(),
            Some(InstructionKind::Phi(PhiKind::Ptr(_)))
        ));
        Ok(())
    })
}

/// Phis are placed at the block's phi head no matter where the cursor is.
/// Before this change the phi landed after the add (cursor position) and
/// only `Module::verify()` caught the misplacement (`PhiNotAtTop`). The
/// builder now inserts at the phi head, so the placement is correct by
/// construction. Exercises `build_int_phi_dyn` (the runtime-width path).
#[test]
fn build_phi_inserts_at_phi_head_not_cursor() -> Result<(), IrError> {
    Module::with_new("phi_head", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let other_label = other.label();
        let join_label = join.label();

        // Two predecessors so the join phi has a full incoming set.
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_br(join_label)?;
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(other);
        b.build_br(join_label)?;

        // In `join`, emit a NON-phi first (`%x = add`), THEN build the phi
        // while the cursor sits at the end of the block. The phi must still
        // land at the block's phi head, ahead of the add.
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let _x = b.build_int_add(a, 1_i32, "x")?;
        let i32_dyn = m.custom_width_int_type(32)?;
        let _phi = b
            .build_int_phi_dyn(i32_dyn, "p")?
            .add_incoming(f.param(0)?, entry_label)?
            .add_incoming(f.param(0)?, other_label)?;
        b.build_ret(a)?;

        let text = format!("{m}");
        let phi_pos = text
            .find("%p = phi")
            .unwrap_or_else(|| panic!("expected `%p = phi` in output; got:\n{text}"));
        let add_pos = text
            .find("%x = add")
            .unwrap_or_else(|| panic!("expected `%x = add` in output; got:\n{text}"));
        // (a) the phi prints BEFORE the add — it was moved to the phi head.
        assert!(
            phi_pos < add_pos,
            "phi must print before the add at the block's phi head; got:\n{text}"
        );
        // (b) the module verifies (no PhiNotAtTop).
        m.verify()?;
        Ok(())
    })
}

/// Two phis built AFTER a non-phi both land at the block's phi head and keep
/// their RELATIVE order: `p1` (built first) prints before `p2` (built
/// second), and both print before the non-phi `%x = add`.
#[test]
fn two_phis_built_after_nonphi_keep_relative_order() -> Result<(), IrError> {
    Module::with_new("phi_head_order", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let other_label = other.label();
        let join_label = join.label();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_br(join_label)?;
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(other);
        b.build_br(join_label)?;

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let _x = b.build_int_add(a, 1_i32, "x")?;
        // p1 then p2, both after the add. Head placement must not reverse
        // them: p1 stays ahead of p2.
        let _p1 = b
            .build_int_phi::<i32, _>("p1")?
            .add_incoming(1_i32, entry_label)?
            .add_incoming(2_i32, other_label)?;
        let _p2 = b
            .build_int_phi::<i32, _>("p2")?
            .add_incoming(3_i32, entry_label)?
            .add_incoming(4_i32, other_label)?;
        b.build_ret(a)?;

        let text = format!("{m}");
        let p1_pos = text
            .find("%p1 = phi")
            .unwrap_or_else(|| panic!("expected `%p1 = phi` in output; got:\n{text}"));
        let p2_pos = text
            .find("%p2 = phi")
            .unwrap_or_else(|| panic!("expected `%p2 = phi` in output; got:\n{text}"));
        let add_pos = text
            .find("%x = add")
            .unwrap_or_else(|| panic!("expected `%x = add` in output; got:\n{text}"));
        assert!(
            p1_pos < p2_pos,
            "p1 (built first) must print before p2 at the phi head; got:\n{text}"
        );
        assert!(
            p2_pos < add_pos,
            "both phis must print before the non-phi add; got:\n{text}"
        );
        m.verify()?;
        Ok(())
    })
}

/// Port of `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)`
/// (the phi-iteration assertion `EXPECT_EQ(std::distance(Phis.begin(),
/// Phis.end()), 3)`). Upstream's filter-iterator over `BB`'s
/// instructions is mirrored here by collecting and counting the
/// phi-handle subset.
#[test]
fn phi_range_iterates_three_phis() -> Result<(), IrError> {
    Module::with_new("p", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<crate::Type>::new(), false);
        let f = m.add_function::<i32, _>("p", fn_ty, Linkage::External)?;
        let bb = f.append_basic_block(&m, "bb");
        let bb_label = bb.label();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(bb);
        let p1 = b.build_int_phi::<i32, _>("phi.1")?;
        let p2 = b.build_int_phi::<i32, _>("phi.2")?;
        let p3 = b.build_int_phi::<i32, _>("phi.3")?;
        // Upstream wires `P1->addIncoming(P2, BB)` etc. via the same `BB`
        // (cycle). We add poisons referencing self -- the structural shape
        // matches the upstream phi count assertion regardless of operand
        // identities.
        let p1_value = p1.as_int_value();
        let p2_value = p2.as_int_value();
        let p3_value = p3.as_int_value();
        p1.add_incoming(0_i32, bb_label)?.finish();
        p2.add_incoming(0_i32, bb_label)?.finish();
        p3.add_incoming(0_i32, bb_label)?.finish();
        let _sum = b.build_int_add(p1_value, p2_value, "sum")?;
        let (terminated_bb, _) = b.build_ret(p3_value)?;

        // Upstream `EXPECT_EQ(std::distance(Phis.begin(), Phis.end()), 3)`.
        let phi_count = terminated_bb
            .instructions()
            .filter(|inst| matches!(inst.kind(), Some(crate::InstructionKind::Phi(_))))
            .count();
        assert_eq!(phi_count, 3);
        Ok(())
    })
}
