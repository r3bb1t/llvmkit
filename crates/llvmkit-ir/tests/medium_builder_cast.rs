//! Phase C cast-builder coverage. Currently only `build_trunc` ships;
//! other cast opcodes (`zext`, `sext`, `bitcast`, …) land in follow-up
//! sessions.

use llvmkit_ir::{B32, B64, IRBuilder, IntValue, IrError, Linkage, Module, RInt};

#[test]
fn build_trunc_emits_trunc_to_dst_type() -> Result<(), IrError> {
    let m = Module::new("t");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
    let f = m.add_function::<RInt<B32>>("narrow", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let arg: IntValue<B64> = f.param(0)?.try_into()?;
    let truncated = b.build_trunc(arg, i32_ty, "narrow")?;
    b.build_ret(truncated)?;

    let text = format!("{m}");
    let expected = "; ModuleID = 't'\n\
        define i32 @narrow(i64 %0) {\n\
        entry:\n\
        \x20\x20%narrow = trunc i64 %0 to i32\n\
        \x20\x20ret i32 %narrow\n\
        }\n";
    assert_eq!(text, expected, "got:\n{text}");
    Ok(())
}

#[test]
fn build_trunc_rejects_widening() -> Result<(), IrError> {
    // `trunc` requires the destination to be strictly narrower; the
    // builder errors with `OperandWidthMismatch` when it isn't.
    let m = Module::new("t");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<RInt<B64>>("bad", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B64>>(&m).position_at_end(entry);
    let arg: IntValue<B32> = f.param(0)?.try_into()?;
    let err = b
        .build_trunc(arg, i64_ty, "bad")
        .expect_err("trunc to wider type must error");
    assert!(matches!(
        err,
        llvmkit_ir::IrError::OperandWidthMismatch { lhs: 32, rhs: 64 }
    ));
    Ok(())
}

#[test]
fn build_trunc_preserves_anonymous_slot_naming() -> Result<(), IrError> {
    // Empty name -> the cast result lands as `%0`/`%1`/... per slot
    // numbering. Mirrors the `cpu_state_add` example.
    let m = Module::new("t");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
    let f = m.add_function::<RInt<B32>>("anon", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let arg: IntValue<B64> = f.param(0)?.try_into()?;
    let t = b.build_trunc(arg, i32_ty, "")?;
    b.build_ret(t)?;

    let text = format!("{m}");
    assert!(text.contains("%1 = trunc i64 %0 to i32\n"), "got:\n{text}");
    Ok(())
}
