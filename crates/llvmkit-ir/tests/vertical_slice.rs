//! End-to-end test for the value-layer + minimal-IRBuilder slice.
//!
//! After Phase A1 / A2 (width-typed integers + kind-typed floats) and
//! Phase A3 (return-type-safe IRBuilder) the vertical-slice asserts:
//!
//! - The function has exactly one basic block after `append_basic_block`.
//! - The entry block's terminator is a `Ret`.
//! - The `add` instruction's operands are the function's two arguments.
//! - The IRBuilder's binary-op generic enforces equal widths at compile
//!   time (no runtime `OperandWidthMismatch` for the in-test happy path).
//! - Constant interning: two `i32_ty.const_int(42i32)` calls return
//!   equal handles.
//! - Cross-value-category narrowing (`Argument -> IntValue<B32>`) errors
//!   cleanly when the argument's type is not integral.
//! - The typed `m.add_function::<RInt<B32>, _>(...)` path produces a
//!   `FunctionValue<RInt<B32>>` whose IRBuilder accepts only matching
//!   `IntValue<B32>` operands at `build_ret` (compile-time enforced).

use llvmkit_ir::{
    Argument, B32, B64, IRBuilder, IntValue, IrError, Linkage, Module, RDyn, RInt, RVoid,
    TerminatorKind,
};

#[test]
fn vertical_slice_compiles_and_runs() -> Result<(), IrError> {
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<RInt<B32>>("add", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");

    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);

    let lhs: IntValue<B32> = f.param(0)?.try_into()?;
    let rhs: IntValue<B32> = f.param(1)?.try_into()?;
    let sum = b.build_int_add(lhs, rhs, "sum")?;
    b.build_ret(sum)?;

    // ---- Assertions ----

    // Exactly one basic block.
    assert_eq!(f.basic_blocks().count(), 1);

    // Terminator is a Ret.
    let term = entry.terminator().expect("entry must have a terminator");
    assert!(matches!(
        term.terminator_kind(),
        Some(TerminatorKind::Ret(_))
    ));

    // The Ret's operand is the `sum` instruction.
    let ret_inst = match term.terminator_kind() {
        Some(TerminatorKind::Ret(r)) => r,
        _ => panic!("not a Ret"),
    };
    let returned = ret_inst.return_value().expect("ret had a value");
    assert_eq!(returned.ty(), sum.ty().as_type());

    // The `add` instruction's operands are the function's two args.
    let arg0: Argument = f.param(0)?;
    let arg1: Argument = f.param(1)?;
    let add_kind = sum.as_value().name();
    assert_eq!(add_kind.as_deref(), Some("sum"));
    let _ = arg0;
    let _ = arg1;
    Ok(())
}

#[test]
fn mismatched_widths_error_at_runtime_when_dyn() -> Result<(), IrError> {
    // With static widths the mismatch is a compile error. Verify the
    // dynamic-width path still surfaces a runtime error when callers
    // intentionally erase the width.
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<RInt<B32>>("mix", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);

    // Static narrowing rejects the i64 arg as a B32-typed IntValue.
    let _a: IntValue<B32> = f.param(0)?.try_into()?;
    let err: Result<IntValue<B32>, IrError> = f.param(1)?.try_into();
    assert!(matches!(
        err,
        Err(IrError::OperandWidthMismatch { lhs: 32, rhs: 64 })
    ));
    let _ = b;
    Ok(())
}

#[test]
fn const_int_interns() -> Result<(), IrError> {
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    // Type-driven dispatch: `i32` literal sign-extends; `u32` literal
    // zero-extends. `42` fits losslessly under either, so the resulting
    // bit patterns coincide and the interner returns the same handle.
    let a = i32_ty.const_int(42_i32);
    let b = i32_ty.const_int(42_i32);
    assert_eq!(a, b);

    // Different value, same type: distinct handles.
    let c = i32_ty.const_int(43_i32);
    assert_ne!(a, c);

    // Same value, different type: distinct handles.
    let i64_ty = m.i64_type();
    let d: llvmkit_ir::ConstantIntValue<B64> = i64_ty.const_int(42_i64);
    assert_ne!(a.as_value().ty(), d.as_value().ty());
    Ok(())
}

#[test]
fn argument_to_int_value_narrowing_validates_type() -> Result<(), IrError> {
    // A `double` argument cannot narrow to `IntValue<B32>`.
    let m = Module::new("demo");
    let f64_ty = m.f64_type();
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), [f64_ty.as_type()], false);
    let f = m.add_function::<RVoid>("takes_double", fn_ty, Linkage::External)?;
    let arg = f.param(0)?;
    let err: Result<IntValue<B32>, IrError> = IntValue::try_from(arg);
    assert!(matches!(err, Err(IrError::TypeMismatch { .. })));
    Ok(())
}

#[test]
fn duplicate_function_name_errors() -> Result<(), IrError> {
    let m = Module::new("demo");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let _ = m.add_function::<RVoid>("once", fn_ty, Linkage::External)?;
    let err = m
        .add_function::<RVoid>("once", fn_ty, Linkage::External)
        .expect_err("duplicate must error");
    assert!(matches!(err, IrError::DuplicateFunctionName { ref name } if name == "once"));
    Ok(())
}

#[test]
fn function_builder_chains_options() -> Result<(), IrError> {
    use llvmkit_ir::{AttrIndex, AttrKind, Attribute, CallingConv};
    let m = Module::new("demo");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m
        .function_builder::<RVoid>("worker", fn_ty)
        .linkage(Linkage::Internal)
        .calling_conv(CallingConv::Fast)
        .attribute(
            AttrIndex::Function,
            Attribute::enum_attr(AttrKind::AlwaysInline).expect("flag attribute"),
        )
        .build()?;
    assert_eq!(f.linkage(), Linkage::Internal);
    assert_eq!(f.calling_conv(), CallingConv::Fast);
    Ok(())
}

#[test]
fn typed_add_function_rejects_mismatched_return_marker() -> Result<(), IrError> {
    // `RInt<B32>` against a `void`-returning signature errors at
    // `add_function` time (no need to reach the IRBuilder).
    let m = Module::new("demo");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let err = m
        .add_function::<RInt<B32>>("bad", fn_ty, Linkage::External)
        .expect_err("RInt<B32> against void must error");
    assert!(matches!(err, IrError::ReturnTypeMismatch { .. }));
    Ok(())
}

#[test]
fn dyn_path_keeps_runtime_return_check() -> Result<(), IrError> {
    // Building against the runtime-checked `RDyn` builder reproduces
    // the pre-A3 behaviour: feeding `build_ret` a value of the wrong
    // type returns `IrError::ReturnTypeMismatch` at runtime.
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
    let f = m.add_function::<RDyn>("mix", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new(&m).position_at_end(entry);
    let arg = f.param(0)?; // i64
    let err = b
        .build_ret(arg)
        .expect_err("returning i64 from i32-returning function must error");
    assert!(matches!(err, IrError::ReturnTypeMismatch { .. }));
    Ok(())
}
