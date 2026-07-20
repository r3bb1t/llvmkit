//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! Closest upstream behaviour: `Verifier::visitBasicBlock` in
//! `lib/IR/Verifier.cpp` rejects a block that has a non-terminator
//! after its terminator at runtime. llvmkit moves the rule to the
//! type system: once a builder consumes its insertion block via a
//! terminator-emitting build, the block is `Terminated` and cannot be
//! re-positioned by `IRBuilder::position_at_end` (which only accepts
//! `BasicBlock<R, Unterminated>`).

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    Module::with_new("c", |m| {
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)
            .unwrap()
            .as_function();
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (terminated_bb, _term) = b.build_ret_void();
        // `terminated_bb` carries `Terminated`, which `position_at_end` does not accept.
        let _ = IRBuilder::new_for::<()>(&m).position_at_end(terminated_bb);
    });
}
