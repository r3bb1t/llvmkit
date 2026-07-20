//! Phase C cast-builder coverage. `build_trunc`/`build_zext`/`build_sext`
//! enforce static width invariants via [`llvmkit_ir::WiderThan`]; the
//! `_dyn` fallbacks keep the runtime check for `IntValue<Dyn>` paths.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
//! CastInst)` (the canonical cast-shape suite) and/or marks `llvmkit-specific:`
//! for the typestate-driven coverage that has no C++ analogue.

use llvmkit_ir::{
    Constant, ConstantIntValue, Dyn, IRBuilder, IntDyn, IntType, IntValue, IrError, Linkage,
    Module, TruncFlags, UIToFpFlags, ZExtFlags,
};

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`Trunc` portion: i64 -> i32 narrowing produces a `trunc` instruction).
#[test]
fn build_trunc_emits_trunc_to_dst_type() -> Result<(), IrError> {
    Module::with_new("t", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
        let f = m.add_function_dyn("narrow", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let arg: IntValue<i64> = f.param(0)?.try_into()?;
        let truncated = b.build_trunc(arg, i32_ty, "narrow")?;
        b.build_ret(truncated)?;

        let text = format!("{m}");
        let expected = "; ModuleID = 't'\n\
            define i32 @narrow(i64 %0) {\n\
            entry:\n\
            \x20\x20%narrow = trunc i64 %0 to i32\n\
            \x20\x20ret i32 %narrow\n\
            }\n";
        assert_eq!(text, expected, "got:\n{text}");
        Ok(())
    })
}

// Static-width `build_trunc::<i32, i64>` is now a compile error: the
// `WiderThan<Dst>` bound on `Src` is only implemented for `Src` widths
// strictly larger than `Dst`. The compile-fail surface is documented
// here rather than tested via `trybuild` (out of scope this session).

/// llvmkit-specific: the dynamic-width fallback `build_trunc_dyn` keeps the
/// runtime srcWidth > dstWidth check that C++ enforces via `assert` in
/// `Instruction::TruncInst::TruncInst`. Closest upstream coverage:
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` (the
/// width-relation invariants for trunc/zext/sext).
#[test]
fn build_trunc_dyn_runtime_check_widening_rejected() -> Result<(), IrError> {
    // The `_dyn` fallback keeps the runtime check for the path where
    // both widths are erased.
    Module::with_new("t", |m| {
        let dyn_i32: IntType<'_, IntDyn> = m.custom_width_int_type(32)?;
        let dyn_i64: IntType<'_, IntDyn> = m.custom_width_int_type(64)?;
        let fn_ty = m.fn_type(dyn_i64.as_type(), [dyn_i32.as_type()], false);
        let f = m.add_function_dyn("bad", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let arg: IntValue<IntDyn> = f.param(0)?.try_into()?;
        let err = b
            .build_trunc_dyn(arg, dyn_i64, "bad")
            .expect_err("trunc to wider type must error");
        assert!(matches!(
            err,
            IrError::OperandWidthMismatch { lhs: 32, rhs: 64 }
        ));
        Ok(())
    })
}

/// llvmkit-specific: empty result name forces AsmWriter slot numbering for the
/// trunc result. Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp` (slot numbering of unnamed values) plus
/// `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`.
#[test]
fn build_trunc_preserves_anonymous_slot_naming() -> Result<(), IrError> {
    // Empty name -> the cast result lands as `%0`/`%1`/... per slot
    // numbering. Mirrors the `cpu_state_add` example.
    Module::with_new("t", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
        let f = m.add_function_dyn("anon", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let arg: IntValue<i64> = f.param(0)?.try_into()?;
        let t = b.build_trunc(arg, i32_ty, "")?;
        b.build_ret(t)?;

        let text = format!("{m}");
        assert!(text.contains("%1 = trunc i64 %0 to i32\n"), "got:\n{text}");
        Ok(())
    })
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`ZExt` portion: i32 -> i64 zero-extension).
#[test]
fn build_zext_static_static_emits_zext() -> Result<(), IrError> {
    Module::with_new("z", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i64_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("widen", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;
        let widened = b.build_zext(arg, i64_ty, "z")?;
        b.build_ret(widened)?;

        let text = format!("{m}");
        assert!(text.contains("%z = zext i32 %0 to i64\n"), "got:\n{text}");
        Ok(())
    })
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`SExt` portion: i32 -> i64 sign-extension).
#[test]
fn build_sext_static_static_emits_sext() -> Result<(), IrError> {
    Module::with_new("s", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i64_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("widen", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;
        let widened = b.build_sext(arg, i64_ty, "s")?;
        b.build_ret(widened)?;

        let text = format!("{m}");
        assert!(text.contains("%s = sext i32 %0 to i64\n"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific regression for
/// `ConstantFold.cpp::ConstantFoldCastInstruction`: the default builder
/// folder must fold all-constant integer casts to constants.
#[test]
fn default_constant_folder_folds_zext_to_constant() -> Result<(), IrError> {
    Module::with_new("zext-fold", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i64_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("widen", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let value: IntValue<i32> = i32_ty.const_int(42_i32).as_value().try_into()?;
        let result = b.build_zext(value, i64_ty, "z")?;
        let folded = ConstantIntValue::<i64>::try_from(Constant::try_from(result.as_value())?)?;
        assert_eq!(folded.ap_int().try_zext_u64(), Some(42));
        Ok(())
    })
}

/// Mirrors `test/Assembler/flags.ll:224-225` (`%res = zext nneg i32 %a to
/// i64`). Typed operands, no `_dyn` erasure needed to spell the `nneg` flag.
/// The `Dst: WiderThan<Src>` bound is the same one `build_zext` uses.
#[test]
fn typed_zext_nneg_prints_flag() -> Result<(), IrError> {
    Module::with_new("z", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i64_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("widen", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;
        let widened = b.build_zext_with_flags(arg, i64_ty, ZExtFlags::new().nneg(), "res")?;
        b.build_ret(widened)?;
        let text = format!("{m}");
        assert!(
            text.contains("%res = zext nneg i32 %0 to i64"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Mirrors `test/Assembler/flags.ll:254-258` (`test_trunc_both`:
/// `%res = trunc nuw nsw i64 %a to i32`). Typed operands, no `_dyn` erasure
/// needed to spell `nuw`/`nsw`. Upstream `IRBuilder::CreateTrunc` returns `V`
/// unchanged (silently dropping any requested nuw/nsw) when `SrcTy ==
/// DestTy`; llvmkit's `Src: WiderThan<Dst>` bound makes that same-type trunc
/// unspellable, so the flag-dropping case cannot arise here (D10).
#[test]
fn typed_trunc_nuw_nsw_prints_flags() -> Result<(), IrError> {
    Module::with_new("t", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
        let f = m.add_function_dyn("narrow", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let arg: IntValue<i64> = f.param(0)?.try_into()?;
        let truncated =
            b.build_trunc_with_flags(arg, i32_ty, TruncFlags::new().nuw().nsw(), "res")?;
        b.build_ret(truncated)?;
        let text = format!("{m}");
        assert!(
            text.contains("%res = trunc nuw nsw i64 %0 to i32"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Mirrors `test/Assembler/flags.ll:230-231` (`%res = uitofp nneg i32 %a to
/// float`). Typed operands, no `_dyn` erasure needed to spell the `nneg`
/// flag.
#[test]
fn typed_uitofp_nneg_prints_flag() -> Result<(), IrError> {
    Module::with_new("u", |m| {
        let i32_ty = m.i32_type();
        let f32_ty = m.f32_type();
        let fn_ty = m.fn_type(f32_ty, [i32_ty.as_type()], false);
        let f = m.add_function_dyn("to_float", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;
        let converted =
            b.build_ui_to_fp_with_flags(arg, f32_ty, UIToFpFlags::new().nneg(), "res")?;
        b.build_ret(converted)?;
        let text = format!("{m}");
        assert!(
            text.contains("%res = uitofp nneg i32 %0 to float"),
            "got:\n{text}"
        );
        Ok(())
    })
}
