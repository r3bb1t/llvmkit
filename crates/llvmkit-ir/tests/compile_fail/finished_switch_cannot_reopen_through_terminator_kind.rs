//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `InstructionView::terminator_kind` is a read-only discriminator. Re-discovering
//! a finished switch through it must not mint a fresh `Open` switch handle.

use llvmkit_ir::{IrBuilder, Linkage, Module, TerminatorKind};

fn main() {
    let m = Module::dynamic("switch-kind");
    let i32_ty = m.i32_type();
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let dest = m.view(f).append_basic_block(&m, "dest");
    let dest_label = dest.id();
    let cond = m.view(f).param(0).unwrap();
    let b = IrBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);
    let (sealed, switch) = b.switch_dyn(cond, dest_label, "").unwrap();
    let _closed = switch.finish();

    if let Some(TerminatorKind::Switch(reopened)) = sealed.terminator().unwrap().terminator_kind() {
        let _ = reopened.add_case(i32_ty.const_int(1_i32), dest_label);
    }
}
