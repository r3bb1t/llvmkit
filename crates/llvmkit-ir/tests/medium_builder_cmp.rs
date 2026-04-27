//! Phase C-cmp coverage. Covers `IRBuilder::build_int_cmp` with
//! representative integer predicates and the `i1` result type.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
//! CmpPredicate)` for predicate coverage. The IR-shape assertions (textual
//! `icmp <pred>` rendering) additionally mirror Assembler fixtures in
//! `test/Assembler/` exercising integer predicates.

use llvmkit_ir::{IRBuilder, IntPredicate, IntValue, IrError, Linkage, Module};

fn build_eq_module() -> Result<String, IrError> {
    let m = Module::new("c");
    let bool_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(bool_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<bool>("is_zero", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let r = b.build_int_cmp::<i32, _, _>(IntPredicate::Eq, n, 0_i32, "r")?;
    b.build_ret(r)?;
    Ok(format!("{m}"))
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CmpPredicate)`
/// (`ICMP_EQ` arm) plus the AsmWriter rendering `icmp eq i32 ...`.
#[test]
fn build_int_cmp_eq_emits_icmp_eq() -> Result<(), IrError> {
    let text = build_eq_module()?;
    assert!(text.contains("%r = icmp eq i32 %0, 0"), "got:\n{text}");
    Ok(())
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CmpPredicate)`
/// (`ICMP_SLT` arm) plus the AsmWriter rendering `icmp slt i32 ...`.
#[test]
fn build_int_cmp_slt_emits_icmp_slt() -> Result<(), IrError> {
    let m = Module::new("c");
    let bool_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(bool_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<bool>("lt", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let bv: IntValue<i32> = f.param(1)?.try_into()?;
    let r = b.build_int_cmp::<i32, _, _>(IntPredicate::Slt, a, bv, "r")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = icmp slt i32 %0, %1"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: typed-result invariant -- `build_int_cmp` returns
/// `IntValue<bool>` so the result is a typestate-checked `i1`. Closest upstream
/// coverage: `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest,
/// CmpPredicate)` (predicate -> i1 result type).
#[test]
fn build_int_cmp_returns_i1_for_chaining() -> Result<(), IrError> {
    // The result of `build_int_cmp` is `IntValue<bool>`, suitable for
    // `build_cond_br` and other `i1` consumers without further
    // narrowing.
    let m = Module::new("c");
    let bool_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(bool_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<bool>("ne", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let r: IntValue<bool> = b.build_int_cmp::<i32, _, _>(IntPredicate::Ne, n, 1_i32, "r")?;
    b.build_ret(r)?;
    Ok(())
}

/// Mirrors `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CmpPredicate)`
/// (`ICMP_ULE` arm) plus the AsmWriter rendering `icmp ule i32 ...`.
#[test]
fn build_int_cmp_ule_emits_icmp_ule() -> Result<(), IrError> {
    let m = Module::new("c");
    let bool_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(bool_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<bool>("ule", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<bool>(&m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let bv: IntValue<i32> = f.param(1)?.try_into()?;
    let r = b.build_int_cmp::<i32, _, _>(IntPredicate::Ule, a, bv, "r")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%r = icmp ule i32 %0, %1"), "got:\n{text}");
    Ok(())
}
