//! Phase C cast-builder coverage. `build_trunc`/`build_zext`/`build_sext`
//! enforce static width invariants via [`llvmkit_ir::WiderThan`]; the
//! `_dyn` fallbacks keep the runtime check for `IntValue<Dyn>` paths.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
//! CastInst)` (the canonical cast-shape suite) and/or marks `llvmkit-specific:`
//! for the typestate-driven coverage that has no C++ analogue.

use llvmkit_ir::{IRBuilder, IntDyn, IntType, IntValue, IrError, Linkage, Module};

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`Trunc` portion: i64 -> i32 narrowing produces a `trunc` instruction).
#[test]
fn build_trunc_emits_trunc_to_dst_type() -> Result<(), IrError> {
    let m = Module::new("t");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
    let f = m.add_function::<i32>("narrow", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
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
    let m = Module::new("t");
    let dyn_i32: IntType<'_, IntDyn> = m.custom_width_int_type(32)?;
    let dyn_i64: IntType<'_, IntDyn> = m.custom_width_int_type(64)?;
    let fn_ty = m.fn_type(dyn_i64.as_type(), [dyn_i32.as_type()], false);
    let f = m.add_function::<IntDyn>("bad", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<IntDyn>(&m).position_at_end(entry);
    let arg: IntValue<IntDyn> = f.param(0)?.try_into()?;
    let err = b
        .build_trunc_dyn(arg, dyn_i64, "bad")
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
    let m = Module::new("t");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i32_ty, [i64_ty.as_type()], false);
    let f = m.add_function::<i32>("anon", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let arg: IntValue<i64> = f.param(0)?.try_into()?;
    let t = b.build_trunc(arg, i32_ty, "")?;
    b.build_ret(t)?;

    let text = format!("{m}");
    assert!(text.contains("%1 = trunc i64 %0 to i32\n"), "got:\n{text}");
    Ok(())
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`ZExt` portion: i32 -> i64 zero-extension).
#[test]
fn build_zext_static_static_emits_zext() -> Result<(), IrError> {
    let m = Module::new("z");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i64>("widen", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let arg: IntValue<i32> = f.param(0)?.try_into()?;
    let widened = b.build_zext(arg, i64_ty, "z")?;
    b.build_ret(widened)?;

    let text = format!("{m}");
    assert!(text.contains("%z = zext i32 %0 to i64\n"), "got:\n{text}");
    Ok(())
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)`
/// (`SExt` portion: i32 -> i64 sign-extension).
#[test]
fn build_sext_static_static_emits_sext() -> Result<(), IrError> {
    let m = Module::new("s");
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(i64_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i64>("widen", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let arg: IntValue<i32> = f.param(0)?.try_into()?;
    let widened = b.build_sext(arg, i64_ty, "s")?;
    b.build_ret(widened)?;

    let text = format!("{m}");
    assert!(text.contains("%s = sext i32 %0 to i64\n"), "got:\n{text}");
    Ok(())
}
