//! Branded module identity coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::collections::HashMap;

use llvmkit_ir::{
    AttrIndex, AttrKind, Attribute, AttributeStorage, Dyn, DynBrand, FunctionValue, IntValue,
    IrBuilder, IrResult, Linkage, Module, ModuleBrand, Unverified, Value, module_new,
};

fn exercise_tables<'ctx, B: ModuleBrand + 'ctx>(module: Module<B, Unverified>) -> IrResult<()> {
    let i64_ty = module.i64_type();
    let fn_ty = module.fn_type(i64_ty.as_type(), [i64_ty.as_type()], false);
    let function = module.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = module.view(function).append_basic_block(&module, "entry");
    let parameter: IntValue<'_, i64, _> = module.view(function).param(0)?.try_into()?;

    let mut values = HashMap::<&str, Value<'_, _>>::new();
    values.insert("parameter", parameter.as_erased());
    let mut integers = HashMap::<&str, IntValue<'_, i64, _>>::new();
    integers.insert("parameter", parameter);

    let lhs = *integers.get("parameter").expect("int value");
    let rhs: IntValue<'_, i64, _> = (*values.get("parameter").expect("value")).try_into()?;
    let builder = IrBuilder::new_for::<Dyn>(&module).position_at_end(entry);
    let sum = builder.int_add(lhs, rhs, "sum")?;
    builder.ret(sum)?;

    let text = format!("{module}");
    assert!(text.contains("add i64"));
    Ok(())
}

/// `llvmkit-specific D7`: user-owned value tables retain the module brand,
/// so values can be stored, retrieved, and reused without weakening to runtime
/// module-id checks.
#[test]
fn user_owned_value_tables_remain_usable() -> IrResult<()> {
    exercise_tables(module_new!("brand-tables")?)
}

fn format_generic_function<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, (), B>,
) -> String {
    format!("{function}")
}

/// `llvmkit-specific D7`: formatting a function handle preserves a caller's
/// module brand instead of pinning one of its own.
#[test]
fn generic_function_display_preserves_brand() -> IrResult<()> {
    let module = module_new!("function-display-brand")?;
    let function = module
        .add_typed_function::<(), (), _>("f", Linkage::External)?
        .as_function();
    let entry = module.view(function).append_basic_block(&module, "entry");
    IrBuilder::new_for::<()>(&module)
        .position_at_end(entry)
        .ret_void();
    assert!(format_generic_function(module.view(function)).contains("define void @f()"));
    Ok(())
}

/// `llvmkit-specific D7`: attribute constructors are brand-generic, and the
/// brand-free `AttributeStorage` accepts a handle of any brand. With the
/// lifetime brand gone there is no implicit default, so a call site that pins
/// no brand of its own names one explicitly.
#[test]
fn brand_generic_attribute_constructors_feed_brand_free_storage() {
    let mut storage = AttributeStorage::new();
    storage.add(
        AttrIndex::Function,
        Attribute::<DynBrand>::enum_attr(AttrKind::NoReturn).expect("enum attr"),
    );
    storage.add(
        AttrIndex::Function,
        Attribute::<DynBrand>::string("target-features", "+sse2"),
    );
    assert!(!storage.is_empty());
}
