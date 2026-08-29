//! NamedMetadataNode storage. Mirrors `llvm/include/llvm/IR/Metadata.h`'s
//! `NamedMDNode` class. Each node is a named list of [`MetadataId`].
//!
//! The vocabulary follows the metadata currency split (`crate::metadata`):
//!
//! - `NamedMetadataSlot` is the bare list index — crate-internal, carrying
//!   neither a `ModuleId` tag nor a brand;
//! - [`NamedMetadataId<B>`] is the public currency: `Copy + Send + 'static`, a
//!   `(tag, slot)` pair that only ever reaches the list through a module-tag
//!   check;
//! - [`NamedMetadataName`] replaces the raw `String` name, spelling the
//!   well-known upstream set as variants with `Custom` for the open rest.

use core::marker::PhantomData;

use crate::Branded;
use crate::error::{IrError, IrResult};
use crate::metadata::{MetadataId, StoredBrand};
use crate::module::{Invariant, ModuleBrand, ModuleId};

// --------------------------------------------------------------------------
// The list index and the public id
// --------------------------------------------------------------------------

/// Stable index into the module-level named-metadata list.
///
/// Crate-internal on purpose, the named-metadata twin of `MetadataSlot`: a
/// slot is a bare `usize` carrying neither a `ModuleId` tag nor a brand, so it
/// means something only inside the module that minted it. The public currency
/// is [`NamedMetadataId<B>`]; a slot is reached from one only through the
/// tag-checking choke point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NamedMetadataSlot(pub(crate) usize);

/// Storable, module-tagged id for a named metadata node.
///
/// The named-metadata member of the id family: `{ tag: ModuleId, slot:
/// NamedMetadataSlot }`, `Copy`, lifetime-free, and `'static` for every brand,
/// exactly like [`MetadataId`]. It carries **no** cached node content; the
/// node is recovered from the owning module when the id is resolved.
///
/// The `tag` is the process-unique [`ModuleId`] of the owning
/// module, checked before the list is touched, so an id from a foreign module
/// can never mis-resolve against an in-range slot. The `_brand` phantom is
/// always `Invariant<B>` (`PhantomData<fn(B) -> B>`): `Send`-neutral and
/// invariant in `B`, exactly like [`MetadataId`] and the value ids, so two
/// distinct named brands are two distinct id types.
///
/// Mint one with
/// [`Module::get_or_insert_named_metadata`](crate::Module::get_or_insert_named_metadata),
/// look an existing node up with
/// [`Module::named_metadata`](crate::Module::named_metadata), and read the
/// node back with
/// [`Module::named_metadata_get`](crate::Module::named_metadata_get).
pub struct NamedMetadataId<B: ModuleBrand> {
    tag: ModuleId,
    slot: NamedMetadataSlot,
    _brand: Invariant<B>,
}

impl<B: ModuleBrand> NamedMetadataId<B> {
    /// Crate-internal: mint an id from an already-resolved tag + slot. The
    /// only caller is the module-level named-metadata surface, which passes
    /// its own [`ModuleId`] and the slot the list just handed
    /// back.
    #[inline]
    pub(crate) fn from_raw(tag: ModuleId, slot: NamedMetadataSlot) -> Self {
        Self {
            tag,
            slot,
            _brand: PhantomData,
        }
    }

    /// **The named-metadata currency's tag check.** Convert a caller-supplied
    /// id into the storage form, rejecting one minted by a different module.
    ///
    /// This is the only route from a caller's [`NamedMetadataId<B>`] to a
    /// `NamedMetadataSlot`: `slot()` exists solely on
    /// `NamedMetadataId<StoredBrand>`, which can only be produced here or by
    /// `from_stored` on an id the module already owns. So the check cannot be
    /// forgotten one level up — a call site that wants the slot must first
    /// name this function and handle its `Err`.
    #[inline]
    pub(crate) fn into_stored(self, owner: ModuleId) -> IrResult<NamedMetadataId<StoredBrand>> {
        if self.tag != owner {
            return Err(IrError::ForeignNamedMetadataId);
        }
        Ok(NamedMetadataId::from_raw(self.tag, self.slot))
    }

    /// Crate-internal: retag an id the module already owns back into the
    /// caller's brand. Infallible by construction — a stored id was minted by
    /// the module that stores it, so its tag already matches.
    #[inline]
    pub(crate) fn from_stored(stored: NamedMetadataId<StoredBrand>) -> Self {
        Self::from_raw(stored.tag, stored.slot)
    }
}

impl NamedMetadataId<StoredBrand> {
    /// Crate-internal: the list slot this **stored** id names.
    ///
    /// Defined only for the storage brand, which is the whole point: a stored
    /// id is native to the module that holds it, so no tag check is owed. A
    /// caller-supplied `NamedMetadataId<B>` has no such accessor and must go
    /// through [`into_stored`](NamedMetadataId::into_stored) instead.
    #[inline]
    pub(crate) fn slot(self) -> NamedMetadataSlot {
        self.slot
    }
}

// Hand-written rather than derived so `Debug` prints `tag`/`slot` only, never
// the brand phantom — the same reason `MetadataId` and `decl_value_id!` write
// these out.
impl<B: ModuleBrand> Clone for NamedMetadataId<B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<B: ModuleBrand> Copy for NamedMetadataId<B> {}
impl<B: ModuleBrand> PartialEq for NamedMetadataId<B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag && self.slot == other.slot
    }
}
impl<B: ModuleBrand> Eq for NamedMetadataId<B> {}
impl<B: ModuleBrand> core::hash::Hash for NamedMetadataId<B> {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        self.slot.hash(state);
    }
}
impl<B: ModuleBrand> core::fmt::Debug for NamedMetadataId<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NamedMetadataId")
            .field("tag", &self.tag)
            .field("slot", &self.slot)
            .finish()
    }
}

// --------------------------------------------------------------------------
// The name vocabulary
// --------------------------------------------------------------------------

/// The name of a named metadata node (`!name = !{...}`), with the well-known
/// LLVM spellings as variants.
///
/// The fixed set is collected from the names upstream code reads and writes
/// through `Module::getNamedMetadata` / `Module::getOrInsertNamedMetadata`
/// (`llvm/include/llvm/IR/Module.h`): the module-level names the verifier
/// knows (`Verifier::verifyModule` and its `visitModuleFlags` /
/// `visitModuleIdents` / `visitModuleCommandLines` / `verifyErrnoTBAA` arms in
/// `lib/IR/Verifier.cpp`) plus the producer-side names emitted by the
/// debug-info, offloading, instrumentation, and LTO layers. Any other spelling
/// is valid IR and stays [`Custom`](Self::Custom).
///
/// `llvm.used`, `llvm.compiler.used`, `llvm.global_ctors`, and
/// `llvm.global_dtors` are deliberately absent: upstream models those as
/// global **variables** with appending linkage, not named metadata, so they
/// are not members of this namespace.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NamedMetadataName {
    /// `llvm.module.flags`
    ModuleFlags,
    /// `llvm.dbg.cu`
    DbgCu,
    /// `llvm.ident`
    Ident,
    /// `llvm.linker.options`
    LinkerOptions,
    /// `llvm.commandline`
    Commandline,
    /// `llvm.dependent-libraries`
    DependentLibraries,
    /// `llvm.embedded.objects`
    EmbeddedObjects,
    /// `llvm.errno.tbaa`
    ErrnoTbaa,
    /// `llvm.stats`
    Stats,
    /// `llvm.gcov`
    Gcov,
    /// `llvm.debugify`
    Debugify,
    /// `llvm.mir.debugify`
    MirDebugify,
    /// `llvm.offloading.symbols`
    OffloadingSymbols,
    /// `llvm.printf.fmts`
    PrintfFmts,
    /// `llvm.tysan.globals`
    TysanGlobals,
    /// `cfi.functions`
    CfiFunctions,
    /// `aliases`
    Aliases,
    /// `symvers`
    Symvers,
    /// Any other spelling — the namespace is open, so an unknown `!name` is
    /// valid IR, not an error.
    Custom(String),
}

impl NamedMetadataName {
    /// Classify a textual name: the well-known spellings map to their
    /// variants, anything else to [`Custom`](Self::Custom).
    pub fn from_name(name: &str) -> Self {
        match name {
            "llvm.module.flags" => Self::ModuleFlags,
            "llvm.dbg.cu" => Self::DbgCu,
            "llvm.ident" => Self::Ident,
            "llvm.linker.options" => Self::LinkerOptions,
            "llvm.commandline" => Self::Commandline,
            "llvm.dependent-libraries" => Self::DependentLibraries,
            "llvm.embedded.objects" => Self::EmbeddedObjects,
            "llvm.errno.tbaa" => Self::ErrnoTbaa,
            "llvm.stats" => Self::Stats,
            "llvm.gcov" => Self::Gcov,
            "llvm.debugify" => Self::Debugify,
            "llvm.mir.debugify" => Self::MirDebugify,
            "llvm.offloading.symbols" => Self::OffloadingSymbols,
            "llvm.printf.fmts" => Self::PrintfFmts,
            "llvm.tysan.globals" => Self::TysanGlobals,
            "cfi.functions" => Self::CfiFunctions,
            "aliases" => Self::Aliases,
            "symvers" => Self::Symvers,
            _ => Self::Custom(name.to_owned()),
        }
    }

    /// The textual name as it prints in `.ll` (without the leading `!`).
    pub fn name(&self) -> &str {
        match self {
            Self::ModuleFlags => "llvm.module.flags",
            Self::DbgCu => "llvm.dbg.cu",
            Self::Ident => "llvm.ident",
            Self::LinkerOptions => "llvm.linker.options",
            Self::Commandline => "llvm.commandline",
            Self::DependentLibraries => "llvm.dependent-libraries",
            Self::EmbeddedObjects => "llvm.embedded.objects",
            Self::ErrnoTbaa => "llvm.errno.tbaa",
            Self::Stats => "llvm.stats",
            Self::Gcov => "llvm.gcov",
            Self::Debugify => "llvm.debugify",
            Self::MirDebugify => "llvm.mir.debugify",
            Self::OffloadingSymbols => "llvm.offloading.symbols",
            Self::PrintfFmts => "llvm.printf.fmts",
            Self::TysanGlobals => "llvm.tysan.globals",
            Self::CfiFunctions => "cfi.functions",
            Self::Aliases => "aliases",
            Self::Symvers => "symvers",
            Self::Custom(name) => name,
        }
    }
}

impl From<&str> for NamedMetadataName {
    fn from(name: &str) -> Self {
        Self::from_name(name)
    }
}

impl From<String> for NamedMetadataName {
    fn from(name: String) -> Self {
        match Self::from_name(&name) {
            // Reuse the caller's allocation for a custom spelling.
            Self::Custom(_) => Self::Custom(name),
            fixed => fixed,
        }
    }
}

// --------------------------------------------------------------------------
// The node
// --------------------------------------------------------------------------

/// A named metadata node. Mirrors `NamedMDNode` in `Metadata.h`.
///
/// Brand-generic because its operands are the tagged metadata currency: a
/// module stores its own nodes under the crate-private storage brand, and
/// `Module::named_metadata_add_operand` tag-checks a caller's
/// [`MetadataId<B>`] before it lands here.
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct NamedMetadataNode<B: ModuleBrand> {
    name: NamedMetadataName,
    operands: Vec<MetadataId<B>>,
}

impl<B: ModuleBrand> NamedMetadataNode<B> {
    /// Construct an empty named metadata node with the given name.
    pub fn new<Name>(name: Name) -> Self
    where
        Name: Into<NamedMetadataName>,
    {
        Self {
            name: name.into(),
            operands: Vec::new(),
        }
    }

    /// The name of this node.
    pub fn name(&self) -> &NamedMetadataName {
        &self.name
    }

    /// The bare textual name (without leading `!`), as printed.
    pub fn name_str(&self) -> &str {
        self.name.name()
    }

    /// Append an operand.
    pub fn add_operand(&mut self, op: MetadataId<B>) {
        self.operands.push(op);
    }

    /// Drop every operand, keeping the node (and its name) in place. Mirrors
    /// `NamedMDNode::clearOperands` (`llvm/include/llvm/IR/Metadata.h`).
    ///
    /// Upstream is `getNMDOps(Operands).clear()` over a
    /// `SmallVector<TrackingMDRef>`, so clearing also drops each operand's
    /// tracking reference. llvmkit's operands are arena ids that track
    /// nothing, so the `Vec` truncation is the whole of it.
    pub fn clear_operands(&mut self) {
        self.operands.clear();
    }

    /// All operands in insertion order.
    pub fn operands(&self) -> &[MetadataId<B>] {
        &self.operands
    }

    /// Number of operands.
    pub fn operand_count(&self) -> usize {
        self.operands.len()
    }

    /// Crate-internal: retag stored node content back into the caller's brand
    /// — the clone-out half of
    /// [`Module::named_metadata_get`](crate::Module::named_metadata_get).
    /// Mirrors `MetadataKind::from_stored`.
    pub(crate) fn from_stored(stored: &NamedMetadataNode<StoredBrand>) -> Self {
        Self {
            name: stored.name.clone(),
            operands: stored
                .operands
                .iter()
                .map(|id| MetadataId::from_stored(*id))
                .collect(),
        }
    }
}
