use llvmkit_ir::{IrResult, Module};

fn main() -> IrResult<()> {
    let module = Module::dynamic("core-escape");
    let core = module.core();
    let _verified = module.verify()?;
    core.append_module_asm("mutated after verify");
    Ok(())
}
