//! Phase C cast-builder coverage. `trunc`/`zext`/`sext`
//! enforce static width invariants via [`llvmkit_ir::WiderThan`]; the
//! `_dyn` fallbacks keep the runtime check for `IntValue<Dyn>` paths.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
//! CastInst)` (the canonical cast-shape suite) and/or marks `llvmkit-specific:`
//! for the typestate-driven coverage that has no C++ analogue.

use llvmkit_ir::{
    CastOpcode, Constant, ConstantIntValue, Dyn, IntDyn, IntType, IntValue, IrBuilder, IrError,
    Linkage, TruncFlags, UiToFpFlags, VectorType, ZextFlags, cast_is_valid, lanes, module_new,
};

/// Ports the `CastInst::castIsValid` assertions of
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`, in
/// upstream's order, with upstream's types and expected answers. Upstream
/// calls the `castIsValid(Instruction::CastOps, const Value *, Type *)`
/// overload on `Constant::getNullValue(SrcTy)`, which asks
/// `castIsValid(op, S->getType(), DstTy)`; [`cast_is_valid`] is that type
/// form, so the source type is passed directly. The test's other checks
/// (`isBitCastable`, `getCastOpcode`, `CreatePointerCast`, …) are not part of
/// this port.
///
/// They pin that a fixed and a scalable vector of the same minimum count are
/// different shapes (`ElementCount` equality includes scalability), including
/// for a bitcast between equally sized vectors (`TypeSize` equality does too).
#[test]
fn cast_is_valid_answers_upstreams_cast_inst_assertions() {
    let m = module_new!("cast-is-valid").expect("fresh module");
    let i16_ty = m.i16_type();
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let ptr_ty = m.ptr_type_in::<0>();
    let ptr_as1_ty = m.ptr_type_in::<1>();

    let v2_int32 = VectorType::of(i32_ty, lanes::<2>()).as_type();
    let v2_int64 = VectorType::of(i64_ty, lanes::<2>()).as_type();
    let v4_int16 = VectorType::of(i16_ty, lanes::<4>()).as_type();
    let v1_int16 = VectorType::of(i16_ty, lanes::<1>()).as_type();

    let vscale_v2_int32 = VectorType::scalable_of(i32_ty, lanes::<2>()).as_type();
    let vscale_v2_int64 = VectorType::scalable_of(i64_ty, lanes::<2>()).as_type();
    let vscale_v4_int16 = VectorType::scalable_of(i16_ty, lanes::<4>()).as_type();
    let vscale_v1_int16 = VectorType::scalable_of(i16_ty, lanes::<1>()).as_type();

    let v4_ptr_as1 = VectorType::of(ptr_as1_ty, lanes::<4>()).as_type();
    let vscale_v4_ptr_as1 = VectorType::scalable_of(ptr_as1_ty, lanes::<4>()).as_type();

    let v2_ptr = VectorType::of(ptr_ty, lanes::<2>()).as_type();
    let v4_ptr = VectorType::of(ptr_ty, lanes::<4>()).as_type();
    let vscale_v2_ptr = VectorType::scalable_of(ptr_ty, lanes::<2>()).as_type();
    let vscale_v4_ptr = VectorType::scalable_of(ptr_ty, lanes::<4>()).as_type();

    use CastOpcode::{AddrSpaceCast, BitCast};
    let cases = [
        (BitCast, v4_ptr, v2_ptr, false),
        (BitCast, v2_ptr, v4_ptr, false),
        (AddrSpaceCast, v4_ptr_as1, v2_ptr, false),
        (AddrSpaceCast, v2_ptr, v4_ptr_as1, false),
        // Address space cast of fixed/scalable vectors of pointers to
        // scalable/fixed vector of pointers.
        (AddrSpaceCast, vscale_v4_ptr_as1, v4_ptr, false),
        (AddrSpaceCast, v4_ptr, vscale_v4_ptr_as1, false),
        // Address space cast of scalable vectors of pointers to scalable
        // vector of pointers.
        (AddrSpaceCast, vscale_v4_ptr_as1, vscale_v2_ptr, false),
        (AddrSpaceCast, vscale_v2_ptr, vscale_v4_ptr_as1, false),
        (AddrSpaceCast, vscale_v4_ptr, vscale_v4_ptr_as1, true),
        // Same number of lanes, different address space.
        (AddrSpaceCast, vscale_v4_ptr_as1, vscale_v4_ptr, true),
        // Same number of lanes, same address space.
        (AddrSpaceCast, vscale_v4_ptr, vscale_v4_ptr, false),
        // Bit casting fixed/scalable vector to scalable/fixed vectors.
        (BitCast, v2_int32, vscale_v2_int32, false),
        (BitCast, v2_int64, vscale_v2_int64, false),
        (BitCast, v4_int16, vscale_v4_int16, false),
        (BitCast, vscale_v2_int32, v2_int32, false),
        (BitCast, vscale_v2_int64, v2_int64, false),
        (BitCast, vscale_v4_int16, v4_int16, false),
        // Bit casting scalable vectors to scalable vectors.
        (BitCast, vscale_v4_int16, vscale_v2_int32, true),
        (BitCast, vscale_v2_int32, vscale_v4_int16, true),
        (BitCast, vscale_v2_int64, vscale_v2_int32, false),
        (BitCast, vscale_v2_int32, vscale_v2_int64, false),
        // Bitcasting to/from <vscale x 1 x Ty>
        (BitCast, vscale_v1_int16, v1_int16, false),
        (BitCast, v1_int16, vscale_v1_int16, false),
    ];
    let wrong: Vec<String> = cases
        .iter()
        .filter(|(op, src, dst, expected)| cast_is_valid(*op, *src, *dst) != *expected)
        .map(|(op, src, dst, expected)| format!("{op:?} {src} -> {dst}: expected {expected}"))
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`Trunc` portion: i64 -> i32 narrowing produces a `trunc` instruction).
#[test]
fn build_trunc_emits_trunc_to_dst_type() -> Result<(), IrError> {
    let m = module_new!("t")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i32_ty, [i64_ty.as_type()])?;
    let f = m.add_function_dyn("narrow", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i64, _> = m.view(f).param(0)?.try_into()?;
    let truncated = b.trunc(arg, i32_ty, "narrow")?;
    b.ret(truncated)?;

    let text = format!("{m}");
    let expected = "; ModuleID = 't'\n\
        \n\
        define i32 @narrow(i64 %0) {\n\
        entry:\n\
        \x20\x20%narrow = trunc i64 %0 to i32\n\
        \x20\x20ret i32 %narrow\n\
        }\n";
    assert_eq!(text, expected, "got:\n{text}");
    Ok(())
}

// Static-width `trunc::<i32, i64>` is now a compile error: the
// `WiderThan<Dst>` bound on `Src` is only implemented for `Src` widths
// strictly larger than `Dst`. The compile-fail surface is documented
// here rather than tested via `trybuild` (out of scope this session).

/// llvmkit-specific: the dynamic-width fallback `trunc_dyn` keeps the
/// runtime srcWidth > dstWidth check that C++ enforces via `assert` in
/// `Instruction::TruncInst::TruncInst`. Closest upstream coverage:
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` (the
/// width-relation invariants for trunc/zext/sext).
#[test]
fn build_trunc_dyn_runtime_check_widening_rejected() -> Result<(), IrError> {
    // The `_dyn` fallback keeps the runtime check for the path where
    // both widths are erased.
    let m = module_new!("t")?;
    let dyn_i32: IntType<'_, IntDyn, _> = m.custom_width_int_type(32)?;
    let dyn_i64: IntType<'_, IntDyn, _> = m.custom_width_int_type(64)?;
    let fn_ty = m.function_type(dyn_i64.as_type(), [dyn_i32.as_type()])?;
    let f = m.add_function_dyn("bad", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, IntDyn, _> = m.view(f).param(0)?.try_into()?;
    let err = b
        .trunc_dyn(arg, dyn_i64, "bad")
        .expect_err("trunc to wider type must error");
    assert!(matches!(
        err,
        IrError::OperandWidthMismatch { lhs: 32, rhs: 64 }
    ));
    Ok(())
}

/// llvmkit-specific: empty result name forces AsmWriter slot numbering for the
/// trunc result. Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp` (slot numbering of unnamed values) plus
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`.
#[test]
fn build_trunc_preserves_anonymous_slot_naming() -> Result<(), IrError> {
    // Empty name -> the cast result lands as `%0`/`%1`/... per slot
    // numbering. Mirrors the `cpu_state_add` example.
    let m = module_new!("t")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i32_ty, [i64_ty.as_type()])?;
    let f = m.add_function_dyn("anon", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i64, _> = m.view(f).param(0)?.try_into()?;
    let t = b.trunc(arg, i32_ty, "")?;
    b.ret(t)?;

    let text = format!("{m}");
    assert!(text.contains("%1 = trunc i64 %0 to i32\n"), "got:\n{text}");
    Ok(())
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`ZExt` portion: i32 -> i64 zero-extension).
#[test]
fn build_zext_static_static_emits_zext() -> Result<(), IrError> {
    let m = module_new!("z")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i64_ty, [i32_ty.as_type()])?;
    let f = m.add_function_dyn("widen", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let widened = b.zext(arg, i64_ty, "z")?;
    b.ret(widened)?;

    let text = format!("{m}");
    assert!(text.contains("%z = zext i32 %0 to i64\n"), "got:\n{text}");
    Ok(())
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`SExt` portion: i32 -> i64 sign-extension).
#[test]
fn build_sext_static_static_emits_sext() -> Result<(), IrError> {
    let m = module_new!("s")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i64_ty, [i32_ty.as_type()])?;
    let f = m.add_function_dyn("widen", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let widened = b.sext(arg, i64_ty, "s")?;
    b.ret(widened)?;

    let text = format!("{m}");
    assert!(text.contains("%s = sext i32 %0 to i64\n"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific regression for
/// `ConstantFold.cpp::ConstantFoldCastInstruction`: the default builder
/// folder must fold all-constant integer casts to constants.
#[test]
fn default_constant_folder_folds_zext_to_constant() -> Result<(), IrError> {
    let m = module_new!("zext-fold")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i64_ty, Vec::<llvmkit_ir::Type<'_, _>>::new())?;
    let f = m.add_function_dyn("widen", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let value: IntValue<'_, i32, _> = i32_ty.const_int(42_i32).as_erased().try_into()?;
    let result = b.zext(value, i64_ty, "z")?;
    let folded =
        ConstantIntValue::<i64, _>::try_from(Constant::try_from(b.view(result).as_erased())?)?;
    assert_eq!(folded.ap_int().try_zext_u64(), Some(42));
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll`'s `@test_zext` (`%res = zext nneg i32 %a to
/// i64`). Typed operands, no `_dyn` erasure needed to spell the `nneg` flag.
/// The `Dst: WiderThan<Src>` bound is the same one `zext` uses.
#[test]
fn typed_zext_nneg_prints_flag() -> Result<(), IrError> {
    let m = module_new!("z")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i64_ty, [i32_ty.as_type()])?;
    let f = m.add_function_dyn("widen", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let widened = b.zext_with_flags(arg, i64_ty, ZextFlags::new().nneg(), "res")?;
    b.ret(widened)?;
    let text = format!("{m}");
    assert!(
        text.contains("%res = zext nneg i32 %0 to i64"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll`'s `@test_trunc_both` (
/// `%res = trunc nuw nsw i64 %a to i32`). Typed operands, no `_dyn` erasure
/// needed to spell `nuw`/`nsw`. Upstream `IRBuilder::CreateTrunc` returns `V`
/// unchanged (silently dropping any requested nuw/nsw) when `SrcTy ==
/// DestTy`; llvmkit's `Src: WiderThan<Dst>` bound makes that same-type trunc
/// unspellable, so the flag-dropping case cannot arise here (D10).
#[test]
fn typed_trunc_nuw_nsw_prints_flags() -> Result<(), IrError> {
    let m = module_new!("t")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.function_type(i32_ty, [i64_ty.as_type()])?;
    let f = m.add_function_dyn("narrow", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i64, _> = m.view(f).param(0)?.try_into()?;
    let truncated = b.trunc_with_flags(arg, i32_ty, TruncFlags::new().nuw().nsw(), "res")?;
    b.ret(truncated)?;
    let text = format!("{m}");
    assert!(
        text.contains("%res = trunc nuw nsw i64 %0 to i32"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll`'s `@test_uitofp` (`%res = uitofp nneg i32 %a to
/// float`). Typed operands, no `_dyn` erasure needed to spell the `nneg`
/// flag.
#[test]
fn typed_uitofp_nneg_prints_flag() -> Result<(), IrError> {
    let m = module_new!("u")?;
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let fn_ty = m.function_type(f32_ty, [i32_ty.as_type()])?;
    let f = m.add_function_dyn("to_float", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let converted = b.ui_to_fp_with_flags(arg, f32_ty, UiToFpFlags::new().nneg(), "res")?;
    b.ret(converted)?;
    let text = format!("{m}");
    assert!(
        text.contains("%res = uitofp nneg i32 %0 to float"),
        "got:\n{text}"
    );
    Ok(())
}
