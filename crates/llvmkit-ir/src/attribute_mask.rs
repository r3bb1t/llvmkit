//! Removal mask for attributes. Mirrors
//! `llvm/include/llvm/IR/AttributeMask.h`.
//!
//! `AttributeMask` describes "the set of attributes I want to strip
//! from an `AttributeSet` / `AttributeList`". The upstream uses
//! `std::bitset<EndAttrKinds>` for enum kinds; this port uses a
//! `HashSet<AttrKind>` for symmetry with the rest of the crate (the
//! kind set is small enough that the constant-factor cost vanishes).

use std::collections::BTreeSet;
use std::collections::HashSet;

use crate::attributes::{AttrKind, Attribute, AttributeSet};

#[derive(Debug, Default, Clone)]
pub struct AttributeMask {
    enum_kinds: HashSet<AttrKind>,
    target_dep_attrs: BTreeSet<String>,
}

impl AttributeMask {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an enum / int / type attribute kind to the mask. String
    /// attributes use [`Self::add_string`].
    pub fn add_kind(&mut self, kind: AttrKind) -> &mut Self {
        self.enum_kinds.insert(kind);
        self
    }

    /// Add a target-dependent string attribute key to the mask.
    pub fn add_string(&mut self, key: impl Into<String>) -> &mut Self {
        self.target_dep_attrs.insert(key.into());
        self
    }

    /// Add every attribute in `set` to the mask. Mirrors the
    /// `AttributeMask(AttributeSet)` constructor in C++.
    pub fn add_set(&mut self, set: &AttributeSet<'_>) -> &mut Self {
        for a in set.iter() {
            match a {
                Attribute::Enum(k) | Attribute::Int(k, _) | Attribute::Type(k, _) => {
                    self.enum_kinds.insert(*k);
                }
                Attribute::String { key, .. } => {
                    self.target_dep_attrs.insert(key.clone());
                }
            }
        }
        self
    }

    /// `true` if `kind` is present in the mask.
    pub fn contains_kind(&self, kind: AttrKind) -> bool {
        self.enum_kinds.contains(&kind)
    }

    /// `true` if a target-dependent attribute with `key` is in the mask.
    pub fn contains_string(&self, key: &str) -> bool {
        self.target_dep_attrs.contains(key)
    }

    /// `true` if `attr` is covered by this mask.
    pub fn contains(&self, attr: &Attribute<'_>) -> bool {
        match attr {
            Attribute::Enum(k) | Attribute::Int(k, _) | Attribute::Type(k, _) => {
                self.contains_kind(*k)
            }
            Attribute::String { key, .. } => self.contains_string(key),
        }
    }
}

/// Upstream provenance: mirrors `class AttributeMask` from
/// `llvm/include/llvm/IR/AttributeMask.h` and `lib/IR/Attributes.cpp`,
/// exercised at runtime by `unittests/IR/AttributesTest.cpp`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::attributes::AttrKind;

    /// Mirrors `AttributeMask::addAttribute(Attribute::AttrKind)` /
    /// `contains` in `include/llvm/IR/AttributeMask.h`.
    #[test]
    fn add_and_query_kinds() {
        let mut m = AttributeMask::new();
        m.add_kind(AttrKind::NoReturn).add_kind(AttrKind::Alignment);
        assert!(m.contains_kind(AttrKind::NoReturn));
        assert!(m.contains_kind(AttrKind::Alignment));
        assert!(!m.contains_kind(AttrKind::AlwaysInline));
    }

    /// Mirrors `AttributeMask::addAttributes(AttributeSet)` in
    /// `include/llvm/IR/AttributeMask.h` (collects both enum and string
    /// attributes from a set).
    #[test]
    fn add_set_collects_kinds_and_strings() {
        let mut s = AttributeSet::<'_>::new();
        s.add(Attribute::Enum(AttrKind::NoReturn));
        s.add(Attribute::Int(AttrKind::Alignment, 8));
        s.add(Attribute::string("target-features", "+sse2"));
        let mut m = AttributeMask::new();
        m.add_set(&s);
        assert!(m.contains_kind(AttrKind::NoReturn));
        assert!(m.contains_kind(AttrKind::Alignment));
        assert!(m.contains_string("target-features"));
    }

    /// Mirrors `AttributeMask::contains(Attribute)` polymorphic dispatch
    /// in `include/llvm/IR/AttributeMask.h`.
    #[test]
    fn contains_dispatches_by_attr_shape() {
        let mut m = AttributeMask::new();
        m.add_kind(AttrKind::NoReturn);
        m.add_string("frame-pointer");
        assert!(m.contains(&Attribute::<'_>::Enum(AttrKind::NoReturn)));
        assert!(!m.contains(&Attribute::<'_>::Enum(AttrKind::Cold)));
        assert!(m.contains(&Attribute::<'_>::string("frame-pointer", "all")));
    }
}
