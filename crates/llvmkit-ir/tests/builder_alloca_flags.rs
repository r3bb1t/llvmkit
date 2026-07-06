//! `inalloca` / `swifterror` alloca markers: AsmWriter printing and the
//! `Verifier::visitAllocaInst` swifterror constraints (must be a non-array
//! pointer allocation).

use llvmkit_ir::{
    AllocaFlags, IRBuilder, IntDyn, IntValue, IrError, Linkage, MaybeAlign, Module, NoFolder, Type,
    VerifierRule,
};

/// A swifterror alloca on a pointer type verifies and prints
/// `alloca swifterror ptr`.
#[test]
fn swifterror_pointer_alloca_verifies_and_prints() -> Result<(), IrError> {
    Module::with_new("se", |m| {
        let fn_ty = m.fn_type(m.void_type().as_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        b.build_alloca_dyn(
            m.ptr_type(0),
            None,
            MaybeAlign::NONE,
            AllocaFlags::none().with_swifterror(),
            "e",
        )?;
        b.build_ret_void();
        m.verify_borrowed()?;
        let text = format!("{m}");
        assert!(
            text.contains("%e = alloca swifterror ptr, align 8"),
            "{text}"
        );
        Ok(())
    })
}

/// An inalloca alloca prints `alloca inalloca <ty>`.
#[test]
fn inalloca_alloca_prints() -> Result<(), IrError> {
    Module::with_new("ia", |m| {
        let fn_ty = m.fn_type(m.void_type().as_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        b.build_alloca_dyn(
            m.i32_type(),
            None,
            MaybeAlign::NONE,
            AllocaFlags::none().with_inalloca(),
            "i",
        )?;
        b.build_ret_void();
        let text = format!("{m}");
        assert!(text.contains("%i = alloca inalloca i32, align 4"), "{text}");
        Ok(())
    })
}

/// `Verifier::visitAllocaInst`: a swifterror alloca must have pointer type.
#[test]
fn swifterror_non_pointer_alloca_rejected() -> Result<(), IrError> {
    Module::with_new("se", |m| {
        let fn_ty = m.fn_type(m.void_type().as_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        b.build_alloca_dyn(
            m.i32_type(),
            None,
            MaybeAlign::NONE,
            AllocaFlags::none().with_swifterror(),
            "e",
        )?;
        b.build_ret_void();
        let err = m
            .verify_borrowed()
            .expect_err("swifterror i32 alloca must be rejected");
        let IrError::VerifierFailure { rule, .. } = err else {
            panic!("expected VerifierFailure, got {err:?}");
        };
        assert_eq!(rule, VerifierRule::SwiftErrorAlloca);
        Ok(())
    })
}

/// `Verifier::visitAllocaInst`: a swifterror alloca must not be an array
/// allocation.
#[test]
fn swifterror_array_alloca_rejected() -> Result<(), IrError> {
    Module::with_new("se", |m| {
        let i32_ty = m.custom_width_int_type(32)?;
        let fn_ty = m.fn_type(m.void_type().as_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let count: IntValue<IntDyn, _> = i32_ty.const_int_checked(4_i64)?.as_value().try_into()?;
        b.build_alloca_dyn(
            m.ptr_type(0),
            Some(count),
            MaybeAlign::NONE,
            AllocaFlags::none().with_swifterror(),
            "e",
        )?;
        b.build_ret_void();
        let err = m
            .verify_borrowed()
            .expect_err("array swifterror alloca must be rejected");
        let IrError::VerifierFailure { rule, .. } = err else {
            panic!("expected VerifierFailure, got {err:?}");
        };
        assert_eq!(rule, VerifierRule::SwiftErrorAlloca);
        Ok(())
    })
}
