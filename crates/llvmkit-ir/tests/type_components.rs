//! The type constructors' component rules: the typed routes that cannot break
//! them, and the erased routes that refuse to.

use llvmkit_ir::{
    ConstantFolder, Dyn, ElemDyn, IrBuilder, IrBuilderFolder, IrError, LenDyn, Linkage, Module,
    ShuffleMaskElem, TypeKindLabel, TypePosition, VectorType,
    constant_fold_shuffle_vector_instruction, lanes,
};

/// `VectorType::of` keeps the element marker and builds in the element's own
/// module. llvmkit-specific (D3): the typed route `FixedVectorType::get`'s
/// asserts cannot reach.
#[test]
fn a_typed_vector_keeps_its_element_marker() {
    let m = Module::dynamic("components");
    let v: VectorType<'_, i32, LenDyn, _, _> = VectorType::of(m.i32_type(), lanes::<4>());
    assert_eq!(v.to_string(), "<4 x i32>");
    assert_eq!(v.min_len().get(), 4);
    let s = VectorType::scalable_of(m.f64_type(), lanes::<2>());
    assert_eq!(s.to_string(), "<vscale x 2 x double>");
    let dyn_int = m.custom_width_int_type(7).expect("i7 is a legal width");
    let d: VectorType<'_, ElemDyn, LenDyn, _, _> = VectorType::of(dyn_int, lanes::<3>());
    assert_eq!(d.to_string(), "<3 x i7>");
}

/// The erased vector constructors — on `Module` and on `ModuleView` — refuse
/// what `VectorType::isValidElementType` refuses, reporting the position and
/// the kind. Mirrors the `assert` in `FixedVectorType::get` /
/// `ScalableVectorType::get` (`lib/IR/Type.cpp`); llvmkit-specific (D10).
#[test]
fn an_erased_vector_refuses_an_invalid_element() {
    let m = Module::dynamic("components");
    let array = m.array_type_n::<i32, 2>().as_type();
    let refused = m.vector_type(array, lanes::<4>());
    assert!(
        matches!(
            refused,
            Err(IrError::InvalidTypeComponent {
                position: TypePosition::VectorElement,
                kind: TypeKindLabel::Array,
            })
        ),
        "{refused:?}"
    );
    assert_eq!(
        refused.expect_err("refused above").to_string(),
        "invalid vector element type: array"
    );
    let scalable = m.scalable_vector_type(m.void_type(), lanes::<4>());
    assert!(matches!(
        scalable,
        Err(IrError::InvalidTypeComponent {
            position: TypePosition::VectorElement,
            kind: TypeKindLabel::Void,
        })
    ));
    m.vector_type(m.i32_type(), lanes::<4>())
        .expect("an integer is a vector element");

    let view = m.as_view();
    let view_array = view.array_type_n::<i32, 2>().as_type();
    let refused = view.vector_type(view_array, lanes::<4>());
    assert!(
        matches!(
            refused,
            Err(IrError::InvalidTypeComponent {
                position: TypePosition::VectorElement,
                kind: TypeKindLabel::Array,
            })
        ),
        "{refused:?}"
    );
    let refused = view.scalable_vector_type(view.void_type(), lanes::<4>());
    assert!(
        matches!(
            refused,
            Err(IrError::InvalidTypeComponent {
                position: TypePosition::VectorElement,
                kind: TypeKindLabel::Void,
            })
        ),
        "{refused:?}"
    );
    view.scalable_vector_type(view.i32_type(), lanes::<4>())
        .expect("an integer is a vector element");
}

/// A `riscv.vector.tuple` with a count of zero has no layout type, so it is
/// unsized: its layout would be `<vscale x 0 x i8>`, which upstream's
/// `getTargetTypeInfo` builds through the asserting `ScalableVectorType::get`
/// and llvmkit cannot represent. llvmkit-specific regression (D10): the layout
/// was a zero-lane vector.
#[test]
fn a_zero_count_riscv_tuple_has_no_layout() {
    let m = Module::dynamic("components");
    let part = VectorType::scalable_of(m.i8_type(), lanes::<8>()).as_type();
    let zero = m
        .target_ext_type("riscv.vector.tuple", [part], [0])
        .as_type();
    assert!(!zero.is_sized());
    let two = m
        .target_ext_type("riscv.vector.tuple", [part], [2])
        .as_type();
    assert!(two.is_sized());
}

/// An intrinsic name whose overload suffix spells a zero-lane vector, or a
/// vector of an element `VectorType::isValidElementType` refuses, does not
/// demangle. Upstream's `getMangledTypeStr` (`lib/IR/Intrinsics.cpp`) spells
/// only types that exist, so no upstream name reads `v0i32` or `v4isVoid`.
/// llvmkit-specific regression (D10): the demangler built `<4 x void>`
/// straight into the arena and declared the intrinsic over it. The zero counts
/// were already refused by the demangler's count parser and stay here as
/// controls of that; `v4i32` is the positive control.
#[test]
fn an_intrinsic_name_cannot_spell_an_invalid_vector() {
    let m = Module::dynamic("components");
    for name in [
        "llvm.vector.reduce.add.v0i32",
        "llvm.vector.reduce.add.nxv0i32",
        "llvm.vector.reduce.add.v4isVoid",
    ] {
        let refused = m.get_or_insert_intrinsic_declaration_by_name(name);
        assert!(
            matches!(refused, Err(IrError::IntrinsicSignatureMismatch { .. })),
            "{name}: {refused:?}"
        );
    }
    m.get_or_insert_intrinsic_declaration_by_name("llvm.vector.reduce.add.v4i32")
        .expect("`<4 x i32>` is a vector overload of `llvm.vector.reduce.add`");
}

/// The builder's `shuffle_vector` refuses an empty mask, whose result would be
/// a zero-lane vector, with `IrError::InvalidShuffleMaskLength`: the
/// `ShuffleVectorInst` constructor computes its type as
/// `VectorType::get(…, Mask.size(), …)`, which asserts a non-zero count.
/// llvmkit-specific regression (D10): the builder interned a `<0 x i32>`
/// result type. A one-lane mask is the positive control.
#[test]
fn the_builder_refuses_a_shuffle_with_an_empty_mask() -> Result<(), IrError> {
    let m = Module::dynamic("components");
    let vec_ty = VectorType::of(m.i32_type(), lanes::<2>());
    let fn_ty = m.function_type(m.void_type(), [vec_ty.as_type(), vec_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let refused = b.shuffle_vector(m.view(f).param(0)?, m.view(f).param(1)?, &[], "");
    assert!(
        matches!(refused, Err(IrError::InvalidShuffleMaskLength { len: 0 })),
        "{refused:?}"
    );
    b.shuffle_vector(
        m.view(f).param(0)?,
        m.view(f).param(1)?,
        &[ShuffleMaskElem::Lane(0)],
        "",
    )?;
    Ok(())
}

/// The builder's `ConstantFolder` refuses an empty shuffle mask as the builder
/// does, with `IrError::InvalidShuffleMaskLength`: its fallback builds the
/// constant expression's result type as
/// `ConstantExpr::getShuffleVector` does, `VectorType::get(EltTy,
/// Mask.size(), …)`, which asserts a non-zero count. llvmkit-specific
/// regression (D10): the folder returned a `<0 x i32> poison`. A one-lane mask
/// is the positive control.
#[test]
fn the_constant_folder_refuses_a_shuffle_with_an_empty_mask() -> Result<(), IrError> {
    let m = Module::dynamic("components");
    let poison = VectorType::of(m.i32_type(), lanes::<2>())
        .as_type()
        .poison()
        .as_erased();
    let refused = ConstantFolder.fold_shuffle_vector_dyn(poison, poison, &[]);
    assert!(
        matches!(refused, Err(IrError::InvalidShuffleMaskLength { len: 0 })),
        "{refused:?}"
    );
    assert!(
        ConstantFolder
            .fold_shuffle_vector_dyn(poison, poison, &[ShuffleMaskElem::Lane(0)])?
            .is_some()
    );
    Ok(())
}

/// `constant_fold_shuffle_vector_instruction` answers "no fold" for an empty
/// mask: `ConstantFoldShuffleVectorInstruction` (`lib/IR/ConstantFold.cpp`)
/// builds its result as `VectorType::get(EltTy, MaskEltCount)`, which asserts
/// a non-zero count. llvmkit-specific regression (D10): it folded to a
/// `<0 x i32> poison`. A one-lane mask is the positive control.
#[test]
fn constant_folding_a_shuffle_with_an_empty_mask_does_not_fold() -> Result<(), IrError> {
    let m = Module::dynamic("components");
    let poison = VectorType::of(m.i32_type(), lanes::<2>())
        .as_type()
        .poison()
        .as_constant();
    assert!(constant_fold_shuffle_vector_instruction(poison, poison, &[])?.is_none());
    assert!(
        constant_fold_shuffle_vector_instruction(poison, poison, &[ShuffleMaskElem::Lane(0)])?
            .is_some()
    );
    Ok(())
}

/// `vector_splat_dyn` refuses a scalar whose type is not a vector element:
/// `IRBuilderBase::CreateVectorSplat` builds `VectorType::get(V->getType(),
/// EC)`, which asserts `isValidElementType`. llvmkit-specific regression
/// (D10): the splat built `<2 x { i32 }>`. An `i32` scalar is the positive
/// control.
#[test]
fn a_splat_of_a_non_vector_element_is_refused() -> Result<(), IrError> {
    let m = Module::dynamic("components");
    let fn_ty = m.function_type_no_parameters(m.void_type());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let record = m.struct_type([m.i32_type().as_type()]).as_type().poison();
    let refused = b.vector_splat_dyn(lanes::<2>(), record, "v");
    assert!(
        matches!(
            refused,
            Err(IrError::InvalidTypeComponent {
                position: TypePosition::VectorElement,
                kind: TypeKindLabel::Struct,
            })
        ),
        "{refused:?}"
    );
    b.vector_splat_dyn(lanes::<2>(), m.i32_type().const_int(7_i32), "v")?;
    Ok(())
}
