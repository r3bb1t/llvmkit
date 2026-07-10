//! CFG-edit vocabulary for *framework-witnessed* analysis preservation.
//!
//! A `ReshapeCfg` pass reshapes control flow through the mutator's structural
//! edit methods (today just [`split_block`]). Each such method records the exact
//! edge decomposition of its own edit as a queue of [`CfgUpdate`]s on the
//! mutator. The driver later drains that queue and offers it to each cached
//! CFG-shaped analysis through the [`CfgIncremental`] hook (`analysis.rs`), so an
//! analysis is only ever marked preserved because the framework *watched* it
//! absorb the recorded edits — never because the author claimed it.
//!
//! This is the honesty backbone of Package 4: authors cannot construct, submit,
//! reorder, or omit updates. [`CfgEdge`]'s fields are private, so a [`CfgUpdate`]
//! can only be minted inside this crate (by the recording edit method); there is
//! no public API to push one onto the queue. The whole class of C++
//! `DomTreeUpdater` misuse (forgetting an edge, double-recording, wrong order) is
//! therefore unconstructible rather than merely discouraged.
//!
//! Vocabulary scope is deliberately CFG-shaped, mirroring LLVM's
//! `DominatorTree::UpdateType`: only edge insertions and deletions. Value-level
//! analyses (KnownBits, DemandedBits) are out of scope — every mutating rung's
//! preservation floor already evicts them, and instruction-level events are a
//! documented possible extension, not designed here.
//!
//! [`split_block`]: crate::pass_context::FnReshape::split_block
//! [`CfgIncremental`]: crate::analysis

#![deny(missing_docs)]

use crate::value::ValueId;

/// A directed CFG edge, identified by its endpoint blocks' stable value IDs.
///
/// Distinct from [`crate::cfg::BasicBlockEdge`] (which carries lifetime-bearing
/// block *labels* for live CFG snapshots): this edge is lifetime-free so the
/// reshape mutator can own a plain `Vec<CfgUpdate>` and the driver can drain it
/// after the borrow of the function ends. The endpoints are the same `ValueId`s
/// the dominator machinery already keys on.
///
/// Fields are private: an edge — and therefore a [`CfgUpdate`] — can only be
/// constructed inside the crate. Downstream analyses implementing
/// [`CfgIncremental`](crate::analysis) read the endpoints through [`Self::from`]
/// / [`Self::to`] but cannot fabricate one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CfgEdge {
    from: ValueId,
    to: ValueId,
}

impl CfgEdge {
    #[inline]
    pub(crate) fn new(from: ValueId, to: ValueId) -> Self {
        Self { from, to }
    }

    /// Predecessor endpoint — the block the edge leaves.
    #[inline]
    pub fn from(&self) -> ValueId {
        self.from
    }

    /// Successor endpoint — the block the edge enters.
    #[inline]
    pub fn to(&self) -> ValueId {
        self.to
    }
}

/// One structural change to a function's CFG, in the LLVM `DomTreeUpdater`
/// vocabulary. The reshape mutator records these as it edits; a
/// [`CfgIncremental`](crate::analysis) analysis consumes a slice of them to
/// repair its cached result.
///
/// Exhaustive by design (no `#[non_exhaustive]`): a future update kind must
/// break every analysis's repair `match`, because silently ignoring an
/// unhandled edit is exactly the incremental-update bug this vocabulary exists
/// to make unrepresentable. Construction stays crate-private via [`CfgEdge`]'s
/// private fields, so exhaustive downstream matching and non-fabrication
/// coexist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CfgUpdate {
    /// A new edge `from → to` was created.
    InsertEdge(CfgEdge),
    /// An existing edge `from → to` was removed.
    DeleteEdge(CfgEdge),
}

impl CfgUpdate {
    /// Record an inserted edge `from → to`. Crate-private: only a structural
    /// edit method may mint one.
    #[inline]
    pub(crate) fn insert(from: ValueId, to: ValueId) -> Self {
        Self::InsertEdge(CfgEdge::new(from, to))
    }

    /// Record a deleted edge `from → to`. Crate-private: only a structural edit
    /// method may mint one.
    #[inline]
    pub(crate) fn delete(from: ValueId, to: ValueId) -> Self {
        Self::DeleteEdge(CfgEdge::new(from, to))
    }

    /// The edge this update concerns, regardless of whether it was inserted or
    /// deleted.
    #[inline]
    pub fn edge(&self) -> CfgEdge {
        match self {
            Self::InsertEdge(e) | Self::DeleteEdge(e) => *e,
        }
    }

    /// Whether this update inserts (rather than deletes) its edge.
    #[inline]
    pub fn is_insert(&self) -> bool {
        matches!(self, Self::InsertEdge(_))
    }
}
