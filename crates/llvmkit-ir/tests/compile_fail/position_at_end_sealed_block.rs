//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! Closest upstream behaviour: `Verifier::visitBasicBlock` in
//! `lib/IR/Verifier.cpp` rejects a block that has a non-terminator
//! after its terminator at runtime. llvmkit moves the rule to the
//! type system: once a builder consumes its insertion block via a
//! terminator-emitting build, the block is `Sealed` and cannot be
//! re-positioned by `IRBuilder::position_at_end` (which only accepts
//! `BasicBlock<R, Unsealed>`).

use llvmkit_ir::{IRBuilder, Linkage, Module};

fn main() {
    let m = Module::new("c");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External).unwrap();
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let (sealed_bb, _term) = b.build_ret_void();
    // `sealed_bb` carries `Sealed`, which `position_at_end` does not accept.
    let _ = IRBuilder::new_for::<()>(&m).position_at_end(sealed_bb);
}
