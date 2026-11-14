//! The permission to change a module's layout, separated from the module's
//! data.
//!
//! llvmkit stores IR behind interior mutability and mutates through
//! `&Module`, so on its own no borrow ends when an instruction is erased or a
//! block is terminated. A builder that remembers where it inserts, or a
//! witness that remembers where an instruction was, could then be invalidated
//! behind its back. A [`LayoutToken`] is the one permission for those edits.
//! A builder and every edit that can invalidate a position borrow it `&mut`,
//! so the borrow checker admits one editor at a time. This is the token half
//! of GhostCell's design (Yanovski, Dang, Jung, Dreyer, *GhostCell: Separating
//! Permissions from Data in Rust*, 2021). The cell half needs `unsafe`, and
//! llvmkit's storage is already safe, so this token is pure permission.
//!
//! Design and decisions: `docs/design/layout-token-design.md`.
//!
//! No upstream counterpart: LLVM's `IRBuilder` and `Instruction` mutators
//! need no permission, and two `IRBuilder`s over one block are legal there.

use crate::module::{Module, ModuleBrand, ModuleId, Unverified};
use crate::{IrError, IrResult};

/// The permission to change one module's layout: which instructions are in
/// which block, and in what order.
///
/// Minted by [`Module::layout_token`], which borrows the module `&mut`, so at
/// most one token over a module exists at a time — the borrow checker's
/// guarantee, with no run-time flag. The module is read through the token
/// ([`module`](Self::module)) while it lives. Not `Copy`, not `Clone`.
///
/// A builder holds it `&mut` for as long as the builder lives, so while a
/// builder is positioned, nothing else can terminate its block, erase its
/// anchor, or split either away.
pub struct LayoutToken<'m, B: ModuleBrand> {
    module: &'m Module<B, Unverified>,
}

impl<'m, B: ModuleBrand> LayoutToken<'m, B> {
    /// Crate-internal: the token over `module`. Only
    /// [`Module::layout_token`], which holds the module `&mut`, may call it.
    pub(crate) fn over(module: &'m Module<B, Unverified>) -> Self {
        Self { module }
    }

    /// The module this token is the permission for. The returned reference
    /// lives as long as the token's borrow of the module, not as long as
    /// this call's borrow of the token, so it stays usable while a builder
    /// holds the token `&mut`.
    #[inline]
    pub fn module(&self) -> &'m Module<B, Unverified> {
        self.module
    }

    /// Crate-internal: refuse this token for an edit of a handle of another
    /// module ([`IrError::ForeignValueId`], the error every entry gives a
    /// handle of another module than the one it works in). Two modules of a
    /// shared brand (every `Module::dynamic` is `DynBrand`) have tokens and
    /// handles of one type, so the brand cannot tell them apart; the module
    /// tag does (D7).
    pub(crate) fn admit(&self, owner: ModuleId) -> IrResult<()> {
        if self.module.core_ref().id() == owner {
            Ok(())
        } else {
            Err(IrError::ForeignValueId)
        }
    }
}

impl<B: ModuleBrand> core::fmt::Debug for LayoutToken<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LayoutToken")
            .field("module", &self.module.core_ref().id())
            .finish()
    }
}
