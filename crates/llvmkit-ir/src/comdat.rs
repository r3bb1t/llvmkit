//! COMDAT support. Mirrors `llvm/include/llvm/IR/Comdat.h` and the
//! comdat slice of `llvm/lib/IR/AsmWriter.cpp::Comdat::print`.
//!
//! A [`ComdatRef`] is a name + selection-kind pair attached to one or
//! more globals (variables or functions). Two globals may share a
//! comdat name only if they share the selection kind; that invariant
//! is enforced by the per-module storage in
//! [`crate::module::Module::get_or_insert_comdat`] returning the
//! existing entry on second lookup.
//!
//! ## Storage model
//!
//! Comdats are owned by the [`Module`] and addressed
//! by name. A [`ComdatRef<'ctx>`] borrows the comdat for the lifetime
//! of the module. Globals store the comdat by name (`Option<String>`)
//! to avoid arena cross-references.

use super::capability::{CanMutate, Capability, CapabilityOf, Mutable, ReadOnly};
use super::module::{Module, ModuleBrand, ModuleId, ModuleRef, Unverified};
use crate::Branded;
use crate::error::{IrError, IrResult};
use core::fmt;
use core::str::FromStr;

/// Comdat arena index. Stable for the lifetime of the owning
/// module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComdatId(pub(crate) u32);

impl ComdatId {
    #[inline]
    pub(crate) fn from_index(index: usize) -> Self {
        let v =
            u32::try_from(index).unwrap_or_else(|_| unreachable!("comdat index exceeds u32::MAX"));
        Self(v)
    }

    #[inline]
    pub(crate) fn arena_index(self) -> usize {
        usize::try_from(self.0)
            .unwrap_or_else(|_| unreachable!("u32 always fits in usize on supported targets"))
    }
}

/// COMDAT selection kind. Mirrors `Comdat::SelectionKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SelectionKind {
    /// `any` -- the linker may choose any COMDAT.
    #[default]
    Any,
    /// `exactmatch` -- the data referenced by the COMDAT must be the same.
    ExactMatch,
    /// `largest` -- the linker will choose the largest COMDAT.
    Largest,
    /// `nodeduplicate` -- no deduplication is performed.
    NoDeduplicate,
    /// `samesize` -- the data referenced by the COMDAT must be the same size.
    SameSize,
}

impl SelectionKind {
    /// Every variant, in declaration order. Exists so [`FromStr`] can invert
    /// [`keyword`](Self::keyword) by searching this list instead of carrying
    /// a second copy of the spelling table; keep it in step with the enum
    /// (the in-file drift-lock test's exhaustive `match` is the tripwire).
    pub const VARIANTS: [Self; 5] = [
        Self::Any,
        Self::ExactMatch,
        Self::Largest,
        Self::NoDeduplicate,
        Self::SameSize,
    ];

    /// `.ll` keyword for this selection kind. Mirrors
    /// `lib/IR/AsmWriter.cpp::Comdat::print`.
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::ExactMatch => "exactmatch",
            Self::Largest => "largest",
            Self::NoDeduplicate => "nodeduplicate",
            Self::SameSize => "samesize",
        }
    }
}

impl fmt::Display for SelectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

impl FromStr for SelectionKind {
    type Err = IrError;

    /// Inverse of [`Display`](fmt::Display) / [`SelectionKind::keyword`] over
    /// [`VARIANTS`](Self::VARIANTS) — the spellings live only in `keyword`,
    /// so the two directions cannot drift. Mirrors `LLParser::parseComdat`'s
    /// selection-kind keyword block (`lib/AsmParser/LLParser.cpp`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .into_iter()
            .find(|kind| kind.keyword() == s)
            .ok_or_else(|| IrError::InvalidKeyword {
                target: "comdat selection kind",
                keyword: s.to_string(),
            })
    }
}

/// Per-module COMDAT entry. Mirrors `class Comdat` in `IR/Comdat.h`.
///
/// Stored inside the module by name. Use
/// [`Module::get_or_insert_comdat`](crate::Module::get_or_insert_comdat)
/// to materialise one and obtain a [`ComdatRef`].
#[derive(Debug)]
pub struct ComdatData {
    pub(crate) name: String,
    pub(crate) selection_kind: core::cell::Cell<SelectionKind>,
}

impl ComdatData {
    pub(crate) fn new(name: String, kind: SelectionKind) -> Self {
        Self {
            name,
            selection_kind: core::cell::Cell::new(kind),
        }
    }
}

/// Borrowed handle for a [`ComdatData`]. Mirrors how upstream LLVM
/// passes `Comdat *` around: cheap, copy-able. Identity is
/// (module, ComdatId).
///
/// `C` is the handle's [`Capability`] (D8): a comdat looked up through an
/// unverified module is [`Mutable`]; one looked up through a verified module
/// is [`ReadOnly`], and its selection kind cannot be changed.
#[derive(Branded)]
#[branded(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComdatRef<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    pub(crate) module: ModuleRef<'ctx, B, C>,
    pub(crate) id: ComdatId,
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for ComdatRef<'_, B, C> {
    type Capability = C;
}

impl<'ctx, B: ModuleBrand, C: Capability> ComdatRef<'ctx, B, C> {
    #[inline]
    pub(crate) fn data(self) -> &'ctx ComdatData {
        self.module.module().comdat_at(self.id)
    }

    /// Comdat name (without the leading `$`).
    #[inline]
    pub fn name(self) -> &'ctx str {
        &self.data().name
    }

    /// This comdat at [`ReadOnly`]. Always sound — reading is a subset of
    /// mutating.
    #[inline]
    pub fn read_only(self) -> ComdatRef<'ctx, B, ReadOnly> {
        ComdatRef {
            module: self.module.read_only(),
            id: self.id,
        }
    }

    /// Crate-internal: the checked door. This comdat's name, if `owner`
    /// minted it; [`IrError::ForeignComdat`] otherwise. A global stores its
    /// comdat by name, so a name taken from another module's table would
    /// name nothing in `owner`'s, or a different comdat — the boundary takes
    /// this door before the name is stored.
    #[inline]
    pub(crate) fn name_in(self, owner: ModuleId) -> IrResult<&'ctx str> {
        if self.module.id() == owner {
            Ok(self.name())
        } else {
            Err(IrError::ForeignComdat)
        }
    }

    // No public `id()`. `ComdatId` is a bare `u32` index carrying neither a
    // `ModuleId` tag nor a brand, so it is not a member of the 2.0 id family
    // and `Module::view` cannot resolve it — which is exactly why
    // `Module::comdat` returns this handle rather than an id. Handing the
    // raw index out anyway would have published a token no public API accepts,
    // strictly weaker than the handle it came from. It is crate-internal
    // storage, reachable through `ComdatRef` and nothing else.

    /// Selection kind currently stored under this comdat.
    pub fn selection_kind(self) -> SelectionKind {
        self.data().selection_kind.get()
    }

    /// Update the selection kind. Mirrors `Comdat::setSelectionKind`.
    ///
    /// Takes the `Unverified` module token, like every other mutator in the
    /// crate, and needs a handle that [`CanMutate`]. The selection kind is
    /// printed (`$name = comdat <kind>`), and `Module::comdat` is
    /// state-generic, so a verified module does hand out a `ComdatRef` — a
    /// [`ReadOnly`] one, which has no setter.
    pub fn set_selection_kind(self, _module_token: &Module<B, Unverified>, kind: SelectionKind)
    where
        C: CanMutate,
    {
        self.data().selection_kind.set(kind);
    }
}

impl<B: ModuleBrand, C: Capability> fmt::Debug for ComdatRef<'_, B, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComdatRef")
            .field("name", &self.name())
            .finish()
    }
}

/// Upstream provenance: mirrors `Comdat::SelectionKind` in
/// `include/llvm/IR/Comdat.h`; the spellings are `Comdat::print`'s
/// (`lib/IR/AsmWriter.cpp`) and `LLParser::parseComdat`'s.
///
/// The test below is **llvmkit-specific**: it is the `Display`/`FromStr`
/// drift lock, the analogue of `attribute_td_drift.rs`. Upstream's printer
/// and parser cannot drift apart because both switch on the same enum, so
/// there is nothing to port.
#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific: Display/FromStr drift lock for [`SelectionKind`].
    /// The exhaustive `match` makes a new variant a compile error here, which
    /// is the prompt to extend `VARIANTS`; the count assertion then fails
    /// until it is extended.
    #[test]
    fn selection_kind_display_and_from_str_round_trip() {
        for kind in SelectionKind::VARIANTS {
            match kind {
                SelectionKind::Any
                | SelectionKind::ExactMatch
                | SelectionKind::Largest
                | SelectionKind::NoDeduplicate
                | SelectionKind::SameSize => {}
            }
            assert_eq!(kind.to_string().parse::<SelectionKind>(), Ok(kind));
        }
        // One entry per arm above.
        assert_eq!(SelectionKind::VARIANTS.len(), 5);
        assert_eq!(
            "noduplicates".parse::<SelectionKind>(),
            Err(IrError::InvalidKeyword {
                target: "comdat selection kind",
                keyword: "noduplicates".to_string(),
            })
        );
    }
}
