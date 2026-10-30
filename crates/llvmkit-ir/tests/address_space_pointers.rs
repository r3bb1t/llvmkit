//! Pointers carry their LLVM address space in their Rust type: `Space<N>` when
//! the call fixes it, `SpaceDyn` when only run-time data knows it.
//!
//! Every test here pins a Rust type-level marker, which upstream cannot have
//! (an LLVM `PointerType *` carries its address space only at run time), so
//! each is llvmkit-specific (D3, D4). The run-time address-space behaviour of
//! the same builders is pinned by ported tests: `builder_cast_ptr_int.rs`
//! (`TEST(InstructionsTest, CastInst)`), `builder_gep.rs`
//! (`TEST(InstructionsTest, GEPIndices)`) and the corpus's
//! `test/Assembler/2007-12-11-AddressSpaces.ll`.

use llvmkit_ir::{
    AddressSpace, ConstantIntValue, ConstantPointerNull, InstructionKind, InstructionView, IntDyn,
    IntValue, IrBuilder, IrError, IrStruct, Linkage, Module, NoFolder, PointerPhiInstId,
    PointerType, PointerValue, PointerValueId, PointerVariable, Ptr, Space, SpaceDyn, SsaBuilder,
    SsaState, TypedPointerValue, address_space, module_new,
};

/// `ptr_type_in::<N>()` names its address space in its type; `ptr_type(n)`,
/// whose space is a run-time number, is `SpaceDyn`. Both answer the same
/// run-time address space. llvmkit-specific (D3).
#[test]
fn a_typed_pointer_type_carries_its_address_space() {
    let m = Module::dynamic("spaces");
    let p: PointerType<'_, Space<3>, _> = m.ptr_type_in::<3>();
    assert_eq!(p.address_space(), 3);
    assert_eq!(p.to_string(), "ptr addrspace(3)");
    let q: PointerType<'_, SpaceDyn, _> = m.ptr_type(address_space::<3>());
    assert_eq!(q.address_space(), 3);
}

/// An address space is 24 bits, and [`AddressSpace`] holds no more:
/// `Type::setSubclassData` (`include/llvm/IR/Type.h`) asserts that a pointer's
/// fits, and `LLParser::parseOptionalAddrSpace` refuses anything wider. So
/// every route that names an address space takes one. llvmkit-specific
/// regression (D10): `ptr_type(1 << 24)` built `ptr addrspace(16777216)`,
/// which no parser reads back. The largest space is the positive control.
#[test]
fn an_address_space_holds_24_bits() {
    assert_eq!(AddressSpace::new(0xFF_FFFF), Some(AddressSpace::MAX));
    assert_eq!(AddressSpace::new(1 << 24), None);
    assert_eq!(AddressSpace::new(u32::MAX), None);
    assert_eq!(AddressSpace::default(), AddressSpace::DEFAULT);
    assert_eq!(address_space::<7>().get(), 7);
    assert_eq!(address_space::<7>().to_string(), "7");
    assert_eq!(u32::from(address_space::<7>()), 7);

    let m = Module::dynamic("spaces");
    assert_eq!(
        m.ptr_type(AddressSpace::MAX).to_string(),
        "ptr addrspace(16777215)"
    );
}

/// Narrowing a `SpaceDyn` pointer to `Space<N>` checks the run-time address
/// space and refuses another one with `IrError::AddressSpaceMismatch`, and
/// `as_dyn` erases the marker back to the same pointer, on the handle and on
/// its id. llvmkit-specific (D3): the two doors between the tiers.
#[test]
fn narrowing_a_dyn_pointer_checks_its_address_space() -> Result<(), IrError> {
    let m = Module::dynamic("spaces");
    let fn_ty = m.function_type(m.void_type(), [m.ptr_type_in::<1>().as_type()])?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let arg: PointerValue<'_, SpaceDyn, _> = m.view(f).param(0)?.try_into()?;
    let one: Result<PointerValue<'_, Space<1>, _>, IrError> = PointerValue::try_from(arg);
    assert!(one.is_ok(), "{one:?}");
    let zero: Result<PointerValue<'_, Space<0>, _>, IrError> = PointerValue::try_from(arg);
    assert_eq!(
        zero.err(),
        Some(IrError::AddressSpaceMismatch {
            expected: 0,
            got: 1
        })
    );
    let one = one?;
    let erased: PointerValue<'_, SpaceDyn, _> = one.as_dyn();
    assert_eq!(erased, arg);
    let erased_id: PointerValueId<SpaceDyn, _> = one.id().as_dyn();
    assert_eq!(erased_id, arg.id());
    Ok(())
}

/// A pointer in a non-zero address space keeps it, in its type, through GEP,
/// `select`, a pointer phi of that address space and `addrspacecast` (the
/// plan's Review Focus 2). llvmkit-specific (D4): result types reflect operand
/// types.
#[test]
fn gep_select_phi_and_casts_keep_their_address_space() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    let f =
        m.add_typed_function_of::<fn(Ptr<3>, Ptr<3>, bool) -> Ptr<5>, _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (p, q, c): (
        PointerValue<'_, Space<3>, _>,
        PointerValue<'_, Space<3>, _>,
        _,
    ) = m.view(f).params();
    let no_indices: [ConstantIntValue<'_, IntDyn, _>; 0] = [];
    let g: PointerValueId<Space<3>, _> = b.gep(m.i8_type(), p, no_indices, "g")?;
    let s: PointerValueId<Space<3>, _> = b.select(c, b.view(g), q, "s")?;
    let phi: PointerPhiInstId<Space<3>, _> =
        b.pointer_phi_in_addrspace(m.ptr_type_in::<3>(), "phi")?;
    let _: PointerValue<'_, Space<3>, _> = b.view(phi).as_pointer_value();
    let r: PointerValueId<Space<5>, _> = b.addrspace_cast(s, m.ptr_type_in::<5>(), "r")?;
    b.ret(r)?;
    let text = format!("{m}");
    for line in [
        "%g = getelementptr i8, ptr addrspace(3) %0",
        "%s = select i1 %2, ptr addrspace(3) %g, ptr addrspace(3) %1",
        "%phi = phi ptr addrspace(3)",
        "%r = addrspacecast ptr addrspace(3) %s to ptr addrspace(5)",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
    Ok(())
}

/// The default alloca takes the DataLayout's alloca address space, a run-time
/// fact, so its pointer is `SpaceDyn`; `alloca_in::<N>` names the space.
/// llvmkit-specific (D3).
#[test]
fn a_typed_alloca_is_in_its_space_and_the_default_is_dyn() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    let f = m.add_typed_function_of::<fn(), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let d: PointerValueId<SpaceDyn, _> = b.alloca(m.i32_type(), "d")?;
    let a: PointerValueId<Space<0>, _> = b.alloca_in::<0, _, _>(m.i32_type(), "a")?;
    assert_eq!(b.view(d).ty().address_space(), 0);
    assert_eq!(b.view(a).ty().address_space(), 0);
    Ok(())
}

/// `pointer_load` loads a `ptr` — address space 0 — and says so in its type;
/// `pointer_load_in::<N>` loads a `ptr addrspace(N)`. llvmkit-specific (D4).
#[test]
fn pointer_load_names_the_space_it_loads() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    let f = m.add_typed_function_of::<fn(), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let slot = b.alloca_in::<0, _, _>(m.ptr_type_in::<0>(), "slot")?;
    let p: PointerValueId<Space<0>, _> = b.pointer_load(slot, "p")?;
    let q: PointerValueId<Space<1>, _> = b.pointer_load_in::<1, _, _>(slot, "q")?;
    let r: PointerValueId<Space<0>, _> = b.load_from(slot).pointer("r")?;
    let _ = (p, q, r);
    let text = format!("{m}");
    assert!(text.contains("%p = load ptr, ptr %slot"), "{text}");
    assert!(text.contains("%r = load ptr, ptr %slot"), "{text}");
    assert!(
        text.contains("%q = load ptr addrspace(1), ptr %slot"),
        "{text}"
    );
    Ok(())
}

/// `Ptr` is address space 0 and `Ptr<N>` address space `N`, in a typed
/// signature and in a return-type check, which now requires the space exactly
/// (it accepted any before). llvmkit-specific (D4).
#[test]
fn ptr_means_address_space_zero() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    m.add_typed_function_of::<fn(Ptr) -> Ptr, _>("zero", Linkage::External)?;
    m.add_typed_function_of::<fn(Ptr<1>), _>("one", Linkage::External)?;
    let text = format!("{m}");
    assert!(text.contains("declare ptr @zero(ptr %0)"), "{text}");
    assert!(
        text.contains("declare void @one(ptr addrspace(1) %0)"),
        "{text}"
    );

    let fn_ty = m.function_type_no_parameters(m.ptr_type_in::<1>().as_type())?;
    m.add_function_dyn("ret_one", fn_ty, Linkage::External)?;
    assert!(m.function::<Ptr>("ret_one").is_err());
    assert!(m.function::<Ptr<1>>("ret_one")?.is_some());
    Ok(())
}

/// A cast's result is in its destination type's address space: `int_to_ptr`
/// to `ptr_type_in::<2>()` is `Space<2>`, and `pointer_cast` to `ptr_type(3)`
/// — a run-time space — is `SpaceDyn`. llvmkit-specific (D4).
#[test]
fn casts_take_the_destination_types_address_space() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    let f = m.add_typed_function_of::<fn(i64), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (x,) = m.view(f).params();
    let p: PointerValueId<Space<2>, _> =
        b.int_to_ptr::<i64, _, _, _>(x, m.ptr_type_in::<2>(), "p")?;
    let q: PointerValueId<SpaceDyn, _> =
        b.pointer_cast(p, m.ptr_type(address_space::<3>()), "q")?;
    let _ = q;
    let text = format!("{m}");
    for line in [
        "%p = inttoptr i64 %0 to ptr addrspace(2)",
        "%q = addrspacecast ptr addrspace(2) %p to ptr addrspace(3)",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
    Ok(())
}

/// A call to a `Ptr<N>`-returning callee yields a `Space<N>` pointer, from
/// the typed `call`'s result and from `call_dyn`'s `return_pointer_value`
/// alike, and a `Ptr<N>` parameter is `Space<N>`. llvmkit-specific (D4).
#[test]
fn a_ptr_call_result_is_in_the_callees_address_space() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    let callee = m.add_typed_function_of::<fn(Ptr<1>) -> Ptr<1>, _>("id1", Linkage::External)?;
    let f = m.add_typed_function_of::<fn(Ptr<1>), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (p,): (PointerValue<'_, Space<1>, _>,) = m.view(f).params();
    let call = b.call(callee, (p,), "r")?;
    let r: PointerValue<'_, Space<1>, _> = b.view(call).result();
    let erased_call = b.call_dyn(callee.as_function(), [r.as_erased()], "s")?;
    let s: PointerValue<'_, Space<1>, _> = b.view(erased_call).return_pointer_value();
    let _ = s;
    let text = format!("{m}");
    for line in [
        "%r = call ptr addrspace(1) @id1(ptr addrspace(1) %0)",
        "%s = call ptr addrspace(1) @id1(ptr addrspace(1) %r)",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
    Ok(())
}

/// A `null` constant is in the address space of the pointer type it is made
/// from: `Space<4>` from `ptr_type_in::<4>()`, `SpaceDyn` from `ptr_type(4)`.
/// llvmkit-specific (D3).
#[test]
fn a_null_constant_is_in_its_types_address_space() {
    let m = Module::dynamic("spaces");
    let typed: ConstantPointerNull<'_, Space<4>, _> = m.ptr_type_in::<4>().const_null();
    let dynamic: ConstantPointerNull<'_, SpaceDyn, _> =
        m.ptr_type(address_space::<4>()).const_null();
    assert_eq!(typed.as_erased(), dynamic.as_erased());
    assert_eq!(typed.as_erased().to_string(), "ptr addrspace(4) null");
}

/// An instruction's pointer operand, read back through its accessor, is
/// `SpaceDyn` — the instruction stores no marker — and still answers the
/// operand's run-time address space. llvmkit-specific (D3).
#[test]
fn an_instruction_accessor_reads_a_dyn_pointer() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    let f = m.add_typed_function_of::<fn(Ptr<3>), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (p,): (PointerValue<'_, Space<3>, _>,) = m.view(f).params();
    let loaded = b.load(m.i32_type().as_type(), p, "v")?;
    let view = InstructionView::try_from(b.view(loaded))?;
    let Some(InstructionKind::Load(load)) = view.kind() else {
        panic!("expected a load instruction");
    };
    let read_back: PointerValue<'_, SpaceDyn, _> = load.pointer();
    assert_eq!(read_back.as_erased(), p.as_erased());
    assert_eq!(read_back.ty().address_space(), 3);
    Ok(())
}

/// A pointer variable keeps the address space it was declared in: the
/// default declaration is `Space<0>`, one from `ptr_type_in::<2>()` is
/// `Space<2>`, and a use reads the variable back in that space.
/// llvmkit-specific (D3).
#[test]
fn a_pointer_variable_keeps_its_address_space() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    let typed = m.add_typed_function::<(), (Ptr<2>,), _>("f", Linkage::External)?;
    let f = typed.as_function();
    let mut state = SsaState::for_function(&m, m.view(f))?;
    let mut b = SsaBuilder::for_function(&m, m.view(f), &mut state)?;
    let entry = b.create_block("entry");
    let zero: PointerVariable<Space<0>, _> = b.declare_pointer_var("zero");
    let two: PointerVariable<Space<2>, _> =
        b.declare_pointer_var_in_addrspace(m.ptr_type_in::<2>(), "two");
    b.switch_to_block(entry)?;
    let (p,): (PointerValue<'_, Space<2>, _>,) = m.view(typed).params();
    b.def_pointer_var(two, p)?;
    let read: PointerValue<'_, Space<2>, _> = b.use_pointer_var(two)?;
    assert_eq!(read.as_erased(), p.as_erased());
    let _ = zero;
    Ok(())
}

/// A pointee-typed pointer keeps its pointer's address space: `with_pointee`
/// on a `Space<3>` pointer is `Space<3>`, `element_gep` keeps it, and
/// `typed_alloca` — the DataLayout's alloca address space — is `SpaceDyn`.
/// llvmkit-specific (D3).
#[test]
fn a_typed_pointer_keeps_its_address_space() -> Result<(), IrError> {
    let m = module_new!("spaces")?;
    let f = m.add_typed_function_of::<fn(Ptr<3>), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (p,): (PointerValue<'_, Space<3>, _>,) = m.view(f).params();
    let typed: TypedPointerValue<'_, i32, Space<3>, _> = p.with_pointee::<i32>();
    let next: TypedPointerValue<'_, i32, Space<3>, _> =
        b.element_gep::<i32, _, i32, _, _>(typed, 1_i32, "next")?;
    let _: IntValue<'_, i32, _> = b.typed_load(next, "v")?;
    let slot: TypedPointerValue<'_, i32, SpaceDyn, _> = b.typed_alloca::<i32, _>("slot")?;
    let _ = slot;
    let text = format!("{m}");
    for line in [
        "%next = getelementptr i32, ptr addrspace(3) %0, i32 1",
        "%v = load i32, ptr addrspace(3) %next",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
    Ok(())
}

/// A derived struct's `Ptr<N>` field is a `ptr addrspace(N)` member, and a
/// plain `Ptr` field is `ptr`; the struct is Rust data too, built with
/// `Ptr::null`. llvmkit-specific (D4): the field type is the marker's
/// `IrField` impl.
#[test]
fn a_ptr_field_is_in_its_address_space() -> Result<(), IrError> {
    #[derive(IrStruct)]
    struct Spaced {
        near: Ptr,
        far: Ptr<1>,
    }

    let rust_value = Spaced {
        near: Ptr::null(),
        far: Ptr::null(),
    };
    assert!(rust_value.near.as_ptr().is_null() && rust_value.far.as_ptr().is_null());

    let m = module_new!("spaces")?;
    let _ = m.get_or_insert_struct_of::<Spaced>()?;
    let text = format!("{m}");
    assert!(
        text.contains("%Spaced = type { ptr, ptr addrspace(1) }"),
        "{text}"
    );
    Ok(())
}
