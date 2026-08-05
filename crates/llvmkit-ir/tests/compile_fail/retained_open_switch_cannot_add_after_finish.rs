//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `SwitchInst::finish` returns a `Closed` view. Retaining the original `Open`
//! handle must not permit more cases to be added through the same switch.

use llvmkit_ir::{Dyn, IrBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("retained-switch");
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let dest = m.view(f).append_basic_block(&m, "dest");
    let dest_label = dest.id();
    let cond = m.view(f).param(0).unwrap();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let (_sealed, switch) = b.build_switch_dyn(cond, dest_label, "").unwrap();

    let _closed = switch.finish();
    let _ = switch.add_case(i32_ty.const_int(1_i32), dest_label);
}
