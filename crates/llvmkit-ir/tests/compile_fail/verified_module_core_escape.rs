use llvmkit_ir::{IrResult, Module};

fn main() -> IrResult<()> {
    Module::with_new::<_, _, _>("core-escape", |module| {
        let core = module.core();
        let _verified = module.verify()?;
        core.append_module_asm("mutated after verify");
        Ok(())
    })
}
