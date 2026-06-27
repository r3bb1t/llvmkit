//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `CatchSwitchInst::finish` returns a `Closed` view. Retaining the original
//! `Open` handle must not permit more handlers to be added.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("retained-catchswitch", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let handler = f.append_basic_block(&m, "handler");
        let handler_label = handler.label();
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, cs) = b.build_catch_switch_within_none_to_caller("cs").unwrap();

        let _closed = cs.finish();
        let _ = cs.add_handler(handler_label);
    });
}
