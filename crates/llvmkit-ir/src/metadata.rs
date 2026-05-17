//! Metadata types. Mirrors `llvm/include/llvm/IR/Metadata.h`.
//!
//! Scope: the constructive subset needed for `!N = !{ ... }` tuples,
//! `!N = !"..."` strings, and named metadata (`!foo = !{ !N }`). Specialized
//! DI nodes (DILocation etc.) are explicitly deferred.

/// Stable index into the module-level metadata arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetadataId(pub(crate) usize);

impl MetadataId {
    /// Construct from a raw index. Used by the parser to map `!N` slots.
    pub fn from_index(index: usize) -> Self {
        Self(index)
    }

    /// Numeric index of this id. Used by the AsmWriter for slot numbering.
    pub fn index(self) -> usize {
        self.0
    }
}

/// Base metadata discriminant. Mirrors `Metadata::MetadataKind` in `Metadata.h`.
#[derive(Debug, Clone)]
pub enum MetadataKind {
    /// `!"..."` — a string node. Mirrors `MDString`.
    String(String),
    /// `!{ op, op, ... }` — a tuple. Mirrors `MDTuple`.
    Tuple(Vec<MetadataRef>),
    /// `!N` — reference to an already-interned metadata node.
    Ref(MetadataId),
}

/// Public metadata reference. `None` is the "null" metadata operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetadataRef(pub MetadataId);

/// Storage arena for all metadata nodes. Owned by `Module`.
/// Mirrors the `LLVMContextImpl::MetadataStore` pattern.
#[derive(Debug, Default)]
pub struct MetadataStore {
    nodes: Vec<MetadataKind>,
}

impl MetadataStore {
    /// Intern a string node. Returns an existing id if an identical string
    /// was already inserted (mirrors `MDString::get`).
    pub fn get_string(&mut self, s: impl Into<String>) -> MetadataId {
        let s = s.into();
        // Linear scan is fine for the constructive subset.
        for (i, node) in self.nodes.iter().enumerate() {
            if let MetadataKind::String(existing) = node {
                if *existing == s {
                    return MetadataId(i);
                }
            }
        }
        let id = MetadataId(self.nodes.len());
        self.nodes.push(MetadataKind::String(s));
        id
    }

    /// Create a tuple node (distinct; not deduplicated). Mirrors `MDTuple::get`
    /// for the distinct case.
    pub fn get_tuple(&mut self, operands: Vec<MetadataRef>) -> MetadataId {
        let id = MetadataId(self.nodes.len());
        self.nodes.push(MetadataKind::Tuple(operands));
        id
    }

    /// Total number of interned metadata nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when the store has no interned nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Look up a metadata node by id.
    pub fn get(&self, id: MetadataId) -> Option<&MetadataKind> {
        self.nodes.get(id.0)
    }

    /// Slice over all nodes, indexed by their `MetadataId.index()`.
    /// Used by the AsmWriter for ordered emission.
    pub(crate) fn nodes(&self) -> &[MetadataKind] {
        &self.nodes
    }
}
