//! llvmkit typestate compile-fail (Doctrine D1), Task 19.
//!
//! Every `SsaBuilder` terminator (`br`/`cond_br`/`switch`/`ret`/
//! `ret_void`/`unreachable`) consumes `self` by value and returns a NEW
//! `Unpositioned` builder -- mirroring `retained_unterminated_block_cannot_reposition.rs`'s
//! plain-`IRBuilder` lock, but for the SSA layer's own terminator family.
//! Retaining and reusing the pre-terminator `Positioned` builder handle
//! after its own `br` call already moved it out is a compile error, not
//! a runtime one: the terminator methods take `mut self`, so a second
//! use of the same binding is `E0382` (use of moved value), exactly like
//! attempting to append to an already-terminated plain `BasicBlock`.

use llvmkit_ir::{Linkage, Module};

fn main() {
    Module::with_new("ssa-use-after-terminator", |m| {
        let f = m
            .add_typed_function::<(), (), _>("f", Linkage::External)
            .unwrap()
            .as_function();
        let mut b = llvmkit_ir::SsaBuilder::for_function(&m, f).unwrap();
        let entry = b.create_block("entry");
        let second = b.create_block("second");

        let positioned = b.switch_to_block(entry).unwrap();
        let _unpositioned = positioned.br(second).unwrap();

        // `positioned` was moved into `br` above -- using it again is a
        // compile error, not a second (runtime) terminator on `entry`.
        let _oops = positioned.br(second);
    });
}
