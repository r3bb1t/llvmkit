//! Public parser-by-product table mapping slot numbers to IR handles.
//!
//! Direct port of `llvm/include/llvm/AsmParser/SlotMapping.h`. Callers who
//! want to parse a textual snippet *outside* the original module's source
//! pass a [`SlotMapping`] to the parser, which fills in the unnamed-value
//! and named-type tables. Subsequent `parseConstantValue` / `parseType`
//! calls reuse the mapping so unnamed slots stay coherent.
//!
//! The upstream struct is:
//!
//! ```cpp
//! struct SlotMapping {
//!   NumberedValues<GlobalValue *> GlobalValues;
//!   std::map<unsigned, TrackingMDNodeRef> MetadataNodes;
//!   StringMap<Type *> NamedTypes;
//!   std::map<unsigned, Type *> Types;
//! };
//! ```
//!
//! The Rust port keeps the same shape, with two adjustments:
//!
//! - `GlobalValue *` becomes the typed [`GlobalRef`] enum so callers don't
//!   have to dyn-cast back to a concrete handle (Doctrine D3: erased forms
//!   are explicitly opt-in).
//! - The `MetadataNodes` slot is intentionally omitted until the metadata
//!   subsystem ships in `llvmkit-ir` (parser Session 4 / metadata substrate).
//!   Adding it now would be an empty stub.

use std::collections::BTreeMap;
use std::collections::HashMap;

use llvmkit_ir::{Dyn, FunctionValue, GlobalVariable, Type};

use crate::numbered_values::NumberedValues;

/// Erased handle for a slot-numbered global. Mirrors the `GlobalValue *`
/// payload of upstream `SlotMapping::GlobalValues`. Aliases / IFuncs are not
/// modeled in `llvmkit-ir` yet; the enum is `non_exhaustive` so adding their
/// arms later is non-breaking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GlobalRef<'ctx> {
    /// Function definition or declaration. Carries the [`Dyn`] return marker
    /// because the parser cannot pin the static return shape without
    /// depending on the IR-side typed-return surface.
    Function(FunctionValue<'ctx, Dyn>),
    /// Module-level data — `@x = global ...` / `@x = constant ...`.
    Variable(GlobalVariable<'ctx>),
}

impl<'ctx> From<FunctionValue<'ctx, Dyn>> for GlobalRef<'ctx> {
    #[inline]
    fn from(v: FunctionValue<'ctx, Dyn>) -> Self {
        GlobalRef::Function(v)
    }
}

impl<'ctx> From<GlobalVariable<'ctx>> for GlobalRef<'ctx> {
    #[inline]
    fn from(v: GlobalVariable<'ctx>) -> Self {
        GlobalRef::Variable(v)
    }
}

/// Slot-numbering tables a textual-IR parser fills in while it walks a
/// module. Public so callers can re-use the tables across follow-on
/// `parse_constant_value` / `parse_type` calls (mirrors the upstream
/// pass-pointer-and-fill-it pattern in `parseAssemblyString` /
/// `parseConstantValue`).
///
/// Lifetime brand: `'ctx` ties every stored handle to a single
/// [`llvmkit_ir::Module`]. Cross-module mixing is rejected by the borrow
/// checker (Doctrine D7).
#[derive(Debug)]
pub struct SlotMapping<'ctx> {
    /// Numbered globals — `@0`, `@1`, ... — keyed by slot id.
    pub global_values: NumberedValues<GlobalRef<'ctx>>,
    /// Named struct / opaque-struct types — `%foo` / `%bar`.
    pub named_types: HashMap<String, Type<'ctx>>,
    /// Numbered types — `%0`, `%1`, ... — sorted by slot id to match
    /// upstream's `std::map<unsigned, Type *>` ordering.
    pub numbered_types: BTreeMap<u32, Type<'ctx>>,
    // Metadata slot (`MetadataNodes`) is deliberately deferred. Lands with the
    // metadata substrate in the parser arc (Session 4).
}

impl<'ctx> Default for SlotMapping<'ctx> {
    #[inline]
    fn default() -> Self {
        Self {
            global_values: NumberedValues::new(),
            named_types: HashMap::new(),
            numbered_types: BTreeMap::new(),
        }
    }
}

impl<'ctx> SlotMapping<'ctx> {
    /// Empty mapping. Equivalent to `SlotMapping{}` in upstream.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llvmkit_ir::Module;

    /// Ports the structural assertions in
    /// `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
    /// SlotMappingTest)` for the surface that doesn't require the parser:
    /// freshly constructed mappings expose empty registries and an
    /// `getNext() == 0` global frontier. The parser-driven part of the
    /// upstream test waits on Session 2.
    #[test]
    fn fresh_mapping_is_empty() {
        let m: SlotMapping<'_> = SlotMapping::new();
        assert_eq!(m.global_values.get_next(), 0);
        assert!(m.global_values.is_empty());
        assert!(m.named_types.is_empty());
        assert!(m.numbered_types.is_empty());
    }

    /// llvmkit-specific: a `SlotMapping<'ctx>` borrows handles from a single
    /// module. Closest upstream anchor: the same `SlotMapping` field shape in
    /// `SlotMapping.h` — upstream uses raw pointers so cross-module mixing is
    /// caught only by post-hoc lookups.
    #[test]
    fn slot_mapping_records_typed_globals() {
        let m = Module::new("slot_mapping_records_typed_globals");
        let i32_ty = m.i32_type();
        let g = m
            .add_external_global("g", i32_ty.as_type())
            .expect("fresh global");

        let mut mapping: SlotMapping<'_> = SlotMapping::new();
        mapping
            .global_values
            .add(0, GlobalRef::Variable(g))
            .expect("first slot");

        assert_eq!(mapping.global_values.get_next(), 1);
        match mapping.global_values.get(0) {
            Some(GlobalRef::Variable(stored)) => assert_eq!(*stored, g),
            other => panic!("unexpected entry: {other:?}"),
        }
    }
}
