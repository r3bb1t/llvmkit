use llvmkit_ir::{IrResult, Linkage, Module};

fn main() -> IrResult<()> {
    Module::with_new::<_, _, _>("saved-global", |module| {
        let i32_ty = module.i32_type();
        let global = module.add_global("g", i32_ty.const_zero())?;
        let _verified = module.verify()?;
        global.set_linkage(&module, Linkage::Internal);
        Ok(())
    })
}
