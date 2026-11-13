//! llvmkit-specific compile-fail (D10): a derived field reads without `?` only
//! through llvmkit's own folders (`TrustedFolder`), whose struct folding is
//! type-correct by construction. A folder of the user's own could hand back a
//! value of another type, so `point.x(&b)` on its builder fails on
//! `TrustedFolder`'s own message; the checked `try_x` consults it and checks
//! its answer (`custom_folder_struct_reads.rs`). Upstream's
//! `IRBuilderBase::CreateExtractValue` takes any `IRBuilderFolder` and checks
//! nothing.

use llvmkit_ir::{IrBuilder, IrBuilderFolder, IrError, IrStruct, Linkage, ModuleBrand, module_new};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

struct MyFolder;

impl<'ctx, B: ModuleBrand + 'ctx> IrBuilderFolder<'ctx, B> for MyFolder {}

fn main() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let f = m.add_typed_function::<(), (Point,), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, MyFolder).position_at_end(entry);
    let (point,) = m.view(f).params();
    let _ = point.x(&b);
    Ok(())
}
