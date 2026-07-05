//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! A terminated block view is read-only rediscovery. It must not start a
//! lifecycle-producing `BlockCursor`.

use llvmkit_ir::{IRBuilder, Linkage, Module, iter::BlockCursor};

fn main() {
    Module::with_new("terminated-cursor", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (terminated, _ret) = b.build_ret_void();

        let _cursor = BlockCursor::at_start(terminated);
    });
}
