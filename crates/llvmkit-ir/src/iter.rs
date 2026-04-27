//! Iteration helpers for IR data structures. Mirrors the pieces of
//! `llvm/include/llvm/IR/BasicBlock.h` that walk an instruction list
//! while permitting in-place mutation (erase, detach, splice).
//!
//! The canonical upstream pattern is the "advance-then-mutate" loop:
//!
//! ```cpp
//! for (auto I = BB->begin(); I != BB->end();) {
//!     auto Next = std::next(I);
//!     if (shouldErase(*I)) I->eraseFromParent();
//!     I = Next;
//! }
//! ```
//!
//! [`BlockCursor`] encodes that protocol in the type system: each step
//! consumes the cursor and returns a fresh one positioned at the next
//! instruction, so the caller cannot accidentally observe iterator
//! state that has been invalidated by a mutation. This is Doctrine D9
//! (iteration safety is structural).

use crate::basic_block::BasicBlock;
use crate::instruction::{Instruction, state};
use crate::marker::ReturnMarker;
use crate::value::ValueId;

/// Single-pass cursor over an instruction list. Each [`Self::next`]
/// call yields the instruction at the current position together with a
/// fresh cursor pointing at what was the *next* instruction *at the
/// time of the call*. The split happens before the caller has a chance
/// to mutate, so erasing or detaching the yielded instruction does not
/// invalidate the cursor.
///
/// Mirrors LLVM's `auto Next = std::next(I);` idiom.
pub struct BlockCursor<'ctx, R: ReturnMarker> {
    block: BasicBlock<'ctx, R>,
    /// Snapshot of the block's instruction list at cursor creation.
    /// We snapshot once and walk by index so subsequent mutations to
    /// the *underlying* list (insertions before us, splices, etc.) do
    /// not perturb the iteration order. The cursor walks the original
    /// observation; the caller is responsible for any extra-curricular
    /// mutations they might perform on the live list.
    snapshot: Vec<ValueId>,
    next_index: usize,
}

impl<'ctx, R: ReturnMarker> BlockCursor<'ctx, R> {
    /// Create a cursor positioned at the start of `block`. Mirrors
    /// `BB->begin()` in C++.
    pub fn at_start(block: BasicBlock<'ctx, R>) -> Self {
        let snapshot: Vec<ValueId> = block.instructions().map(|i| i.as_value().id).collect();
        Self {
            block,
            snapshot,
            next_index: 0,
        }
    }

    /// Yield the instruction at the current position, returning `Some`
    /// of it together with a fresh cursor advanced past it. Returns
    /// `None` when the snapshot is exhausted.
    pub fn next(self) -> Option<(Instruction<'ctx, state::Attached>, BlockCursor<'ctx, R>)> {
        let id = *self.snapshot.get(self.next_index)?;
        let module = self.block.as_value().module();
        let inst = Instruction::from_parts(id, module);
        let next = BlockCursor {
            block: self.block,
            snapshot: self.snapshot,
            next_index: self.next_index + 1,
        };
        Some((inst, next))
    }
}
