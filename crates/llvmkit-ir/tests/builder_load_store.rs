//! `load` / `store` print form, plus a round-trip
//! load+add+store body, plus the `Align` invariant.
//!
//! ## Upstream provenance
//!
//! Print-form fixtures locked from
//! `test/Assembler/align-inst-load.ll` and
//! `test/Assembler/align-inst-store.ll`. Closest upstream IRBuilder
//! coverage: the `Builder.CreateLoad` / `Builder.CreateStore` calls
//! used throughout `unittests/IR/IRBuilderTest.cpp` (e.g. inside
//! `TEST_F(IRBuilderTest, FastMathFlags)`).

use llvmkit_ir::{Align, IRBuilder, IrError, Linkage, Module};

/// Mirrors `test/Assembler/align-inst-load.ll` for the no-align
/// `load <ty>, ptr %x` print form.
#[test]
fn load_plain() -> Result<(), IrError> {
    let m = Module::new("ls");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("ld", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let r = b.build_int_load::<i32, _>(p, "v")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(text.contains("%v = load i32, ptr %0"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/align-inst-load.ll` for the explicit-align
/// `load <ty>, ptr %x, align N` form.
#[test]
fn load_aligned() -> Result<(), IrError> {
    let m = Module::new("ls");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("ld", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let r = b.build_int_load_with_align::<i32, _>(p, Align::new(4)?, "v")?;
    b.build_ret(r)?;
    let text = format!("{m}");
    assert!(
        text.contains("%v = load i32, ptr %0, align 4"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/align-inst-store.ll` for the no-align
/// `store <ty> %v, ptr %x` print form.
#[test]
fn store_plain() -> Result<(), IrError> {
    let m = Module::new("ls");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(
        m.void_type().as_type(),
        [i32_ty.as_type(), ptr_ty.as_type()],
        false,
    );
    let f = m.add_function::<()>("st", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let v: llvmkit_ir::IntValue<i32> = f.param(0)?.try_into()?;
    let p: llvmkit_ir::PointerValue = f.param(1)?.try_into()?;
    b.build_store(v, p)?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(text.contains("store i32 %0, ptr %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/align-inst-store.ll` for the explicit-align
/// `store <ty> %v, ptr %x, align N` form.
#[test]
fn store_aligned() -> Result<(), IrError> {
    let m = Module::new("ls");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(
        m.void_type().as_type(),
        [i32_ty.as_type(), ptr_ty.as_type()],
        false,
    );
    let f = m.add_function::<()>("st", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let v: llvmkit_ir::IntValue<i32> = f.param(0)?.try_into()?;
    let p: llvmkit_ir::PointerValue = f.param(1)?.try_into()?;
    b.build_store_with_align(v, p, Align::new(4)?)?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("store i32 %0, ptr %1, align 4"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific: end-to-end load+add+store round-trip exercising the
/// builder. Closest upstream functional coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)`
/// uses the same `CreateLoad` + arithmetic + ret pattern.
#[test]
fn load_add_store_round_trip() -> Result<(), IrError> {
    let m = Module::new("ls");
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(m.void_type().as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function::<()>("inc", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
    let v = b.build_int_load::<i32, _>(p, "v")?;
    let n = b.build_int_add(v, 1_i32, "n")?;
    b.build_store(n, p)?;
    b.build_ret_void();
    let text = format!("{m}");
    assert!(text.contains("%v = load i32, ptr %0"), "got:\n{text}");
    assert!(text.contains("%n = add i32 %v, 1"), "got:\n{text}");
    assert!(text.contains("store i32 %n, ptr %0"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: enforces the `Align` newtype invariant (must be a
/// power of two and \u2265 1). No direct upstream test \u2014 LLVM's `Align`
/// invariant is enforced via `Align::isLegal` assertions in
/// `lib/Support/Alignment.cpp`.
#[test]
fn align_invariant() {
    assert!(Align::new(0).is_err());
    assert!(Align::new(3).is_err());
    assert!(Align::new(4).is_ok());
    assert_eq!(Align::new(8).unwrap().value(), 8);
}
