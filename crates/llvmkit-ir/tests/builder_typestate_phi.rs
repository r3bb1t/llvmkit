//! Phi-finalisation typestate coverage (session T2).

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest,
/// CreateCondBr)` constructive shape, extended to exercise the phi
/// `Open -> Closed` typestate. The structural assertions
/// (`incoming_count`, two distinct incoming blocks) mirror upstream's
/// `EXPECT_EQ(P->getNumIncomingValues(), 2)` style.
#[test]
fn phi_finishes_after_all_incomings() -> Result<(), IrError> {
    let m = Module::new("phi_finish");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let other = f.append_basic_block("other");
    let join = f.append_basic_block("join");

    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    b.build_br(join)?;
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(other);
    b.build_br(join)?;

    let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
    let phi_open = b.build_int_phi::<i32>("p")?;
    let phi_closed = phi_open
        .add_incoming(1_i32, entry)?
        .add_incoming(2_i32, other)?
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
}
