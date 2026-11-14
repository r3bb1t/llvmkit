//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! No upstream counterpart: LLVM's `IRBuilder` keeps a raw iterator to its
//! anchor, and erasing that anchor while the builder is positioned before it
//! leaves the iterator dangling.
//!
//! llvmkit makes the erase unspellable while the builder lives: erasing an
//! instruction takes the module's `LayoutToken` `&mut`, and a builder holds
//! that token `&mut` for as long as it lives. Primary error is rustc's stable
//! `E0499`. The run-time refusal that remains is `restore_insert_point` of a
//! saved snapshot whose anchor was erased since
//! (`tests/builder_errors.rs::a_snapshot_before_an_erased_anchor_refuses_to_restore`).

use llvmkit_ir::{IrBuilder, Linkage, Module};

fn main() {
    let mut module = Module::dynamic("c"); let mut key = module.layout_token(); let m = key.module();
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let (_, ret) = IrBuilder::new_for::<()>(&mut key).try_position_at_end(entry).unwrap().ret_void();
    let before = IrBuilder::new_for::<()>(&mut key).position_before(ret.placed()).unwrap();
    // `before` still holds `key`: its anchor cannot be erased under it.
    ret.erase_from_parent(&mut key).unwrap();
    let _ = before.ret_void();
}
