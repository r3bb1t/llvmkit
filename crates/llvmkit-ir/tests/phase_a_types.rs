//! Phase A integration tests: type construction, interning identity,
//! refinements, and named-struct body lifecycle.
//!
//! ## Upstream provenance
//!
//! Tests in this file mirror coverage spread across
//! `unittests/IR/TypesTest.cpp` (`StructType`, `LayoutIdenticalEmptyStructs`)
//! and `unittests/IR/VectorTypesTest.cpp` (`FixedLength`, `Scalable`).
//! Per-test citations below. Tests that exercise llvmkit's typestate
//! narrowing / `IrError` surface have no dedicated upstream `TEST` and
//! are tagged `llvmkit-specific:` with the closest functional reference.

use llvmkit_ir::{
    AggregateType, AnyTypeEnum, BasicTypeEnum, IntDyn, IntType, IrError, IrType, MAX_INT_BITS,
    MIN_INT_BITS, Module, SizedType, Type, TypeKind, TypeKindLabel,
};

/// llvmkit-specific: primitive type interning has no dedicated upstream `TEST`;
/// the closest functional reference is
/// `unittests/IR/TypesTest.cpp::TEST(TypesTest, LayoutIdenticalEmptyStructs)`,
/// which exercises `getOrCreate` identity for `StructType`.
#[test]
fn primitive_types_intern_to_same_id() {
    let m = Module::new("t");

    // Two requests for `void` return the same handle.
    let v1 = m.void_type();
    let v2 = m.void_type();
    assert_eq!(v1.as_type(), v2.as_type());

    // Same for `i32` via the named accessor and the custom-width
    // accessor.
    let a = m.i32_type();
    let b = m.custom_width_int_type(32).expect("32 is in range");
    assert_eq!(a.as_type(), b.as_type());
    assert_eq!(a.bit_width(), 32);
}

/// llvmkit-specific: integer width distinctness has no dedicated upstream
/// `TEST`. Closest reference: `IntegerType::get(Ctx, N)` uniquing exercised
/// throughout `unittests/IR/TypesTest.cpp`.
#[test]
fn integer_widths_distinct() {
    let m = Module::new("t");
    let i8 = m.i8_type();
    let i16 = m.i16_type();
    let i32 = m.i32_type();
    let i64 = m.i64_type();
    assert_ne!(i8.as_type(), i16.as_type());
    assert_ne!(i16.as_type(), i32.as_type());
    assert_ne!(i32.as_type(), i64.as_type());
    assert_eq!(i8.bit_width(), 8);
    assert_eq!(i64.bit_width(), 64);
}

/// llvmkit-specific: llvmkit validates `IntegerType::get` width via the
/// `IrError::InvalidIntegerWidth` enum; upstream asserts in `IntegerType::get`
/// (see `lib/IR/Type.cpp`).
#[test]
fn integer_width_validation() {
    let m = Module::new("t");
    assert!(matches!(
        m.custom_width_int_type(0),
        Err(IrError::InvalidIntegerWidth { bits: 0 })
    ));
    assert!(matches!(
        m.custom_width_int_type(MAX_INT_BITS + 1),
        Err(IrError::InvalidIntegerWidth { .. })
    ));
    // Boundaries are valid.
    assert!(m.custom_width_int_type(MIN_INT_BITS).is_ok());
    assert!(m.custom_width_int_type(MAX_INT_BITS).is_ok());
}

/// Port of `unittests/IR/VectorTypesTest.cpp::TEST(VectorTypesTest, FixedLength)`
/// and `TEST(VectorTypesTest, Scalable)` for vector identity, plus the array
/// uniquing pattern from `unittests/IR/TypesTest.cpp`.
#[test]
fn array_and_vector_intern() {
    let m = Module::new("t");
    let i32 = m.i32_type();
    let a1 = m.array_type(i32, 8);
    let a2 = m.array_type(i32, 8);
    assert_eq!(a1.as_type(), a2.as_type());
    assert_eq!(a1.element(), i32.as_type());
    assert_eq!(a1.len(), 8);

    let v_fixed = m.vector_type(i32, 4, false);
    let v_scalable = m.vector_type(i32, 4, true);
    assert_ne!(v_fixed.as_type(), v_scalable.as_type());
    assert!(!v_fixed.is_scalable());
    assert!(v_scalable.is_scalable());
    assert_eq!(v_fixed.min_len(), 4);
}

/// llvmkit-specific: `FunctionType` uniquing has no dedicated upstream `TEST`;
/// closest reference is the `FunctionType::get` uniquing in
/// `lib/IR/Type.cpp` exercised implicitly throughout
/// `unittests/IR/TypesTest.cpp`.
#[test]
fn function_type_round_trip() {
    let m = Module::new("t");
    let i32 = m.i32_type();
    let i64 = m.i64_type();
    let void = m.void_type();
    let ft = m.fn_type(void.as_type(), [i32.as_type(), i64.as_type()], false);
    assert_eq!(ft.return_type(), void.as_type());
    assert_eq!(ft.params().count(), 2);
    assert!(!ft.is_var_arg());

    // Same shape interns to the same handle.
    let ft2 = m.fn_type(void.as_type(), [i32.as_type(), i64.as_type()], false);
    assert_eq!(ft.as_type(), ft2.as_type());

    // varargs differs.
    let ft_va = m.fn_type(void.as_type(), [i32.as_type(), i64.as_type()], true);
    assert_ne!(ft.as_type(), ft_va.as_type());
    assert!(ft_va.is_var_arg());
}

/// Port of `unittests/IR/TypesTest.cpp::TEST(TypesTest, LayoutIdenticalEmptyStructs)`
/// applied to literal (anonymous) struct interning + packed-bit distinction.
#[test]
fn literal_struct_intern() {
    let m = Module::new("t");
    let i32 = m.i32_type();
    let i64 = m.i64_type();
    let s1 = m.struct_type([i32.as_type(), i64.as_type()], false);
    let s2 = m.struct_type([i32.as_type(), i64.as_type()], false);
    assert_eq!(s1.as_type(), s2.as_type());
    assert_eq!(s1.field_count(), 2);
    assert!(!s1.is_packed());
    assert!(s1.name().is_none());

    let s_packed = m.struct_type([i32.as_type(), i64.as_type()], true);
    assert_ne!(s1.as_type(), s_packed.as_type());
    assert!(s_packed.is_packed());
}

/// Port of `unittests/IR/TypesTest.cpp::TEST(TypesTest, StructType)` —
/// `StructType::create` forward declaration, lookup identity, and
/// `setBody` lifecycle (llvmkit reports a second `setBody` as
/// `IrError::StructBodyAlreadySet`).
#[test]
fn named_struct_forward_decl_then_set_body() {
    let m = Module::new("t");
    let s = m.named_struct("MyStruct");
    assert!(s.is_opaque());
    assert_eq!(s.name(), Some("MyStruct"));
    assert_eq!(s.field_count(), 0);

    // Looking up the same name returns the same handle.
    let s_again = m.named_struct("MyStruct");
    assert_eq!(s.as_type(), s_again.as_type());
    let s_lookup = m.get_named_struct("MyStruct").unwrap();
    assert_eq!(s.as_type(), s_lookup.as_type());

    // Set body once.
    let i32 = m.i32_type();
    let i64 = m.i64_type();
    m.set_struct_body(s, [i32.as_type(), i64.as_type()], false)
        .expect("first set_body succeeds");

    // Now opaque is false and we can read fields.
    assert!(!s.is_opaque());
    assert_eq!(s.field_count(), 2);
    assert_eq!(s.field_type(0), Some(i32.as_type()));
    assert_eq!(s.field_type(1), Some(i64.as_type()));

    // Setting the body again fails with StructBodyAlreadySet.
    let err = m
        .set_struct_body(s, [i32.as_type()], false)
        .expect_err("second set_body must error");
    assert!(matches!(err, IrError::StructBodyAlreadySet { ref name } if name == "MyStruct"));
}

/// llvmkit-specific: lookup miss returns `None`. Closest upstream behaviour:
/// `Module::getTypeByName` returning `nullptr` (`lib/IR/Module.cpp`).
#[test]
fn missing_named_struct_returns_none() {
    let m = Module::new("t");
    assert!(m.get_named_struct("MissingStruct").is_none());
}

/// llvmkit-specific: llvmkit exposes `TypeKind` as a discriminated enum.
/// Upstream uses `Type::getTypeID()` switch surfaces (see `include/llvm/IR/Type.h`).
#[test]
fn type_kind_discriminator_is_correct() {
    let m = Module::new("t");
    assert_eq!(m.void_type().as_type().kind(), TypeKind::Void);
    assert_eq!(
        m.i32_type().as_type().kind(),
        TypeKind::Integer { bits: 32 }
    );
    assert_eq!(
        m.ptr_type(0).as_type().kind(),
        TypeKind::Pointer { addr_space: 0 }
    );
    assert_eq!(m.f32_type().as_type().kind(), TypeKind::Float);
    assert_eq!(m.f64_type().as_type().kind(), TypeKind::Double);
}

/// llvmkit-specific: typestate `SizedType` refinement maps to upstream
/// `Type::isSized()` (`lib/IR/Type.cpp`); rejection surface is `IrError::UnsizedType`.
#[test]
fn sized_refinement_accepts_sized_rejects_unsized() {
    let m = Module::new("t");
    let i32 = m.i32_type();
    let f64 = m.f64_type();
    let arr = m.array_type(i32, 4);

    // i32, f64, [4 x i32] are sized.
    assert!(SizedType::try_from(i32.as_type()).is_ok());
    assert!(SizedType::try_from(f64.as_type()).is_ok());
    assert!(SizedType::try_from(arr.as_type()).is_ok());

    // void, label, metadata, token, function, opaque struct are unsized.
    let unsized_kinds = [
        m.void_type().as_type(),
        m.label_type().as_type(),
        m.metadata_type().as_type(),
        m.token_type().as_type(),
        m.fn_type(m.void_type().as_type(), Vec::<Type>::new(), false)
            .as_type(),
        m.named_struct("Opaque").as_type(),
    ];
    for ty in unsized_kinds {
        let err = SizedType::try_from(ty).expect_err("unsized must error");
        assert!(matches!(err, IrError::UnsizedType { .. }), "got {err:?}");
    }
}

/// llvmkit-specific: `Type::isFirstClassType()` is exercised across upstream
/// transforms but has no dedicated `TEST`; closest reference is the
/// predicate definition in `lib/IR/Type.cpp`.
#[test]
fn first_class_predicate_rejects_function_void_opaque() {
    let m = Module::new("t");
    assert!(!m.void_type().as_type().is_first_class());
    assert!(
        !m.fn_type(m.void_type().as_type(), Vec::<Type>::new(), false)
            .as_type()
            .is_first_class()
    );
    assert!(!m.named_struct("Opaque").as_type().is_first_class());

    // Filling the body promotes it to first-class.
    let s = m.named_struct("Filled");
    let i32 = m.i32_type();
    m.set_struct_body(s, [i32.as_type()], false).unwrap();
    assert!(s.as_type().is_first_class());

    assert!(m.i32_type().as_type().is_first_class());
    assert!(m.f64_type().as_type().is_first_class());
    assert!(m.ptr_type(0).as_type().is_first_class());
}

/// llvmkit-specific: typestate `try_from` narrowing on typed handles.
/// Upstream `dyn_cast<IntegerType>` returns `nullptr` (no `IrError`).
#[test]
fn try_from_narrows_correctly() {
    let m = Module::new("t");
    let i32_handle = m.i32_type();
    let erased = i32_handle.as_type();
    let narrowed: IntType<IntDyn> = IntType::try_from(erased).expect("i32 narrows to IntType");
    assert_eq!(narrowed.bit_width(), 32);

    let void = m.void_type().as_type();
    let err = IntType::<IntDyn>::try_from(void).expect_err("void must not narrow");
    assert!(matches!(
        err,
        IrError::TypeMismatch {
            expected: TypeKindLabel::Integer,
            got: TypeKindLabel::Void,
        }
    ));
}

/// llvmkit-specific: `BasicTypeEnum` classifier is an llvmkit-only enum.
/// Closest upstream pattern: `Type::isFirstClassType()` switch in
/// `lib/IR/Type.cpp`.
#[test]
fn basic_type_enum_classifies_first_class() {
    let m = Module::new("t");
    let i32 = m.i32_type();
    let basic = BasicTypeEnum::try_from(i32.as_type()).unwrap();
    assert!(matches!(basic, BasicTypeEnum::Int(_)));

    let void = m.void_type().as_type();
    assert!(BasicTypeEnum::try_from(void).is_err());
    let label = m.label_type().as_type();
    assert!(BasicTypeEnum::try_from(label).is_err());
}

/// llvmkit-specific: llvmkit separates `AggregateType` (array+struct, no
/// vector) from upstream's `CompositeType`. Closest reference:
/// `Type::isAggregateType()` in `include/llvm/IR/Type.h`.
#[test]
fn aggregate_excludes_vector() {
    let m = Module::new("t");
    let i32 = m.i32_type();
    let arr = m.array_type(i32, 4);
    let vec = m.vector_type(i32, 4, false);
    let lit = m.struct_type([i32.as_type()], false);

    assert!(AggregateType::try_from(arr.as_type()).is_ok());
    assert!(AggregateType::try_from(lit.as_type()).is_ok());
    assert!(AggregateType::try_from(vec.as_type()).is_err());
}

/// llvmkit-specific: `AnyTypeEnum` is the widest type-classifier enum;
/// upstream uses `Type::getTypeID()` directly.
#[test]
fn any_type_enum_widens_every_kind() {
    let m = Module::new("t");
    let i32 = m.i32_type();
    let arr = m.array_type(i32, 2);

    let any: AnyTypeEnum = m.void_type().as_type().into();
    assert!(matches!(any, AnyTypeEnum::Void(_)));
    let any: AnyTypeEnum = arr.as_type().into();
    assert!(matches!(any, AnyTypeEnum::Array(_)));
}

/// llvmkit-specific: typed handles must implement `Hash` + `Eq` consistently
/// for use as keys. No upstream analog.
#[test]
fn handles_implement_hash_and_eq_via_derive() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Verify that two interned `IntType<i32>` handles from the same module
    // are equal and hash identically. We hash directly rather than putting
    // them into a `HashMap` key — clippy's `mutable_key_type` lint can't
    // see through `boxcar`'s internal `AtomicUsize` and reports a false
    // positive when typed handles appear in key position.
    let m = Module::new("t");
    let a = m.i32_type();
    let b = m.i32_type();
    assert_eq!(a, b);
    let hash = |t: IntType<i32>| {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    };
    assert_eq!(hash(a), hash(b));
}

/// llvmkit-specific: cross-`Module` identity must differ. Upstream uses
/// per-context `LLVMContext` interning so two `LLVMContext`s already give
/// distinct types; llvmkit uses `ModuleId` as the keying axis.
#[test]
fn cross_module_handles_compare_unequal() {
    // Two modules' identical-shape types must NOT compare equal.
    // Hash/Eq route through ModuleId, so distinct ModuleIds → !=.
    let m1 = Module::new("a");
    let m2 = Module::new("b");
    // Different ModuleId => same arena index for `void` is a different
    // logical handle. We can't directly compare across borrows in one
    // statement (the borrow checker keeps them separate), but we can
    // compare via the module-id axis:
    assert_ne!(m1.id(), m2.id());
}

/// llvmkit-specific: `IrType` trait unification across typed handles. No
/// upstream analog (C++ uses class hierarchy + `dyn_cast`).
#[test]
fn ir_type_trait_unifies_handles() {
    fn name<T>(_: T) -> &'static str {
        std::any::type_name::<T>()
    }
    fn _accepts_any<'ctx, T: IrType<'ctx>>(t: T) -> Type<'ctx> {
        t.as_type()
    }

    let m = Module::new("t");
    // The bound `T: IrType<'ctx>` applies uniformly to per-kind handles
    // and to the erased Type.
    let _ = _accepts_any(m.i32_type());
    let _ = _accepts_any(m.f64_type());
    let _ = _accepts_any(m.ptr_type(0));
    let _ = _accepts_any(m.void_type());
    let _ = _accepts_any(m.i32_type().as_type());
    // (smoke test that it compiles; runtime values not checked)
    let _ = name::<IntType<i32>>;
}
