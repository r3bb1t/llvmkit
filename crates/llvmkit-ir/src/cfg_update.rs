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
//! `cfg::Update<BasicBlock *>` (`Support/CFGUpdate.h`), which
//! `DominatorTree::UpdateType` names: only edge insertions and deletions, each
//! naming its two blocks. The blocks are storable [`BlockId`]s, as upstream's
//! are the blocks themselves. Value-level analyses (KnownBits, DemandedBits)
//! are out of scope — every mutating rung's preservation floor already evicts
//! them, and instruction-level events are a documented possible extension, not
//! designed here.
//!
//! [`split_block`]: crate::pass_context::FnReshape::split_block
//! [`CfgIncremental`]: crate::analysis::CfgIncremental

#![deny(missing_docs)]

use crate::Branded;
use crate::marker::Dyn;
use crate::module::ModuleBrand;
use crate::value_id::BlockId;

/// A directed CFG edge, identified by its endpoint blocks' storable ids.
/// Mirrors the `From` / `To` pair of `cfg::Update` (`Support/CFGUpdate.h`).
///
/// Distinct from [`crate::cfg::BasicBlockEdge`], as upstream keeps
/// `BasicBlockEdge` (`IR/Dominators.h`) apart from `cfg::Update`: this edge is
/// the payload of a recorded update, so the reshape mutator can own a plain
/// `Vec<CfgUpdate<B>>` and the driver can drain it after the borrow of the
/// function ends. Like every id, each endpoint carries its module's tag;
/// resolve one with [`Module::view`](crate::Module::view) to read the block.
///
/// Fields are private: an edge — and therefore a [`CfgUpdate`] — can only be
/// constructed inside the crate. Downstream analyses implementing
/// [`CfgIncremental`](crate::analysis::CfgIncremental) read the endpoints through [`Self::from`]
/// / [`Self::to`] but cannot fabricate one.
#[derive(Branded)]
pub struct CfgEdge<B: ModuleBrand> {
    from: BlockId<Dyn, B>,
    to: BlockId<Dyn, B>,
}

impl<B: ModuleBrand> CfgEdge<B> {
    #[inline]
    pub(crate) fn new(from: BlockId<Dyn, B>, to: BlockId<Dyn, B>) -> Self {
        Self { from, to }
    }

    /// Predecessor endpoint — the block the edge leaves. Mirrors
    /// `cfg::Update::getFrom`.
    #[inline]
    pub fn from(&self) -> BlockId<Dyn, B> {
        self.from
    }

    /// Successor endpoint — the block the edge enters. Mirrors
    /// `cfg::Update::getTo`.
    #[inline]
    pub fn to(&self) -> BlockId<Dyn, B> {
        self.to
    }
}

/// One structural change to a function's CFG, in the LLVM `DomTreeUpdater`
/// vocabulary (`cfg::Update`'s `UpdateKind::Insert` / `Delete`). The reshape
/// mutator records these as it edits; a
/// [`CfgIncremental`](crate::analysis::CfgIncremental) analysis consumes a slice of them to
/// repair its cached result.
///
/// Exhaustive by design (no `#[non_exhaustive]`): a future update kind must
/// break every analysis's repair `match`, because silently ignoring an
/// unhandled edit is exactly the incremental-update bug this vocabulary exists
/// to make unrepresentable. Construction stays crate-private via [`CfgEdge`]'s
/// private fields, so exhaustive downstream matching and non-fabrication
/// coexist.
#[derive(Branded)]
pub enum CfgUpdate<B: ModuleBrand> {
    /// A new edge `from → to` was created.
    InsertEdge(CfgEdge<B>),
    /// An existing edge `from → to` was removed.
    DeleteEdge(CfgEdge<B>),
}

impl<B: ModuleBrand> CfgUpdate<B> {
    /// Record an inserted edge `from → to`. Crate-private: only a structural
    /// edit method may mint one.
    #[inline]
    pub(crate) fn insert(from: BlockId<Dyn, B>, to: BlockId<Dyn, B>) -> Self {
        Self::InsertEdge(CfgEdge::new(from, to))
    }

    /// Record a deleted edge `from → to`. Crate-private: only a structural edit
    /// method may mint one.
    #[inline]
    pub(crate) fn delete(from: BlockId<Dyn, B>, to: BlockId<Dyn, B>) -> Self {
        Self::DeleteEdge(CfgEdge::new(from, to))
    }

    /// The edge this update concerns, regardless of whether it was inserted or
    /// deleted.
    #[inline]
    pub fn edge(&self) -> CfgEdge<B> {
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
