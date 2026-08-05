//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `BasicBlock::terminator` returns an `InstructionView`. Read-only block
//! rediscovery must not expose lifecycle erasure.

use llvmkit_ir::{IrBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("terminator-view");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type<_>>::new(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);
    let (sealed, _ret) = b.ret_void().unwrap();
    let term = sealed.terminator().unwrap();

    term.erase_from_parent(&m);
}
