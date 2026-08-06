//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `InstructionView::kind` is a read-only discriminator. Re-discovering a
//! finished landingpad through it must not mint a fresh `Open` landingpad handle.

use llvmkit_ir::{IrBuilder, InstructionKind, Linkage, Module};

fn main() {
    let m = Module::dynamic("landingpad-kind");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.function_type(void_ty, Vec::<llvmkit_ir::Type<_>>::new());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let null_ptr = ptr_ty.const_null();
    let b = IrBuilder::new_for::<llvmkit_ir::marker::Dyn>(&m).position_at_end(entry);
    let lp = b.landingpad(i32_ty.as_type(), true, "lp").unwrap();
    let closed = lp.finish();

    if let Some(InstructionKind::LandingPad(reopened)) = closed.as_view().kind() {
        let _ = reopened.add_catch_clause(null_ptr);
    }
}
