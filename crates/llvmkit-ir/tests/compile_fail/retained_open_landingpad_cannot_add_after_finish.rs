//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `LandingPadInst::finish` returns a `Closed` view. Retaining the original
//! `Open` handle must not permit more clauses to be added.

use llvmkit_ir::{Dyn, IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("retained-landingpad", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let null_ptr = ptr_ty.const_null();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let lp = b.build_landingpad(i32_ty.as_type(), true, "lp").unwrap();

        let _closed = lp.finish();
        let _ = lp.add_catch_clause(null_ptr);
    });
}
