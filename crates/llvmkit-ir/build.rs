#![forbid(unsafe_code)]

#[path = "tools/gen_intrinsics.rs"]
mod gen_intrinsics;

fn main() {
    gen_intrinsics::main();
}
