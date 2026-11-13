//! `ConstantAggregateZero`: the null value of an aggregate is one constant,
//! whatever its length, and every element list upstream's constructors turn
//! into a uniqued constant is that constant here too.
//!
//! The canonicalisations are `ConstantArray::getImpl`, `ConstantStruct::get`,
//! `ConstantVector::getImpl` and `ConstantVector::getSplat`, and the
//! `AllSame` arms of `ConstantArray::handleOperandChangeImpl` and
//! `ConstantStruct::handleOperandChangeImpl`. `Type::null_value` is
//! `Constant::getNullValue`, and `Constant::aggregate_element` answers a zero
//! as `ConstantAggregateZero::getElementValue` does.

use llvmkit_ir::{
    ArrayType, Constant, ConstantAggregateKind, ConstantDataKind, ConstantKind, DynBrand, IrResult,
    Module, Type, VectorType, constant_fold_extract_value_instruction, lanes,
    possibly_demanded_elements_in_mask,
};

/// `ty`'s null value, which every type these tests ask about has.
fn zero_of<'m>(ty: Type<'m, DynBrand>) -> Constant<'m, DynBrand> {
    ty.null_value()
        .unwrap_or_else(|| panic!("{ty} has a null value"))
}

/// The class of `ty`'s null value, after checking it is a null value of `ty`.
fn null_kind_of<'m>(ty: Type<'m, DynBrand>) -> ConstantKind<'m, DynBrand> {
    let null = zero_of(ty);
    assert_eq!(null.ty(), ty);
    assert!(null.is_null_value(), "{ty}");
    null.kind()
}

/// The initializer of a global `name` built by `build` around one `i32`
/// placeholder, read after the placeholder is replaced by `replacement`.
fn initializer_after_replacing<'m>(
    m: &'m Module<DynBrand>,
    name: &str,
    build: impl FnOnce(Constant<'m, DynBrand>) -> IrResult<Constant<'m, DynBrand>>,
    replacement: Constant<'m, DynBrand>,
) -> IrResult<Constant<'m, DynBrand>> {
    let placeholder = m.forward_ref_value_placeholder(m.i32_type().as_type())?;
    let global = m.add_global(name, build(placeholder.as_constant())?)?;
    placeholder.replace_all_uses_with(replacement.as_erased())?;
    Ok(m.view(global).initializer().expect("an initializer"))
}

/// llvmkit-specific, no upstream counterpart: the element count of a zero
/// aggregate costs nothing, as upstream's `ConstantAggregateZero` costs nothing.
#[test]
fn a_zero_aggregate_is_one_constant() {
    let m = Module::dynamic("zero");
    let ty = ArrayType::of(m.i8_type(), 1 << 40).as_type();
    let zero = ty.null_value().expect("an array has a null value");
    assert!(matches!(
        zero.kind(),
        ConstantKind::Data(ConstantDataKind::AggregateZero)
    ));
    assert!(zero.is_null_value());
    // `getAggregateElement`'s bound is `getElementCount()`, an `unsigned`: the
    // 2^40 length keeps its low 32 bits, which are 0, so upstream has no
    // element 7 to give. `an_element_count_keeps_its_low_32_bits` pins a
    // length whose narrowed count still covers it.
    assert_eq!(zero.aggregate_element(7), None);
    assert_eq!(zero.to_string(), "[1099511627776 x i8] zeroinitializer");
}

/// Pins the `unsigned` counts `Constant::getAggregateElement` bounds an index
/// with: `ConstantAggregateZero::getElementCount()` (an `ElementCount`,
/// `FixedOrScalableQuantity<ElementCount, unsigned>`) and
/// `UndefValue::getNumElements()`, which returns `unsigned`. Each converts the
/// `uint64_t` `ArrayType::getNumElements()`, keeping its low 32 bits, so
/// `[4294967304 x i8]` (2^32 + 8) has elements 0 to 7 and no element 8, and
/// `[1099511627776 x i8]` (2^40) has none. llvmkit-specific (D11): no upstream
/// test isolates the narrowing.
#[test]
fn an_element_count_keeps_its_low_32_bits() {
    let m = Module::dynamic("narrowing");
    let i8_ty = m.i8_type();
    let past = ArrayType::of(i8_ty, (1 << 32) + 8).as_type();
    let none_left = ArrayType::of(i8_ty, 1 << 40).as_type();

    let zero = zero_of(past);
    assert_eq!(
        zero.aggregate_element(7),
        Some(i8_ty.const_zero().as_constant())
    );
    assert_eq!(zero.aggregate_element(8), None);

    let undef_element = i8_ty.as_type().undef().as_constant();
    assert_eq!(
        past.undef().as_constant().aggregate_element(7),
        Some(undef_element)
    );
    assert_eq!(past.undef().as_constant().aggregate_element(8), None);
    assert_eq!(
        past.poison().as_constant().aggregate_element(7),
        Some(i8_ty.as_type().poison().as_constant())
    );
    assert_eq!(none_left.undef().as_constant().aggregate_element(0), None);
    assert_eq!(zero_of(none_left).aggregate_element(0), None);
}

/// `ConstantFoldExtractValueInstruction` (`ConstantFold.cpp`) reads one element
/// per index and walks nothing: `if (Constant *C =
/// Agg->getAggregateElement(Idxs[0])) return
/// ConstantFoldExtractValueInstruction(C, Idxs.slice(1));`. So element 3 of a
/// `[4294967304 x i8]` zero is `i8 0`, read under the `unsigned` count that
/// `an_element_count_keeps_its_low_32_bits` pins. The same holds of that array
/// as a struct's field, and element 3 of its `undef` is `i8 undef`. A
/// `[1099511627776 x i8]` `undef` has no element 3 to give.
///
/// Regression: the fold walked every element of the aggregate. It declined the
/// zero, whose walk refuses a narrowed count, and it would have built one
/// handle per element of the `undef`. llvmkit-specific (D11): no upstream test
/// extracts from an array this long.
#[test]
fn extractvalue_reads_one_element_of_a_long_array() -> IrResult<()> {
    let m = Module::dynamic("extractvalue");
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let past = ArrayType::of(i8_ty, (1 << 32) + 8).as_type();
    let zero_byte = i8_ty.const_zero().as_constant();

    assert_eq!(
        constant_fold_extract_value_instruction(zero_of(past), &[3]),
        Some(zero_byte)
    );
    let record = m.struct_type([i32_ty.as_type(), past])?.as_type();
    assert_eq!(
        constant_fold_extract_value_instruction(zero_of(record), &[1, 3]),
        Some(zero_byte)
    );
    assert_eq!(
        constant_fold_extract_value_instruction(past.undef().as_constant(), &[3]),
        Some(i8_ty.as_type().undef().as_constant())
    );
    let none_left = ArrayType::of(i8_ty, 1 << 40).as_type();
    assert_eq!(
        constant_fold_extract_value_instruction(none_left.undef().as_constant(), &[3]),
        None
    );
    Ok(())
}

/// Pins the canonicalisation in `ConstantArray::getImpl` — `if
/// (C->isNullValue() && rangeOnlyContains(V.begin(), V.end(), C)) return
/// ConstantAggregateZero::get(Ty);` — and its siblings: the `isZero` arms of
/// `ConstantStruct::get` and `ConstantVector::getImpl`, and, for the scalable
/// vector, `ConstantVector::getSplat`'s scalable `if (V->isNullValue()) return
/// ConstantAggregateZero::get(VTy);`. A list of zeros is the one
/// `ConstantAggregateZero` of its type, so it has the id `null_value` answers.
/// An empty array is `getImpl`'s first arm. llvmkit-specific (D11): no
/// upstream test isolates these arms.
#[test]
fn two_spellings_of_zero_are_one_constant() -> IrResult<()> {
    let m = Module::dynamic("zero");
    let i32_ty = m.i32_type();
    let same_id = |spelled: Constant<'_, DynBrand>, ty: Type<'_, DynBrand>| {
        let zero = ty.null_value().expect("an aggregate has a null value");
        assert_eq!(spelled.as_erased().id(), zero.as_erased().id(), "{ty}");
    };

    let array = ArrayType::of(i32_ty, 2);
    same_id(
        array.const_array([i32_ty.const_zero(); 2])?.as_constant(),
        array.as_type(),
    );
    let empty = ArrayType::of(i32_ty, 0);
    same_id(
        empty
            .const_array(Vec::<Constant<'_, DynBrand>>::new())?
            .as_constant(),
        empty.as_type(),
    );
    let pair = m.struct_type([i32_ty.as_type(), m.i64_type().as_type()])?;
    same_id(
        pair.const_struct([
            i32_ty.const_zero().as_constant(),
            m.i64_type().const_zero().as_constant(),
        ])?
        .as_constant(),
        pair.as_type(),
    );
    let vector = VectorType::of(i32_ty, lanes::<2>());
    same_id(
        vector.const_vector([i32_ty.const_zero(); 2])?.as_constant(),
        vector.as_type(),
    );
    let scalable = VectorType::scalable_of(i32_ty, lanes::<2>());
    same_id(
        scalable
            .const_vector([i32_ty.const_zero(); 2])?
            .as_constant(),
        scalable.as_type(),
    );
    Ok(())
}

/// Ports `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest,
/// CheckElementWiseUndefPoison)` whole: a vector, a struct or an array of two
/// `undef`s is the `undef` of its type, of two `poison`s the `poison` of it, and
/// a list mixing the two is neither. These are the `isUndef` / `isPoison` arms
/// of `ConstantVector::getImpl` and `ConstantStruct::get`, and the
/// `isa<PoisonValue>` / `isa<UndefValue>` arms of `ConstantArray::getImpl`.
#[test]
fn an_all_poison_list_is_the_poison_marker() -> IrResult<()> {
    let m = Module::dynamic("undef-poison");
    let int32_ty = m.i32_type();
    let s_ty = m.struct_type([int32_ty.as_type(), int32_ty.as_type()])?;
    let a_ty = ArrayType::of(int32_ty, 2);
    let cu = int32_ty.as_type().undef().as_constant();
    let cp = int32_ty.as_type().poison().as_constant();

    {
        let v_ty = VectorType::of(int32_ty, lanes::<2>());
        let cuu = v_ty.const_vector([cu, cu])?.as_constant();
        let cpp = v_ty.const_vector([cp, cp])?.as_constant();
        let cup = v_ty.const_vector([cu, cp])?.as_constant();
        let cpu = v_ty.const_vector([cp, cu])?.as_constant();
        assert_eq!(cuu, cuu.ty().undef().as_constant());
        assert_eq!(cpp, cpp.ty().poison().as_constant());
        assert_ne!(cup, cup.ty().undef().as_constant());
        assert_ne!(cpu, cpu.ty().undef().as_constant());
    }

    {
        let cuu = s_ty.const_struct([cu, cu])?.as_constant();
        let cpp = s_ty.const_struct([cp, cp])?.as_constant();
        let cup = s_ty.const_struct([cu, cp])?.as_constant();
        let cpu = s_ty.const_struct([cp, cu])?.as_constant();
        assert_eq!(cuu, cuu.ty().undef().as_constant());
        assert_eq!(cpp, cpp.ty().poison().as_constant());
        assert_ne!(cup, cup.ty().undef().as_constant());
        assert_ne!(cpu, cpu.ty().undef().as_constant());
    }

    {
        let cuu = a_ty.const_array([cu, cu])?.as_constant();
        let cpp = a_ty.const_array([cp, cp])?.as_constant();
        let cup = a_ty.const_array([cu, cp])?.as_constant();
        let cpu = a_ty.const_array([cp, cu])?.as_constant();
        assert_eq!(cuu, cuu.ty().undef().as_constant());
        assert_eq!(cpp, cpp.ty().poison().as_constant());
        assert_ne!(cup, cup.ty().undef().as_constant());
        assert_ne!(cpu, cpu.ty().undef().as_constant());
    }
    Ok(())
}

/// Pins each `case` of `Constant::getNullValue` (`Constants.cpp`), which
/// `Type::null_value` ports: `ConstantInt::get(Ty, 0)`, `+0.0` at each of the
/// seven floating-point types, `ConstantPointerNull`, `ConstantAggregateZero`
/// for a struct (an opaque one included: `ConstantAggregateZero::get` asserts
/// only that `Ty->isStructTy()`), an array, a fixed and a scalable vector,
/// `ConstantTokenNone` and `ConstantTargetNone`. `None` stands for the
/// `llvm_unreachable` of the `default` arm — `void`, `label`, `metadata`,
/// `x86_amx` and a function type — and for the assert in
/// `ConstantTargetNone::get` on a target type without `HasZeroInit`. `exnref`
/// is llvmkit's own type, with no upstream `TypeID` (`docs/divergences.md`
/// entry 103), and has no null value either. llvmkit-specific (D11): no
/// upstream test isolates these arms.
#[test]
fn a_null_value_is_the_null_of_its_type() -> IrResult<()> {
    let m = Module::dynamic("null-value");

    assert!(matches!(
        null_kind_of(m.i32_type().as_type()),
        ConstantKind::Data(ConstantDataKind::Int(_))
    ));
    for ty in [
        m.half_type().as_type(),
        m.bfloat_type().as_type(),
        m.f32_type().as_type(),
        m.f64_type().as_type(),
        m.x86_fp80_type().as_type(),
        m.fp128_type().as_type(),
        m.ppc_fp128_type().as_type(),
    ] {
        assert!(
            matches!(
                null_kind_of(ty),
                ConstantKind::Data(ConstantDataKind::Float(_))
            ),
            "{ty}"
        );
    }
    assert!(matches!(
        null_kind_of(m.ptr_type_in::<1>().as_type()),
        ConstantKind::Data(ConstantDataKind::PointerNull)
    ));
    for ty in [
        m.struct_type([m.i32_type().as_type()])?.as_type(),
        m.opaque_struct("opaque")?.as_type(),
        ArrayType::of(m.i8_type(), 3).as_type(),
        VectorType::of(m.i8_type(), lanes::<3>()).as_type(),
        VectorType::scalable_of(m.i8_type(), lanes::<3>()).as_type(),
    ] {
        assert!(
            matches!(
                null_kind_of(ty),
                ConstantKind::Data(ConstantDataKind::AggregateZero)
            ),
            "{ty}"
        );
    }
    assert!(matches!(
        null_kind_of(m.token_type().as_type()),
        ConstantKind::Data(ConstantDataKind::TokenNone)
    ));
    let zeroable = m.target_ext_type("spirv.Event", Vec::<Type<'_, _>>::new(), [])?;
    assert!(matches!(
        null_kind_of(zeroable.as_type()),
        ConstantKind::Data(ConstantDataKind::TargetNone)
    ));

    let unzeroable = m.target_ext_type("other", Vec::<Type<'_, _>>::new(), [])?;
    let function = m.function_type(m.void_type(), [m.i32_type().as_type()])?;
    for ty in [
        unzeroable.as_type(),
        m.void_type().as_type(),
        m.label_type().as_type(),
        m.metadata_type().as_type(),
        m.x86_amx_type(),
        m.wasm_exnref_type(),
        function.as_type(),
    ] {
        assert_eq!(ty.null_value(), None, "{ty}");
    }
    Ok(())
}

/// Pins `Constant::getAggregateElement`'s `dyn_cast<ConstantAggregateZero>`
/// arm: `Elt < CAZ->getElementCount().getKnownMinValue() ?
/// CAZ->getElementValue(Elt) : nullptr`. `getElementValue` is the element
/// type's null value for an array or a vector and the field's for a struct.
/// The arm sits above the `isa<ScalableVectorType>` bail-out, so a scalable
/// zero answers below its known minimum, where a scalable `undef` and a
/// scalable splat of a non-zero value answer nothing. An opaque struct has no
/// elements. llvmkit-specific (D11): no upstream test isolates the arm.
#[test]
fn a_zero_aggregate_answers_each_element_from_its_type() -> IrResult<()> {
    let m = Module::dynamic("zero-elements");
    let i8_ty = m.i8_type();
    let i64_ty = m.i64_type();

    let pair = m.struct_type([i8_ty.as_type(), i64_ty.as_type()])?;
    let zero_pair = zero_of(pair.as_type());
    assert_eq!(
        zero_pair.aggregate_element(0),
        Some(i8_ty.const_zero().as_constant())
    );
    assert_eq!(
        zero_pair.aggregate_element(1),
        Some(i64_ty.const_zero().as_constant())
    );
    assert_eq!(zero_pair.aggregate_element(2), None);

    let nested = m.array_type(pair.as_type(), 4)?.as_type();
    assert_eq!(zero_of(nested).aggregate_element(3), Some(zero_pair));
    assert_eq!(zero_of(nested).aggregate_element(4), None);

    let scalable = VectorType::scalable_of(i8_ty, lanes::<2>()).as_type();
    assert_eq!(
        zero_of(scalable).aggregate_element(1),
        Some(i8_ty.const_zero().as_constant())
    );
    assert_eq!(zero_of(scalable).aggregate_element(2), None);
    assert_eq!(scalable.undef().as_constant().aggregate_element(1), None);
    // A scalable splat that is no zero is the `shufflevector` expression
    // `ConstantVector::getSplat` builds, which no `dyn_cast` arm admits: it
    // reaches the bail-out, though llvmkit keeps its lanes as a list.
    let one = VectorType::scalable_of(i8_ty, lanes::<2>())
        .const_vector([i8_ty.const_int(1_i8); 2])?
        .as_constant();
    assert!(matches!(one.kind(), ConstantKind::Expr(_)));
    assert_eq!(one.aggregate_element(0), None);
    // The positive control: the same lanes at a fixed width are a
    // `ConstantDataVector`, whose element is there.
    let fixed_one = VectorType::of(i8_ty, lanes::<2>())
        .const_vector([i8_ty.const_int(1_i8); 2])?
        .as_constant();
    assert_eq!(
        fixed_one.aggregate_element(0),
        Some(i8_ty.const_int(1_i8).as_constant())
    );

    assert_eq!(
        zero_of(m.opaque_struct("opaque")?.as_type()).aggregate_element(0),
        None
    );
    Ok(())
}

/// Pins the `AllSame` arms of `ConstantArray::handleOperandChangeImpl` and
/// `ConstantStruct::handleOperandChangeImpl`, which run before `getImpl` and
/// `replaceOperandsInPlace`: when every new operand is the replacement, a null
/// replacement answers `ConstantAggregateZero::get(getType())`, and an `undef`
/// one `UndefValue::get(getType())` — a `poison` too, because
/// `isa<UndefValue>` holds for it, so an array of two `poison`s that RAUW
/// produced is `undef` where `ConstantArray::get` of the same two would be
/// `poison`. `ConstantVector::handleOperandChangeImpl` has no such arm and asks
/// `getImpl`, which keeps the `poison`. A struct whose operands are not all the
/// replacement is updated in place even when every one is null: the
/// `ConstantStruct` that leaves is not a `ConstantAggregateZero`, prints its
/// elements, and is not a null value. llvmkit-specific (D11): no upstream test
/// isolates these arms.
#[test]
fn an_operand_change_to_one_null_or_undef_replaces_the_aggregate() -> IrResult<()> {
    let m = Module::dynamic("operand-change");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let pair = m.struct_type([i32_ty.as_type(), i32_ty.as_type()])?;
    let mixed = m.struct_type([i32_ty.as_type(), i64_ty.as_type()])?;
    let array = ArrayType::of(i32_ty, 2);
    let vector = VectorType::of(i32_ty, lanes::<2>());
    let zero = i32_ty.const_zero().as_constant();
    let poison = i32_ty.as_type().poison().as_constant();
    let aggregate_zero = |constant: Constant<'_, DynBrand>| {
        matches!(
            constant.kind(),
            ConstantKind::Data(ConstantDataKind::AggregateZero)
        )
    };

    let struct_zero = initializer_after_replacing(
        &m,
        "struct_zero",
        |p| Ok(pair.const_struct([p, p])?.as_constant()),
        zero,
    )?;
    assert_eq!(struct_zero.to_string(), "{ i32, i32 } zeroinitializer");
    assert!(aggregate_zero(struct_zero));
    assert_eq!(
        initializer_after_replacing(
            &m,
            "struct_poison",
            |p| Ok(pair.const_struct([p, p])?.as_constant()),
            poison,
        )?
        .to_string(),
        "{ i32, i32 } undef"
    );
    let array_zero = initializer_after_replacing(
        &m,
        "array_zero",
        |p| Ok(array.const_array([p, p])?.as_constant()),
        zero,
    )?;
    assert_eq!(array_zero.to_string(), "[2 x i32] zeroinitializer");
    assert!(aggregate_zero(array_zero));
    assert_eq!(
        initializer_after_replacing(
            &m,
            "array_poison",
            |p| Ok(array.const_array([p, p])?.as_constant()),
            poison,
        )?
        .to_string(),
        "[2 x i32] undef"
    );
    assert_eq!(
        initializer_after_replacing(
            &m,
            "vector_poison",
            |p| Ok(vector.const_vector([p, p])?.as_constant()),
            poison,
        )?
        .to_string(),
        "<2 x i32> poison"
    );

    // Positive control: one operand is not the replacement.
    assert_eq!(
        initializer_after_replacing(
            &m,
            "struct_partial",
            |p| {
                Ok(pair
                    .const_struct([p, i32_ty.const_int(1_i32).as_constant()])?
                    .as_constant())
            },
            zero,
        )?
        .to_string(),
        "{ i32, i32 } { i32 0, i32 1 }"
    );

    let in_place = initializer_after_replacing(
        &m,
        "struct_in_place",
        |p| {
            Ok(mixed
                .const_struct([p, i64_ty.const_zero().as_constant()])?
                .as_constant())
        },
        zero,
    )?;
    assert_eq!(in_place.to_string(), "{ i32, i64 } { i32 0, i64 0 }");
    assert!(matches!(
        in_place.kind(),
        ConstantKind::Aggregate(ConstantAggregateKind::Struct)
    ));
    assert!(!in_place.is_null_value());
    Ok(())
}

/// Pins `llvm::possiblyDemandedEltsInMask` (`VectorUtils.cpp`), whose element
/// loop is reached only through `dyn_cast<ConstantVector>(Mask)`. A
/// `zeroinitializer` mask is a `ConstantAggregateZero`, and so is a list of
/// zeros, so both answer every lane demanded, though neither demands one. A
/// `ConstantVector` clears its null lanes and keeps the rest, an `undef` lane
/// among them. Regression: the loop ran for any constant, so a zero mask
/// answered that no lane is demanded. llvmkit-specific (D11):
/// `VectorUtilsTest.cpp` has no case for it.
#[test]
fn a_zero_mask_demands_every_lane_as_upstream_answers() -> IrResult<()> {
    let m = Module::dynamic("mask");
    let i1_ty = m.bool_type();
    let mask_ty = VectorType::of(i1_ty, lanes::<4>());
    let lane_off = i1_ty.const_int(false).as_constant();
    let lane_on = i1_ty.const_int(true).as_constant();
    let undef = i1_ty.as_type().undef().as_constant();
    let demanded = |mask: Constant<'_, DynBrand>| {
        possibly_demanded_elements_in_mask(mask.as_erased())
            .expect("a fixed-width mask")
            .limited_value(u64::MAX)
    };

    assert_eq!(demanded(zero_of(mask_ty.as_type())), 0b1111);
    assert_eq!(
        demanded(mask_ty.const_vector([lane_off; 4])?.as_constant()),
        0b1111
    );
    assert_eq!(
        demanded(
            mask_ty
                .const_vector([lane_on, lane_off, lane_on, lane_off])?
                .as_constant()
        ),
        0b0101
    );
    assert_eq!(
        demanded(
            mask_ty
                .const_vector([undef, lane_off, lane_on, lane_off])?
                .as_constant()
        ),
        0b0101
    );
    Ok(())
}
