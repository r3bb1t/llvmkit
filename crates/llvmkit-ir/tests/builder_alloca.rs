//! `alloca` print form, default and aligned.
//!
//! ## Upstream provenance
//!
//! Per-test citations name the upstream `unittests/IR/IRBuilderTest.cpp`
//! `TEST_F` or `test/Assembler/*.ll` fixture each Rust test ports.

use llvmkit_ir::{Align, Dyn, IRBuilder, IrError, Linkage, Module};

/// llvmkit-specific: AsmWriter byte-for-byte parity check for the
/// no-align `alloca` print form. Closest upstream functional coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, Lifetime)`
/// exercises `Builder.CreateAlloca` at runtime.
#[test]
fn alloca_plain() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(ptr_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("a", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let p = b.build_alloca(i32_ty, "p")?;
        b.build_ret(p)?;
        let text = format!("{m}");
        assert!(text.contains("%p = alloca i32"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific: AsmWriter parity check for the array-size
/// `alloca <ty>, <n>` print form. Closest upstream functional coverage:
/// `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, Lifetime)`.
#[test]
fn alloca_array_size() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(ptr_ty.as_type(), [i32_ty.as_type()], false);
        let f = m.add_function_dyn("a", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let n: llvmkit_ir::IntValue<llvmkit_ir::IntDyn> = f.param(0)?.try_into()?;
        let p = b.build_array_alloca(i32_ty, n, "p")?;
        b.build_ret(p)?;
        let text = format!("{m}");
        assert!(text.contains("%p = alloca i32, i32 %0"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific: positive `alloca <ty>, align N` print-form check.
/// Upstream `test/Assembler/align-inst-alloca.ll` is a negative alignment fixture.
#[test]
fn alloca_aligned() -> Result<(), IrError> {
    Module::with_new("a", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(ptr_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("a", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let p = b.build_alloca_with_align(i32_ty, Align::new(8)?, "p")?;
        b.build_ret(p)?;
        let text = format!("{m}");
        assert!(text.contains("%p = alloca i32, align 8\n"), "got:\n{text}");
        Ok(())
    })
}
