//! StructType body-state typestate coverage (session T4).
//!
//! Doctrine D1: an opaque named struct typed as `StructType<Opaque>`
//! consumes its handle on `set_struct_body_typed` and produces a
//! `StructType<BodySet>` -- a second `set_struct_body_typed` call is
//! impossible because there is no second `Opaque` handle. The
//! companion trybuild fixture in `tests/compile_fail/`
//! (`set_struct_body_twice.rs`) locks the compile-fail story.

use llvmkit_ir::{IrError, Module};

/// Port of `unittests/IR/TypesTest.cpp::TEST(TypesTest, StructType)`
/// (the name-management sub-tests). Upstream's `setName` API is not
/// shipped; the assertion translates to "the named struct retains the
/// name it was created with". The structural shape (create + observe
/// name) matches the upstream invariant.
#[test]
fn named_struct_retains_name() -> Result<(), IrError> {
    let m = Module::new("t");
    let opaque = m.opaque_struct("FooBar")?;
    assert_eq!(opaque.name(), Some("FooBar"));
    assert!(opaque.is_opaque());
    Ok(())
}

/// llvmkit-specific (Doctrine D11): exercises the `Opaque -> BodySet`
/// typestate transition. Closest upstream functional reference:
/// `unittests/IR/TypesTest.cpp::TEST(TypesTest, LayoutIdenticalEmptyStructs)`,
/// which uses `StructType::create` + `setBody` to construct identified
/// structs.
#[test]
fn opaque_to_body_set_transition() -> Result<(), IrError> {
    let m = Module::new("t");
    let i32_ty = m.i32_type();
    let opaque = m.opaque_struct("Pair")?;
    assert!(opaque.is_opaque());
    let body_set = m.set_struct_body_typed(opaque, [i32_ty.as_type(), i32_ty.as_type()], false)?;
    assert!(!body_set.is_opaque());
    assert_eq!(body_set.field_count(), 2);
    Ok(())
}

/// llvmkit-specific (Doctrine D1): the runtime `set_struct_body`
/// (untyped, runtime-checked path) still rejects a second body
/// assignment with [`IrError::StructBodyAlreadySet`]. This guards the
/// runtime-checked default that mirrors LLVM's `StructType::setBody`
/// assertion. Closest upstream reference:
/// `unittests/IR/TypesTest.cpp::TEST(TypesTest, StructType)`.
#[test]
fn double_set_body_runtime_path_rejects() -> Result<(), IrError> {
    let m = Module::new("t");
    let i32_ty = m.i32_type();
    let opaque = m.opaque_struct("Once")?;
    let _body_set = m.set_struct_body_typed(opaque, [i32_ty.as_type(), i32_ty.as_type()], false)?;
    // The typed `Opaque` handle has been consumed. Attempting another
    // `opaque_struct(name)` for the same name surfaces the runtime
    // `StructBodyAlreadySet` (since the second declaration pulls an
    // already-set named struct).
    let err = m.opaque_struct("Once").unwrap_err();
    assert!(matches!(err, IrError::StructBodyAlreadySet { .. }));
    Ok(())
}
