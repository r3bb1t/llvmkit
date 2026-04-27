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
    AShrFlags, AddFlags, IRBuilder, Instruction, InstructionKind, IrError, LShrFlags, Linkage,
    Module, MulFlags, SDivFlags, ShlFlags, SubFlags, UDivFlags,
};

/// Port of `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`
/// for the `add nuw nsw` case: builds the instruction with both flags
/// set and asserts both bits round-trip on the `Add` handle.
#[test]
fn add_nuw_nsw_flags_round_trip() -> Result<(), IrError> {
    let m = Module::new("flags");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<i32>("addf", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let r = b.build_int_add_with_flags::<i32, _, _>(
        f.param(0)?,
        f.param(1)?,
        AddFlags::new().nuw().nsw(),
        "r",
    )?;
    let inst: Instruction = r.as_value().try_into()?;
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
    let m = Module::new("f");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);

    let sub_fn = m.add_function::<i32>("sub_f", fn_ty, Linkage::External)?;
    let entry = sub_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let r = b.build_int_sub_with_flags::<i32, _, _>(
        sub_fn.param(0)?,
        sub_fn.param(1)?,
        SubFlags::new().nuw(),
        "r",
    )?;
    let inst: Instruction = r.as_value().try_into()?;
    if let Some(InstructionKind::Sub(s)) = inst.kind() {
        assert!(s.has_no_unsigned_wrap());
    } else {
        panic!("expected Sub");
    }
    b.build_ret(r)?;

    let mul_fn = m.add_function::<i32>("mul_f", fn_ty, Linkage::External)?;
    let entry = mul_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let r = b.build_int_mul_with_flags::<i32, _, _>(
        mul_fn.param(0)?,
        mul_fn.param(1)?,
        MulFlags::new().nuw(),
        "r",
    )?;
    let inst: Instruction = r.as_value().try_into()?;
    if let Some(InstructionKind::Mul(s)) = inst.kind() {
        assert!(s.has_no_unsigned_wrap());
    } else {
        panic!("expected Mul");
    }
    b.build_ret(r)?;

    let shl_fn = m.add_function::<i32>("shl_f", fn_ty, Linkage::External)?;
    let entry = shl_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let r = b.build_int_shl_with_flags::<i32, _, _>(
        shl_fn.param(0)?,
        shl_fn.param(1)?,
        ShlFlags::new().nuw(),
        "r",
    )?;
    let inst: Instruction = r.as_value().try_into()?;
    if let Some(InstructionKind::Shl(s)) = inst.kind() {
        assert!(s.has_no_unsigned_wrap());
    } else {
        panic!("expected Shl");
    }
    b.build_ret(r)?;
    Ok(())
}

/// llvmkit-specific extension of `IRBuilderTest::WrapFlags` to the
/// `exact` flag on `udiv` / `sdiv` / `lshr` / `ashr`. Upstream
/// `WrapFlags` covers nuw/nsw on `add`/`sub`/`mul`/`shl`; the `exact`
/// flag is exercised in upstream lit fixtures (`test/Assembler/flags.ll`)
/// rather than in `IRBuilderTest`. Closest IRBuilder analogue:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)`.
#[test]
fn div_shr_exact_round_trip() -> Result<(), IrError> {
    let m = Module::new("e");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);

    let udiv_fn = m.add_function::<i32>("udiv_f", fn_ty, Linkage::External)?;
    let entry = udiv_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let r = b.build_int_udiv_with_flags::<i32, _, _>(
        udiv_fn.param(0)?,
        udiv_fn.param(1)?,
        UDivFlags::new().exact(),
        "r",
    )?;
    let inst: Instruction = r.as_value().try_into()?;
    if let Some(InstructionKind::UDiv(s)) = inst.kind() {
        assert!(s.is_exact());
    } else {
        panic!("expected UDiv");
    }
    b.build_ret(r)?;

    let sdiv_fn = m.add_function::<i32>("sdiv_f", fn_ty, Linkage::External)?;
    let entry = sdiv_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let r = b.build_int_sdiv_with_flags::<i32, _, _>(
        sdiv_fn.param(0)?,
        sdiv_fn.param(1)?,
        SDivFlags::new().exact(),
        "r",
    )?;
    let inst: Instruction = r.as_value().try_into()?;
    if let Some(InstructionKind::SDiv(s)) = inst.kind() {
        assert!(s.is_exact());
    } else {
        panic!("expected SDiv");
    }
    b.build_ret(r)?;

    let lshr_fn = m.add_function::<i32>("lshr_f", fn_ty, Linkage::External)?;
    let entry = lshr_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let r = b.build_int_lshr_with_flags::<i32, _, _>(
        lshr_fn.param(0)?,
        lshr_fn.param(1)?,
        LShrFlags::new().exact(),
        "r",
    )?;
    let inst: Instruction = r.as_value().try_into()?;
    if let Some(InstructionKind::LShr(s)) = inst.kind() {
        assert!(s.is_exact());
    } else {
        panic!("expected LShr");
    }
    b.build_ret(r)?;

    let ashr_fn = m.add_function::<i32>("ashr_f", fn_ty, Linkage::External)?;
    let entry = ashr_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let r = b.build_int_ashr_with_flags::<i32, _, _>(
        ashr_fn.param(0)?,
        ashr_fn.param(1)?,
        AShrFlags::new().exact(),
        "r",
    )?;
    let inst: Instruction = r.as_value().try_into()?;
    if let Some(InstructionKind::AShr(s)) = inst.kind() {
        assert!(s.is_exact());
    } else {
        panic!("expected AShr");
    }
    b.build_ret(r)?;
    Ok(())
}
