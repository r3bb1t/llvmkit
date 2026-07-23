//! llvmkit typestate compile-fail (Doctrine D1), Task 19.
//!
//! `SsaBuilder::finish` is defined ONLY in the `Unpositioned`-state impl
//! block (`crates/llvmkit-ir/src/ssa_builder.rs`) -- consuming `self` on
//! `Unpositioned` gives two static guarantees together: no def/use/
//! terminator call is reachable after `finish`, and no block can be
//! mid-construction (`Positioned`) when it runs. A `Positioned` builder
//! (mid-construction of some block, terminator not yet emitted) exposes
//! no `finish` method at all -- "seal every remaining block, then
//! require every block filled" would be unsound to run while a block is
//! still open for insertion. Closest upstream behaviour: LLVM has no
//! on-the-fly SSA layer with an analogous "finish construction" step;
//! the nearest functional idea is `Function::back()->getTerminator()`
//! being required non-null before a function is considered
//! well-formed, enforced here at the type level instead of via
//! `Verifier::visitBasicBlock`'s runtime "has no terminator" check.

use llvmkit_ir::{Linkage, Module};

fn main() {
    Module::with_new("ssa-finish-positioned", |m| {
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)
            .unwrap()
            .as_function();
        let mut b = llvmkit_ir::SsaBuilder::for_function(&m, f).unwrap();
        let entry = b.create_block("entry");

        let b = b.switch_to_block(entry).unwrap();
        // `b` is `Positioned` here (no terminator emitted yet on
        // `entry`) -- `finish` does not exist on this state, only on
        // `Unpositioned`.
        b.finish().unwrap();
    });
}
