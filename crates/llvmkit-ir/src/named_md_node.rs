//! NamedMDNode storage. Mirrors `llvm/include/llvm/IR/Metadata.h`'s
//! `NamedMDNode` class. Each node is a named list of [`MetadataId`].

use crate::Branded;
use crate::metadata::MetadataId;
use crate::module::ModuleBrand;

/// A named metadata node. Mirrors `NamedMDNode` in `Metadata.h`.
///
/// Brand-generic because its operands are the tagged metadata currency: a
/// module stores its own nodes under the crate-private storage brand, and
/// `Module::named_metadata_add_operand` tag-checks a caller's
/// [`MetadataId<B>`] before it lands here.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct NamedMDNode<B: ModuleBrand> {
    name: String,
    operands: Vec<MetadataId<B>>,
}

impl<B: ModuleBrand> NamedMDNode<B> {
    /// Construct an empty named metadata node with the given name.
    pub fn new<Name>(name: Name) -> Self
    where
        Name: Into<String>,
    {
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
    pub fn add_operand(&mut self, op: MetadataId<B>) {
        self.operands.push(op);
    }

    /// All operands in insertion order.
    pub fn operands(&self) -> &[MetadataId<B>] {
        &self.operands
    }

    /// Number of operands.
    pub fn operand_count(&self) -> usize {
        self.operands.len()
    }
}
