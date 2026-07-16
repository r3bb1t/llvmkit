//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `InstructionView::terminator_kind` is a read-only discriminator. Re-discovering
//! a finished switch through it must not mint a fresh `Open` switch handle.

use llvmkit_ir::{IRBuilder, Linkage, Module, TerminatorKind};

fn main() {
    Module::with_new("switch-kind", |m| {
        let i32_ty = m.i32_type();
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();
        let cond = f.param(0).unwrap();
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (sealed, switch) = b.build_switch_dyn(cond, dest_label, "").unwrap();
        let _closed = switch.finish();

        if let Some(TerminatorKind::Switch(reopened)) = sealed.terminator().unwrap().terminator_kind() {
            let _ = reopened.add_case(i32_ty.const_int(1_i32), dest_label);
        }
    });
}
