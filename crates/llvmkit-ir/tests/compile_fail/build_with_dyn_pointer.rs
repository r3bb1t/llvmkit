//! llvmkit-specific compile-fail (D3, D10): a `Ptr` field is a pointer in
//! address space 0, and the infallible `build` checks nothing at run time — so
//! its argument must be an address-space-0 pointer by type. A `SpaceDyn`
//! pointer (address space known only at run time) is no `FieldArg` for it
//! (`E0277`); `try_build` takes it and checks. No upstream counterpart:
//! `IRBuilderBase::CreateInsertValue` takes any `Value *`.

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
    let _ = node.build(&b, slot, "n");
    Ok(())
}
