//! llvmkit derive compile-fail (Doctrine D4).
//!
//! Closest upstream behaviour: `extractvalue` result type is the selected field
//! type. The derived accessor exposes that type statically.

use llvmkit_ir::{IrBuilder, IrError, IrStruct, Linkage, Module, PointerValue};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

fn expects_pointer<'ctx, B: llvmkit_ir::ModuleBrand + 'ctx>(value: PointerValue<'ctx, B>) {
    let _ = value;
}

fn main() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let f = m.add_typed_function::<(), (Point,), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<()>(&m).position_at_end(entry);
    let (point,) = m.view(f).params();
    expects_pointer(point.x(&b)?);
    b.build_ret_void();
    Ok(())
}
