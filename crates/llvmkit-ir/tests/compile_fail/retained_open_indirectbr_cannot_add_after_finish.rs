//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! `IndirectBrInst::finish` returns a `Closed` view. Retaining the original
//! `Open` handle must not permit more destinations to be added.

use llvmkit_ir::{Dyn, IrBuilder, Linkage, Module, PointerValue};

fn main() {
    let m = Module::dynamic("retained-indirectbr");
    let ptr_ty = m.ptr_type(0);
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let dest = m.view(f).append_basic_block(&m, "dest");
    let dest_label = dest.id();
    // Narrow explicitly: after the strict cut an erased `Argument` no
    // longer lifts into a typed pointer position, and this fixture's
    // subject is the retained-`Open`-handle lifecycle below, not the
    // operand typing.
    let addr: PointerValue<'_, _> = m.view(f).param(0).unwrap().try_into().unwrap();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let (_sealed, ibr) = b.indirectbr(addr, "").unwrap();

    let _closed = ibr.finish();
    let _ = ibr.add_destination(dest_label);
}
