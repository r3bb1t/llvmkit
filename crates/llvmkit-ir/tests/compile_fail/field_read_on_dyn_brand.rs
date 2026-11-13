//! llvmkit-specific compile-fail (D7): a derived field reads without `?` only
//! on a `UniqueBrand` module, whose registry keeps one live module per brand.
//! `Module::dynamic`'s `DynBrand` names many live modules, so `point.x(&b)`
//! fails on `UniqueBrand`'s own message; the checked `try_x` is the read for
//! it (`derived_struct_schema.rs::a_dynamic_module_reads_with_try`). No
//! upstream counterpart: LLVM's `Module` has no brand.

use llvmkit_ir::{IrBuilder, IrError, IrStruct, Linkage, Module, NoFolder};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

fn main() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let f = m.add_typed_function::<(), (Point,), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (point,) = m.view(f).params();
    let _ = point.x(&b);
    Ok(())
}
