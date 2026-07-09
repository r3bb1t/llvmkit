//! Branded module identity coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::collections::HashMap;

use llvmkit_ir::{
    AttrIndex, AttrKind, Attribute, AttributeStorage, FunctionValue, IRBuilder, IntValue, IrResult,
    Linkage, Module, ModuleBrand, Type, Value,
};

fn exercise_tables<'ctx>(module: Module<'ctx>) -> IrResult<()> {
    let i64_ty = module.i64_type();
    let fn_ty = module.fn_type(i64_ty.as_type(), [i64_ty.as_type()], false);
    let function = module.add_function::<i64, _>("f", fn_ty, Linkage::External)?;
    let entry = function.append_basic_block(&module, "entry");
    let parameter: IntValue<'ctx, i64> = function.param(0)?.try_into()?;

    let mut values = HashMap::<&str, Value<'ctx>>::new();
    values.insert("parameter", parameter.as_value());
    let mut integers = HashMap::<&str, IntValue<'ctx, i64>>::new();
    integers.insert("parameter", parameter);

    let lhs = *integers.get("parameter").expect("int value");
    let rhs: IntValue<'ctx, i64> = (*values.get("parameter").expect("value")).try_into()?;
    let builder = IRBuilder::new_for::<i64>(&module).position_at_end(entry);
    let sum = builder.build_int_add(lhs, rhs, "sum")?;
    builder.build_ret(sum)?;

    let text = format!("{module}");
    assert!(text.contains("add i64"));
    Ok(())
}

/// `llvmkit-specific D7`: user-owned value tables retain the module brand,
/// so values can be stored, retrieved, and reused without weakening to runtime
/// module-id checks.
#[test]
fn user_owned_value_tables_remain_usable() -> IrResult<()> {
    Module::with_new::<_, _, _>("brand-tables", exercise_tables)
}

fn format_generic_function<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionValue<'ctx, (), B>,
) -> String {
    format!("{function}")
}

/// `llvmkit-specific D7`: formatting a function handle preserves a caller's
/// module brand instead of requiring the default `Brand<'ctx>`.
#[test]
fn generic_function_display_preserves_brand() -> IrResult<()> {
    Module::with_new::<_, _, _>("function-display-brand", |module| {
        let void_ty = module.void_type();
        let fn_ty = module.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
        let function = module.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = function.append_basic_block(&module, "entry");
        IRBuilder::new_for::<()>(&module)
            .position_at_end(entry)
            .build_ret_void();
        assert!(format_generic_function(function).contains("define void @f()"));
        Ok(())
    })
}

/// `llvmkit-specific D7`: brandless attribute constructors stay ergonomic for
/// ordinary default-branded module code.
#[test]
fn brandless_attribute_constructors_infer_default_brand() {
    let mut storage = AttributeStorage::new();
    storage.add(
        AttrIndex::Function,
        Attribute::enum_attr(AttrKind::NoReturn).expect("enum attr"),
    );
    storage.add(
        AttrIndex::Function,
        Attribute::string("target-features", "+sse2"),
    );
    assert!(!storage.is_empty());
}
