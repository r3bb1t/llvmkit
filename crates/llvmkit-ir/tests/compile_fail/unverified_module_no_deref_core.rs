use llvmkit_ir::Module;

fn main() {
    let module = Module::dynamic("no-deref");
    let _ = module.i64_type();
    let _ = (*module).i64_type();
}
