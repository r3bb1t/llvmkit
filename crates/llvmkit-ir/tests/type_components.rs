//! The type constructors' component rules: the typed routes that cannot break
//! them, and the erased routes that refuse to.

use llvmkit_ir::{
    ArrLenDyn, ArrayType, ConstantFolder, Dyn, ElemDyn, IntrinsicId, IrBuilder, IrBuilderFolder,
    IrError, LenDyn, Linkage, Module, ShuffleMaskElem, TypeKindLabel, TypePosition, VectorType,
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

/// `ArrayType::of` keeps the element marker and takes every element kind
/// `ArrayType::isValidElementType` admits that has a typed handle.
/// llvmkit-specific (D3).
#[test]
fn a_typed_array_takes_any_valid_element() {
    let m = Module::dynamic("components");
    let a: ArrayType<'_, f64, ArrLenDyn, _, _> = ArrayType::of(m.f64_type(), 3);
    assert_eq!(a.to_string(), "[3 x double]");
    let nested = ArrayType::of(a, 2);
    assert_eq!(nested.to_string(), "[2 x [3 x double]]");
    let of_vectors = ArrayType::of(VectorType::of(m.i8_type(), lanes::<4>()), 0);
    assert_eq!(of_vectors.to_string(), "[0 x <4 x i8>]");
}

/// The erased array constructor refuses what `ArrayType::isValidElementType`
/// refuses — `x86_amx` included, which a struct accepts. Mirrors
/// `ArrayType::get`'s `assert`; llvmkit-specific (D10).
///
/// The `ModuleView` half below runs the same refusals through the view's
/// constructor, as `an_erased_vector_refuses_an_invalid_element` does for the
/// vector ones.
#[test]
fn an_erased_array_refuses_an_invalid_element() {
    let m = Module::dynamic("components");
    for (element, kind) in [
        (m.label_type().as_type(), TypeKindLabel::Label),
        (m.x86_amx_type(), TypeKindLabel::X86Amx),
        (m.token_type().as_type(), TypeKindLabel::Token),
    ] {
        let refused = m.array_type(element, 2);
        assert!(
            matches!(
                refused,
                Err(IrError::InvalidTypeComponent { position: TypePosition::ArrayElement, kind: k }) if k == kind
            ),
            "{refused:?}"
        );
    }
    m.array_type(m.ptr_type(0), 2)
        .expect("a pointer is an array element");

    assert_eq!(
        m.array_type(m.x86_amx_type(), 2)
            .expect_err("`x86_amx` is no array element")
            .to_string(),
        "invalid array element type: x86_amx"
    );

    let view = m.as_view();
    for (element, kind) in [
        (view.label_type().as_type(), TypeKindLabel::Label),
        (view.x86_amx_type(), TypeKindLabel::X86Amx),
        (view.token_type().as_type(), TypeKindLabel::Token),
    ] {
        let refused = view.array_type(element, 2);
        assert!(
            matches!(
                refused,
                Err(IrError::InvalidTypeComponent { position: TypePosition::ArrayElement, kind: k }) if k == kind
            ),
            "{refused:?}"
        );
    }
    view.array_type(view.ptr_type(0), 2)
        .expect("a pointer is an array element");
}

/// An intrinsic name whose overload suffix spells an array of an element
/// `ArrayType::isValidElementType` refuses does not demangle. Upstream's
/// `getMangledTypeStr` (`lib/IR/Intrinsics.cpp`) spells only types that exist,
/// so no upstream name reads `a2x86amx`, `a2isVoid` or `a2Metadata`.
/// llvmkit-specific regression (D10): the demangler built `[2 x x86_amx]`
/// straight into the arena and declared `llvm.ssa.copy` (whose one overload is
/// `llvm_any_ty`) over it. `a2i32` is the positive control.
#[test]
fn an_intrinsic_name_cannot_spell_an_invalid_array() {
    let m = Module::dynamic("components");
    for name in [
        "llvm.ssa.copy.a2x86amx",
        "llvm.ssa.copy.a2isVoid",
        "llvm.ssa.copy.a2Metadata",
    ] {
        let refused = m.get_or_insert_intrinsic_declaration_by_name(name);
        assert!(
            matches!(refused, Err(IrError::IntrinsicSignatureMismatch { .. })),
            "{name}: {refused:?}"
        );
    }
    m.get_or_insert_intrinsic_declaration_by_name("llvm.ssa.copy.a2i32")
        .expect("`[2 x i32]` is an overload of `llvm.ssa.copy`");
}

/// An intrinsic name whose overload suffix spells a zero-length array
/// demangles, and names the declaration the same overload gets by id.
/// `ArrayType::get` takes a zero length, and `getMangledTypeStr`
/// (`lib/IR/Intrinsics.cpp`) spells `[0 x i32]` as `"a" + utostr(0) + "i32"`
/// — which is the name llvmkit's own mangling gives the by-id declaration.
/// llvmkit-specific regression (D10): the demangler refused a zero count, so
/// `llvm.ssa.copy.a0i32` was refused as a signature mismatch and the by-name
/// entry could not read back the name the by-id entry wrote. The `a2i32`
/// overload in `an_intrinsic_name_cannot_spell_an_invalid_array` is the
/// nonzero control.
#[test]
fn an_intrinsic_name_can_spell_a_zero_length_array() -> Result<(), IrError> {
    let m = Module::dynamic("components");
    let ssa_copy = IntrinsicId::lookup("llvm.ssa.copy").expect("`llvm.ssa.copy` is an intrinsic");
    let empty = ArrayType::of(m.i32_type(), 0).as_type();
    let by_id = m.get_or_insert_intrinsic_declaration_by_id(ssa_copy, [empty])?;
    assert_eq!(m.view(by_id).name().as_deref(), Some("llvm.ssa.copy.a0i32"));
    let by_name = m.get_or_insert_intrinsic_declaration_by_name("llvm.ssa.copy.a0i32")?;
    assert_eq!(by_name, by_id);
    Ok(())
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
