//! llvmkit typestate compile-fail (Doctrine D1).
//!
//! Closest upstream behaviour: `Verifier::visitBasicBlock` in
//! `lib/IR/Verifier.cpp` rejects a block that has a non-terminator
//! after its terminator at runtime. llvmkit moves the rule to the
//! type system: once a builder consumes its insertion block via a
//! terminator-emitting build, the block is `Terminated` and cannot be
//! re-positioned by `IrBuilder::try_position_at_end` (which, like
//! `position_at_end`, only accepts `BasicBlock<R, Unterminated>`). The
//! module is `Module::dynamic`, whose `DynBrand` is not a `UniqueBrand`, so
//! the checked `try_` form is the one that reaches the block-typestate check
//! without a second, unrelated brand error.

use llvmkit_ir::{IrBuilder, Linkage, Module};

fn main() {
    let mut module = Module::dynamic("c"); let mut key = module.layout_token(); let m = key.module();
    let f = m
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .unwrap()
        .as_function();
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<()>(&mut key).try_position_at_end(entry).unwrap();
    let (terminated_bb, _term) = b.ret_void();
    // `terminated_bb` carries `Terminated`, which `try_position_at_end` does not accept.
    let _ = IrBuilder::new_for::<()>(&mut key).try_position_at_end(terminated_bb);
}
