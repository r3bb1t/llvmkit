//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! A terminator builder consumes its positioned builder and returns a `Sealed`
//! block view. Retaining an earlier `Unsealed` block handle must not permit a
//! second builder to append after the terminator.

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("retained-block", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External).unwrap();
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        b.build_ret_void();

        let _ = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    });
}
