//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `IndirectBrInst::finish` returns a `Closed` view. Retaining the original
//! `Open` handle must not permit more destinations to be added.

use llvmkit_ir::{Dyn, IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("retained-indirectbr", |m| {
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, [ptr_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");
        let dest = f.append_basic_block(&m, "dest");
        let dest_label = dest.label();
        let addr = f.param(0).unwrap();
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let (_sealed, ibr) = b.build_indirectbr(addr, "").unwrap();

        let _closed = ibr.finish();
        let _ = ibr.add_destination(dest_label);
    });
}
