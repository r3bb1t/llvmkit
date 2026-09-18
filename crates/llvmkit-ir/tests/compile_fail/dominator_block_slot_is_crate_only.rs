//! Compile-fail lock (error-surface cleanup Task 24, D7): a
//! `DominatorTreeBlock` bound does not hand out a block's bare arena slot.
//! The trait is sealed, and the seal carries the method the tree reads a
//! block's slot through. Sealing alone did not hide it: a bound on the public
//! trait brings the seal's methods into scope even where the seal cannot be
//! named, so before the method took an argument (at `bbd732d`) this call
//! returned the slot. It now also takes a value only llvmkit can build, so
//! the call has no argument a caller can supply (`E0061`).

use llvmkit_ir::DominatorTreeBlock;

fn leak<'ctx, T: DominatorTreeBlock<'ctx>>(block: T) {
    let _ = block.dominator_block_id();
}

fn main() {}
