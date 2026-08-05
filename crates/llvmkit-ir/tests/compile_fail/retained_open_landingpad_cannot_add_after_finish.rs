//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `LandingPadInst::finish` returns a `Closed` view. Retaining the original
//! `Open` handle must not permit more clauses to be added.

use llvmkit_ir::{Dyn, IrBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("retained-landingpad");
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type<_>>::new(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let null_ptr = ptr_ty.const_null();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lp = b.landingpad(i32_ty.as_type(), true, "lp").unwrap();

    let _closed = lp.finish();
    let _ = lp.add_catch_clause(null_ptr);
}
