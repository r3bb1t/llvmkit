//! llvmkit-specific compile-fail: a schema names an identified struct, and
//! `StructType::create(Context, "")` (`lib/IR/Type.cpp`) makes an anonymous
//! struct rather than one printed `%""` — so a hand-written `StructSchema`
//! whose `NAME` is empty is refused when its type is minted, by a `const`
//! assertion (`E0080`). It surfaces only because the harness runs under
//! `cargo build` (the `t.pass` registration in `typestate_compile_fail.rs`).
//! The derive's form of the same refusal is `ir_struct_empty_name.rs`.

use llvmkit_ir::{Capability, ModuleBrand, Mutable, SchemaValue, StructSchema, StructSchemaValue, module_new};

struct EmptyName;

struct EmptyNameValue<'ctx, B: ModuleBrand, C: Capability = Mutable>(SchemaValue<'ctx, EmptyName, B, C>);

impl<'ctx, B: ModuleBrand, C: Capability> Clone for EmptyNameValue<'ctx, B, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> Copy for EmptyNameValue<'ctx, B, C> {}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> StructSchemaValue<'ctx, EmptyName, B, C>
    for EmptyNameValue<'ctx, B, C>
{
    fn from_schema_value(value: SchemaValue<'ctx, EmptyName, B, C>) -> Self {
        Self(value)
    }

    fn schema_value(self) -> SchemaValue<'ctx, EmptyName, B, C> {
        self.0
    }
}

impl StructSchema for EmptyName {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = EmptyNameValue<'ctx, B, C>;
    type Fields = (i32,);

    const NAME: &'static str = "";
}

fn main() -> Result<(), llvmkit_ir::IrError> {
    let m = module_new!("empty_name")?;
    let _ = m.get_or_insert_struct_of::<EmptyName>();
    Ok(())
}
