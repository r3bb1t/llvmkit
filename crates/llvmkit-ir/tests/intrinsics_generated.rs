use llvmkit_ir::{
    AnyTypeEnum, AttrIndex, AttrKind, Attribute, BinaryIntrinsic, Dyn, IRBuilder, IntValue,
    IntrinsicDescriptor, IntrinsicId, IntrinsicNameResolution, IrError, LifetimeIntrinsic, Linkage,
    MemIntrinsic, Module, PointerValue, Type, resolve_intrinsic_name,
};

/// Mirrors `llvm/lib/IR/Intrinsics.cpp::Intrinsic::lookupIntrinsicID` for
/// generic overloaded and target-specific intrinsic name-table lookups.
#[test]
fn generated_lookup_resolves_generic_and_target_intrinsics() {
    let abs = IntrinsicId::lookup("llvm.abs.i32").expect("abs intrinsic");
    assert_eq!(abs.base_name(), "llvm.abs");
    assert!(abs.is_overloaded());

    let kill = IntrinsicId::lookup("llvm.amdgcn.kill").expect("amdgcn kill intrinsic");
    assert_eq!(kill.base_name(), "llvm.amdgcn.kill");
    assert_eq!(kill.target_prefix(), Some("amdgcn"));
    assert!(!kill.is_overloaded());
}

/// Mirrors `llvm/lib/IR/Intrinsics.cpp::lookupLLVMIntrinsicByName`: unknown
/// `llvm.*` names do not resolve to ordinary functions.
#[test]
fn generated_lookup_rejects_unknown_intrinsic_names() {
    assert!(IntrinsicId::lookup("llvm.not.a.real.intrinsic").is_none());
}

/// Mirrors `llvm/lib/IR/Intrinsics.cpp::findTargetSubtable`: callers can
/// distinguish ordinary names from reserved but unknown `llvm.*` names.
#[test]
fn name_resolution_distinguishes_non_intrinsic_and_unknown_intrinsic() {
    assert!(matches!(
        resolve_intrinsic_name("not.llvm"),
        IntrinsicNameResolution::NonIntrinsic
    ));
    assert!(matches!(
        resolve_intrinsic_name("llvm.not.a.real.intrinsic"),
        IntrinsicNameResolution::UnknownIntrinsic
    ));
    let IntrinsicNameResolution::Known(kill) = resolve_intrinsic_name("llvm.amdgcn.kill") else {
        panic!("expected known intrinsic");
    };
    assert_eq!(
        kill,
        IntrinsicId::lookup("llvm.amdgcn.kill").expect("amdgcn kill intrinsic")
    );
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.h::ID`: generated raw IDs are
/// observable and round-trip only through a checked constructor.
#[test]
fn raw_intrinsic_ids_round_trip_through_checked_api() {
    let id = IntrinsicId::lookup("llvm.abs.i32").expect("abs intrinsic");

    assert_eq!(IntrinsicId::from_raw(id.raw()), Some(id));
    assert!(IntrinsicId::from_raw(0).is_none());
    assert!(IntrinsicId::all().len() > 1_000);
    assert!(IntrinsicId::all().any(|candidate| candidate == id));
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.h::ID`: generated semantic IDs are
/// available as stable typed constants without string lookup.
#[test]
fn semantic_intrinsic_ids_are_published_as_constants() {
    assert_eq!(
        IntrinsicId::ABS,
        IntrinsicId::lookup("llvm.abs.i32").expect("abs intrinsic")
    );
    assert_eq!(
        IntrinsicId::FSHL,
        IntrinsicId::lookup("llvm.fshl.i32").expect("fshl intrinsic")
    );
    assert_eq!(
        IntrinsicId::TRAP,
        IntrinsicId::lookup("llvm.trap").expect("trap intrinsic")
    );
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::int_memcpy` and siblings:
/// canonical memory intrinsic names include pointer overloads plus length type.
#[test]
fn canonical_memory_intrinsic_suffixes_build_signatures() -> Result<(), IrError> {
    Module::with_new("intrinsic-memory", |m| {
        let ptr_ty = m.ptr_type(0).as_type();
        let i64_ty = m.i64_type().as_type();
        let memcpy = IntrinsicId::lookup("llvm.memcpy.p0.p0.i64").expect("memcpy intrinsic");
        assert_eq!(
            format!("{}", memcpy.function_type(&m, &[ptr_ty, ptr_ty, i64_ty])?),
            "void (ptr, ptr, i64, i1)"
        );
        assert!(memcpy.function_type(&m, &[ptr_ty, ptr_ty]).is_err());

        let memset = IntrinsicId::lookup("llvm.memset.p0.i64").expect("memset intrinsic");
        assert_eq!(
            format!("{}", memset.function_type(&m, &[ptr_ty, i64_ty])?),
            "void (ptr, i8, i64, i1)"
        );
        assert!(memset.function_type(&m, &[ptr_ty]).is_err());
        Ok(())
    })
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::int_expect`: overloaded expect
/// returns and accepts the same integer or fixed-vector integer type.
#[test]
fn expect_intrinsic_builds_overloaded_signature() -> Result<(), IrError> {
    Module::with_new("intrinsic-expect", |m| {
        let i32_ty = m.i32_type().as_type();
        let expect = IntrinsicId::lookup("llvm.expect.i32").expect("expect intrinsic");
        assert_eq!(
            format!("{}", expect.function_type(&m, &[i32_ty])?),
            "i32 (i32, i32)"
        );
        assert!(expect.function_type(&m, &[]).is_err());
        Ok(())
    })
}

/// Mirrors `IntrinsicsAMDGPU.td::int_amdgcn_kill` and
/// `IntrinsicsX86.td::int_x86_mmx_padd_b`: generated IIT entries decode fixed
/// primitive and MMX vector types without hand-written semantic cases.
#[test]
fn generated_target_and_mmx_signatures_decode() -> Result<(), IrError> {
    Module::with_new("intrinsic-generated-signatures", |m| {
        let kill = IntrinsicId::lookup("llvm.amdgcn.kill").expect("amdgcn kill intrinsic");
        assert_eq!(format!("{}", kill.function_type(&m, &[])?), "void (i1)");

        let padd = IntrinsicId::lookup("llvm.x86.mmx.padd.b").expect("x86 mmx padd intrinsic");
        assert_eq!(
            format!("{}", padd.function_type(&m, &[])?),
            "<1 x i64> (<1 x i64>, <1 x i64>)"
        );
        Ok(())
    })
}

/// Mirrors `Intrinsics.td::int_acos`: generated IIT entries decode overloaded
/// floating-point type arguments from the mangled intrinsic name.
#[test]
fn generated_overloaded_float_signature_decodes() -> Result<(), IrError> {
    Module::with_new("intrinsic-generated-overload", |m| {
        let f32_ty = m.f32_type().as_type();
        let i32_ty = m.i32_type().as_type();
        let acos = IntrinsicId::lookup("llvm.acos.f32").expect("acos intrinsic");
        assert_eq!(
            format!("{}", acos.function_type(&m, &[f32_ty])?),
            "float (float)"
        );
        assert!(acos.function_type(&m, &[]).is_err());
        assert!(acos.function_type(&m, &[i32_ty]).is_err());
        Ok(())
    })
}

/// Mirrors `llvm/include/llvm/IR/DerivedTypes.h::getTruncatedElementVectorType`
/// and `IntrinsicsARM.td::int_arm_neon_vmulls`: `LLVMTruncatedType` decodes
/// floating-point vector overloads by narrowing `float` elements to `half`.
#[test]
fn generated_truncated_floating_vector_signature_decodes() -> Result<(), IrError> {
    Module::with_new("intrinsic-generated-float-trunc", |m| {
        let f32_vec = m.vector_type(m.f32_type().as_type(), 4, false).as_type();
        let vmulls = IntrinsicId::lookup("llvm.arm.neon.vmulls.v4f32").expect("vmulls intrinsic");

        assert_eq!(
            format!("{}", vmulls.function_type(&m, &[f32_vec])?),
            "<4 x float> (<4 x half>, <4 x half>)"
        );
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/IntrinsicInst.cpp` semantic handling: only modeled
/// intrinsics with valid semantic suffixes are accepted by binary-only analyses
/// and folders.
#[test]
fn binary_intrinsic_mapping_is_narrow() {
    let umax = IntrinsicId::lookup("llvm.umax.i32").expect("umax intrinsic");
    assert_eq!(
        BinaryIntrinsic::from_intrinsic_id(umax),
        Some(BinaryIntrinsic::UMax)
    );

    let fshl = IntrinsicId::lookup("llvm.fshl.i32").expect("fshl intrinsic");
    assert_eq!(
        BinaryIntrinsic::from_intrinsic_id(fshl),
        Some(BinaryIntrinsic::FShl)
    );

    let expect = IntrinsicId::lookup("llvm.expect.i32").expect("expect intrinsic");
    assert_eq!(BinaryIntrinsic::from_intrinsic_id(expect), None);
    assert_eq!(expect.as_binary_intrinsic(), None);
    assert_eq!(fshl.as_binary_intrinsic(), Some(BinaryIntrinsic::FShl));
    assert_eq!(
        BinaryIntrinsic::from_intrinsic_name("llvm.umax.notatype"),
        None
    );
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitFunction`: intrinsic declarations
/// must carry generated attributes, but verifier-valid extra function
/// attributes are accepted by the generic function-attribute verifier.
#[test]
fn verifier_accepts_extra_valid_intrinsic_declaration_attribute() -> Result<(), IrError> {
    Module::with_new("intrinsic-extra-attr", |m| {
        let i32_ty = m.i32_type();
        let descriptor = IntrinsicDescriptor::new(IntrinsicId::ABS, [i32_ty.as_type()])?;
        let abs = m.get_or_insert_intrinsic_declaration(&descriptor)?;
        abs.add_attribute(
            &m,
            AttrIndex::Function,
            Attribute::enum_attr_for_brand(AttrKind::NoInline).expect("enum attribute"),
        );
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitCallBase`: `immarg` intrinsic
/// operands must be integer or floating immediate constants.
#[test]
fn verifier_rejects_nonconstant_immarg_operand() -> Result<(), IrError> {
    Module::with_new("intrinsic-immarg", |m| {
        let i1_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let caller_ty = m.fn_type(
            void_ty.as_type(),
            [i32_ty.as_type(), i1_ty.as_type()],
            false,
        );
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let descriptor = IntrinsicDescriptor::new(IntrinsicId::ABS, [i32_ty.as_type()])?;
        let abs = m.get_or_insert_intrinsic_declaration(&descriptor)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: IntValue<i32> = caller.param(0)?.try_into()?;
        let is_poison: IntValue<bool> = caller.param(1)?.try_into()?;
        b.call_builder(abs)
            .arg(x)
            .arg(is_poison)
            .name("abs")
            .build()?;
        b.build_ret_void()?;

        let err = m
            .verify_borrowed()
            .expect_err("nonconstant immarg rejected");
        assert!(
            err.to_string()
                .contains("immarg operand has non-immediate parameter"),
            "{err}"
        );
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/IntrinsicInst.cpp` wrappers: a direct call to a
/// canonical intrinsic declaration can be viewed as an intrinsic call.
#[test]
fn descriptor_call_builder_returns_intrinsic_view() -> Result<(), IrError> {
    Module::with_new("intrinsic-call-view", |m| {
        let i1_ty = m.bool_type();
        let i32_ty = m.i32_type();
        let descriptor = IntrinsicDescriptor::new(IntrinsicId::ABS, [i32_ty.as_type()])?;
        let caller_ty = m.fn_type(i32_ty.as_type(), [i32_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: IntValue<i32> = caller.param(0)?.try_into()?;
        let view = b
            .intrinsic_call_builder(&descriptor)?
            .arg(x)
            .arg(i1_ty.const_int(false))
            .name("abs")
            .build()?;
        assert_eq!(view.id(), IntrinsicId::ABS);
        assert_eq!(view.descriptor()?, descriptor);
        let ret: IntValue<i32> = view.return_value().expect("abs returns value").try_into()?;
        b.build_ret(ret)?;
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/IntrinsicInst.cpp` memory wrappers: a canonical
/// `llvm.memcpy` call can be narrowed to `MemIntrinsic`.
#[test]
fn mem_intrinsic_wrapper_narrows_generated_memory_call() -> Result<(), IrError> {
    Module::with_new("intrinsic-mem-wrapper", |m| {
        let ptr_ty = m.ptr_type(0);
        let i1_ty = m.bool_type();
        let i64_ty = m.i64_type();
        let descriptor = IntrinsicDescriptor::new(
            IntrinsicId::MEMCPY,
            [ptr_ty.as_type(), ptr_ty.as_type(), i64_ty.as_type()],
        )?;
        let caller_ty = m.fn_type(
            m.void_type().as_type(),
            [ptr_ty.as_type(), ptr_ty.as_type(), i64_ty.as_type()],
            false,
        );
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let dst: PointerValue = caller.param(0)?.try_into()?;
        let src: PointerValue = caller.param(1)?.try_into()?;
        let len: IntValue<i64> = caller.param(2)?.try_into()?;
        let view = b.build_intrinsic_call(
            &descriptor,
            &[
                dst.as_value(),
                src.as_value(),
                len.as_value(),
                i1_ty.const_int(false).as_value(),
            ],
            "",
        )?;

        let mem = MemIntrinsic::try_from_intrinsic(view)?;
        assert_eq!(mem.inner().intrinsic_id(), IntrinsicId::MEMCPY);
        assert!(LifetimeIntrinsic::try_from_intrinsic(view).is_err());
        b.build_ret_void()?;
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/IntrinsicInst.cpp::MemIntrinsic::classof`: inline
/// memory intrinsics are memory intrinsics too.
#[test]
fn mem_intrinsic_wrapper_narrows_generated_inline_memory_calls() -> Result<(), IrError> {
    Module::with_new("intrinsic-inline-mem-wrapper", |m| {
        let ptr_ty = m.ptr_type(0);
        let i1_ty = m.bool_type();
        let i8_ty = m.i8_type();
        let i64_ty = m.i64_type();
        let caller_ty = m.fn_type(
            m.void_type().as_type(),
            [ptr_ty.as_type(), ptr_ty.as_type(), i64_ty.as_type()],
            false,
        );
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let dst: PointerValue = caller.param(0)?.try_into()?;
        let src: PointerValue = caller.param(1)?.try_into()?;
        let len: IntValue<i64> = caller.param(2)?.try_into()?;

        let memcpy_inline =
            IntrinsicId::lookup("llvm.memcpy.inline.p0.p0.i64").expect("memcpy.inline intrinsic");
        let memcpy_descriptor = IntrinsicDescriptor::new(
            memcpy_inline,
            [ptr_ty.as_type(), ptr_ty.as_type(), i64_ty.as_type()],
        )?;
        let memcpy = b.build_intrinsic_call(
            &memcpy_descriptor,
            &[
                dst.as_value(),
                src.as_value(),
                len.as_value(),
                i1_ty.const_int(false).as_value(),
            ],
            "",
        )?;
        assert!(MemIntrinsic::try_from_intrinsic(memcpy).is_ok());

        let memset_inline =
            IntrinsicId::lookup("llvm.memset.inline.p0.i64").expect("memset.inline intrinsic");
        let memset_descriptor =
            IntrinsicDescriptor::new(memset_inline, [ptr_ty.as_type(), i64_ty.as_type()])?;
        let memset = b.build_intrinsic_call(
            &memset_descriptor,
            &[
                dst.as_value(),
                i8_ty.const_int(0_i8).as_value(),
                len.as_value(),
                i1_ty.const_int(false).as_value(),
            ],
            "",
        )?;
        assert!(MemIntrinsic::try_from_intrinsic(memset).is_ok());

        b.build_ret_void()?;
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/IntrinsicInst.cpp` lifetime wrappers: a canonical
/// `llvm.lifetime.start` call can be narrowed to `LifetimeIntrinsic`.
#[test]
fn lifetime_intrinsic_wrapper_narrows_generated_lifetime_call() -> Result<(), IrError> {
    Module::with_new("intrinsic-lifetime-wrapper", |m| {
        let ptr_ty = m.ptr_type(0);
        let descriptor = IntrinsicDescriptor::new(IntrinsicId::LIFETIME_START, [ptr_ty.as_type()])?;
        let caller_ty = m.fn_type(m.void_type().as_type(), [ptr_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let ptr: PointerValue = caller.param(0)?.try_into()?;
        let view = b.build_intrinsic_call(&descriptor, &[ptr.as_value()], "")?;

        let lifetime = LifetimeIntrinsic::try_from_intrinsic(view)?;
        assert_eq!(lifetime.inner().intrinsic_id(), IntrinsicId::LIFETIME_START);
        assert!(MemIntrinsic::try_from_intrinsic(view).is_err());
        b.build_ret_void()?;
        m.verify_borrowed()?;
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Verifier.cpp::visitCallBase`: descriptor-backed
/// intrinsic builders reject missing operands before mutating the block.
#[test]
fn descriptor_call_builder_rejects_wrong_argument_count() -> Result<(), IrError> {
    Module::with_new("intrinsic-call-arity", |m| {
        let i32_ty = m.i32_type();
        let descriptor = IntrinsicDescriptor::new(IntrinsicId::ABS, [i32_ty.as_type()])?;
        let caller_ty = m.fn_type(i32_ty.as_type(), [i32_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let x: IntValue<i32> = caller.param(0)?.try_into()?;
        let err = b
            .build_intrinsic_call(&descriptor, &[x.as_value()], "bad")
            .expect_err("missing immarg is rejected before call emission");
        assert!(
            matches!(err, IrError::IntrinsicSignatureMismatch { .. }),
            "{err:?}"
        );
        Ok(())
    })
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::llvm_vararg_ty`: generated IIT
/// entries preserve LLVM's vararg marker instead of encoding it as `void`.
#[test]
fn generated_vararg_intrinsic_decodes_vararg_function_type() -> Result<(), IrError> {
    Module::with_new("intrinsic-vararg", |m| {
        let id = IntrinsicId::lookup("llvm.localescape").expect("localescape intrinsic");
        let fn_ty = id.function_type(&m, &[])?;
        assert!(fn_ty.is_var_arg());
        assert_eq!(fn_ty.params().len(), 0);
        assert_eq!(format!("{fn_ty}"), "void (...)");
        Ok(())
    })
}

/// Mirrors `IntrinsicsWebAssembly.td::int_wasm_ref_null_exn`: WebAssembly
/// exception references are a distinct primitive, not a pointer address space.
#[test]
fn wasm_exnref_intrinsic_uses_distinct_primitive_type() -> Result<(), IrError> {
    Module::with_new("intrinsic-wasm-exnref", |m| {
        let id = IntrinsicId::lookup("llvm.wasm.ref.null.exn").expect("wasm exnref intrinsic");
        let fn_ty = id.function_type(&m, &[])?;
        assert_eq!(fn_ty.return_type(), m.wasm_exnref_type());
        assert_eq!(format!("{fn_ty}"), "exnref ()");
        Ok(())
    })
}

/// Mirrors `llvm/include/llvm/IR/Type.h::TypeID`: `exnref` and `x86_amx`
/// are primitive type IDs, not target-extension payloads, while
/// `llvm/lib/IR/Type.cpp::TargetExtType::get` still models `target("...")`.
#[test]
fn primitive_and_target_extension_types_widen_to_distinct_type_enum_variants() -> Result<(), IrError>
{
    Module::with_new("intrinsic-type-enum", |m| {
        assert!(matches!(
            AnyTypeEnum::from(m.wasm_exnref_type()),
            AnyTypeEnum::WasmExnRef(_)
        ));
        assert!(matches!(
            AnyTypeEnum::from(m.x86_amx_type()),
            AnyTypeEnum::X86Amx(_)
        ));

        let target_ext = m
            .target_ext_type("dx.Resource", Vec::<Type>::new(), Vec::<u32>::new())
            .as_type();
        assert!(matches!(
            AnyTypeEnum::from(target_ext),
            AnyTypeEnum::TargetExt(_)
        ));
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Intrinsics.cpp::getMangledTypeStr`: every target
/// extension type spelling emitted for an overload can be parsed back.
#[test]
fn target_extension_overload_name_round_trips() -> Result<(), IrError> {
    Module::with_new("intrinsic-target-ext", |m| {
        let handle = m
            .target_ext_type("dx.Resource", Vec::<Type>::new(), Vec::<u32>::new())
            .as_type();
        let id = IntrinsicId::lookup("llvm.dx.resource.handlefrombinding.tdx.Resourcet")
            .expect("dx handle intrinsic");
        let descriptor = IntrinsicDescriptor::new(id, [handle])?;
        let name = descriptor.mangled_name()?;
        assert_eq!(name, "llvm.dx.resource.handlefrombinding.tdx.Resourcet");
        let matched =
            m.intrinsic_descriptor_from_signature(&name, descriptor.function_type(&m)?)?;
        assert_eq!(matched, descriptor);

        let element = m
            .struct_type(
                [
                    m.vector_type(m.f32_type().as_type(), 4, false).as_type(),
                    m.vector_type(m.i32_type().as_type(), 4, false).as_type(),
                ],
                false,
            )
            .as_type();
        let raw_buffer = m
            .target_ext_type("dx.RawBuffer", [element], [0, 0])
            .as_type();
        let raw_name = "llvm.dx.resource.handlefrombinding.tdx.RawBuffer_sl_v4f32v4i32s_0_0t";
        let raw_id = IntrinsicId::lookup(raw_name).expect("dx raw buffer handle intrinsic");
        let raw_descriptor = IntrinsicDescriptor::new(raw_id, [raw_buffer])?;
        assert_eq!(raw_descriptor.mangled_name()?, raw_name);
        let raw_matched =
            m.intrinsic_descriptor_from_signature(raw_name, raw_descriptor.function_type(&m)?)?;
        assert_eq!(raw_matched, raw_descriptor);

        let layout_struct = m.named_struct("__cblayout_d").as_type();
        let layout = m
            .target_ext_type("dx.Layout", [layout_struct], [16, 0, 4, 8, 12])
            .as_type();
        let cbuffer = m
            .target_ext_type("dx.CBuffer", [layout], Vec::<u32>::new())
            .as_type();
        let cbuffer_name = concat!(
            "llvm.dx.resource.handlefromimplicitbinding.tdx.CBuffer_",
            "tdx.Layout_s___cblayout_ds_16_0_4_8_12tt",
        );
        let cbuffer_id = IntrinsicId::lookup(cbuffer_name).expect("dx cbuffer handle intrinsic");
        let cbuffer_descriptor = IntrinsicDescriptor::new(cbuffer_id, [cbuffer])?;
        assert_eq!(cbuffer_descriptor.mangled_name()?, cbuffer_name);
        let cbuffer_matched = m.intrinsic_descriptor_from_signature(
            cbuffer_name,
            cbuffer_descriptor.function_type(&m)?,
        )?;
        assert_eq!(cbuffer_matched, cbuffer_descriptor);
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Intrinsics.cpp::getMangledTypeStr`: function overload
/// suffixes close with `f`, preserve `vararg`, and can contain named structs.
#[test]
fn function_and_named_struct_mangled_overload_suffix_round_trips() -> Result<(), IrError> {
    Module::with_new("intrinsic-function-struct-overload", |m| {
        let point = m.named_struct("Point").as_type();
        let fn_overload = m
            .fn_type(
                m.i32_type().as_type(),
                [point, m.ptr_type(0).as_type()],
                true,
            )
            .as_type();
        let name = "llvm.dx.resource.handlefrombinding.f_i32s_Pointsp0varargf";
        let id = IntrinsicId::lookup(name).expect("dx function overload intrinsic");
        let descriptor = IntrinsicDescriptor::new(id, [fn_overload])?;
        assert_eq!(descriptor.mangled_name()?, name);
        let matched = m.intrinsic_descriptor_from_signature(name, descriptor.function_type(&m)?)?;
        assert_eq!(matched, descriptor);
        Ok(())
    })
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::LLVMVectorOfAnyPointersToElt`:
/// vector-of-pointer overloads must stay vectors with the referenced lane count.
#[test]
fn vector_pointer_overloads_reject_shape_and_lane_mismatches() -> Result<(), IrError> {
    Module::with_new("intrinsic-vector-pointer-overloads", |m| {
        let value_vec = m.vector_type(m.i32_type().as_type(), 4, false).as_type();
        let ptr_vec = m.vector_type(m.ptr_type(0).as_type(), 4, false).as_type();
        let id =
            IntrinsicId::lookup("llvm.masked.gather.v4i32.v4p0").expect("masked gather intrinsic");
        let descriptor = IntrinsicDescriptor::new(id, [value_vec, ptr_vec])?;
        let name = descriptor.mangled_name()?;
        assert_eq!(name, "llvm.masked.gather.v4i32.v4p0");
        let fn_ty = descriptor.function_type(&m)?;
        assert_eq!(
            format!("{fn_ty}"),
            "<4 x i32> (<4 x ptr>, <4 x i1>, <4 x i32>)"
        );
        let matched = m.intrinsic_descriptor_from_signature(&name, fn_ty)?;
        assert_eq!(matched, descriptor);

        assert!(IntrinsicDescriptor::new(id, [value_vec, m.ptr_type(0).as_type()]).is_err());
        assert!(
            IntrinsicDescriptor::new(
                id,
                [
                    value_vec,
                    m.vector_type(m.ptr_type(0).as_type(), 2, false).as_type(),
                ],
            )
            .is_err()
        );
        assert!(IntrinsicDescriptor::new(id, [m.i32_type().as_type(), ptr_vec]).is_err());
        Ok(())
    })
}

/// Mirrors `llvm/include/llvm/IR/Intrinsics.td::LLVMVectorElementType`
/// and `llvm/lib/IR/Intrinsics.cpp::matchIntrinsicType`: vector-element
/// overloads require a vector overload and reject scalar descriptors.
#[test]
fn vector_element_overloads_reject_scalar_descriptors() -> Result<(), IrError> {
    Module::with_new("intrinsic-vector-element-overloads", |m| {
        let vec = m.vector_type(m.f32_type().as_type(), 4, false).as_type();
        let id = IntrinsicId::lookup("llvm.spv.length").expect("spv length intrinsic");
        let descriptor = IntrinsicDescriptor::new(id, [vec])?;
        assert_eq!(
            format!("{}", descriptor.function_type(&m)?),
            "float (<4 x float>)"
        );
        assert!(IntrinsicDescriptor::new(id, [m.f32_type().as_type()]).is_err());
        Ok(())
    })
}

/// Mirrors `IntrinsicsSPIRV.td::int_spv_fdot` and
/// `llvm/lib/IR/Intrinsics.cpp::matchIntrinsicType`: `SameVecWidthArgument`
/// may carry a nested `VecElementArgument` descriptor that must be matched
/// with the already-resolved vector overload.
#[test]
fn same_vec_width_nested_vector_element_signature_matches() -> Result<(), IrError> {
    Module::with_new("intrinsic-same-vec-width-vector-element", |m| {
        let vec = m.vector_type(m.f32_type().as_type(), 4, false).as_type();
        let id = IntrinsicId::lookup("llvm.spv.fdot.v4f32").expect("spv fdot intrinsic");
        let descriptor = IntrinsicDescriptor::new(id, [vec])?;
        let fn_ty = descriptor.function_type(&m)?;

        assert_eq!(format!("{fn_ty}"), "float (<4 x float>, <4 x float>)");
        assert_eq!(
            m.intrinsic_descriptor_from_signature("llvm.spv.fdot.v4f32", fn_ty)?,
            descriptor
        );
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/AsmWriter.cpp::printInstruction` and
/// `llvm/utils/TableGen/Basic/IntrinsicEmitter.cpp::EmitPrettyPrintArguments`:
/// generated `ArgName`/`ImmArgPrinter` metadata is emitted as inline
/// comments before intrinsic immediate operands.
#[test]
fn asm_writer_prints_generated_intrinsic_immediate_argument_comments() -> Result<(), IrError> {
    Module::with_new("intrinsic-pretty-immarg", |m| {
        let ptr_ty = m.ptr_type(0);
        let i32_ty = m.i32_type();
        let id = IntrinsicId::lookup("llvm.nvvm.tensormap.replace.fill.mode")
            .expect("nvvm tensormap replace fill mode intrinsic");
        let descriptor = IntrinsicDescriptor::new(id, [ptr_ty.as_type()])?;
        let caller_ty = m.fn_type(m.void_type().as_type(), [ptr_ty.as_type()], false);
        let caller = m.add_function_dyn("caller", caller_ty, Linkage::External)?;
        let entry = caller.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let ptr: PointerValue = caller.param(0)?.try_into()?;
        b.build_intrinsic_call(
            &descriptor,
            &[ptr.as_value(), i32_ty.const_int(1_i32).as_value()],
            "",
        )?;
        b.build_ret_void()?;

        let text = format!("{m}");
        assert!(
            text.contains("/* fill_mode=OOB-NaN fill */ i32 1"),
            "{text}"
        );
        Ok(())
    })
}
