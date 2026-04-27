//! Phase A4-int coverage. Verifies that the
//! [`IntoIntValue`](llvmkit_ir::IntoIntValue) trait makes `IntValue`,
//! `ConstantIntValue`, and Rust scalar literals all valid operands at
//! the same call site.
//!
//! ## Upstream provenance
//!
//! Per-test citations below. Each `#[test]` carries a doc comment naming the
//! upstream `unittests/IR/IRBuilderTest.cpp` TEST_F it ports, or marks itself
//! `llvmkit-specific:` (e.g. the Rust-literal coercion that has no C++ analogue).

use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module};

/// llvmkit-specific: exercises `IntoIntValue` for `IntValue` LHS plus a Rust
/// `i32` literal RHS at the same `build_int_add` call site (no C++ analogue;
/// upstream callers always materialise a `Value*`). Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`
/// exercises `Builder.CreateAdd(getInt32(1), getInt32(2), "add")`.
#[test]
fn build_int_add_accepts_int_value_and_rust_literal() -> Result<(), IrError> {
    let m = Module::new("a");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("inc", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    // Rust literal as RHS.
    let next = b.build_int_add(n, 1_i32, "next")?;
    b.build_ret(next)?;

    let text = format!("{m}");
    assert!(text.contains("%next = add i32 %0, 1\n"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: `ConstantIntValue` LHS + `IntValue` RHS through
/// `IntoIntValue`. Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)`.
#[test]
fn build_int_sub_accepts_constant_and_argument() -> Result<(), IrError> {
    let m = Module::new("s");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32>("dec", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let n: IntValue<i32> = f.param(0)?.try_into()?;
    let c = i32_ty.const_int(10_i32);
    // ConstantIntValue as LHS, IntValue as RHS.
    let r = b.build_int_sub(c, n, "r")?;
    b.build_ret(r)?;

    let text = format!("{m}");
    // Folder doesn't fire (one operand is non-constant); the
    // instruction must materialise.
    assert!(text.contains("%r = sub i32 10, %0\n"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: typed builder `IRBuilder::<i32>::build_ret` accepts a Rust
/// `i32` literal directly via `IntoIntValue`. Closest upstream coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)` (a
/// builder-driven module that round-trips through the AsmWriter).
#[test]
fn build_ret_accepts_rust_literal_directly() -> Result<(), IrError> {
    // `i32` builder: `b.build_ret(1_i32)?` works without the
    // caller materialising an `IntValue` first.
    let m = Module::new("r");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<i32>("one", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    b.build_ret(1_i32)?;

    let text = format!("{m}");
    assert!(text.contains("ret i32 1\n"), "got:\n{text}");
    Ok(())
}
