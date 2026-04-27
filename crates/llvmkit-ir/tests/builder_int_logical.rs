//! Integer logical opcodes: `and`, `or`, `xor`.
//!
//! ## Upstream provenance
//!
//! Print-form fixtures locked from `test/Assembler/flags.ll` (no
//! flags apply to these opcodes, but the surrounding `; CHECK:` lines
//! pin the textual form). Closest upstream functional coverage in
//! `unittests/IR/IRBuilderTest.cpp` is the `TEST_F(IRBuilderTest, ...)`
//! family that exercises `Builder.CreateAnd` / `CreateOr` / `CreateXor`
//! indirectly. The shared `module_with` helper factors module setup.

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

fn module_with(op: &str) -> Result<String, IrError> {
    let m = Module::new("logical");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<i32>(op, fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
    let x = f.param(0)?;
    let y = f.param(1)?;
    let r = match op {
        "and" => b.build_int_and::<i32, _, _>(x, y, "z")?,
        "or" => b.build_int_or::<i32, _, _>(x, y, "z")?,
        "xor" => b.build_int_xor::<i32, _, _>(x, y, "z")?,
        _ => unreachable!(),
    };
    b.build_ret(r)?;
    Ok(format!("{m}"))
}

/// Mirrors `test/Assembler/flags.ll` for `and` print form.
#[test]
fn and_print_form() -> Result<(), IrError> {
    let text = module_with("and")?;
    assert!(text.contains("%z = and i32 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for `or` print form.
#[test]
fn or_print_form() -> Result<(), IrError> {
    let text = module_with("or")?;
    assert!(text.contains("%z = or i32 %0, %1"), "got:\n{text}");
    Ok(())
}

/// Mirrors `test/Assembler/flags.ll` for `xor` print form.
#[test]
fn xor_print_form() -> Result<(), IrError> {
    let text = module_with("xor")?;
    assert!(text.contains("%z = xor i32 %0, %1"), "got:\n{text}");
    Ok(())
}
