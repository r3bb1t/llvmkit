//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `BasicBlock::terminator` returns an `InstructionView`. Read-only block
//! rediscovery must not expose lifecycle erasure.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("terminator-view", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);
        let (sealed, _ret) = b.build_ret_void().unwrap();
        let term = sealed.terminator().unwrap();

        term.erase_from_parent(&m);
    });
}
