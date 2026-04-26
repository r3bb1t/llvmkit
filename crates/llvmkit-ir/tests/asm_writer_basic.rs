//! AsmWriter round-trip / format tests. Mirrors the early pieces of
//! `llvm/lib/IR/AsmWriter.cpp` exercised by the supported opcode set.

use llvmkit_ir::{B32, IRBuilder, IntValue, IrError, Linkage, Module, RInt, RVoid};

#[test]
fn module_prints_simple_add_function() -> Result<(), IrError> {
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<RInt<B32>>("add", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");

    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let lhs: IntValue<B32> = f.param(0)?.try_into()?;
    let rhs: IntValue<B32> = f.param(1)?.try_into()?;
    let sum = b.build_int_add(lhs, rhs, "sum")?;
    b.build_ret(sum)?;

    let text = format!("{m}");
    let expected = "; ModuleID = 'demo'\n\
        define i32 @add(i32 %0, i32 %1) {\n\
        entry:\n\
        \x20\x20%sum = add i32 %0, %1\n\
        \x20\x20ret i32 %sum\n\
        }\n";
    assert_eq!(text, expected, "got:\n{text}");
    // Default state has no `local_unnamed_addr` token.
    assert!(!text.contains("local_unnamed_addr"));
    assert!(!text.contains(" unnamed_addr"));
    Ok(())
}

#[test]
fn module_prints_const_folded_arithmetic() -> Result<(), IrError> {
    // Two integer constants fed through the constant folder produce a
    // pre-folded ConstantInt operand for `ret`.
    let m = Module::new("folded");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<RInt<B32>>("answer", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");

    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let a = i32_ty.const_int(40_i32);
    let bb = i32_ty.const_int(2_i32);
    // build_int_add on two constants: the folder produces a constant.
    // We feed it through build_int_add to exercise the fold path; the
    // folded value reaches the `ret` operand directly with no `add`
    // instruction emitted.
    let folded = b.build_int_add(
        IntValue::<B32>::try_from(a.as_value())?,
        IntValue::<B32>::try_from(bb.as_value())?,
        "sum",
    )?;
    b.build_ret(folded)?;

    let text = format!("{m}");
    // The folded value is a constant; it should print as `42`.
    assert!(text.contains("ret i32 42\n"), "got:\n{text}");
    Ok(())
}

#[test]
fn function_print_standalone_matches_module_section() -> Result<(), IrError> {
    let m = Module::new("standalone");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<RInt<B32>>("identity", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");

    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let arg: IntValue<B32> = f.param(0)?.try_into()?;
    b.build_ret(arg)?;

    let standalone = format!("{f}");
    let module = format!("{m}");
    assert!(module.contains(&standalone), "module did not include f");
    Ok(())
}

#[test]
fn declare_form_for_empty_function() -> Result<(), IrError> {
    let m = Module::new("declare_only");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let _ = m.add_function::<RVoid>("ext", fn_ty, Linkage::External)?;
    let text = format!("{m}");
    assert!(text.contains("declare void @ext()\n"), "got:\n{text}");
    Ok(())
}

#[test]
fn unnamed_basic_block_uses_slot_label() -> Result<(), IrError> {
    let m = Module::new("slots");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<RInt<B32>>("anon", fn_ty, Linkage::External)?;
    // No name on the entry block.
    let entry = f.append_basic_block("");
    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let arg: IntValue<B32> = f.param(0)?.try_into()?;
    b.build_ret(arg)?;
    let text = format!("{m}");
    // Block 0 (the only block) should label as `1:` because slot 0 is
    // claimed by the unnamed argument %0.
    assert!(
        text.contains("1:\n"),
        "expected slot-labelled block; got:\n{text}"
    );
    Ok(())
}
