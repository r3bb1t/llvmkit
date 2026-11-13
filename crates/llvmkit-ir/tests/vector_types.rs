//! Ports `unittests/IR/VectorTypesTest.cpp`, all six `TEST`s, in its order.
//!
//! `ElementCount` has no public llvmkit spelling, so [`element_count`] reads
//! the same two facts apart: `VectorType::min_len` plus
//! `VectorType::is_scalable`. `TypeSize` is `llvmkit_ir::TypeSize`, compared as
//! upstream compares it. `EXPECT_EQ` of a `TypeSize` against a number is
//! upstream's implicit `uint64_t`, which is `fixed_value`.
//!
//! `EXPECT_VTY_EQ` / `EXPECT_VTY_NE` are `assert_eq!` / `assert_ne!`. Their
//! null checks port upstream's `dyn_cast` to a vector kind: every `dyn_cast`
//! result here is checked by [`assert_fixed`] or [`assert_scalable`], because
//! the Rust handle is one type for both kinds.

use core::num::NonZeroU32;

use llvmkit_ir::{
    DataLayout, DynBrand, ElemDyn, LenDyn, Module, TypeKind, TypeSize, VectorType, lanes,
};

type Vector<'m> = VectorType<'m, ElemDyn, LenDyn, DynBrand>;

/// Upstream's `getElementCount()`.
fn element_count(vector: Vector<'_>) -> (u32, bool) {
    (vector.min_len().get(), vector.is_scalable())
}

/// `dyn_cast<FixedVectorType>` succeeded.
fn assert_fixed(vector: Vector<'_>) {
    assert_eq!(vector.as_type().kind(), TypeKind::FixedVector, "{vector}");
}

/// `dyn_cast<ScalableVectorType>` succeeded.
fn assert_scalable(vector: Vector<'_>) {
    assert_eq!(
        vector.as_type().kind(),
        TypeKind::ScalableVector,
        "{vector}"
    );
}

/// `ElementCount::divideCoefficientBy`.
fn divide(count: NonZeroU32, divisor: u32) -> NonZeroU32 {
    NonZeroU32::new(count.get() / divisor).expect("the quotient is a count")
}

/// `ElementCount::operator*`.
fn multiply(count: NonZeroU32, factor: NonZeroU32) -> NonZeroU32 {
    count.checked_mul(factor).expect("the product is a count")
}

fn fixed<'m>(
    m: &'m Module<DynBrand>,
    element: impl llvmkit_ir::IrType<'m, DynBrand>,
    count: NonZeroU32,
) -> Vector<'m> {
    m.vector_type(element, count).expect("a valid element")
}

fn scalable<'m>(
    m: &'m Module<DynBrand>,
    element: impl llvmkit_ir::IrType<'m, DynBrand>,
    count: NonZeroU32,
) -> Vector<'m> {
    m.scalable_vector_type(element, count)
        .expect("a valid element")
}

/// Port of `TEST(VectorTypesTest, FixedLength)`.
#[test]
fn upstream_fixed_length() {
    let m = Module::dynamic("VectorTypesTest");

    let int8_ty = m.i8_type();
    let int16_ty = m.i16_type();
    let int32_ty = m.i32_type();
    let int64_ty = m.i64_type();
    let float64_ty = m.f64_type();

    let v16_int8_ty = fixed(&m, int8_ty, lanes::<16>());
    assert_eq!(v16_int8_ty.min_len().get(), 16);
    assert_eq!(v16_int8_ty.element().scalar_size_in_bits(), 8);

    let v8_int32_ty = fixed(&m, int32_ty, lanes::<8>());
    assert_fixed(v8_int32_ty);
    assert_eq!(v8_int32_ty.min_len().get(), 8);
    assert_eq!(v8_int32_ty.element().scalar_size_in_bits(), 32);

    let v8_int8_ty = v8_int32_ty.with_element(int8_ty).expect("a valid element");
    assert_fixed(v8_int8_ty);
    assert_ne!(v8_int32_ty, v8_int8_ty);
    assert_eq!(element_count(v8_int8_ty), element_count(v8_int32_ty));
    assert_eq!(v8_int8_ty.element().scalar_size_in_bits(), 8);

    let v8_int32_ty2 = v8_int32_ty.with_element(int32_ty).expect("a valid element");
    assert_fixed(v8_int32_ty2);
    assert_eq!(v8_int32_ty, v8_int32_ty2);

    let v8_int16_ty = fixed(&m, int16_ty, lanes::<8>());
    assert_fixed(v8_int16_ty);
    assert_eq!(v8_int16_ty.min_len().get(), 8);
    assert_eq!(v8_int16_ty.element().scalar_size_in_bits(), 16);

    let elt_cnt = lanes::<4>();
    let v4_int64_ty = fixed(&m, int64_ty, elt_cnt);
    assert_fixed(v4_int64_ty);
    assert_eq!(v4_int64_ty.min_len().get(), 4);
    assert_eq!(v4_int64_ty.element().scalar_size_in_bits(), 64);

    let v2_int64_ty = fixed(&m, int64_ty, divide(elt_cnt, 2));
    assert_fixed(v2_int64_ty);
    assert_eq!(v2_int64_ty.min_len().get(), 2);
    assert_eq!(v2_int64_ty.element().scalar_size_in_bits(), 64);

    let v8_int64_ty = fixed(&m, int64_ty, multiply(elt_cnt, lanes::<2>()));
    assert_fixed(v8_int64_ty);
    assert_eq!(v8_int64_ty.min_len().get(), 8);
    assert_eq!(v8_int64_ty.element().scalar_size_in_bits(), 64);

    let v4_float64_ty = fixed(&m, float64_ty, elt_cnt);
    assert_fixed(v4_float64_ty);
    assert_eq!(v4_float64_ty.min_len().get(), 4);
    assert_eq!(v4_float64_ty.element().scalar_size_in_bits(), 64);

    let ext_ty = v8_int16_ty.extended_element().expect("an integer element");
    assert_fixed(ext_ty);
    assert_eq!(ext_ty, v8_int32_ty);
    assert_eq!(ext_ty.min_len().get(), 8);
    assert_eq!(ext_ty.element().scalar_size_in_bits(), 32);

    let trunc_ty = v8_int32_ty.truncated_element().expect("an even width");
    assert_fixed(trunc_ty);
    assert_eq!(trunc_ty, v8_int16_ty);
    assert_eq!(trunc_ty.min_len().get(), 8);
    assert_eq!(trunc_ty.element().scalar_size_in_bits(), 16);

    let halved_ty = v4_int64_ty.half_elements().expect("an even count");
    assert_fixed(halved_ty);
    assert_eq!(halved_ty, v2_int64_ty);
    assert_eq!(halved_ty.min_len().get(), 2);
    assert_eq!(halved_ty.element().scalar_size_in_bits(), 64);

    let doubled_ty = v4_int64_ty.double_elements().expect("the count fits");
    assert_fixed(doubled_ty);
    assert_eq!(doubled_ty, v8_int64_ty);
    assert_eq!(doubled_ty.min_len().get(), 8);
    assert_eq!(doubled_ty.element().scalar_size_in_bits(), 64);

    let conv_ty = v4_float64_ty.integer().expect("a sized element");
    assert_fixed(conv_ty);
    assert_eq!(conv_ty, v4_int64_ty);
    assert_eq!(conv_ty.min_len().get(), 4);
    assert_eq!(conv_ty.element().scalar_size_in_bits(), 64);

    let (known_min, is_scalable) = element_count(v8_int64_ty);
    assert_eq!(known_min, 8);
    assert!(!is_scalable);

    let sub_ty = v4_int64_ty.subdivided(2).expect("twice subdivisible");
    assert_eq!(element_count(sub_ty), (16, false));
    assert!(sub_ty.element().is_integer_of_width(16));
}

/// Port of `TEST(VectorTypesTest, Scalable)`.
#[test]
fn upstream_scalable() {
    let m = Module::dynamic("VectorTypesTest");

    let int8_ty = m.i8_type();
    let int16_ty = m.i16_type();
    let int32_ty = m.i32_type();
    let int64_ty = m.i64_type();
    let float64_ty = m.f64_type();

    let sc_v16_int8_ty = scalable(&m, int8_ty, lanes::<16>());
    assert_eq!(sc_v16_int8_ty.min_len().get(), 16);
    assert_eq!(sc_v16_int8_ty.as_type().scalar_size_in_bits(), 8);

    let sc_v8_int32_ty = scalable(&m, int32_ty, lanes::<8>());
    assert_scalable(sc_v8_int32_ty);
    assert_eq!(sc_v8_int32_ty.min_len().get(), 8);
    assert_eq!(sc_v8_int32_ty.element().scalar_size_in_bits(), 32);

    let sc_v8_int8_ty = sc_v8_int32_ty
        .with_element(int8_ty)
        .expect("a valid element");
    assert_scalable(sc_v8_int8_ty);
    assert_ne!(sc_v8_int32_ty, sc_v8_int8_ty);
    assert_eq!(element_count(sc_v8_int8_ty), element_count(sc_v8_int32_ty));
    assert_eq!(sc_v8_int8_ty.element().scalar_size_in_bits(), 8);

    let sc_v8_int32_ty2 = sc_v8_int32_ty
        .with_element(int32_ty)
        .expect("a valid element");
    assert_scalable(sc_v8_int32_ty2);
    assert_eq!(sc_v8_int32_ty, sc_v8_int32_ty2);

    let sc_v8_int16_ty = scalable(&m, int16_ty, lanes::<8>());
    assert_scalable(sc_v8_int16_ty);
    assert_eq!(sc_v8_int16_ty.min_len().get(), 8);
    assert_eq!(sc_v8_int16_ty.element().scalar_size_in_bits(), 16);

    let elt_cnt = lanes::<4>();
    let sc_v4_int64_ty = scalable(&m, int64_ty, elt_cnt);
    assert_scalable(sc_v4_int64_ty);
    assert_eq!(sc_v4_int64_ty.min_len().get(), 4);
    assert_eq!(sc_v4_int64_ty.element().scalar_size_in_bits(), 64);

    let sc_v2_int64_ty = scalable(&m, int64_ty, divide(elt_cnt, 2));
    assert_scalable(sc_v2_int64_ty);
    assert_eq!(sc_v2_int64_ty.min_len().get(), 2);
    assert_eq!(sc_v2_int64_ty.element().scalar_size_in_bits(), 64);

    let sc_v8_int64_ty = scalable(&m, int64_ty, multiply(elt_cnt, lanes::<2>()));
    assert_scalable(sc_v8_int64_ty);
    assert_eq!(sc_v8_int64_ty.min_len().get(), 8);
    assert_eq!(sc_v8_int64_ty.element().scalar_size_in_bits(), 64);

    let sc_v4_float64_ty = scalable(&m, float64_ty, elt_cnt);
    assert_scalable(sc_v4_float64_ty);
    assert_eq!(sc_v4_float64_ty.min_len().get(), 4);
    assert_eq!(sc_v4_float64_ty.element().scalar_size_in_bits(), 64);

    let ext_ty = sc_v8_int16_ty
        .extended_element()
        .expect("an integer element");
    assert_scalable(ext_ty);
    assert_eq!(ext_ty, sc_v8_int32_ty);
    assert_eq!(ext_ty.min_len().get(), 8);
    assert_eq!(ext_ty.element().scalar_size_in_bits(), 32);

    let trunc_ty = sc_v8_int32_ty.truncated_element().expect("an even width");
    assert_scalable(trunc_ty);
    assert_eq!(trunc_ty, sc_v8_int16_ty);
    assert_eq!(trunc_ty.min_len().get(), 8);
    assert_eq!(trunc_ty.element().scalar_size_in_bits(), 16);

    let halved_ty = sc_v4_int64_ty.half_elements().expect("an even count");
    assert_scalable(halved_ty);
    assert_eq!(halved_ty, sc_v2_int64_ty);
    assert_eq!(halved_ty.min_len().get(), 2);
    assert_eq!(halved_ty.element().scalar_size_in_bits(), 64);

    let doubled_ty = sc_v4_int64_ty.double_elements().expect("the count fits");
    assert_scalable(doubled_ty);
    assert_eq!(doubled_ty, sc_v8_int64_ty);
    assert_eq!(doubled_ty.min_len().get(), 8);
    assert_eq!(doubled_ty.element().scalar_size_in_bits(), 64);

    let conv_ty = sc_v4_float64_ty.integer().expect("a sized element");
    assert_scalable(conv_ty);
    assert_eq!(conv_ty, sc_v4_int64_ty);
    assert_eq!(conv_ty.min_len().get(), 4);
    assert_eq!(conv_ty.element().scalar_size_in_bits(), 64);

    let (known_min, is_scalable) = element_count(sc_v8_int64_ty);
    assert_eq!(known_min, 8);
    assert!(is_scalable);
}

/// Port of `TEST(VectorTypesTest, BaseVectorType)`.
///
/// `getTypeID()` is `Type::kind`: it answers `FixedVector` or
/// `ScalableVector`, as `Type::FixedVectorTyID` and
/// `Type::ScalableVectorTyID` do.
#[test]
fn upstream_base_vector_type() {
    let m = Module::dynamic("VectorTypesTest");

    let int16_ty = m.i16_type();
    let int32_ty = m.i32_type();

    let v_tys = [
        scalable(&m, int16_ty, lanes::<4>()),
        fixed(&m, int16_ty, lanes::<4>()),
        scalable(&m, int16_ty, lanes::<2>()),
        fixed(&m, int16_ty, lanes::<2>()),
        scalable(&m, int32_ty, lanes::<4>()),
        fixed(&m, int32_ty, lanes::<4>()),
        scalable(&m, int32_ty, lanes::<2>()),
        fixed(&m, int32_ty, lanes::<2>()),
    ];

    // The comparison matrix is symmetric, so we only check the upper triangle.
    for (i, &vi) in v_tys.iter().enumerate() {
        // test I == J
        let (_, is_scalable) = element_count(vi);
        assert_eq!(
            vi.as_type().kind() == TypeKind::ScalableVector,
            is_scalable,
            "{vi}"
        );

        for &vj in &v_tys[i + 1..] {
            // test I < J
            assert_ne!(vi, vj);

            let vj_prime = vj.with_element(vi.element()).expect("a valid element");
            if vi.element() == vj.element() {
                assert_eq!(vj, vj_prime);
            } else {
                assert_ne!(vj, vj_prime);
            }

            assert_eq!(
                vj.as_type().kind(),
                vj_prime.as_type().kind(),
                "VJ and VJPrime are the same sort of vector"
            );
        }
    }
}

/// Port of `TEST(VectorTypesTest, FixedLenComparisons)`.
#[test]
fn upstream_fixed_len_comparisons() {
    let m = Module::dynamic("VectorTypesTest");
    let dl = DataLayout::default();

    let int32_ty = m.i32_type();
    let int64_ty = m.i64_type();

    let v2_int32_ty = fixed(&m, int32_ty, lanes::<2>());
    let v4_int32_ty = fixed(&m, int32_ty, lanes::<4>());

    let v2_int64_ty = fixed(&m, int64_ty, lanes::<2>());

    let (v2_i32, v4_i32, v2_i64) = (
        v2_int32_ty.as_type(),
        v4_int32_ty.as_type(),
        v2_int64_ty.as_type(),
    );

    let v2_i32_len = v2_i32.primitive_size_in_bits();
    assert_eq!(v2_i32_len.known_min_value(), 64);
    assert!(!v2_i32_len.is_scalable());

    let fixed_value = |size: TypeSize| size.fixed_value().expect("a fixed vector's size");
    assert!(
        fixed_value(v2_i32.primitive_size_in_bits()) < fixed_value(v4_i32.primitive_size_in_bits())
    );
    assert!(
        fixed_value(v2_i64.primitive_size_in_bits()) > fixed_value(v2_i32.primitive_size_in_bits())
    );
    assert_eq!(
        v4_i32.primitive_size_in_bits(),
        v2_i64.primitive_size_in_bits()
    );
    assert_ne!(
        v2_i32.primitive_size_in_bits(),
        v2_i64.primitive_size_in_bits()
    );

    // Check that a fixed-only comparison works for fixed size vectors.
    assert_eq!(
        fixed_value(v2_i64.primitive_size_in_bits()),
        fixed_value(v4_i32.primitive_size_in_bits())
    );

    // Check the DataLayout interfaces.
    assert_eq!(dl.type_size_in_bits(v2_i64), dl.type_size_in_bits(v4_i32));
    assert_eq!(dl.type_size_in_bits(v2_i32).fixed_value(), Some(64));
    assert_eq!(dl.type_size_in_bits(v2_i64).fixed_value(), Some(128));
    assert_eq!(dl.type_store_size(v2_i64), dl.type_store_size(v4_i32));
    assert_ne!(
        dl.type_store_size_in_bits(v2_i32),
        dl.type_store_size_in_bits(v2_i64)
    );
    assert_eq!(dl.type_store_size_in_bits(v2_i32).fixed_value(), Some(64));
    assert_eq!(dl.type_store_size(v2_i64).fixed_value(), Some(16));
    assert_eq!(dl.type_alloc_size(v4_i32), dl.type_alloc_size(v2_i64));
    assert_ne!(
        dl.type_alloc_size_in_bits(v2_i32),
        dl.type_alloc_size_in_bits(v2_i64)
    );
    assert_eq!(dl.type_alloc_size_in_bits(v4_i32).fixed_value(), Some(128));
    assert_eq!(dl.type_alloc_size(v2_i32).fixed_value(), Some(8));
    assert!(dl.type_size_equals_store_size(v4_i32));
}

/// Port of `TEST(VectorTypesTest, ScalableComparisons)`.
#[test]
fn upstream_scalable_comparisons() {
    let m = Module::dynamic("VectorTypesTest");
    let dl = DataLayout::default();

    let int32_ty = m.i32_type();
    let int64_ty = m.i64_type();

    let sc_v2_int32_ty = scalable(&m, int32_ty, lanes::<2>());
    let sc_v4_int32_ty = scalable(&m, int32_ty, lanes::<4>());

    let sc_v2_int64_ty = scalable(&m, int64_ty, lanes::<2>());

    let (v2_i32, v4_i32, v2_i64) = (
        sc_v2_int32_ty.as_type(),
        sc_v4_int32_ty.as_type(),
        sc_v2_int64_ty.as_type(),
    );

    let sc_v2_i32_len = v2_i32.primitive_size_in_bits();
    assert_eq!(sc_v2_i32_len.known_min_value(), 64);
    assert!(sc_v2_i32_len.is_scalable());

    let v2_i32_min = v2_i32.primitive_size_in_bits().known_min_value();
    let v4_i32_min = v4_i32.primitive_size_in_bits().known_min_value();
    let v2_i64_min = v2_i64.primitive_size_in_bits().known_min_value();
    assert!(v2_i32_min < v4_i32_min);
    assert!(v2_i64_min > v2_i32_min);
    assert_eq!(v4_i32_min, v2_i64_min);
    assert_ne!(v2_i32_min, v2_i64_min);

    // Check the DataLayout interfaces.
    assert_eq!(dl.type_size_in_bits(v2_i64), dl.type_size_in_bits(v4_i32));
    assert_eq!(dl.type_size_in_bits(v2_i32).known_min_value(), 64);
    assert_eq!(dl.type_store_size(v2_i64), dl.type_store_size(v4_i32));
    assert_ne!(
        dl.type_store_size_in_bits(v2_i32),
        dl.type_store_size_in_bits(v2_i64)
    );
    assert_eq!(dl.type_store_size_in_bits(v2_i32).known_min_value(), 64);
    assert_eq!(dl.type_store_size(v2_i64).known_min_value(), 16);
    assert_eq!(dl.type_alloc_size(v4_i32), dl.type_alloc_size(v2_i64));
    assert_ne!(
        dl.type_alloc_size_in_bits(v2_i32),
        dl.type_alloc_size_in_bits(v2_i64)
    );
    assert_eq!(dl.type_alloc_size_in_bits(v4_i32).known_min_value(), 128);
    assert_eq!(dl.type_alloc_size(v2_i32).known_min_value(), 8);
    assert!(dl.type_size_equals_store_size(v4_i32));
}

/// Port of `TEST(VectorTypesTest, CrossComparisons)`.
#[test]
fn upstream_cross_comparisons() {
    let m = Module::dynamic("VectorTypesTest");

    let int32_ty = m.i32_type();

    let v4_int32_ty = fixed(&m, int32_ty, lanes::<4>());
    let sc_v4_int32_ty = scalable(&m, int32_ty, lanes::<4>());

    // Even though the minimum size is the same, a scalable vector could be
    // larger so we don't consider them to be the same size.
    assert_ne!(
        v4_int32_ty.as_type().primitive_size_in_bits(),
        sc_v4_int32_ty.as_type().primitive_size_in_bits()
    );
    // If we are only checking the minimum, then they are the same size.
    assert_eq!(
        v4_int32_ty
            .as_type()
            .primitive_size_in_bits()
            .known_min_value(),
        sc_v4_int32_ty
            .as_type()
            .primitive_size_in_bits()
            .known_min_value()
    );

    // We can't use ordering comparisons (<,<=,>,>=) between scalable and
    // non-scalable vector sizes.
}

/// Each derived vector refuses what its upstream routine asserts on, with
/// `None`; `with_element` refuses an invalid element through the constructor it
/// calls. Rule anchor: `VectorType::getInteger`,
/// `getExtendedElementVectorType`, `getTruncatedElementVectorType`,
/// `getHalfElementsVectorType` and `getDoubleElementsVectorType`
/// (`include/llvm/IR/DerivedTypes.h`) only `assert` these, so no upstream test
/// reaches them.
#[test]
fn a_derived_vector_refuses_what_its_routine_asserts_on() {
    let m = Module::dynamic("VectorTypesTest");

    let pointers: Vector<'_> = fixed(&m, m.ptr_type_in::<0>(), lanes::<2>());
    assert_eq!(pointers.integer(), None, "pointers have no primitive size");

    let floats = fixed(&m, m.f64_type(), lanes::<2>());
    assert_eq!(floats.extended_element(), None, "not an integer element");

    let odd_width = fixed(&m, m.custom_width_int_type(3).expect("i3"), lanes::<2>());
    assert_eq!(odd_width.truncated_element(), None, "odd element width");
    assert_eq!(pointers.truncated_element(), None, "zero element width");
    let half = fixed(&m, m.half_type(), lanes::<2>());
    assert_eq!(half.truncated_element(), None, "no narrower float");

    let widest = fixed(
        &m,
        m.custom_width_int_type(llvmkit_ir::MAX_INT_BITS)
            .expect("the widest integer"),
        lanes::<2>(),
    );
    assert_eq!(widest.extended_element(), None, "past MAX_INT_BITS");

    let odd_count = fixed(&m, m.i32_type(), lanes::<3>());
    assert_eq!(odd_count.half_elements(), None, "odd element count");
    let one = fixed(&m, m.i32_type(), lanes::<1>());
    assert_eq!(one.half_elements(), None, "no vector has zero elements");

    let most = fixed(&m, m.i32_type(), NonZeroU32::MAX);
    assert_eq!(most.double_elements(), None, "the count overflows");
    assert_eq!(most.subdivided(1), None, "the count overflows");
    assert_eq!(most.subdivided(0), Some(most), "no subdivision");

    assert!(most.with_element(m.void_type()).is_err());
}
