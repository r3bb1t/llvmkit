//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `InstructionView::kind` is a read-only discriminator. Re-discovering a
//! finished landingpad through it must not mint a fresh `Open` landingpad handle.

use llvmkit_ir::{IRBuilder, InstructionKind, Linkage, Module};

fn main() {
    Module::with_new("landingpad-kind", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let null_ptr = ptr_ty.const_null();
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let lp = b.build_landingpad(i32_ty.as_type(), true, "lp").unwrap();
        let closed = lp.finish();

        if let Some(InstructionKind::LandingPad(reopened)) = closed.as_view().kind() {
            let _ = reopened.add_catch_clause(null_ptr);
        }
    });
}
