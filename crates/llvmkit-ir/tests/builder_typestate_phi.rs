//! Phi-finalisation typestate coverage (session T2).

use llvmkit_ir::{
    FloatDyn, FloatValue, IRBuilder, InstructionKind, IrError, Linkage, Module, PhiKind, Type,
};

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
