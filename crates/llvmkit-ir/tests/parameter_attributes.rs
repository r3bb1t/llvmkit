//! AsmWriter coverage for parameter / return attribute slots, plus
//! the typed `RInt<B32>` IRBuilder.

use llvmkit_ir::{
    AttrIndex, AttrKind, Attribute, B32, IRBuilder, IntValue, IrError, Linkage, Module, RInt,
};

#[test]
fn function_with_noundef_param_and_return() -> Result<(), IrError> {
    let m = Module::new("p");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m
        .function_builder::<RInt<B32>>("identity", fn_ty)
        .linkage(Linkage::External)
        .return_attribute(AttrKind::NoUndef)
        .param_attribute(0, AttrKind::NoUndef)
        .param_name(0, "x")
        .build()?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let x: IntValue<B32> = f.param(0)?.try_into()?;
    b.build_ret(x)?;

    let text = format!("{m}");
    let expected = "; ModuleID = 'p'\n\
        define noundef i32 @identity(i32 noundef %x) {\n\
        entry:\n\
        \x20\x20ret i32 %x\n\
        }\n";
    assert_eq!(text, expected, "got:\n{text}");
    Ok(())
}

#[test]
fn attribute_added_via_attribute_method_path() -> Result<(), IrError> {
    let m = Module::new("p");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m
        .function_builder::<RInt<B32>>("zext_arg", fn_ty)
        .attribute(
            AttrIndex::Param(0),
            Attribute::enum_attr(AttrKind::ZExt).expect("enum"),
        )
        .param_name(0, "n")
        .build()?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let n: IntValue<B32> = f.param(0)?.try_into()?;
    b.build_ret(n)?;

    let text = format!("{m}");
    assert!(
        text.contains("define i32 @zext_arg(i32 zeroext %n)"),
        "got:\n{text}"
    );
    Ok(())
}
