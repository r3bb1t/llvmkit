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

use llvmkit_ir::{B32, IRBuilder, IntValue, IrError, Linkage, Module, RInt};

fn build() -> Result<(), IrError> {
    let m = Module::new("demo");
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
    let f = m.add_function::<RInt<B32>>("add", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");

    let b = IRBuilder::new_for::<RInt<B32>>(&m).position_at_end(entry);
    let lhs: IntValue<B32> = f.param(0)?.try_into()?;
    let rhs: IntValue<B32> = f.param(1)?.try_into()?;
    let sum = b.build_int_add(lhs, rhs, "sum")?;
    b.build_ret(sum)?;

    print!("{m}");
    Ok(())
}

fn main() {
    if let Err(e) = build() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
