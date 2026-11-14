//! llvmkit typestate compile-fail (Doctrine D1, D3).
//!
//! A builder positioned at the end of a block builds `ret void` with no
//! `Result`: nothing in the block can move out from under that insertion
//! point. A builder positioned before an instruction cannot: the builder holds
//! `&Module`, so the anchor can be moved or erased after the builder was
//! positioned, and upstream's `IRBuilder` is undefined there. Its `ret_void`
//! returns an `IrResult`, so using it as the at-end tuple is a type error.
//! Primary error is rustc's stable `E0308`.

use llvmkit_ir::{IrBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("c");
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let (_, ret) = IrBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .ret_void();
    let before = IrBuilder::new_for::<()>(&m)
        .position_before(ret.placed())
        .unwrap();
    // Before an anchor, `ret_void` can be refused: it is an `IrResult`.
    let (_block, _ret) = before.ret_void();
}
