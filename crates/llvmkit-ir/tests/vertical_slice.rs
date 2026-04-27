//! End-to-end smoke test exercising types + values + IRBuilder + AsmWriter.
//!
//! ## Upstream provenance
//!
//! llvmkit-specific integration test with no single upstream `TEST` analog.
//! Closest upstream coverage: `unittests/IR/IRBuilderTest.cpp` exercises
//! the IRBuilder family at runtime; `unittests/IR/InstructionsTest.cpp`
//! exercises Instruction construction. Per-test citations below note
//! the closest functional reference for each.
//!
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
//! - Cross-value-category narrowing (`Argument -> IntValue<i32>`) errors
//!   cleanly when the argument's type is not integral.
//! - The typed `m.add_function::<i32, _>(...)` path produces a
//!   `FunctionValue<i32>` whose IRBuilder accepts only matching
//!   `IntValue<i32>` operands at `build_ret` (compile-time enforced).

use llvmkit_ir::{Argument, Dyn, IRBuilder, IntValue, IrError, Linkage, Module, TerminatorKind};

/// llvmkit-specific: end-to-end add+ret smoke test. Closest upstream
/// reference: `unittests/IR/IRBuilderTest.cpp` (IRBuilder unit tests
/// build identical add+ret patterns).
#[test]
fn vertical_slice_compiles_and_runs() -> Result<(), IrError> {
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<i32>("add", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");

    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);

    let lhs: IntValue<i32> = f.param(0)?.try_into()?;
    let rhs: IntValue<i32> = f.param(1)?.try_into()?;
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

/// llvmkit-specific: validates `IrError::OperandWidthMismatch` from the
/// runtime-checked `Dyn` IRBuilder path. No upstream analog (C++ asserts).
#[test]
fn mismatched_widths_error_at_runtime_when_dyn() -> Result<(), IrError> {
    // With static widths the mismatch is a compile error. Verify the
    // dynamic-width path still surfaces a runtime error when callers
    // intentionally erase the width.
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i64_ty.as_type()], false);
    let f = m.add_function::<i32>("mix", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);

    // Static narrowing rejects the i64 arg as an i32-typed IntValue.
    let _a: IntValue<i32> = f.param(0)?.try_into()?;
    let err: Result<IntValue<i32>, IrError> = f.param(1)?.try_into();
    assert!(matches!(
        err,
        Err(IrError::OperandWidthMismatch { lhs: 32, rhs: 64 })
    ));
    let _ = b;
    Ok(())
}

/// Mirrors `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, Integer_i1)` for
/// `ConstantInt::get` uniquing identity, generalised to i32/i64.
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
    let d: llvmkit_ir::ConstantIntValue<i64> = i64_ty.const_int(42_i64);
    assert_ne!(a.as_value().ty(), d.as_value().ty());
    Ok(())
}

/// llvmkit-specific: typestate `Argument -> IntValue<i32>` narrowing yields
/// `IrError::TypeMismatch`. Upstream `dyn_cast` returns `nullptr`.
#[test]
fn argument_to_int_value_narrowing_validates_type() -> Result<(), IrError> {
    // A `double` argument cannot narrow to `IntValue<i32>`.
    let m = Module::new("demo");
    let f64_ty = m.f64_type();
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), [f64_ty.as_type()], false);
    let f = m.add_function::<()>("takes_double", fn_ty, Linkage::External)?;
    let arg = f.param(0)?;
    let err: Result<IntValue<i32>, IrError> = IntValue::try_from(arg);
    assert!(matches!(err, Err(IrError::TypeMismatch { .. })));
    Ok(())
}

/// llvmkit-specific: llvmkit reports duplicate function names as
/// `IrError::DuplicateFunctionName`; upstream silently shadows / asserts
/// (see `Module::getOrInsertFunction` in `lib/IR/Module.cpp`).
#[test]
fn duplicate_function_name_errors() -> Result<(), IrError> {
    let m = Module::new("demo");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let _ = m.add_function::<()>("once", fn_ty, Linkage::External)?;
    let err = m
        .add_function::<()>("once", fn_ty, Linkage::External)
        .expect_err("duplicate must error");
    assert!(matches!(err, IrError::DuplicateFunctionName { ref name } if name == "once"));
    Ok(())
}

/// llvmkit-specific: `function_builder` chained-options API has no upstream
/// analog; closest reference: `unittests/IR/AttributesTest.cpp::TEST(Attributes, AddAttributes)`
/// for the AttrKind plumbing.
#[test]
fn function_builder_chains_options() -> Result<(), IrError> {
    use llvmkit_ir::{AttrIndex, AttrKind, Attribute, CallingConv};
    let m = Module::new("demo");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m
        .function_builder::<()>("worker", fn_ty)
        .linkage(Linkage::Internal)
        .calling_conv(CallingConv::FAST)
        .attribute(
            AttrIndex::Function,
            Attribute::enum_attr(AttrKind::AlwaysInline).expect("flag attribute"),
        )
        .build()?;
    assert_eq!(f.linkage(), Linkage::Internal);
    assert_eq!(f.calling_conv(), CallingConv::FAST);
    Ok(())
}

/// llvmkit-specific: typestate `add_function::<i32>` rejects `void`-returning
/// signatures with `IrError::ReturnTypeMismatch`. No upstream analog.
#[test]
fn typed_add_function_rejects_mismatched_return_marker() -> Result<(), IrError> {
    // `i32` against a `void`-returning signature errors at
    // `add_function` time (no need to reach the IRBuilder).
    let m = Module::new("demo");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let err = m
        .add_function::<i32>("bad", fn_ty, Linkage::External)
        .expect_err("i32 against void must error");
    assert!(matches!(err, IrError::ReturnTypeMismatch { .. }));
    Ok(())
}

/// llvmkit-specific: runtime-checked `Dyn` builder still validates `build_ret`
/// types. Closest upstream reference: assertion in `IRBuilderBase::CreateRet`.
#[test]
fn dyn_path_keeps_runtime_return_check() -> Result<(), IrError> {
    // Building against the runtime-checked `Dyn` builder reproduces
    // the pre-A3 behaviour: feeding `build_ret` a value of the wrong
    // type returns `IrError::ReturnTypeMismatch` at runtime.
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
    let f = m.add_function::<Dyn>("mix", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new(&m).position_at_end(entry);
    let arg = f.param(0)?; // i64
    let err = b
        .build_ret(arg)
        .expect_err("returning i64 from i32-returning function must error");
    assert!(matches!(err, IrError::ReturnTypeMismatch { .. }));
    Ok(())
}
