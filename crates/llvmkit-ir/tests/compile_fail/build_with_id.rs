//! llvmkit-specific compile-fail (D7): the infallible `build` admits nothing,
//! which is sound only for view handles — two live views of a `UniqueBrand`
//! brand borrow the same module. A storable id outlives its module, and a
//! brand is issued again once its module drops, so an id is no `FieldArg`
//! (`E0277`); `try_build` takes ids and refuses a stale one
//! (`derived_struct_schema.rs::a_stale_id_is_refused_by_try_build`). No
//! upstream counterpart: LLVM's `Module` has no brand.

use llvmkit_ir::{IrBuilder, IrError, IrStruct, Linkage, NoFolder, module_new};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

fn main() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let f = m.add_typed_function::<Point, (i32,), _>("make", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (x,) = m.view(f).params();
    let point = m.get_or_insert_struct_of::<Point>()?;
    let _ = point.build(&b, x.id(), 7_i32, "p");
    Ok(())
}
