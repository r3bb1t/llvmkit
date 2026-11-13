//! llvmkit-specific compile-fail (D3, D10): the checked `try_build` is no door
//! out of `SpaceDyn` either. A `Ptr` field's `try_build` argument is a pointer
//! known to be in address space 0 (`E0271`), and `PointerValue::try_from` is the
//! one way to make one, as `build_with_dyn_pointer.rs`'s diagnostic says.
//! `a_narrowed_space_dyn_pointer_goes_through_try_build` is the accepted half.
//! No upstream counterpart: `IRBuilderBase::CreateInsertValue` takes any
//! `Value *`.

use llvmkit_ir::{IrBuilder, IrError, IrStruct, Linkage, NoFolder, Ptr, module_new};

#[derive(IrStruct)]
struct Node {
    next: Ptr,
}

fn main() -> Result<(), IrError> {
    let m = module_new!("m")?;
    let f = m.add_typed_function::<Node, (), _>("make", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let slot = m.view(b.alloca(m.i32_type(), "slot")?);
    let node = m.get_or_insert_struct_of::<Node>()?;
    let _ = node.try_build(&b, slot, "n")?;
    Ok(())
}
