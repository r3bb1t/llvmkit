//! Transient view of a single use-edge. Mirrors
//! `llvm/include/llvm/IR/Use.h`.
//!
//! ## Why a transient view, not an intrusive list
//!
//! Upstream LLVM links `Use` nodes into an intrusive doubly-linked use
//! list rooted at each `Value`. The list is updated under
//! `setOperand`, dropped on `eraseFromParent`, and walked by every
//! analysis. The shape requires hung-off operands and a small graveyard
//! of pointer-tagging tricks (see `User.h` and `Use.cpp`).
//!
//! For the foundation we deliberately do **not** ship that machinery.
//! Operand storage lives inside the per-instruction storage record
//! as a plain `Vec<ValueId>`, and a [`Use`] view is materialized on
//! demand by [`User::operand_use`](crate::user::User::operand_use).
//! Rebuilding the view is `O(1)` per edge, and analyses that need to
//! iterate `value.users()` will get a cheap linear scan over the arena
//! once those analyses land. If a future profile says we need the
//! intrusive list for compile-time, we can layer it back on without
//! changing the public [`Use`] surface.

use crate::value::Value;

/// A read-only view of one operand-edge: "user `U` references value
/// `V` at operand index `i`".
///
/// Lifetimes match the originating [`User`](crate::user::User) borrow.
/// The view is `Copy`; mutating the use-graph goes through `User`'s
/// own (yet-to-land) editing methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Use<'ctx> {
    user: Value<'ctx>,
    operand: Value<'ctx>,
    index: u32,
}

impl<'ctx> Use<'ctx> {
    /// Crate-internal constructor.
    #[inline]
    pub(crate) fn new(user: Value<'ctx>, operand: Value<'ctx>, index: u32) -> Self {
        Self {
            user,
            operand,
            index,
        }
    }

    /// The using-side of the edge.
    #[inline]
    pub fn user(self) -> Value<'ctx> {
        self.user
    }

    /// The operand value.
    #[inline]
    pub fn operand(self) -> Value<'ctx> {
        self.operand
    }

    /// 0-based operand index within the user.
    #[inline]
    pub fn index(self) -> u32 {
        self.index
    }
}
