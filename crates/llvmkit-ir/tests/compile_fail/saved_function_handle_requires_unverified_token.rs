use llvmkit_ir::{Dyn, IRBuilder, IrResult, Linkage, Module, Type};

fn main() -> IrResult<()> {
    Module::with_new::<_, _, _>("saved-function", |module| {
        let void_ty = module.void_type();
        let fn_ty = module.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
        let function = module.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = function.append_basic_block(&module, "entry");
        IRBuilder::new_for::<Dyn>(&module)
            .position_at_end(entry)
            .build_ret_void()?;

        let _verified = module.verify()?;
        function.set_linkage(&module, Linkage::Internal);
        Ok(())
    })
}
