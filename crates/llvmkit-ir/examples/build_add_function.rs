//! Build the equivalent of:
//!
//! ```llvm
//! define i32 @add(i32 %0, i32 %1) {
//! entry:
//!   %sum = add i32 %0, %1
//!   ret i32 %sum
//! }
//! ```
//!
//! programmatically via the `IrBuilder` analog. Run with:
//!
//! ```text
//! cargo run -p llvmkit-ir --example build_add_function
//! ```
//!
//! Prints real `.ll` thanks to the [`Display`](core::fmt::Display) impl
//! on [`Module`].

use llvmkit_ir::{IrBuilder, IrError, Linkage, module_new};

fn build() -> Result<(), IrError> {
    let mut module = module_new!("demo")?;
    // The layout token is the permission to place instructions; builders
    // borrow it `&mut`, and `key.module()` reads the module meanwhile.
    let mut key = module.layout_token();
    let m = key.module();
    let f = m.add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");

    let b = IrBuilder::at_end(&mut key, entry);
    let (lhs, rhs) = m.view(f).params();
    let sum = b.int_add(lhs, rhs, "sum")?;
    b.ret(sum)?;

    print!("{module}");
    Ok(())
}

pub fn main() {
    if let Err(e) = build() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
