//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `PhiInst::finish` returns a `Closed` view. Retaining the original `Open`
//! handle must not permit more incoming edges to be added through the same phi.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("retained-phi", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let entry_label = entry.label();
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let phi = b.build_int_phi::<i32, _>("p").unwrap();

        let _closed = phi.finish();
        let _ = phi.add_incoming(1_i32, entry_label);
    });
}
