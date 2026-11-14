//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `CatchSwitchInst::finish` returns a `Closed` view. Retaining the original
//! `Open` handle must not permit more handlers to be added.

use llvmkit_ir::{Dyn, IrBuilder, Linkage, Module};

fn main() {
    let mut module = Module::dynamic("retained-catchswitch"); let mut key = module.layout_token(); let m = key.module();
    let void_ty = m.void_type();
    let fn_ty = m.function_type(void_ty, Vec::<llvmkit_ir::Type<_>>::new()).expect("valid function type");
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let handler = m.view(f).append_basic_block(&m, "handler");
    let handler_label = handler.id();
    let b = IrBuilder::new_for::<Dyn>(&mut key).try_position_at_end(entry).unwrap();
    let (_sealed, cs) = b.catch_switch_within_none_to_caller("cs");

    let _closed = cs.finish();
    let _ = cs.add_handler(handler_label);
}
