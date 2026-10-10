//! `Constant::kind`: which of upstream's constant classes a constant is.
//!
//! llvmkit stores every element list — `zeroinitializer` included — in one
//! form, so the class of an array, struct or vector constant is recovered by
//! replaying the canonicalisation upstream applies when it builds one
//! (`ConstantArray::getImpl`, `ConstantStruct::get`, `ConstantVector::getImpl`,
//! `ConstantVector::getSplat`). The upstream ports come first; the rule
//! anchors after them pin the branches no upstream test isolates.

use llvmkit_ir::{
    ApFloat, ApFloatSign, Constant, ConstantAggregateKind, ConstantDataKind, ConstantExprOpcode,
    ConstantKind, ConstantSequentialKind, DynBrand, IrResult, Linkage, Module, Undefined,
};

/// The class of a two-element fixed vector of `elements`.
fn vector_kind<'m>(
    m: &'m Module<DynBrand>,
    elements: [Constant<'m, DynBrand>; 2],
) -> IrResult<ConstantKind<'m, DynBrand>> {
    let ty = m.vector_type(elements[0].ty(), 2);
    Ok(ty.const_vector(elements)?.as_constant().kind())
}

/// The class of a two-element array of `elements`.
fn array_kind<'m>(
    m: &'m Module<DynBrand>,
    elements: [Constant<'m, DynBrand>; 2],
) -> IrResult<ConstantKind<'m, DynBrand>> {
    let ty = m.array_type(elements[0].ty(), 2);
    Ok(ty.const_array(elements)?.as_constant().kind())
}

/// `ConstantInt::get(T, 0)` and `ConstantInt::get(T, 1)` for the four integer
/// types upstream's `BuildConstantData*` tests loop over.
fn integer_zero_and_one(m: &Module<DynBrand>) -> [[Constant<'_, DynBrand>; 2]; 4] {
    [
        [
            m.i8_type().const_int(0_i8).as_constant(),
            m.i8_type().const_int(1_i8).as_constant(),
        ],
        [
            m.i16_type().const_int(0_i16).as_constant(),
            m.i16_type().const_int(1_i16).as_constant(),
        ],
        [
            m.i32_type().const_int(0_i32).as_constant(),
            m.i32_type().const_int(1_i32).as_constant(),
        ],
        [
            m.i64_type().const_int(0_i64).as_constant(),
            m.i64_type().const_int(1_i64).as_constant(),
        ],
    ]
}

/// `ConstantFP::get(T, 0)` and `ConstantFP::get(T, 1)` for `half`, `bfloat`,
/// `float` and `double` — upstream's `APFloat::getZero` / `getOne` at each
/// type's semantics.
fn float_zero_and_one(m: &Module<DynBrand>) -> IrResult<[[Constant<'_, DynBrand>; 2]; 4]> {
    let half = m.half_type();
    let bfloat = m.bfloat_type();
    let float = m.f32_type();
    let double = m.f64_type();
    Ok([
        [
            half.const_ap_float(&ApFloat::zero(half.semantics(), ApFloatSign::Positive))?
                .as_constant(),
            half.const_ap_float(&ApFloat::one(half.semantics(), ApFloatSign::Positive))?
                .as_constant(),
        ],
        [
            bfloat
                .const_ap_float(&ApFloat::zero(bfloat.semantics(), ApFloatSign::Positive))?
                .as_constant(),
            bfloat
                .const_ap_float(&ApFloat::one(bfloat.semantics(), ApFloatSign::Positive))?
                .as_constant(),
        ],
        [
            float
                .const_ap_float(&ApFloat::zero(float.semantics(), ApFloatSign::Positive))?
                .as_constant(),
            float
                .const_ap_float(&ApFloat::one(float.semantics(), ApFloatSign::Positive))?
                .as_constant(),
        ],
        [
            double
                .const_ap_float(&ApFloat::zero(double.semantics(), ApFloatSign::Positive))?
                .as_constant(),
            double
                .const_ap_float(&ApFloat::one(double.semantics(), ApFloatSign::Positive))?
                .as_constant(),
        ],
    ])
}

/// Ports `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest,
/// BuildConstantDataArrays)`'s class assertions: `ConstantArray::get` of
/// `ConstantInt`s, or of `ConstantFP`s, of a compatible element type is a
/// `ConstantDataArray`.
///
/// **Blocked half, by name:** each loop's second assertion round-trips the
/// array through `ConstantDataArray::getRaw(getRawDataValues(), …)` and
/// checks it is the same uniqued constant. llvmkit has no raw-data
/// constructor or accessor (`rg -n "raw_data|get_raw" crates/llvmkit-ir/src`
/// finds nothing on 2026-10-10), so that assertion is not ported;
/// `docs/future-work.md` records the gap.
#[test]
fn build_constant_data_arrays() -> IrResult<()> {
    let m = Module::dynamic("constants");
    for elements in integer_zero_and_one(&m) {
        let ty = elements[0].ty();
        assert!(
            matches!(
                array_kind(&m, elements)?,
                ConstantKind::Data(ConstantDataKind::Sequential(ConstantSequentialKind::Array))
            ),
            "T = {ty}"
        );
    }
    for elements in float_zero_and_one(&m)? {
        let ty = elements[0].ty();
        assert!(
            matches!(
                array_kind(&m, elements)?,
                ConstantKind::Data(ConstantDataKind::Sequential(ConstantSequentialKind::Array))
            ),
            "T = {ty}"
        );
    }
    Ok(())
}

/// Ports `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest,
/// BuildConstantDataVectors)`'s class assertions: `ConstantVector::get` of
/// `ConstantInt`s, or of `ConstantFP`s, of a compatible element type is a
/// `ConstantDataVector`.
///
/// **Blocked half, by name:** the `ConstantDataVector::getRaw` round trip, for
/// the reason [`build_constant_data_arrays`] gives.
#[test]
fn build_constant_data_vectors() -> IrResult<()> {
    let m = Module::dynamic("constants");
    for elements in integer_zero_and_one(&m) {
        let ty = elements[0].ty();
        assert!(
            matches!(
                vector_kind(&m, elements)?,
                ConstantKind::Data(ConstantDataKind::Sequential(ConstantSequentialKind::Vector))
            ),
            "T = {ty}"
        );
    }
    for elements in float_zero_and_one(&m)? {
        let ty = elements[0].ty();
        assert!(
            matches!(
                vector_kind(&m, elements)?,
                ConstantKind::Data(ConstantDataKind::Sequential(ConstantSequentialKind::Vector))
            ),
            "T = {ty}"
        );
    }
    Ok(())
}

/// An element list whose elements are all null is a `ConstantAggregateZero`,
/// at every aggregate shape: the `isZero` arms of `ConstantVector::getImpl`
/// and `ConstantStruct::get`, the `isNullValue` arm of
/// `ConstantArray::getImpl`, and its "empty arrays are canonicalized to
/// ConstantAggregateZero" head. llvmkit-specific rule anchor (D11): no
/// upstream test isolates these branches.
#[test]
fn an_all_null_element_list_is_a_constant_aggregate_zero() -> IrResult<()> {
    let m = Module::dynamic("constants");
    let zero = m.i32_type().const_int(0_i32).as_constant();
    let aggregate_zero = |kind: ConstantKind<'_, DynBrand>| {
        matches!(kind, ConstantKind::Data(ConstantDataKind::AggregateZero))
    };
    assert!(aggregate_zero(vector_kind(&m, [zero, zero])?));
    assert!(aggregate_zero(array_kind(&m, [zero, zero])?));
    let pair = m.struct_type([m.i32_type(), m.i32_type()]);
    assert!(aggregate_zero(
        pair.const_struct([zero, zero])?.as_constant().kind()
    ));
    let empty = m.array_type(m.i32_type(), 0);
    assert!(aggregate_zero(
        empty
            .const_array(Vec::<Constant<'_, DynBrand>>::new())?
            .as_constant()
            .kind()
    ));
    Ok(())
}

/// An element list of one `poison`, or one `undef`, repeated is that
/// `UndefValue`: the `isPoison` / `isUndef` arms of all three constructors.
/// A list mixing the two is neither, and since its first element is no
/// `ConstantInt`, `getSequenceIfElementsMatch` refuses it too. llvmkit-specific
/// rule anchor (D11).
#[test]
fn an_all_poison_or_all_undef_element_list_is_that_undef_value() -> IrResult<()> {
    let m = Module::dynamic("constants");
    let i32_ty = m.i32_type().as_type();
    let poison = i32_ty.poison().as_constant();
    let undef = i32_ty.undef().as_constant();
    let pair = m.struct_type([m.i32_type(), m.i32_type()]);
    let poison_value = |kind: ConstantKind<'_, DynBrand>| {
        matches!(
            kind,
            ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison))
        )
    };
    let undef_value = |kind: ConstantKind<'_, DynBrand>| {
        matches!(
            kind,
            ConstantKind::Data(ConstantDataKind::Undef(Undefined::Undef))
        )
    };

    assert!(poison_value(vector_kind(&m, [poison, poison])?));
    assert!(poison_value(array_kind(&m, [poison, poison])?));
    assert!(poison_value(
        pair.const_struct([poison, poison])?.as_constant().kind()
    ));

    assert!(undef_value(vector_kind(&m, [undef, undef])?));
    assert!(undef_value(array_kind(&m, [undef, undef])?));
    assert!(undef_value(
        pair.const_struct([undef, undef])?.as_constant().kind()
    ));

    assert!(matches!(
        vector_kind(&m, [poison, undef])?,
        ConstantKind::Aggregate(ConstantAggregateKind::Vector)
    ));
    assert!(matches!(
        array_kind(&m, [poison, undef])?,
        ConstantKind::Aggregate(ConstantAggregateKind::Array)
    ));
    assert!(matches!(
        pair.const_struct([poison, undef])?.as_constant().kind(),
        ConstantKind::Aggregate(ConstantAggregateKind::Struct)
    ));
    Ok(())
}

/// A list that is not "all simple" stays a `ConstantAggregate`: an element
/// type `isElementTypeCompatible` refuses (`i128`, `ptr`), and a compatible
/// one whose elements are not all `ConstantInt`s (`getIntSequenceIfElementsMatch`
/// returns `nullptr`). A struct is never sequential. llvmkit-specific rule
/// anchor (D11).
#[test]
fn a_list_that_is_not_all_simple_is_a_constant_aggregate() -> IrResult<()> {
    let m = Module::dynamic("constants");
    let wide = [
        m.i128_type().const_int(1_i128).as_constant(),
        m.i128_type().const_int(2_i128).as_constant(),
    ];
    assert!(matches!(
        vector_kind(&m, wide)?,
        ConstantKind::Aggregate(ConstantAggregateKind::Vector)
    ));
    assert!(matches!(
        array_kind(&m, wide)?,
        ConstantKind::Aggregate(ConstantAggregateKind::Array)
    ));

    let g = m.add_global("g", m.i32_type().const_int(0_i32))?;
    let pointers = [
        m.ptr_type(0).const_null().as_constant(),
        m.view(g).as_constant(),
    ];
    assert!(matches!(
        vector_kind(&m, pointers)?,
        ConstantKind::Aggregate(ConstantAggregateKind::Vector)
    ));

    let one = m.i32_type().const_int(1_i32).as_constant();
    let undef = m.i32_type().as_type().undef().as_constant();
    assert!(matches!(
        vector_kind(&m, [one, undef])?,
        ConstantKind::Aggregate(ConstantAggregateKind::Vector)
    ));

    let pair = m.struct_type([m.i32_type(), m.i32_type()]);
    assert!(matches!(
        pair.const_struct([one, one])?.as_constant().kind(),
        ConstantKind::Aggregate(ConstantAggregateKind::Struct)
    ));
    Ok(())
}

/// A scalable vector constant is classified as `ConstantVector::getSplat`
/// builds it — the only constructor upstream has for one: a null splat is a
/// `ConstantAggregateZero`, a `poison` splat a `PoisonValue`, and any other
/// splat the `shufflevector` constant expression `getSplat` returns.
/// llvmkit-specific rule anchor (D11).
#[test]
fn a_scalable_vector_is_classified_as_get_splat_builds_it() -> IrResult<()> {
    let m = Module::dynamic("constants");
    let scalable = m.scalable_vector_type(m.i32_type(), 2);
    let seven = m.i32_type().const_int(7_i32).as_constant();
    let zero = m.i32_type().const_int(0_i32).as_constant();
    let poison = m.i32_type().as_type().poison().as_constant();
    assert!(matches!(
        scalable.const_vector([seven, seven])?.as_constant().kind(),
        ConstantKind::Expr(ConstantExprOpcode::ShuffleVector)
    ));
    assert!(matches!(
        scalable.const_vector([zero, zero])?.as_constant().kind(),
        ConstantKind::Data(ConstantDataKind::AggregateZero)
    ));
    assert!(matches!(
        scalable
            .const_vector([poison, poison])?
            .as_constant()
            .kind(),
        ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison))
    ));
    Ok(())
}

/// Each scalar storage form names its class, and an integer or float carries
/// its typed handle. llvmkit-specific rule anchor (D11) for `Value.def`'s
/// `ConstantData` members.
#[test]
fn each_scalar_form_names_its_class() -> IrResult<()> {
    let m = Module::dynamic("constants");
    let ConstantKind::Data(ConstantDataKind::Int(int)) =
        m.i32_type().const_int(42_i32).as_constant().kind()
    else {
        panic!("isa<ConstantInt>")
    };
    assert_eq!(int.value_zext_u128(), Some(42));
    let double = m.f64_type();
    let ConstantKind::Data(ConstantDataKind::Float(float)) =
        double.const_double(3.5).as_constant().kind()
    else {
        panic!("isa<ConstantFP>")
    };
    assert!(float.ap_float().is_exactly_value_f64(3.5));
    assert!(matches!(
        m.ptr_type(0).const_null().as_constant().kind(),
        ConstantKind::Data(ConstantDataKind::PointerNull)
    ));
    assert!(matches!(
        m.token_none().kind(),
        ConstantKind::Data(ConstantDataKind::TokenNone)
    ));
    let i32_ty = m.i32_type().as_type();
    assert!(matches!(
        i32_ty.undef().as_constant().kind(),
        ConstantKind::Data(ConstantDataKind::Undef(Undefined::Undef))
    ));
    assert!(matches!(
        i32_ty.poison().as_constant().kind(),
        ConstantKind::Data(ConstantDataKind::Undef(Undefined::Poison))
    ));
    Ok(())
}

/// A global's own handle, widened with `as_constant`, is a `GlobalValue` —
/// upstream's `GlobalValue` is a `Constant`. Regression: `is_constant_data`
/// assumed a `Constant` only ever wraps a constant-kind slot and reached
/// `unreachable!` on a global's. llvmkit-specific (D11).
#[test]
fn a_global_widened_to_a_constant_is_a_global_value() -> IrResult<()> {
    let m = Module::dynamic("constants");
    let g = m.add_global("g", m.i32_type().const_int(0_i32))?;
    let as_constant = m.view(g).as_constant();
    assert!(matches!(as_constant.kind(), ConstantKind::GlobalValue));
    assert!(!as_constant.is_constant_data());
    let f = m.add_function_dyn(
        "f",
        m.function_type_no_parameters(m.void_type()),
        Linkage::External,
    )?;
    let address = m.view(f).as_global_constant_ptr();
    assert!(matches!(address.kind(), ConstantKind::GlobalValue));
    Ok(())
}

/// llvmkit's forward-reference placeholder is its own class: upstream's
/// placeholder is a `GlobalVariable`, llvmkit's a bare constant with no global
/// behind it (`docs/divergences.md`). Neither is `ConstantData`.
/// llvmkit-specific (D11).
#[test]
fn a_forward_reference_placeholder_is_its_own_kind() -> IrResult<()> {
    let m = Module::dynamic("constants");
    let placeholder = m.forward_ref_value_placeholder(m.ptr_type(0).as_type())?;
    assert!(matches!(
        placeholder.as_constant().kind(),
        ConstantKind::ForwardReference
    ));
    assert!(!placeholder.as_constant().is_constant_data());
    Ok(())
}

/// `isa<ConstantData>` — `is_constant_data`, and `Value::has_use_list`, its
/// negation — follows the class. Regression: `is_constant_data` counted an
/// element list as `ConstantData` only when every element was null, so a
/// `ConstantDataVector` and an all-`undef` list answered `false` where
/// upstream answers `true`. `Value.def` lists `ConstantDataArray`,
/// `ConstantDataVector` and `UndefValue` in the `ConstantData` range.
/// llvmkit-specific (D11).
#[test]
fn constant_data_membership_follows_the_class() -> IrResult<()> {
    let m = Module::dynamic("constants");
    let zero = m.i32_type().const_int(0_i32).as_constant();
    let one = m.i32_type().const_int(1_i32).as_constant();
    let undef = m.i32_type().as_type().undef().as_constant();

    let data_vector = m
        .vector_type(m.i32_type(), 2)
        .const_vector([zero, one])?
        .as_constant();
    assert!(data_vector.is_constant_data());
    assert!(!data_vector.as_erased().has_use_list());

    let undef_vector = m
        .vector_type(m.i32_type(), 2)
        .const_vector([undef, undef])?
        .as_constant();
    assert!(undef_vector.is_constant_data());

    let aggregate = m
        .vector_type(m.i32_type(), 2)
        .const_vector([one, undef])?
        .as_constant();
    assert!(!aggregate.is_constant_data());
    assert!(aggregate.as_erased().has_use_list());
    Ok(())
}
