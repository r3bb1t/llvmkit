use llvmkit_ir::{Dyn, IrBuilder, IrResult, Linkage, Module, Type};

fn main() -> IrResult<()> {
    let mut owned = Module::dynamic("saved-function"); let mut key = owned.layout_token(); let module = key.module();
    let void_ty = module.void_type();
    let fn_ty = module.function_type(void_ty.as_type(), Vec::<Type<_>>::new())?;
    let function = module.view(module.add_function_dyn("f", fn_ty, Linkage::External)?);
    let entry = function.append_basic_block(&module, "entry");
    IrBuilder::new_for::<Dyn>(&mut key)
        .try_position_at_end(entry)?
        .ret_void()?;

    let _verified = owned.verify()?;
    function.set_linkage(&module, Linkage::Internal);
    Ok(())
}
