use llvmkit_ir::{Dyn, IrBuilder, IrResult, Linkage, Module, Type};

fn main() -> IrResult<()> {
    let module = Module::dynamic("saved-function");
    let void_ty = module.void_type();
    let fn_ty = module.fn_type(void_ty.as_type(), Vec::<Type<_>>::new(), false);
    let function = module.view(module.add_function_dyn("f", fn_ty, Linkage::External)?);
    let entry = function.append_basic_block(&module, "entry");
    IrBuilder::new_for::<Dyn>(&module)
        .position_at_end(entry)
        .build_ret_void()?;

    let _verified = module.verify()?;
    function.set_linkage(&module, Linkage::Internal);
    Ok(())
}
