//! llvmkit typestate compile-fail (Doctrine D1, D2).
//!
//! Closest upstream behaviour: `Verifier::visitBasicBlock` in
//! `lib/IR/Verifier.cpp` rejects a block carrying two terminators at run time.
//! In LLVM C++ nothing stops the caller — `IRBuilder` keeps its insertion point
//! after `CreateRetVoid()`, so a second `CreateRetVoid()` on the same builder
//! silently appends a second terminator and the module is malformed until the
//! verifier is run.
//!
//! llvmkit makes the second call unspellable: every terminator-emitting build
//! takes `self` **by value**, so the builder is *consumed* by the first one.
//! This is the linearity half of the rule, and it is distinct from the block
//! typestate that `position_at_end_terminated_block.rs` locks: that fixture
//! proves a `Terminated` *block* cannot be re-positioned into, while this one
//! proves the *builder* itself no longer exists. Primary error is rustc's
//! stable `E0382` use-of-moved-value.

use llvmkit_ir::{IrBuilder, Linkage, Module};

fn main() {
    let m = Module::dynamic("c");
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<()>(&m).position_at_end(entry);
    let (_terminated_bb, _term) = b.ret_void();
    // `ret_void` took `b` by value: there is no builder left to terminate
    // a second time.
    let (_again, _term2) = b.ret_void();
}
