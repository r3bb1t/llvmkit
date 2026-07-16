//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `SwitchInst::finish` returns a `Closed` view. Retaining the original `Open`
//! handle must not permit more cases to be added through the same switch.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("retained-switch", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();
        let cond = f.param(0).unwrap();
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (_sealed, switch) = b.build_switch_dyn(cond, dest_label, "").unwrap();

        let _closed = switch.finish();
        let _ = switch.add_case(i32_ty.const_int(1_i32), dest_label);
    });
}
