//! llvmkit-specific compile-fail (Doctrine D7), not a 1:1 LLVM test port.
//!
//! Closest upstream behaviour: `Instruction::setMetadata` in
//! `lib/IR/Metadata.cpp` takes an `MDNode *` with no notion of which module
//! owns it, so attaching one module's node inside another is accepted and only
//! surfaces (if at all) as a corrupt `.ll` dump. llvmkit rejects the *storable
//! id* form at compile time.
//!
//! The metadata twin of `cross_named_brand_id_view.rs`, and the compile-time
//! half of the runtime property locked by
//! `tests/module_ownership.rs::a_metadata_id_from_another_module_is_refused_everywhere`:
//!
//! - two **different** named brands separate two modules' metadata statically —
//!   a `MetadataId<Left>` is not even the right *type* to hand to the `Right`
//!   module, which is what this fixture pins;
//! - two **generations of the same** brand (or two `DynBrand` modules) share one
//!   type, so they can only be separated by the runtime `ModuleId` tag, which is
//!   what the runtime test pins.
//!
//! Until the polish cycle a metadata handle was a bare `usize` arena
//! index carrying neither half, so this program compiled *and* mis-resolved.

use llvmkit_ir::{Module, ModuleBrand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Left;
impl ModuleBrand for Left {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Right;
impl ModuleBrand for Right {}

fn main() {
    let left = Module::branded::<Left, _>("left").unwrap();
    let left_node = left.metadata_string("from-left");

    let right = Module::branded::<Right, _>("right").unwrap();
    let named = right.get_or_insert_named_metadata("right.named");
    // `left_node` is a `MetadataId<Left>`; `right` accepts only `MetadataId<Right>`.
    let _ = right.named_metadata_add_operand(named, left_node);
}
