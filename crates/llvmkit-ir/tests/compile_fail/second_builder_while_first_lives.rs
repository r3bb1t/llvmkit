//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! No upstream counterpart: two `IRBuilder`s over one block are legal in LLVM
//! C++, and a second one appending after the first one's terminator is how a
//! builder's insertion point went stale behind its back.
//!
//! llvmkit makes the second builder unspellable while the first lives: a
//! builder holds the module's `LayoutToken` `&mut` for as long as it lives, so
//! minting another builder from the same token is a second mutable borrow.
//! Primary error is rustc's stable `E0499`.

use llvmkit_ir::{IrBuilder, Linkage, Module};

fn main() {
    let mut module = Module::dynamic("c"); let mut key = module.layout_token(); let m = key.module();
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let first = IrBuilder::new_for::<()>(&mut key).try_position_at_end(entry).unwrap();
    // `first` still holds `key`: a second builder cannot borrow it.
    let second = IrBuilder::new_for::<()>(&mut key);
    let _ = first.ret_void();
    let _ = second;
}
