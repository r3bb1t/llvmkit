use llvmkit_ir::{IrResult, Linkage, Module};

fn main() -> IrResult<()> {
    let module = Module::dynamic("saved-global");
    let i32_ty = module.i32_type();
    let global = module.view(module.add_global("g", i32_ty.const_zero())?);
    let _verified = module.verify()?;
    global.set_linkage(&module, Linkage::Internal);
    Ok(())
}
