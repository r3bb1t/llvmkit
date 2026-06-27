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
//! programmatically via the `IRBuilder` analog. Run with:
//!
//! ```text
//! cargo run -p llvmkit-ir --example build_add_function
//! ```
//!
//! Prints real `.ll` thanks to the [`Display`](core::fmt::Display) impl
//! on [`Module`].

use llvmkit_ir::{IRBuilder, IrError, Linkage, Module};

fn build() -> Result<(), IrError> {
    Module::with_new("demo", |m| {
        let f = m.add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (lhs, rhs) = f.params();
        let sum = b.build_int_add::<i32, _, _, _>(lhs, rhs, "sum")?;
        b.build_ret(sum)?;

        print!("{m}");
        Ok(())
    })
}

pub fn main() {
    if let Err(e) = build() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
