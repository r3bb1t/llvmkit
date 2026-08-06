//! `inalloca` / `swifterror` alloca markers: AsmWriter printing and the
//! `Verifier::visitAllocaInst` swifterror constraints (must be a non-array
//! pointer allocation).

use llvmkit_ir::{
    InstructionKind, InstructionView, IntDyn, IntValue, IrBuilder, IrError, Linkage, NoFolder,
    VerifierRule, module_new,
};

/// A swifterror alloca on a pointer type verifies and prints
/// `alloca swifterror ptr`, and reads back through
/// `AllocaInst::isSwiftError` / `isUsedWithInAlloca`. Constructed through the
/// [`llvmkit_ir::AllocaBuilder`] chain.
#[test]
fn swifterror_pointer_alloca_verifies_and_prints() -> Result<(), IrError> {
    let m = module_new!("se")?;
    let fn_ty = m.function_type_no_parameters(m.void_type().as_type());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let e = b
        .alloca_builder(m.ptr_type(0))
        .swifterror()
        .name("e")
        .build()?;
    b.ret_void()?;
    m.verify_borrowed()?;
    let Some(InstructionKind::Alloca(alloca)) =
        InstructionView::try_from(m.view(e).as_erased())?.kind()
    else {
        panic!("expected %e to be an alloca");
    };
    assert!(alloca.is_swifterror());
    assert!(!alloca.is_inalloca());
    let text = format!("{m}");
    assert!(
        text.contains("%e = alloca swifterror ptr, align 8"),
        "{text}"
    );
    Ok(())
}

/// An inalloca alloca prints `alloca inalloca <ty>`, and reads back through
/// `AllocaInst::isUsedWithInAlloca` / `isSwiftError`. Constructed through
/// the [`llvmkit_ir::AllocaBuilder`] chain.
#[test]
fn inalloca_alloca_prints() -> Result<(), IrError> {
    let m = module_new!("ia")?;
    let fn_ty = m.function_type_no_parameters(m.void_type().as_type());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let i = b
        .alloca_builder(m.i32_type())
        .inalloca()
        .name("i")
        .build()?;
    b.ret_void()?;
    let Some(InstructionKind::Alloca(alloca)) =
        InstructionView::try_from(m.view(i).as_erased())?.kind()
    else {
        panic!("expected %i to be an alloca");
    };
    assert!(alloca.is_inalloca());
    assert!(!alloca.is_swifterror());
    let text = format!("{m}");
    assert!(text.contains("%i = alloca inalloca i32, align 4"), "{text}");
    Ok(())
}

/// `Verifier::visitAllocaInst`: a swifterror alloca must have pointer type.
/// Constructed through the [`llvmkit_ir::AllocaBuilder`] chain.
#[test]
fn swifterror_non_pointer_alloca_rejected() -> Result<(), IrError> {
    let m = module_new!("se")?;
    let fn_ty = m.function_type_no_parameters(m.void_type().as_type());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    b.alloca_builder(m.i32_type())
        .swifterror()
        .name("e")
        .build()?;
    b.ret_void()?;
    let err = m
        .verify_borrowed()
        .expect_err("swifterror i32 alloca must be rejected");
    let IrError::VerifierFailure { rule, .. } = err else {
        panic!("expected VerifierFailure, got {err:?}");
    };
    assert_eq!(rule, VerifierRule::SwiftErrorAlloca);
    Ok(())
}

/// `Verifier::visitAllocaInst`: a swifterror alloca must not be an array
/// allocation. Constructed through the [`llvmkit_ir::AllocaBuilder`] chain
/// (`.array(..)` supplies the size operand).
#[test]
fn swifterror_array_alloca_rejected() -> Result<(), IrError> {
    let m = module_new!("se")?;
    let i32_ty = m.custom_width_int_type(32)?;
    let fn_ty = m.function_type_no_parameters(m.void_type().as_type());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let count: IntValue<IntDyn, _> = i32_ty.const_int_checked(4_i64)?.as_erased().try_into()?;
    b.alloca_builder(m.ptr_type(0))
        .array(count)
        .swifterror()
        .name("e")
        .build()?;
    b.ret_void()?;
    let err = m
        .verify_borrowed()
        .expect_err("array swifterror alloca must be rejected");
    let IrError::VerifierFailure { rule, .. } = err else {
        panic!("expected VerifierFailure, got {err:?}");
    };
    assert_eq!(rule, VerifierRule::SwiftErrorAlloca);
    Ok(())
}

/// `isArrayAllocation()` is false for a constant-`1` size, so a swifterror
/// alloca with an explicit `i32 1` size is valid (not an array allocation),
/// and the canonical size is dropped when printed (AsmWriter suppresses it).
/// Constructed through the [`llvmkit_ir::AllocaBuilder`] chain.
#[test]
fn swifterror_size_one_alloca_verifies_and_drops_canonical_size() -> Result<(), IrError> {
    let m = module_new!("se1")?;
    let i32_ty = m.custom_width_int_type(32)?;
    let fn_ty = m.function_type_no_parameters(m.void_type().as_type());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let one: IntValue<IntDyn, _> = i32_ty.const_int_checked(1_i64)?.as_erased().try_into()?;
    b.alloca_builder(m.ptr_type(0))
        .array(one)
        .swifterror()
        .name("e")
        .build()?;
    b.ret_void()?;
    m.verify_borrowed()?;
    let text = format!("{m}");
    assert!(
        text.contains("%e = alloca swifterror ptr, align 8"),
        "{text}"
    );
    assert!(
        !text.contains(", i32 1"),
        "canonical size must be dropped:\n{text}"
    );
    Ok(())
}
