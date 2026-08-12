//! StructType body-state typestate coverage (session T4).
//!
//! Doctrine D1: an opaque named struct typed as `StructType<Opaque>`
//! consumes its handle on `set_struct_body` and produces a
//! `StructType<BodySet>` -- a second `set_struct_body` call is
//! impossible because there is no second `Opaque` handle. The
//! companion trybuild fixture in `tests/compile_fail/`
//! (`set_struct_body_twice.rs`) locks the compile-fail story.

use llvmkit_ir::{IrError, module_new};

/// Port of `unittests/IR/TypesTest.cpp::TEST(TypesTest, StructType)`
/// (the name-management sub-tests). Upstream's `setName` API is not
/// shipped; the assertion translates to "the named struct retains the
/// name it was created with". The structural shape (create + observe
/// name) matches the upstream invariant.
#[test]
fn named_struct_retains_name() -> Result<(), IrError> {
    let m = module_new!("t")?;
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
    let m = module_new!("t")?;
    let i32_ty = m.i32_type();
    let opaque = m.opaque_struct("Pair")?;
    assert!(opaque.is_opaque());
    let body_set = m.set_struct_body(opaque, [i32_ty.as_type(), i32_ty.as_type()], false)?;
    assert!(!body_set.is_opaque());
    assert_eq!(body_set.field_count(), 2);
    Ok(())
}

/// llvmkit-specific (Doctrine D1): the runtime `set_struct_body_dyn`
/// (untyped, runtime-checked path) still rejects a second body
/// assignment with [`IrError::StructBodyAlreadySet`]. This guards the
/// runtime-checked default that mirrors LLVM's `StructType::setBody`
/// assertion. Closest upstream reference:
/// `unittests/IR/TypesTest.cpp::TEST(TypesTest, StructType)`.
#[test]
fn double_set_body_runtime_path_rejects() -> Result<(), IrError> {
    let m = module_new!("t")?;
    let i32_ty = m.i32_type();
    let opaque = m.opaque_struct("Once")?;
    let _body_set = m.set_struct_body(opaque, [i32_ty.as_type(), i32_ty.as_type()], false)?;
    // The typed `Opaque` handle has been consumed. Attempting another
    // `opaque_struct(name)` for the same name surfaces the runtime
    // `StructBodyAlreadySet` (since the second declaration pulls an
    // already-set named struct).
    let err = m.opaque_struct("Once").unwrap_err();
    assert!(matches!(err, IrError::StructBodyAlreadySet { .. }));
    Ok(())
}

/// Mirrors `StructType::checkBody` (`lib/IR/Type.cpp`), whose message this
/// reproduces verbatim: a body that reaches the struct being defined is
/// rejected, so the cycle `Type::isSized` and `Type::isScalableTy` guard
/// against with their `Visited` sets cannot be built in the first place.
///
/// This is why those two predicates' visited sets are belt-and-braces rather
/// than load-bearing. They are still threaded — upstream threads them, and a
/// predicate that recurses on its input should not depend on a guard in a
/// different file staying complete.
#[test]
fn self_referential_struct_body_is_rejected() -> Result<(), IrError> {
    let m = module_new!("t")?;
    let opaque = m.opaque_struct("rec")?;
    let err = m
        .set_struct_body(opaque, [opaque.as_type()], false)
        .expect_err("a self-referential body is rejected");
    assert_eq!(
        err,
        IrError::RecursiveStructBody {
            name: String::from("rec"),
        }
    );
    assert_eq!(
        err.to_string(),
        "identified structure type 'rec' is recursive"
    );
    Ok(())
}

/// llvmkit-specific (Doctrine D11): `StructType::isSized`'s scalable-vector
/// rule and its `containsHomogeneousScalableVectorTypes` exception, asserted
/// on the predicate directly because upstream pins them only through
/// `test/Verifier/scalable-vector-struct-{alloca,load,store}.ll`, whose
/// instructions belong to a later wave.
#[test]
fn struct_sizedness_follows_the_scalable_vector_rule() -> Result<(), IrError> {
    let m = module_new!("t")?;
    let i32_ty = m.i32_type().as_type();
    let scalable = m.scalable_vector_type(m.i32_type(), 2).as_type();

    let mixed = m.set_struct_body(m.opaque_struct("mixed")?, [scalable, i32_ty], false)?;
    assert!(!mixed.as_type().is_sized());
    assert!(mixed.as_type().is_scalable());

    let homogeneous =
        m.set_struct_body(m.opaque_struct("homogeneous")?, [scalable, scalable], false)?;
    assert!(homogeneous.as_type().is_sized());
    assert!(homogeneous.as_type().is_scalable());

    // `Type::isScalableTy` walks array elements; `isSized` then refuses the
    // struct that holds the array.
    let array = m.array_type(scalable, 2).as_type();
    let wrapped = m.set_struct_body(m.opaque_struct("wrapped")?, [array], false)?;
    assert!(wrapped.as_type().is_scalable());
    assert!(!wrapped.as_type().is_sized());
    Ok(())
}

/// llvmkit-specific (Doctrine D11): `Type::isScalableTargetExtTy` — a target
/// extension type counts as scalable exactly when its layout type is a
/// scalable vector. `target("aarch64.svcount")` lays out as
/// `<vscale x 16 x i1>`; `target("spirv.Image")` lays out as `ptr`.
#[test]
fn scalable_target_extension_types_are_scalable() -> Result<(), IrError> {
    let m = module_new!("t")?;
    let svcount = m
        .target_ext_type(
            "aarch64.svcount",
            Vec::<llvmkit_ir::Type<'_, _>>::new(),
            Vec::<u32>::new(),
        )
        .as_type();
    let image = m
        .target_ext_type(
            "spirv.Image",
            Vec::<llvmkit_ir::Type<'_, _>>::new(),
            Vec::<u32>::new(),
        )
        .as_type();
    assert!(svcount.is_scalable());
    assert!(!image.is_scalable());
    // Both are sized: sizedness follows the layout type, and a scalable
    // vector is sized. Only a *struct holding* one is not.
    assert!(svcount.is_sized());
    assert!(image.is_sized());
    Ok(())
}
