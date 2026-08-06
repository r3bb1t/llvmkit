//! AsmWriter coverage for parameter / return attribute slots, plus
//! the typed `i32` IrBuilder.
//!
//! ## Upstream provenance
//!
//! Mirrors `unittests/IR/AttributesTest.cpp` (`TEST(Attributes, AddAttributes)`,
//! `TEST(Attributes, OverflowGet)`) for the AttrKind plumbing, with AsmWriter
//! print-form locked against `test/Assembler/2008-09-29-RetAttr.ll` and
//! related `test/Assembler/*-attr*.ll` fixtures. Per-test citations below.

use llvmkit_ir::{
    AttrIndex, AttrKind, Attribute, IntValue, IrBuilder, IrError, Linkage, StrBoolAttrKind,
    module_new,
};

/// llvmkit-specific reader-layer check with `Attribute::getValueAsBool`
/// (`lib/IR/Attributes.cpp`) as the functional reference: a `StrBoolAttr`
/// string attribute reads `Some(true)` exactly when its value text is
/// `"true"` (upstream asserts the stored text is `""`, `"false"`, or
/// `"true"`), and an absent attribute reads `None` (upstream folds absence
/// into `false`; the `Option` keeps it distinguishable). No upstream
/// unittest drives `getValueAsBool` directly.
#[test]
fn str_bool_attribute_reads_true_false_and_absent() -> Result<(), IrError> {
    let m = module_new!("p")?;
    let fn_ty = m.function_type_no_parameters(m.void_type());
    let f = m.add_function_dyn("g", fn_ty, Linkage::External)?;

    assert_eq!(
        m.view(f).str_bool_attribute(StrBoolAttrKind::NoNansFpMath),
        None,
        "absent attribute must read None, not false"
    );

    m.view(f)
        .set_string_attribute(&m, AttrIndex::Function, "no-nans-fp-math", "true");
    m.view(f)
        .set_string_attribute(&m, AttrIndex::Function, "no-jump-tables", "false");
    // Empty text is one of the three states upstream's assert admits; it is
    // not `"true"`, so it reads `false`.
    m.view(f)
        .set_string_attribute(&m, AttrIndex::Function, "use-sample-profile", "");

    assert_eq!(
        m.view(f).str_bool_attribute(StrBoolAttrKind::NoNansFpMath),
        Some(true)
    );
    assert_eq!(
        m.view(f).str_bool_attribute(StrBoolAttrKind::NoJumpTables),
        Some(false)
    );
    assert_eq!(
        m.view(f)
            .str_bool_attribute(StrBoolAttrKind::UseSampleProfile),
        Some(false)
    );
    Ok(())
}

/// Mirrors `test/Assembler/2008-09-29-RetAttr.ll` for return-attribute
/// print form, with the AttrKind plumbing from
/// `unittests/IR/AttributesTest.cpp::TEST(Attributes, AddAttributes)`.
#[test]
fn function_with_noundef_param_and_return() -> Result<(), IrError> {
    let m = module_new!("p")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m
        .function_builder::<i32, _>("identity", fn_ty)
        .linkage(Linkage::External)
        .return_attribute(AttrKind::NoUndef)
        .param_attribute(0, AttrKind::NoUndef)
        .param_name(0, "x")
        .build()?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    b.ret(x)?;

    let text = format!("{m}");
    let expected = "; ModuleID = 'p'\n\
        define noundef i32 @identity(i32 noundef %x) {\n\
        entry:\n\
        \x20\x20ret i32 %x\n\
        }\n";
    assert_eq!(text, expected, "got:\n{text}");
    Ok(())
}

/// Mirrors `unittests/IR/AttributesTest.cpp::TEST(Attributes, AddAttributes)`
/// for the param-index `Attribute::get` path, asserting AsmWriter print form
/// (`zeroext` keyword) defined in `lib/IR/AsmWriter.cpp`.
#[test]
fn attribute_added_via_attribute_method_path() -> Result<(), IrError> {
    let m = module_new!("p")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m
        .function_builder::<i32, _>("zext_arg", fn_ty)
        .attribute(
            AttrIndex::Param(0),
            Attribute::enum_attr(AttrKind::Zext).expect("enum"),
        )
        .param_name(0, "n")
        .build()?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    b.ret(n)?;

    let text = format!("{m}");
    assert!(
        text.contains("define i32 @zext_arg(i32 zeroext %n)"),
        "got:\n{text}"
    );
    Ok(())
}
