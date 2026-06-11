//! NamedMDNode storage. Mirrors `llvm/include/llvm/IR/Metadata.h`'s
//! `NamedMDNode` class. Each node is a named list of `MetadataRef`.

use crate::metadata::MetadataRef;

/// A named metadata node. Mirrors `NamedMDNode` in `Metadata.h`.
#[derive(Debug, Clone)]
pub struct NamedMDNode {
    name: String,
    operands: Vec<MetadataRef>,
}

impl NamedMDNode {
    /// Construct an empty named metadata node with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            operands: Vec::new(),
        }
    }

    /// The bare name of this node (without leading `!`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Append an operand.
    pub fn add_operand(&mut self, op: MetadataRef) {
        self.operands.push(op);
    }

    /// All operands in insertion order.
    pub fn operands(&self) -> &[MetadataRef] {
        &self.operands
    }

    /// Number of operands.
    pub fn operand_count(&self) -> usize {
        self.operands.len()
    }
}
