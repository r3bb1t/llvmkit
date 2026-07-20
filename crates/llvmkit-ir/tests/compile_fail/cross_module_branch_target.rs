//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Verifier::visitTerminator` / successor checks in
//! `lib/IR/Verifier.cpp` reject malformed cross-function or cross-module control
//! flow at runtime. llvmkit pushes the module-provenance part into the Rust type
//! system: a branch target carrying one [`Module`] brand cannot be used by an
//! `IRBuilder` positioned in another branded [`Module`].

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new::<_, _, _>("left", |left| {
        let f = left
            .add_typed_function::<(), (), _>("left_f", Linkage::External)
            .unwrap()
            .as_function();
        let left_target = f.append_basic_block(&left, "target");

        Module::with_new::<_, _, _>("right", |right| {
            let f = right
                .add_typed_function::<(), (), _>("right_f", Linkage::External)
                .unwrap()
                .as_function();
            let entry = f.append_basic_block(&right, "entry");
            let builder = IRBuilder::new_for::<()>(&right).position_at_end(entry);
            let _ = builder.build_br(left_target);
        });
    });
}
