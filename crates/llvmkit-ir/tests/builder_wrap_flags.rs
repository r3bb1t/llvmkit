//! Round-trip the per-opcode wrap/exact flags through the typed
//! `*Flags` setters and inspect the resulting `Instruction` handle.
//!
//! ## Upstream provenance
//!
//! Direct port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`,
//! which builds `add` / `sub` / `mul` / `shl` with `Builder.CreateNSW*` /
//! `CreateNUW*` / `CreateShl(..., NUW, NSW)` and asserts the resulting
//! `BinaryOperator` reports `hasNoSignedWrap()` /
//! `hasNoUnsignedWrap()`. The `*_exact` extensions on `udiv` / `sdiv`
//! / `lshr` / `ashr` follow the same shape against `isExact()`.

use llvmkit_ir::{
    AShrFlags, AddFlags, Dyn, InstructionKind, InstructionView, IntValue, IrBuilder, IrError,
    LShrFlags, Linkage, MulFlags, SDivFlags, ShlFlags, SubFlags, UDivFlags, module_new,
};

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`
/// for the `add nuw nsw` case: builds the instruction with both flags
/// set and asserts both bits round-trip on the `Add` handle.
#[test]
fn add_nuw_nsw_flags_round_trip() -> Result<(), IrError> {
    let m = module_new!("flags")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function_dyn("addf", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;
    let r = b.int_add_with_flags(lhs, rhs, AddFlags::new().nuw().nsw(), "r")?;
    let inst = InstructionView::try_from(b.view(r).as_erased())?;
    let add = match inst.kind() {
        Some(InstructionKind::Add(a)) => a,
        _ => panic!("expected Add"),
    };
    assert!(add.has_no_unsigned_wrap());
    assert!(add.has_no_signed_wrap());
    Ok(())
}

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`
/// for the `sub` / `mul` / `shl` `nuw` cases \u2014 each opcode runs its
/// own builder call with its own flag type, mirroring the per-opcode
/// `Builder.CreateNUW{Sub,Mul}` / `CreateShl(..., NUW=true, ...)`
/// branches in the upstream test.
#[test]
fn sub_mul_shl_flags_round_trip() -> Result<(), IrError> {
    // Each opcode runs its own builder call with its own flag type,
    // verifying the flags propagate to the per-opcode handle.
    let m = module_new!("f")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);

    let sub_fn = m.add_function_dyn("sub_f", fn_ty, Linkage::External)?;
    let entry = m.view(sub_fn).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(sub_fn).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(sub_fn).param(1)?.try_into()?;
    let r = b.int_sub_with_flags(lhs, rhs, SubFlags::new().nuw(), "r")?;
    let inst = InstructionView::try_from(b.view(r).as_erased())?;
    if let Some(InstructionKind::Sub(s)) = inst.kind() {
        assert!(s.has_no_unsigned_wrap());
    } else {
        panic!("expected Sub");
    }
    b.ret(r)?;

    let mul_fn = m.add_function_dyn("mul_f", fn_ty, Linkage::External)?;
    let entry = m.view(mul_fn).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(mul_fn).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(mul_fn).param(1)?.try_into()?;
    let r = b.int_mul_with_flags(lhs, rhs, MulFlags::new().nuw(), "r")?;
    let inst = InstructionView::try_from(b.view(r).as_erased())?;
    if let Some(InstructionKind::Mul(s)) = inst.kind() {
        assert!(s.has_no_unsigned_wrap());
    } else {
        panic!("expected Mul");
    }
    b.ret(r)?;

    let shl_fn = m.add_function_dyn("shl_f", fn_ty, Linkage::External)?;
    let entry = m.view(shl_fn).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(shl_fn).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(shl_fn).param(1)?.try_into()?;
    let r = b.int_shl_with_flags(lhs, rhs, ShlFlags::new().nuw(), "r")?;
    let inst = InstructionView::try_from(b.view(r).as_erased())?;
    if let Some(InstructionKind::Shl(s)) = inst.kind() {
        assert!(s.has_no_unsigned_wrap());
    } else {
        panic!("expected Shl");
    }
    b.ret(r)?;
    Ok(())
}

/// llvmkit-specific extension of `IRBuilderTest::WrapFlags` to the
/// `exact` flag on `udiv` / `sdiv` / `lshr` / `ashr`. Upstream
/// `WrapFlags` covers nuw/nsw on `add`/`sub`/`mul`/`shl`; the `exact`
/// flag is exercised in upstream lit fixtures (`test/Assembler/flags.ll`)
/// rather than in `IRBuilderTest`. Closest IrBuilder analogue:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`.
#[test]
fn div_shr_exact_round_trip() -> Result<(), IrError> {
    let m = module_new!("e")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);

    let udiv_fn = m.add_function_dyn("udiv_f", fn_ty, Linkage::External)?;
    let entry = m.view(udiv_fn).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(udiv_fn).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(udiv_fn).param(1)?.try_into()?;
    let r = b.int_udiv_with_flags(lhs, rhs, UDivFlags::new().exact(), "r")?;
    let inst = InstructionView::try_from(b.view(r).as_erased())?;
    if let Some(InstructionKind::UDiv(s)) = inst.kind() {
        assert!(s.is_exact());
    } else {
        panic!("expected UDiv");
    }
    b.ret(r)?;

    let sdiv_fn = m.add_function_dyn("sdiv_f", fn_ty, Linkage::External)?;
    let entry = m.view(sdiv_fn).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(sdiv_fn).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(sdiv_fn).param(1)?.try_into()?;
    let r = b.int_sdiv_with_flags(lhs, rhs, SDivFlags::new().exact(), "r")?;
    let inst = InstructionView::try_from(b.view(r).as_erased())?;
    if let Some(InstructionKind::SDiv(s)) = inst.kind() {
        assert!(s.is_exact());
    } else {
        panic!("expected SDiv");
    }
    b.ret(r)?;

    let lshr_fn = m.add_function_dyn("lshr_f", fn_ty, Linkage::External)?;
    let entry = m.view(lshr_fn).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(lshr_fn).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(lshr_fn).param(1)?.try_into()?;
    let r = b.int_lshr_with_flags(lhs, rhs, LShrFlags::new().exact(), "r")?;
    let inst = InstructionView::try_from(b.view(r).as_erased())?;
    if let Some(InstructionKind::LShr(s)) = inst.kind() {
        assert!(s.is_exact());
    } else {
        panic!("expected LShr");
    }
    b.ret(r)?;

    let ashr_fn = m.add_function_dyn("ashr_f", fn_ty, Linkage::External)?;
    let entry = m.view(ashr_fn).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(ashr_fn).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(ashr_fn).param(1)?.try_into()?;
    let r = b.int_ashr_with_flags(lhs, rhs, AShrFlags::new().exact(), "r")?;
    let inst = InstructionView::try_from(b.view(r).as_erased())?;
    if let Some(InstructionKind::AShr(s)) = inst.kind() {
        assert!(s.is_exact());
    } else {
        panic!("expected AShr");
    }
    b.ret(r)?;
    Ok(())
}
