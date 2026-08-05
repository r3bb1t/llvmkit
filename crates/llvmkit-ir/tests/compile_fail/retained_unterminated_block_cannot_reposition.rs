//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! A terminator builder consumes its positioned builder and returns a
//! `Terminated` block view. Retaining an earlier `Unterminated` block handle
//! must not permit a second builder to append after the terminator.

use llvmkit_ir::{IrBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("retained-block");
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = m.view(f).append_basic_block(&m, "entry");

    let b = IrBuilder::new_for::<()>(&m).position_at_end(entry);
    b.build_ret_void();

    let _ = IrBuilder::new_for::<()>(&m).position_at_end(entry);
}
