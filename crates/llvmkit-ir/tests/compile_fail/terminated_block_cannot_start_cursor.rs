//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! A terminated block view is read-only rediscovery. It must not start a
//! lifecycle-producing `BlockCursor`.

use llvmkit_ir::{IRBuilder, Linkage, Module, iter::BlockCursor};

fn main() {
    Module::with_new("terminated-cursor", |m| {
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)
            .unwrap()
            .as_function();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (terminated, _ret) = b.build_ret_void();

        let _cursor = BlockCursor::at_start(terminated);
    });
}
