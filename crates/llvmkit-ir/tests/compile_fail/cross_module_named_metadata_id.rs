//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! No upstream counterpart: upstream `Module::getOrInsertNamedMetadata`
//! (`lib/IR/Module.cpp`) returns a bare `NamedMDNode *` whose identity is the
//! pointer itself — nothing records which module owns it, so a node obtained
//! from one module can be handed to code operating on another and the mix-up
//! surfaces (if at all) only as a corrupt `.ll` dump. llvmkit's
//! `NamedMetadataId<B>` carries the brand, so two **distinct** named brands
//! make the mix-up a type error.
//!
//! The named-metadata sibling of `cross_module_metadata_attachment.rs`, which
//! pins the same law for the *operand* currency (`MetadataId<B>`); the runtime
//! `ModuleId`-tag half for same-brand / `DynBrand` modules is
//! `tests/module_ownership.rs::a_named_metadata_id_from_another_module_is_refused`.
//!
//! Until W6 a named-metadata handle was a bare `usize` list index carrying
//! neither a brand nor a tag, so this program compiled *and* mis-resolved.

use llvmkit_ir::{Module, ModuleBrand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Left;
impl ModuleBrand for Left {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Right;
impl ModuleBrand for Right {}

fn main() {
    let left = Module::branded::<Left, _>("left").unwrap();
    let left_named = left.get_or_insert_named_metadata("left.named");

    let right = Module::branded::<Right, _>("right").unwrap();
    let right_node = right.metadata_string("from-right");
    // `left_named` is a `NamedMetadataId<Left>`; `right` accepts only a
    // `NamedMetadataId<Right>`.
    let _ = right.named_metadata_add_operand(left_named, right_node);
}
