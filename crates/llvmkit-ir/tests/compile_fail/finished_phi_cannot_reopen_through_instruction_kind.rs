//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `Instruction::kind` is a read-only discriminator. Re-discovering a finished
//! phi through it must not mint a fresh `Open` phi handle.

use llvmkit_ir::{IRBuilder, InstructionKind, Linkage, Module};

fn main() {
    Module::with_new("phi-kind", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        let join = f.append_basic_block(&m, "join");
        let entry_label = entry.label();
        let other_label = other.label();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(join);
        let phi = b
            .build_int_phi::<i32, _>("p")
            .unwrap()
            .add_incoming(1_i32, entry_label)
            .unwrap()
            .finish();

        if let Some(InstructionKind::Phi(reopened)) = phi.as_view().kind() {
            let _ = reopened.add_incoming(2_i32, other_label);
        }
    });
}
