//! `inalloca` / `swifterror` alloca markers: AsmWriter printing and the
//! `Verifier::visitAllocaInst` swifterror constraints (must be a non-array
//! pointer allocation).

use llvmkit_ir::{
    AllocaFlags, IntDyn, IntValue, IrBuilder, IrError, Linkage, MaybeAlign, NoFolder, VerifierRule,
    module_new,
};

/// A swifterror alloca on a pointer type verifies and prints
/// `alloca swifterror ptr`.
#[test]
fn swifterror_pointer_alloca_verifies_and_prints() -> Result<(), IrError> {
    let m = module_new!("se")?;
    let fn_ty = m.fn_type_no_params(m.void_type().as_type(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    b.alloca_dyn(
        m.ptr_type(0),
        None,
        MaybeAlign::NONE,
        None,
        AllocaFlags::none().with_swifterror(),
        "e",
    )?;
    b.ret_void()?;
    m.verify_borrowed()?;
    let text = format!("{m}");
    assert!(
        text.contains("%e = alloca swifterror ptr, align 8"),
        "{text}"
    );
    Ok(())
}

/// An inalloca alloca prints `alloca inalloca <ty>`.
#[test]
fn inalloca_alloca_prints() -> Result<(), IrError> {
    let m = module_new!("ia")?;
    let fn_ty = m.fn_type_no_params(m.void_type().as_type(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    b.alloca_dyn(
        m.i32_type(),
        None,
        MaybeAlign::NONE,
        None,
        AllocaFlags::none().with_inalloca(),
        "i",
    )?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(text.contains("%i = alloca inalloca i32, align 4"), "{text}");
    Ok(())
}

/// `Verifier::visitAllocaInst`: a swifterror alloca must have pointer type.
#[test]
fn swifterror_non_pointer_alloca_rejected() -> Result<(), IrError> {
    let m = module_new!("se")?;
    let fn_ty = m.fn_type_no_params(m.void_type().as_type(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    b.alloca_dyn(
        m.i32_type(),
        None,
        MaybeAlign::NONE,
        None,
        AllocaFlags::none().with_swifterror(),
        "e",
    )?;
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
/// allocation.
#[test]
fn swifterror_array_alloca_rejected() -> Result<(), IrError> {
    let m = module_new!("se")?;
    let i32_ty = m.custom_width_int_type(32)?;
    let fn_ty = m.fn_type_no_params(m.void_type().as_type(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let count: IntValue<IntDyn, _> = i32_ty.const_int_checked(4_i64)?.as_erased().try_into()?;
    b.alloca_dyn(
        m.ptr_type(0),
        Some(count),
        MaybeAlign::NONE,
        None,
        AllocaFlags::none().with_swifterror(),
        "e",
    )?;
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
#[test]
fn swifterror_size_one_alloca_verifies_and_drops_canonical_size() -> Result<(), IrError> {
    let m = module_new!("se1")?;
    let i32_ty = m.custom_width_int_type(32)?;
    let fn_ty = m.fn_type_no_params(m.void_type().as_type(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let one: IntValue<IntDyn, _> = i32_ty.const_int_checked(1_i64)?.as_erased().try_into()?;
    b.alloca_dyn(
        m.ptr_type(0),
        Some(one),
        MaybeAlign::NONE,
        None,
        AllocaFlags::none().with_swifterror(),
        "e",
    )?;
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
