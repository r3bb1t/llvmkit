//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Verifier::visitTerminator` / successor checks in
//! `lib/IR/Verifier.cpp` reject malformed cross-function or cross-module control
//! flow at runtime. llvmkit pushes the module-provenance part into the Rust type
//! system: a branch target carrying one [`Module`] brand cannot be used by an
//! `IRBuilder` positioned in another branded [`Module`].

use llvmkit_ir::{IRBuilder, Linkage, Module, Type};

fn main() {
    Module::with_new::<_, _, _>("left", |left| {
        let void_ty = left.void_type();
        let fn_ty = left.fn_type(void_ty.as_type(), Vec::<Type<'_, _>>::new(), false);
        let f = left.add_function::<(), _>("left_f", fn_ty, Linkage::External).unwrap();
        let left_target = f.append_basic_block(&left, "target");

        Module::with_new::<_, _, _>("right", |right| {
            let void_ty = right.void_type();
            let fn_ty = right.fn_type(void_ty.as_type(), Vec::<Type<'_, _>>::new(), false);
            let f = right.add_function::<(), _>("right_f", fn_ty, Linkage::External).unwrap();
            let entry = f.append_basic_block(&right, "entry");
            let builder = IRBuilder::new_for::<()>(&right).position_at_end(entry);
            let _ = builder.build_br(left_target);
        });
    });
}
