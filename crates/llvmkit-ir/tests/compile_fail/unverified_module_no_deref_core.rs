use llvmkit_ir::Module;

fn main() {
    Module::with_new::<_, _, _>("no-deref", |module| {
        let _ = module.i64_type();
        let _ = (*module).i64_type();
    });
}
