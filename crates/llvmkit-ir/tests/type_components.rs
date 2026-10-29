//! The type constructors' component rules: the typed routes that cannot break
//! them, and the erased routes that refuse to.

use llvmkit_ir::{
    ArrLenDyn, ArrayType, ConstantFolder, Dyn, ElemDyn, IntrinsicId, IrBuilder, IrBuilderFolder,
    IrError, LenDyn, Linkage, Module, ShuffleMaskElem, TargetExtParameterRule, Type, TypeKindLabel,
    TypePosition, TypedPointee, TypedPointerType, VectorType,
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
        .expect("valid target type")
        .as_type();
    assert!(!zero.is_sized());
    let two = m
        .target_ext_type("riscv.vector.tuple", [part], [2])
        .expect("valid target type")
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

/// An intrinsic name whose overload suffix spells a target type with
/// parameter counts `TargetExtType::checkParams` refuses does not demangle.
/// Upstream's `getMangledTypeStr` (`lib/IR/Intrinsics.cpp`) spells only types
/// that exist — `"t" + name`, then `"_" + param` per type and integer
/// parameter, then `"t"` — and `TargetExtType::get` would not build these.
/// llvmkit-specific regression (D10): the demangler interned the target type
/// with no count check. `tamdgcn.named.barrier_0t` is the positive control.
#[test]
fn an_intrinsic_name_cannot_spell_an_invalid_target_type() {
    let m = Module::dynamic("components");
    for name in [
        "llvm.ssa.copy.taarch64.svcount_i32t",
        "llvm.ssa.copy.triscv.vector.tuple_2t",
        "llvm.ssa.copy.tamdgcn.named.barriert",
    ] {
        let refused = m.get_or_insert_intrinsic_declaration_by_name(name);
        assert!(
            matches!(refused, Err(IrError::IntrinsicSignatureMismatch { .. })),
            "{name}: {refused:?}"
        );
    }
    m.get_or_insert_intrinsic_declaration_by_name("llvm.ssa.copy.tamdgcn.named.barrier_0t")
        .expect("`target(\"amdgcn.named.barrier\", 0)` is an overload of `llvm.ssa.copy`");
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

/// An intrinsic name whose overload suffix spells a literal struct of an
/// element `StructType::isValidElementType` refuses does not demangle.
/// Upstream's `getMangledTypeStr` (`lib/IR/Intrinsics.cpp`) spells a literal
/// struct `"sl_"` + its elements + `"s"` from a struct that exists, so no
/// upstream name reads `sl_isVoids` or `sl_Metadatas`. llvmkit-specific
/// regression (D10): the demangler built `{ void }` straight into the arena and
/// declared `llvm.ssa.copy` over it. `sl_i32s` is the positive control.
#[test]
fn an_intrinsic_name_cannot_spell_an_invalid_struct() {
    let m = Module::dynamic("components");
    for name in ["llvm.ssa.copy.sl_isVoids", "llvm.ssa.copy.sl_Metadatas"] {
        let refused = m.get_or_insert_intrinsic_declaration_by_name(name);
        assert!(
            matches!(refused, Err(IrError::IntrinsicSignatureMismatch { .. })),
            "{name}: {refused:?}"
        );
    }
    m.get_or_insert_intrinsic_declaration_by_name("llvm.ssa.copy.sl_i32s")
        .expect("`{ i32 }` is an overload of `llvm.ssa.copy`");
}

/// Demangling an intrinsic name creates no type: a named-struct overload
/// (`"s_" + name + "s"`, `getMangledTypeStr`) names a struct the module must
/// already hold. Upstream never demangles — `Intrinsic::getOrInsertDeclaration`
/// is handed the overload types — so a name cannot conjure one.
/// llvmkit-specific regression (D10): `s_Missings` created an opaque
/// `%Missing` as a side effect and declared `llvm.ssa.copy` over it. With
/// `%Present` created first, `s_Presents` is the positive control.
#[test]
fn demangling_a_struct_name_creates_no_struct() -> Result<(), IrError> {
    let m = Module::dynamic("components");
    let refused = m.get_or_insert_intrinsic_declaration_by_name("llvm.ssa.copy.s_Missings");
    assert!(
        matches!(refused, Err(IrError::IntrinsicSignatureMismatch { .. })),
        "{refused:?}"
    );
    assert!(
        m.named_struct("Missing").is_none(),
        "demangling must not create `%Missing`"
    );
    let present = m.opaque_struct("Present")?;
    m.set_struct_body(present, [m.i32_type()], false)?;
    m.get_or_insert_intrinsic_declaration_by_name("llvm.ssa.copy.s_Presents")?;
    Ok(())
}

/// The struct constructors refuse `void`, `label`, `metadata`, function types
/// and `token`, naming the element's index. Stricter than upstream's
/// `StructType::get`, which checks nothing: `LLParser::parseStructBody`
/// refuses the same elements (`invalid element type for struct`),
/// `Verifier.cpp`'s header calls a label inside a structure illegal, and MLIR's
/// `LLVMStructType::verifyInvariants` refuses them at construction.
/// llvmkit-specific (D10); `docs/divergences.md` records the difference.
///
/// The second half runs every refused kind through the packed constructor and
/// the `ModuleView` receiver, as `an_erased_array_refuses_an_invalid_element`
/// does for arrays.
#[test]
fn a_struct_refuses_an_invalid_element() {
    let m = Module::dynamic("components");
    let i32_ty = m.i32_type().as_type();
    let refused = m.struct_type([i32_ty, m.label_type().as_type()]);
    assert!(matches!(
        refused,
        Err(IrError::InvalidTypeComponent {
            position: TypePosition::StructElement { index: 1 },
            kind: TypeKindLabel::Label,
        })
    ));
    assert_eq!(
        refused.expect_err("refused above").to_string(),
        "invalid element type for struct (element 1): label"
    );
    let opaque = m.opaque_struct("S").expect("fresh name");
    let body = m.set_struct_body(opaque, [m.token_type().as_type()], false);
    assert!(matches!(
        body,
        Err(IrError::InvalidTypeComponent {
            position: TypePosition::StructElement { index: 0 },
            ..
        })
    ));
    m.struct_type([i32_ty, m.x86_amx_type()])
        .expect("x86_amx is a struct element");

    let view = m.as_view();
    let view_i32 = view.i32_type().as_type();
    for (element, kind) in [
        (view.void_type().as_type(), TypeKindLabel::Void),
        (view.label_type().as_type(), TypeKindLabel::Label),
        (view.metadata_type().as_type(), TypeKindLabel::Metadata),
        (
            view.function_type_no_parameters(view.void_type())
                .expect("valid function type")
                .as_type(),
            TypeKindLabel::Function,
        ),
        (view.token_type().as_type(), TypeKindLabel::Token),
    ] {
        for refused in [
            view.packed_struct_type([view_i32, view_i32, element]),
            view.struct_type([view_i32, view_i32, element]),
        ] {
            assert!(
                matches!(
                    refused,
                    Err(IrError::InvalidTypeComponent {
                        position: TypePosition::StructElement { index: 2 },
                        kind: k,
                    }) if k == kind
                ),
                "{refused:?}"
            );
        }
    }
    view.packed_struct_type([view_i32, view.x86_amx_type()])
        .expect("x86_amx is a struct element");
}

/// `target_ext_type` refuses a parameter count `TargetExtType::checkParams`
/// refuses, with upstream's exact message, at construction — as
/// `TargetExtType::getOrError` does (`lib/IR/Type.cpp`). llvmkit-specific
/// (D10): upstream's `TargetExtType::get` wraps the check in `cantFail`.
#[test]
fn a_target_type_checks_its_parameters_when_built() {
    let m = Module::dynamic("components");
    let i32_ty = m.i32_type().as_type();
    let no_types = Vec::<Type<'_, _>>::new();
    for (name, types, ints, rule, message) in [
        (
            "aarch64.svcount",
            vec![i32_ty],
            vec![],
            TargetExtParameterRule::Aarch64Svcount,
            "target extension type aarch64.svcount should have no parameters",
        ),
        (
            "riscv.vector.tuple",
            no_types.clone(),
            vec![2],
            TargetExtParameterRule::RiscvVectorTuple,
            "target extension type riscv.vector.tuple should have one type parameter and one integer parameter",
        ),
        (
            "amdgcn.named.barrier",
            no_types.clone(),
            vec![],
            TargetExtParameterRule::AmdgcnNamedBarrier,
            "target extension type amdgcn.named.barrier should have no type parameters and one integer parameter",
        ),
    ] {
        let refused = m.target_ext_type(name, types, ints);
        assert!(
            matches!(refused, Err(IrError::TargetExtTypeParameters { rule: r }) if r == rule),
            "{name}: {refused:?}"
        );
        assert_eq!(refused.expect_err("refused above").to_string(), message);
    }
    m.target_ext_type("amdgcn.named.barrier", no_types, [0])
        .expect("one integer parameter");
}

/// A typed pointer prints as AsmWriter prints one — `typedptr(<pointee>,
/// <addrspace>)`, `TypePrinting::print`'s `TypedPointerTyID` arm — and nests,
/// as `TypedPointerType::isValidElementType` allows. llvmkit-specific (the
/// printer arm has no upstream test).
#[test]
fn a_typed_pointer_prints_as_the_asm_writer_does() {
    let m = Module::dynamic("components");
    let inner = TypedPointerType::of(m.i32_type(), 0);
    assert_eq!(inner.to_string(), "typedptr(i32, 0)");
    let outer = TypedPointerType::of(inner, 3);
    assert_eq!(outer.to_string(), "typedptr(typedptr(i32, 0), 3)");
    assert!(matches!(outer.pointee(), TypedPointee::Typed(p) if p == inner));
    assert_eq!(outer.address_space(), 3);
}

/// The erased constructor refuses a pointee `TypedPointerType::isValidElementType`
/// refuses (`void`, `label`, `metadata`, `token`, `x86_amx`), mirroring the
/// `assert` in `TypedPointerType::get` (`lib/IR/TypedPointerType.cpp`).
/// llvmkit-specific (D10).
#[test]
fn an_erased_typed_pointer_refuses_an_invalid_pointee() {
    let m = Module::dynamic("components");
    assert!(matches!(
        m.typed_pointer_type(m.void_type(), 0),
        Err(IrError::InvalidTypeComponent {
            position: TypePosition::PointerElement,
            kind: TypeKindLabel::Void,
        })
    ));
    let ok = m
        .typed_pointer_type(m.i32_type(), 0)
        .expect("an integer pointee is valid");
    assert_eq!(ok, TypedPointerType::of(m.i32_type(), 0));

    let view = m.as_view();
    for (pointee, kind) in [
        (view.void_type().as_type(), TypeKindLabel::Void),
        (view.label_type().as_type(), TypeKindLabel::Label),
        (view.metadata_type().as_type(), TypeKindLabel::Metadata),
        (view.token_type().as_type(), TypeKindLabel::Token),
        (view.x86_amx_type(), TypeKindLabel::X86Amx),
    ] {
        let refused = view.typed_pointer_type(pointee, 0);
        assert!(
            matches!(
                refused,
                Err(IrError::InvalidTypeComponent {
                    position: TypePosition::PointerElement,
                    kind: k,
                }) if k == kind
            ),
            "{refused:?}"
        );
    }
    view.typed_pointer_type(view.ptr_type(1), 2)
        .expect("a pointer pointee is valid");
}

/// The function-type constructors refuse a result `FunctionType::isValidReturnType`
/// refuses and a parameter `isValidArgumentType` refuses, naming the
/// parameter. Mirrors the two `assert`s in `FunctionType::FunctionType`
/// (`lib/IR/Type.cpp`), which check the result first, then each parameter in
/// order; llvmkit-specific (D10).
///
/// The second half runs every refused result and parameter kind through the
/// four constructors on the `ModuleView` receiver, as
/// `a_struct_refuses_an_invalid_element` does for structs.
#[test]
fn a_function_type_refuses_an_invalid_result_or_parameter() {
    let m = Module::dynamic("components");
    let i32_ty = m.i32_type().as_type();
    assert!(matches!(
        m.function_type_no_parameters(m.label_type()),
        Err(IrError::InvalidTypeComponent {
            position: TypePosition::FunctionReturn,
            kind: TypeKindLabel::Label,
        })
    ));
    let refused = m.function_type(m.void_type(), [i32_ty, m.void_type().as_type()]);
    assert!(matches!(
        refused,
        Err(IrError::InvalidTypeComponent {
            position: TypePosition::FunctionParameter { index: 1 },
            kind: TypeKindLabel::Void,
        })
    ));
    assert_eq!(
        refused.expect_err("refused above").to_string(),
        "invalid type for function argument (parameter 1): void"
    );
    // The result is checked before any parameter.
    assert!(matches!(
        m.variadic_function_type(m.metadata_type(), [m.label_type().as_type()]),
        Err(IrError::InvalidTypeComponent {
            position: TypePosition::FunctionReturn,
            kind: TypeKindLabel::Metadata,
        })
    ));
    m.function_type(m.token_type(), [m.metadata_type().as_type()])
        .expect("token results and metadata parameters are valid");

    let view = m.as_view();
    let view_i32 = view.i32_type().as_type();
    let function = view
        .function_type_no_parameters(view.void_type())
        .expect("a void function type")
        .as_type();
    for (result, kind) in [
        (function, TypeKindLabel::Function),
        (view.label_type().as_type(), TypeKindLabel::Label),
        (view.metadata_type().as_type(), TypeKindLabel::Metadata),
    ] {
        for refused in [
            view.function_type(result, [view_i32]).map(|_| ()),
            view.variadic_function_type(result, [view_i32]).map(|_| ()),
            view.function_type_no_parameters(result).map(|_| ()),
            view.variadic_function_type_no_parameters(result)
                .map(|_| ()),
        ] {
            assert!(
                matches!(
                    refused,
                    Err(IrError::InvalidTypeComponent {
                        position: TypePosition::FunctionReturn,
                        kind: k,
                    }) if k == kind
                ),
                "{refused:?}"
            );
        }
    }
    for (parameter, kind) in [
        (view.void_type().as_type(), TypeKindLabel::Void),
        (view.label_type().as_type(), TypeKindLabel::Label),
        (function, TypeKindLabel::Function),
    ] {
        for refused in [
            view.function_type(view.void_type(), [view_i32, parameter])
                .map(|_| ()),
            view.variadic_function_type(view.void_type(), [view_i32, parameter])
                .map(|_| ()),
        ] {
            assert!(
                matches!(
                    refused,
                    Err(IrError::InvalidTypeComponent {
                        position: TypePosition::FunctionParameter { index: 1 },
                        kind: k,
                    }) if k == kind
                ),
                "{refused:?}"
            );
        }
    }
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
    let fn_ty = m.function_type(m.void_type(), [vec_ty.as_type(), vec_ty.as_type()])?;
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
    let fn_ty = m.function_type_no_parameters(m.void_type())?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let record = m.struct_type([m.i32_type().as_type()])?.as_type().poison();
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
