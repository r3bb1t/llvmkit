//! Generic [`Value`] handle plus per-kind refinements. Mirrors
//! `llvm/include/llvm/IR/Value.h` and `llvm/lib/IR/Value.cpp`.
//!
//! ## Representation
//!
//! Storage is index-based, just like [`Type`]: every interned value
//! lives in the owning module's value arena, identified by a
//! crate-internal arena id. The payload is a lifetime-free record so
//! the `boxcar::Vec` backing the arena and the Hash/Eq derives both
//! stay simple.
//!
//! Per the IR foundation plan (Pivot 1, "dual-view"):
//!
//! - **Storage:** an internal record — one variant per LLVM value
//!   category (`Constant`, `Argument`, `BasicBlock`, `Function`,
//!   `Instruction`).
//! - **Public handle:** [`Value<'ctx, B>`] is `(ValueSlot, ModuleRef<'ctx, B>,
//!   ty: TypeSlot)`. `ty` is cached so `value.ty()` is a thin wrapper
//!   instead of an arena round-trip — the type of a value is an
//!   immutable property by construction.
//! - **Per-kind handles:** [`IntValue`], [`FloatValue`],
//!   [`PointerValue`], etc. carry the same triple but with the
//!   additional invariant that the wrapped value's *type* belongs to
//!   the matching kind. Bound generic code with the sealed
//!   [`IsValue`] / [`Typed`] / [`HasName`] / [`HasDebugLoc`] traits.

use crate::Branded;
use core::cell::RefCell;
use core::iter::FusedIterator;
use core::num::NonZeroUsize;

use super::argument::Argument;
use super::basic_block::BasicBlockData;
use super::capability::{CanMutate, Capability, CapabilityOf, Mutable};
use super::constant::{Constant, ConstantData};
use super::constants::ConstantPointerNull;
use super::debug_loc::DebugLoc;
use super::derived_types::{
    ArrayType, FloatType, FunctionType, IntType, PointerType, StructType, VectorType,
};
use super::error::{InvalidValueNameReason, IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use super::function::FunctionData;
use super::instruction::{Instruction, InstructionData, InstructionView, state::Attached};
use super::module::{Module, ModuleBrand, ModuleId, ModuleRef, ModuleView, Unverified};
use super::struct_body_state::StructBodyDyn;
use super::r#type::{Type, TypeData, TypeSlot, TypeSlotAccess};
use super::value_id::{FloatValueId, IntValueId, PointerValueId, ValueId};
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use super::ap_int::ApInt;
use super::array_len::{ArrLen, ArrLenDyn, ArrayLen};
use super::constants::ConstantIntValue;
use super::element::{ElemDyn, StaticVecElem, VecElem};
use super::float_kind::{Bfloat, FloatDyn, FloatKind, Fp128, Half, PpcFp128, X86Fp80};
use super::function::FunctionValue;
use super::global_alias::GlobalAliasData;
use super::global_ifunc::GlobalIfuncData;
use super::global_variable::GlobalVariableData;
use super::inline_asm::InlineAsmData;
use super::int_width::{IntDyn, IntWidth, Width};
use super::marker::Dyn;
use super::metadata::MetadataSlot;
use super::vec_len::{Len, LenDyn, VecLen};

// --------------------------------------------------------------------------
// ValueSlot
// --------------------------------------------------------------------------

/// Stable index into the value arena — crate-private, like `TypeSlot` and
/// `MetadataSlot`. A slot carries no module tag and means something only in
/// the module that minted it, so none leaves the crate: the public currency is
/// the tagged id family ([`ValueId`](crate::ValueId), [`BlockId`](crate::BlockId),
/// …) and the handles. Inside the crate a slot leaves a handle or an id only
/// through the checked door `slot_in` or the unchecked
/// `slot_trusting_same_module` ([`ValueSlotAccess`]).
///
/// Ordered by arena position, so within one module the order is creation
/// order. Opaque numerically, but a total order is what lets an id key a
/// `BTreeMap` and give a pass deterministic iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ValueSlot(NonZeroUsize);

mod sealed_value_slot {
    /// A [`ValueSlot`](super::ValueSlot) as it appears in the signature of a
    /// public trait's hidden or sealed method — `ViewIn::id_from_raw` and the
    /// dominator seal's block slot — which a bound on the public trait makes
    /// reachable from outside the crate. The module that declares it is
    /// private and its field is crate-private, so code outside llvmkit can
    /// neither build one, and so call a method that takes one, nor read the
    /// slot inside one it is handed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SealedValueSlot(pub(crate) super::ValueSlot);
}

pub(crate) use sealed_value_slot::SealedValueSlot;

impl ValueSlot {
    /// Build from a 0-based arena index.
    #[inline]
    pub(super) fn from_index(index: usize) -> Self {
        // `index + 1 == 0` requires `index == usize::MAX`, which would
        // mean we've allocated `usize::MAX` values — physically
        // impossible on any addressable target.
        let raw = index.wrapping_add(1);
        match NonZeroUsize::new(raw) {
            Some(nz) => Self(nz),
            None => unreachable!(
                "ValueSlot arena exhausted: usize::MAX values allocated, exceeds addressable memory"
            ),
        }
    }

    /// Recover the 0-based arena index.
    #[inline]
    pub(super) fn arena_index(self) -> usize {
        // `self.0` was always produced from `index + 1`, so the
        // subtraction never wraps for ids built via `from_index`.
        self.0.get() - 1
    }
}

// --------------------------------------------------------------------------
// Internal payload
// --------------------------------------------------------------------------

/// Internal payload for a single interned value.
///
/// Mirrors the closed enum in `Value::ValueTy` (`Value.h`). The discriminator
/// is an enum rather than a packed tag so each variant carries the data its
/// kind needs without hung-off operands.
#[derive(Debug)]
pub(super) struct ValueData {
    pub(super) ty: TypeSlot,
    pub(super) name: RefCell<Option<String>>,
    pub(super) debug_loc: Option<DebugLoc>,
    pub(super) kind: ValueKindData,
    /// Reverse-direction use-list: every structural user that references this
    /// value. Mirrors LLVM's `Value::use_list_` (`Value.h`) while keeping
    /// non-`User` edges explicit: metadata and debug records are not ordinary
    /// instructions, but they still keep values alive and must be updated by
    /// RAUW / erase.
    ///
    /// Entries may appear more than once if the same user references this value
    /// in multiple slots (e.g. `add %x, %x`).
    ///
    /// **Order is newest-first**, mirroring upstream exactly — see
    /// [`ValueData::add_use`]. The order is observable: it is what a
    /// `uselistorder` index vector permutes and what
    /// `AsmWriter::predictValueUseListOrder` states its shuffle in terms of.
    pub(super) use_list: RefCell<Vec<ValueUse>>,
}

impl ValueData {
    /// Register `edge` as a new use of this value.
    ///
    /// Mirrors `Value::addUse` (`Value.h`), which forwards to
    /// `Use::addToList` (`Use.h`): upstream threads its use list through the
    /// uses themselves and a new one becomes the **head**, so the list reads
    /// newest-first. llvmkit stores the same sequence rather than appending,
    /// because the sequence is contractual — `LLParser::sortUseListOrder`
    /// keys each use by its position in it, and
    /// `AsmWriter::predictValueUseListOrder` emits a shuffle against it.
    pub(super) fn add_use(&self, edge: ValueUse) {
        self.use_list.borrow_mut().insert(0, edge);
    }

    /// Move `edges` — a head-first snapshot of another value's use list —
    /// onto the head of this one's.
    ///
    /// Mirrors the drain loop in `Value::replaceAllUsesWith`
    /// (`lib/IR/Value.cpp`), which repeatedly takes the *head* of the old
    /// list and `Use::set`s it, prepending each in turn to the new list. The
    /// net effect reverses the moved run — which is exactly the reversal
    /// `AsmWriter::predictValueUseListOrder` compensates for with its
    /// `GetsReversed` flag, so reproducing it here is what makes a
    /// forward-referenced value's printed shuffle match upstream's.
    /// The use list as upstream's `Value::uses()` reads it: the edges that
    /// correspond to a `Use`, in use-list order, named by their *user*.
    ///
    /// This — not [`Value::num_uses`] — is what `getNumUses`, a
    /// `uselistorder` index vector, `sortUseListOrder` and
    /// `predictValueUseListOrder` all count. See
    /// [`ValueUse::is_operand_use`] for why the two differ.
    pub(super) fn operand_users(&self) -> Vec<ValueSlot> {
        self.use_list
            .borrow()
            .iter()
            .filter_map(|edge| edge.user())
            .collect()
    }

    pub(super) fn prepend_moved_uses(&self, edges: &[ValueUse]) {
        if edges.is_empty() {
            return;
        }
        let mut list = self.use_list.borrow_mut();
        let mut merged = Vec::with_capacity(edges.len() + list.len());
        merged.extend(edges.iter().rev().copied());
        merged.append(&mut list);
        *list = merged;
    }
}

/// One reverse use-list edge for a value. Instruction operands, constants,
/// metadata nodes, and debug records have different mutation paths, so the
/// edge kind is part of the stored fact rather than inferred from a `ValueSlot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ValueUse {
    Instruction(ValueSlot),
    Constant(ValueSlot),
    Metadata(MetadataSlot),
    DebugRecord {
        inst: ValueSlot,
        record: usize,
    },
    /// A single-slot field of a global object — an initializer, an aliasee, an
    /// ifunc resolver, or a function's personality / prefix / prologue.
    ///
    /// Upstream these are ordinary `Use` edges on a `GlobalValue`, which is a
    /// `User`. llvmkit stores each as its own `Cell<Option<ValueSlot>>` on the
    /// owning data struct, so the edge has to name which cell it is; without
    /// it, RAUW could not find the field and `num_uses` would undercount.
    GlobalField {
        owner: ValueSlot,
        field: GlobalFieldKind,
    },
}

impl ValueUse {
    /// The `User` this edge hangs off, when upstream models the edge as a
    /// `Use` at all.
    ///
    /// llvmkit's use list is deliberately wider than upstream's: a metadata
    /// node or a debug record keeps a value alive and must be reached by
    /// RAUW, so each gets an edge. Upstream tracks those through
    /// `ReplaceableMetadataImpl` instead — a `ValueAsMetadata` creates no
    /// `Use`, which is exactly why `AsmWriter::orderModule` has to reach
    /// *through* `MetadataAsValue` wrappers and debug records to find the
    /// constants hiding behind them. Anything phrased in terms of
    /// `Value::uses()` therefore sees only the narrower set.
    pub(super) fn user(self) -> Option<ValueSlot> {
        match self {
            ValueUse::Instruction(user) | ValueUse::Constant(user) => Some(user),
            ValueUse::GlobalField { owner, .. } => Some(owner),
            ValueUse::Metadata(_) | ValueUse::DebugRecord { .. } => None,
        }
    }

    /// Whether this edge is one upstream models as a `Use`. See
    /// [`Self::user`].
    pub(super) fn is_operand_use(self) -> bool {
        self.user().is_some()
    }
}

/// Why a `uselistorder` index vector could not be applied to a value's use
/// list. One variant per `error(...)` arm of `LLParser::sortUseListOrder`;
/// the `Display` texts are that routine's, byte for byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum UseListOrderError {
    /// The named value is not used anywhere.
    #[error("value has no uses")]
    NoUses,
    /// Exactly one use — there is no order to state.
    #[error("value only has one use")]
    OneUse,
    /// The vector does not name every use exactly once. `expected` is the
    /// value's actual use count, which is what upstream's `Twine` renders.
    #[error("wrong number of indexes, expected {expected}")]
    WrongIndexCount { expected: usize },
}

/// Which single-slot field of a global object a [`ValueUse::GlobalField`]
/// edge points at. One variant per `Cell` that holds a value id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum GlobalFieldKind {
    Initializer,
    Aliasee,
    IfuncResolver,
    PersonalityFn,
    PrefixData,
    PrologueData,
}

/// Discriminator over the closed value-category set.
///
/// Variants are populated as the per-kind layers land. Keeping the enum
/// closed (no `#[non_exhaustive]`) is intentional: every IR pass needs
/// to know it has covered every category. New categories require
/// thinking through every handler.
#[derive(Debug)]
pub(super) enum ValueKindData {
    Constant(ConstantData),
    Argument {
        parent_fn: ValueSlot,
        slot: u32,
    },
    BasicBlock(BasicBlockData),
    Function(Box<FunctionData>),
    Instruction(InstructionData),
    GlobalAlias(GlobalAliasData),
    GlobalIfunc(GlobalIfuncData),
    GlobalVariable(GlobalVariableData),
    /// A metadata node used in a value context. Mirrors LLVM's
    /// `MetadataAsValue` (`llvm/include/llvm/IR/Metadata.h`): it lets a
    /// metadata node (e.g. `!0`) appear where a `Value` is expected,
    /// such as a `call` argument of `metadata` type. Like a constant,
    /// it is context-global — it has no function-local SSA definition
    /// and is never assigned a `%N` slot.
    MetadataAsValue(MetadataSlot),
    /// An inline-assembly value used as a `call` callee. Mirrors LLVM's
    /// `InlineAsm` (`llvm/include/llvm/IR/InlineAsm.h`). Like a
    /// `Function` or `Constant`, it is context-global — it has no
    /// function-local SSA definition and is never assigned a `%N` slot;
    /// a `call` whose callee is one of these prints the `asm ...` form
    /// instead of an `@name` operand.
    InlineAsm(InlineAsmData),
}

// --------------------------------------------------------------------------
// Public erased handle
// --------------------------------------------------------------------------

/// Erased public handle for any IR value.
///
/// Three-field record:
/// - `id: ValueSlot` — arena index.
/// - `module: ModuleRef<'ctx>` — brand carrier; equality routes through
///   the process-global [`ModuleId`].
/// - `ty: TypeSlot` — cached type. Values do not change type, so caching
///   here saves an arena lookup on every `value.ty()` access.
///
/// Equality and hashing compare the branded module reference by `ModuleId`,
/// so the handle remains cheap to copy and store in maps.
///
/// `C` is the [`Capability`] (D8): a value reached from an unverified module is
/// [`Mutable`]; one reached from a verified module, a [`ModuleView`] or a
/// `ReadOnly` type is [`ReadOnly`](crate::ReadOnly), and has no setters.
pub struct Value<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    // Private to this module: the slot leaves a handle only through the two
    // doors of `ValueSlotAccess`, and the cached type only as a `Type` handle.
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B, C>,
    ty: TypeSlot,
}

impl<B: ModuleBrand, C: Capability> Clone for Value<'_, B, C> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: ModuleBrand, C: Capability> Copy for Value<'_, B, C> {}

impl<B: ModuleBrand, C: Capability> PartialEq for Value<'_, B, C> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module
    }
}

impl<B: ModuleBrand, C: Capability> Eq for Value<'_, B, C> {}

impl<B: ModuleBrand, C: Capability> Hash for Value<'_, B, C> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.module.hash(state);
        self.ty.hash(state);
    }
}

impl<B: ModuleBrand, C: Capability> fmt::Debug for Value<'_, B, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Value")
            .field("id", &self.id)
            .field("ty", &self.ty)
            .finish()
    }
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for Value<'_, B, C> {
    type Capability = C;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> Value<'ctx, B, C> {
    /// Construct from raw parts. Crate-internal: only the value-arena
    /// constructors hand these out.
    #[inline]
    pub(super) fn from_parts<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B, C>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
        }
    }

    /// Borrow the underlying payload via the module's value arena.
    #[inline]
    pub(super) fn data(self) -> &'ctx ValueData {
        self.module.value_data(self.id)
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    /// Storable, module-tagged [`ValueId`] for this value (0.0.4).
    ///
    /// The id carries the owning [`ModuleId`] and can be resolved back into a
    /// handle with [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view). A handle hands out no
    /// bare arena slot: the slot means something only in its own module, so a
    /// tagless one could be carried into another.
    #[inline]
    pub fn id(self) -> ValueId<B> {
        ValueId::from_raw(self.module.id(), self.id)
    }

    /// Cached IR type of this value, at this value's capability.
    #[inline]
    pub fn ty(self) -> Type<'ctx, B, C> {
        Type::new(self.ty, self.module)
    }

    /// This value at [`ReadOnly`](crate::ReadOnly). Always sound — reading is
    /// a subset of mutating — and the way to compare a value reached through
    /// a read-only route with one reached from the module: equality is
    /// defined within one capability.
    #[inline]
    pub fn read_only(self) -> Value<'ctx, B, crate::ReadOnly> {
        Value {
            id: self.id,
            module: self.module.read_only(),
            ty: self.ty,
        }
    }

    /// Crate-internal: this value admitted at `module` through the checked
    /// door — [`IrError::ForeignValueId`] unless `module` is this value's own
    /// — and re-minted at `module`'s capability. How an authority lifts an
    /// operand of any capability: the capability comes from a reference the
    /// authority already holds, never from the operand.
    #[inline]
    pub(crate) fn admitted_at<C2: Capability>(
        self,
        module: ModuleRef<'ctx, B, C2>,
    ) -> IrResult<Value<'ctx, B, C2>> {
        let id = self.slot_in(module.id())?;
        Ok(Value {
            id,
            module,
            ty: self.ty,
        })
    }

    /// Optional textual name. `None` for slot-numbered (`%0`, `%1`)
    /// values. The interned `ptr @g` constant answers the name of the global
    /// it stands for, as `getName` on `@g` does (`docs/divergences.md` D3).
    pub fn name(self) -> Option<String> {
        let named = self.global_value_slot().unwrap_or(self.id);
        self.module.value_data(named).name.borrow().clone()
    }

    /// Set the textual name. Mirrors `Value::setName`: a name another value
    /// in the same symbol table holds is uniqued, and an empty name leaves the
    /// value unnamed.
    ///
    /// # Errors
    ///
    /// [`IrError::InvalidValueName`] for a request `Value::setNameImpl`
    /// asserts against, and the value keeps its name: a name containing a NUL
    /// byte, a name for a `void` value, or a name for an inline-asm or
    /// metadata value, which no symbol table holds.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module — reachable with
    /// two modules of one brand, such as two [`Module::dynamic`] values.
    pub fn set_name<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        name: Name,
    ) -> IrResult<()>
    where
        Name: Into<String>,
        C: CanMutate,
    {
        assert_eq!(
            module_token.id(),
            self.module.id(),
            "set_name: the module token belongs to a different module than this value"
        );
        let requested = name.into();
        self.rename(&requested)
    }

    /// Clear the textual name. Mirrors `Value::setName("")`.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module — reachable with
    /// two modules of one brand, such as two [`Module::dynamic`] values.
    pub fn clear_name(self, module_token: &'ctx Module<B, Unverified>)
    where
        C: CanMutate,
    {
        assert_eq!(
            module_token.id(),
            self.module.id(),
            "clear_name: the module token belongs to a different module than this value"
        );
        // `setNameImpl("")`. The fast path returns for an unnamed value. For a
        // named one, the NUL assertion has nothing to find in `""` and the
        // unchanged-name return cannot fire, since the current name is not
        // empty. That leaves the `void` assertion, which only a void value
        // carrying a name reaches — a state no naming path builds:
        // `set_name` refuses one, and the builders leave a void instruction
        // unnamed. A clear assigns no name, so llvmkit skips that assertion
        // rather than refusing, and the value ends up unnamed either way.
        if self.name().is_some() {
            self.update_symbol_table(None);
        }
        self.update_after_name_change();
    }

    /// `Value::setName`: [`Self::admit_rename`] runs `setNameImpl`'s guards,
    /// and [`AdmittedRename::apply`] its update, then
    /// `Function::updateAfterNameChange` on a function — which upstream runs
    /// however `setNameImpl` returned. A refusal changes nothing, so it skips
    /// the tail.
    fn rename(self, requested: &str) -> IrResult<()>
    where
        C: CanMutate,
    {
        self.admit_rename(requested)?.apply();
        Ok(())
    }

    /// `Value::setNameImpl`'s guards, in upstream's order, without its
    /// update: every refusal is made here, and the [`AdmittedRename`] that
    /// comes back performs the rest, which cannot fail. The
    /// `shouldDiscardValueNames` fast path has no counterpart: llvmkit keeps
    /// every name, so `NeedNewName` is always true. Crate-visible for
    /// `getOrInsertIntrinsicDeclarationImpl`'s
    /// `F->setName(F->getName() + ".invalid")`, which must not fail once it
    /// has renamed.
    pub(crate) fn admit_rename(self, requested: &str) -> IrResult<AdmittedRename<'ctx, B, C>>
    where
        C: CanMutate,
    {
        let current = self.name();
        // `if (NewName.isTriviallyEmpty() && !hasName()) return;`
        if requested.is_empty() && current.is_none() {
            return Ok(AdmittedRename {
                value: self,
                step: AdmittedNameStep::Return,
            });
        }
        // `assert(!NameRef.contains(0) && "Null bytes are not allowed in
        // names")`, hardened to a refusal.
        if requested.contains('\0') {
            return Err(IrError::InvalidValueName {
                name: requested.to_owned(),
                reason: InvalidValueNameReason::ContainsNul,
            });
        }
        // `if (getName() == NameRef) return;`
        if current.as_deref().unwrap_or_default() == requested {
            return Ok(AdmittedRename {
                value: self,
                step: AdmittedNameStep::Return,
            });
        }
        // `assert(!getType()->isVoidTy() && "Cannot assign a name to void
        // values!")`, hardened to a refusal.
        if self.ty().is_void() {
            return Err(IrError::InvalidValueName {
                name: requested.to_owned(),
                reason: InvalidValueNameReason::VoidValue,
            });
        }
        // `getSymTab`'s last arm asserts `isa<Constant>(V)`: inline asm and
        // metadata are `Value`s outside every symbol table. Refused, as above.
        match &self.data().kind {
            ValueKindData::InlineAsm(_) => {
                return Err(IrError::InvalidValueName {
                    name: requested.to_owned(),
                    reason: InvalidValueNameReason::InlineAsm,
                });
            }
            ValueKindData::MetadataAsValue(_) => {
                return Err(IrError::InvalidValueName {
                    name: requested.to_owned(),
                    reason: InvalidValueNameReason::MetadataAsValue,
                });
            }
            ValueKindData::Constant(_)
            | ValueKindData::Argument { .. }
            | ValueKindData::BasicBlock(_)
            | ValueKindData::Function(_)
            | ValueKindData::Instruction(_)
            | ValueKindData::GlobalVariable(_)
            | ValueKindData::GlobalAlias(_)
            | ValueKindData::GlobalIfunc(_) => {}
        }
        Ok(AdmittedRename {
            value: self,
            step: AdmittedNameStep::Update(
                Some(requested)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
            ),
        })
    }

    /// The rest of `Value::setNameImpl`, on the table `getSymTab` answers:
    /// take `requested` (or uniquify it), drop the old name, and leave the
    /// value unnamed for [`None`].
    fn update_symbol_table(self, requested: Option<&str>)
    where
        C: CanMutate,
    {
        // `getSymTab`'s `Instruction`, `BasicBlock` and `Argument` arms: the
        // parent function's table.
        if let Some(parent_fn_id) = self.local_parent_function_id() {
            let parent_fn = FunctionValue::<Dyn, B>::from_parts_unchecked(
                parent_fn_id,
                self.module.proven_mutable(),
            );
            parent_fn.set_local_value_name(self.id, requested);
            return;
        }
        // `getSymTab`'s `GlobalValue` arm: the parent module's table. The
        // interned `ptr @g` constant renames the global it stands for, since
        // upstream's `GlobalValue` *is* that constant.
        if let Some(global) = self.global_value_slot() {
            self.module
                .module()
                .set_global_value_name(global, requested);
            return;
        }
        // An instruction or block in no function: `getSymTab` hands back no
        // table, and `setNameImpl`'s `if (!ST)` arm sets the name directly.
        if self.is_parentless_local_nameable() {
            self.set_name_internal(requested.map(str::to_owned));
        }
        // Anything else is a constant: `getSymTab` returns `true` and
        // `setNameImpl` returns ("Cannot set a name on this value (e.g.
        // constant)") — upstream's silent answer, not an assertion.
    }

    /// `Value::setName`'s tail: `if (Function *F = dyn_cast<Function>(this))
    /// F->updateAfterNameChange();`. The interned `ptr @f` constant is the
    /// function here too (`docs/divergences.md` D3).
    fn update_after_name_change(self)
    where
        C: CanMutate,
    {
        if let Some(global) = self.global_value_slot()
            && matches!(
                self.module.value_data(global).kind,
                ValueKindData::Function(_)
            )
        {
            FunctionValue::<Dyn, B>::from_parts_unchecked(global, self.module.proven_mutable())
                .update_after_name_change();
        }
    }

    /// The global value this value is or stands for: a function, global
    /// variable, alias or ifunc itself, or the interned `ptr @g` constant
    /// that names one (`docs/divergences.md` D3 — upstream's `GlobalValue` is
    /// that constant). [`None`] for every other value.
    fn global_value_slot(self) -> Option<ValueSlot> {
        match &self.data().kind {
            ValueKindData::Function(_)
            | ValueKindData::GlobalVariable(_)
            | ValueKindData::GlobalAlias(_)
            | ValueKindData::GlobalIfunc(_) => Some(self.id),
            ValueKindData::Constant(ConstantData::GlobalValueRef { value }) => Some(*value),
            ValueKindData::Constant(_)
            | ValueKindData::Argument { .. }
            | ValueKindData::BasicBlock(_)
            | ValueKindData::Instruction(_)
            | ValueKindData::MetadataAsValue(_)
            | ValueKindData::InlineAsm(_) => None,
        }
    }

    /// Raw assignment for already-uniqued names and parentless fabrication.
    /// Do not use this for attached local values until their owning
    /// `ValueSymbolTable` has returned the final unique name.
    pub(super) fn set_name_internal(self, name: Option<String>) {
        *self.data().name.borrow_mut() = name;
    }

    pub(super) fn local_parent_function_id(self) -> Option<ValueSlot> {
        match &self.data().kind {
            ValueKindData::Argument { parent_fn, .. } => Some(*parent_fn),
            ValueKindData::BasicBlock(data) => *data.parent.borrow(),
            ValueKindData::Instruction(data) => {
                // In no block, so in no function either — as upstream's
                // `Instruction::getFunction` answers through a null
                // `getParent()`.
                let parent_block_id = data.parent.get()?;
                let parent_block = self.module.value_data(parent_block_id);
                match &parent_block.kind {
                    ValueKindData::BasicBlock(block) => {
                        if block.instructions.borrow().contains(&self.id) {
                            *block.parent.borrow()
                        } else {
                            None
                        }
                    }
                    _ => unreachable!("Instruction parent invariant: parent id is a basic block"),
                }
            }
            ValueKindData::Constant(_)
            | ValueKindData::Function(_)
            | ValueKindData::GlobalAlias(_)
            | ValueKindData::GlobalIfunc(_)
            | ValueKindData::GlobalVariable(_)
            | ValueKindData::MetadataAsValue(_)
            | ValueKindData::InlineAsm(_) => None,
        }
    }

    fn is_parentless_local_nameable(self) -> bool {
        matches!(
            &self.data().kind,
            ValueKindData::BasicBlock(_) | ValueKindData::Instruction(_)
        )
    }

    /// Read the optional debug-location attached to this value.
    /// Currently always `None` (debug-location wiring is future work).
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.data().debug_loc
    }

    /// Pattern-match category. Mirrors the role of `Value::getValueID`
    /// in C++: read-only inspection of the closed value-kind set.
    pub fn category(self) -> ValueCategory {
        match &self.data().kind {
            ValueKindData::Constant(_) => ValueCategory::Constant,
            ValueKindData::Argument { .. } => ValueCategory::Argument,
            ValueKindData::BasicBlock(_) => ValueCategory::BasicBlock,
            ValueKindData::Function(_) => ValueCategory::Function,
            ValueKindData::Instruction(_) => ValueCategory::Instruction,
            ValueKindData::GlobalVariable(_) => ValueCategory::GlobalVariable,
            ValueKindData::GlobalAlias(_) => ValueCategory::GlobalAlias,
            ValueKindData::GlobalIfunc(_) => ValueCategory::GlobalIfunc,
            ValueKindData::MetadataAsValue(_) => ValueCategory::MetadataAsValue,
            ValueKindData::InlineAsm(_) => ValueCategory::InlineAsm,
        }
    }

    /// Snapshot the instruction views that use this value, at this value's
    /// capability. Metadata and debug-record uses are tracked structurally and
    /// counted by [`Self::num_uses`], but are intentionally omitted here
    /// because callers of `users()` expect concrete instruction views.
    ///
    /// The list is a snapshot, not a live view: callers may mutate the IR
    /// (erase, RAUW) without invalidating the iterator. Order is the use-list
    /// order — newest-first, as upstream's `Value::uses()` reads — and user
    /// ids may appear more than once if the same instruction references this
    /// value in multiple slots.
    pub fn users(
        self,
    ) -> impl ExactSizeIterator<Item = InstructionView<'ctx, B, C>>
    + DoubleEndedIterator
    + FusedIterator
    + 'ctx {
        let module = self.module;
        let snapshot: Vec<ValueSlot> = self
            .data()
            .use_list
            .borrow()
            .iter()
            .filter_map(|edge| match edge {
                ValueUse::Instruction(id) => Some(*id),
                ValueUse::Constant(_)
                | ValueUse::Metadata(_)
                | ValueUse::DebugRecord { .. }
                | ValueUse::GlobalField { .. } => None,
            })
            .collect();
        snapshot
            .into_iter()
            .map(move |id| InstructionView::<'ctx, B, C>::from_parts(id, module))
    }

    /// `true` when at least one structural user references this value.
    /// Mirrors `Value::hasUses`. Cheaper than [`Self::users`] for the
    /// common "is this dead?" check.
    #[inline]
    pub fn has_uses(self) -> bool {
        !self.data().use_list.borrow().is_empty()
    }

    /// `true` when exactly one use references this value. Mirrors
    /// `Value::hasOneUse` — the gate peephole rewrites use (via
    /// `m_one_use`) to avoid duplicating a shared sub-expression.
    #[inline]
    pub fn has_one_use(self) -> bool {
        self.data().use_list.borrow().len() == 1
    }

    /// Number of currently-registered structural uses. Mirrors `Value::getNumUses`.
    #[inline]
    pub fn num_uses(self) -> usize {
        self.data().use_list.borrow().len()
    }

    /// `true` when this value keeps a use list at all. Mirrors
    /// `Value::hasUseList` (`Value.h`), spelled there as
    /// `!isa<ConstantData>(this)`.
    ///
    /// `ConstantData` is the operand-less corner of upstream's constant
    /// hierarchy — `ConstantInt`, `ConstantFP`, `ConstantPointerNull`,
    /// `ConstantTokenNone`, `ConstantTargetNone`, `UndefValue`,
    /// `PoisonValue`, `ConstantAggregateZero` and `ConstantDataSequential`.
    /// Its members are shared by every module in a context, so upstream
    /// tracks no users for them and a `uselistorder` naming one is a silent
    /// no-op rather than an error.
    ///
    /// llvmkit *does* record uses for these values — one arena per module
    /// makes that affordable and `num_uses` is the better answer for a
    /// caller asking who reads a constant. This predicate is therefore about
    /// upstream's classification, not about whether
    /// [`Self::users`] will yield anything.
    #[inline]
    pub fn has_use_list(self) -> bool {
        !crate::Constant::try_from(self).is_ok_and(crate::Constant::is_constant_data)
    }

    /// Sort this value's use list with `compare`. Mirrors
    /// `Value::sortUseList` (`Value.h`), a stable merge sort over the uses.
    ///
    /// Upstream's comparator takes two `Use`s; llvmkit hands it each use's
    /// **user** instead, which is what upstream's own comparators read —
    /// `LLParser::sortUseListOrder`'s keys off a side map,
    /// `AsmWriter::predictValueUseListOrder`'s off `getUser()`, and
    /// `UseTest`'s off `getUser()->getName()`. The one thing a `Use` offers
    /// that a user does not is `getOperandNo`, which llvmkit's use list
    /// cannot answer at all (`docs/divergences.md` D4).
    ///
    /// Only the edges upstream models as `Use`s take part; metadata and
    /// debug-record edges keep their slots. A value with fewer than two of
    /// them is left alone, as upstream's `!UseList || !UseList->Next` guard
    /// does.
    ///
    /// Reordering a use list changes the printed `uselistorder`, so this is a
    /// mutator: it needs a handle that [`CanMutate`].
    pub fn sort_use_list_by<F>(self, mut compare: F)
    where
        F: FnMut(Value<'ctx, B, C>, Value<'ctx, B, C>) -> core::cmp::Ordering,
        C: CanMutate,
    {
        let module = self.module;
        let user_value = |slot: ValueSlot| {
            let data = module.value_data(slot);
            Value::from_parts(slot, module, data.ty)
        };
        let mut uses = self.data().use_list.borrow_mut();
        let positions: Vec<usize> = uses
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge.is_operand_use())
            .map(|(position, _)| position)
            .collect();
        if positions.len() < 2 {
            return;
        }
        let mut selected: Vec<ValueUse> =
            positions.iter().map(|position| uses[*position]).collect();
        selected.sort_by(|left, right| {
            match (left.user(), right.user()) {
                (Some(left), Some(right)) => compare(user_value(left), user_value(right)),
                // `is_operand_use` filtered the list, so both sides have a
                // user by construction.
                _ => core::cmp::Ordering::Equal,
            }
        });
        for (position, edge) in positions.iter().zip(selected) {
            uses[*position] = edge;
        }
    }

    /// Permute this value's use list so that the use currently sitting at
    /// position `i` moves to index `indexes[i]`. Mirrors
    /// `LLParser::sortUseListOrder`, whose three `error(...)` arms are the
    /// three [`UseListOrderError`] variants.
    ///
    /// A value with no use list (see [`Self::has_use_list`]) is accepted and
    /// left alone, exactly as upstream's leading `if (!V->hasUseList())
    /// return false` does.
    ///
    /// A mutator, like [`Self::sort_use_list_by`]: it needs a handle that
    /// [`CanMutate`].
    pub fn sort_use_list(self, indexes: &[u32]) -> Result<(), UseListOrderError>
    where
        C: CanMutate,
    {
        if !self.has_use_list() {
            return Ok(());
        }
        let mut uses = self.data().use_list.borrow_mut();
        // Only the edges upstream models as `Use`s take part, and the ones
        // that do not keep their slots — see [`ValueUse::is_operand_use`].
        let positions: Vec<usize> = uses
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge.is_operand_use())
            .map(|(position, _)| position)
            .collect();
        if positions.is_empty() {
            return Err(UseListOrderError::NoUses);
        }
        // Upstream walks the use list keying each use by its position, and
        // stops one use *past* what the vector can name — so `walked` is the
        // count its `NumUses` ends on and `keyed` the size its `Order` map
        // ends on. Both comparisons below are its own.
        let keyed = positions.len().min(indexes.len());
        let walked = positions.len().min(indexes.len().saturating_add(1));
        if walked < 2 {
            return Err(UseListOrderError::OneUse);
        }
        if keyed != indexes.len() || walked > indexes.len() {
            return Err(UseListOrderError::WrongIndexCount {
                expected: positions.len(),
            });
        }
        // `Value::sortUseList` is a stable merge sort under
        // `Order.lookup(&L) < Order.lookup(&R)`; `sort_by_key` is stable too,
        // so a duplicated key orders the same way. The parser rejects
        // duplicates upstream of here regardless.
        let mut keyed_uses: Vec<(u32, ValueUse)> = indexes
            .iter()
            .copied()
            .zip(positions.iter().map(|position| uses[*position]))
            .collect();
        keyed_uses.sort_by_key(|(index, _)| *index);
        for (position, (_, edge)) in positions.iter().zip(keyed_uses) {
            uses[*position] = edge;
        }
        Ok(())
    }

    /// If this value is a constant integer, its arbitrary-precision value.
    /// Mirrors reading a `ConstantInt`'s `getValue()`; backs the matcher
    /// constant predicates (`m_zero`, `m_all_ones`, `m_ap_int`, ...).
    /// Scalar only — vector splats are not unwrapped here.
    pub fn to_const_int(self) -> Option<ApInt> {
        let constant = Constant::try_from(self).ok()?;
        let int: ConstantIntValue<'_, IntDyn, B, C> = ConstantIntValue::try_from(constant).ok()?;
        Some(int.ap_int())
    }
}

/// Which `Value` subclass a handle names. Mirrors the `dyn_cast` ladder
/// `lib/IR/AsmWriter.cpp` and `Verifier.cpp` walk over `Value`'s subclasses;
/// llvmkit answers it as data rather than a chain of casts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueCategory {
    Constant,
    Argument,
    BasicBlock,
    Function,
    Instruction,
    GlobalVariable,
    GlobalAlias,
    GlobalIfunc,
    MetadataAsValue,
    InlineAsm,
}

impl From<ValueCategory> for ValueCategoryLabel {
    fn from(c: ValueCategory) -> Self {
        match c {
            ValueCategory::Constant => Self::Constant,
            ValueCategory::Argument => Self::Argument,
            ValueCategory::BasicBlock => Self::BasicBlock,
            ValueCategory::Function => Self::Function,
            ValueCategory::Instruction => Self::Instruction,
            ValueCategory::GlobalVariable => Self::GlobalVariable,
            ValueCategory::GlobalAlias => Self::GlobalAlias,
            ValueCategory::GlobalIfunc => Self::GlobalIfunc,
            ValueCategory::MetadataAsValue => Self::MetadataAsValue,
            ValueCategory::InlineAsm => Self::InlineAsm,
        }
    }
}

pub(super) fn category_label_for_kind(kind: &ValueKindData) -> ValueCategoryLabel {
    match kind {
        ValueKindData::Constant(_) => ValueCategoryLabel::Constant,
        ValueKindData::Argument { .. } => ValueCategoryLabel::Argument,
        ValueKindData::BasicBlock(_) => ValueCategoryLabel::BasicBlock,
        ValueKindData::Function(_) => ValueCategoryLabel::Function,
        ValueKindData::Instruction(_) => ValueCategoryLabel::Instruction,
        ValueKindData::GlobalVariable(_) => ValueCategoryLabel::GlobalVariable,
        ValueKindData::GlobalAlias(_) => ValueCategoryLabel::GlobalAlias,
        ValueKindData::GlobalIfunc(_) => ValueCategoryLabel::GlobalIfunc,
        ValueKindData::MetadataAsValue(_) => ValueCategoryLabel::MetadataAsValue,
        ValueKindData::InlineAsm(_) => ValueCategoryLabel::InlineAsm,
    }
}

// --------------------------------------------------------------------------
// Sealed marker traits
// --------------------------------------------------------------------------

pub(super) mod sealed {
    pub trait Sealed {}
}

/// Marker trait implemented by every typed value-handle plus the
/// erased [`Value`] itself.
///
/// Sealed: the closed set of LLVM value categories is part of the IR
/// spec, not an extension point.
pub trait IsValue<'ctx, B: ModuleBrand>:
    sealed::Sealed + Copy + Sized + core::fmt::Debug + CapabilityOf
{
    /// Widen to the erased [`Value`] handle, at the same capability.
    fn as_erased(self) -> Value<'ctx, B, Self::Capability>;
}

/// A value handle's route to the arena [`ValueSlot`] it names: one checked
/// door and one unchecked door.
///
/// A handle is a slot plus the module that minted it, and the slot means
/// something only in that module's arena. Each module owns its own arenas, and
/// two modules that share a brand — every `Module::dynamic` is `DynBrand` — can
/// be handed each other's handles without a type error, so under D7 the module
/// tag is the backstop. These two methods are where it is applied:
///
/// - [`slot_in`](Self::slot_in) is the **checked door**. It compares the
///   handle's module with `owner`, the module about to store or look up the
///   slot, and refuses a foreign handle with [`IrError::ForeignValueId`]. A
///   *boundary* — a site where a caller's handle meets a second module — takes
///   this door, before anything is looked up, stored or linked into a use list.
/// - [`slot_trusting_same_module`](Self::slot_trusting_same_module) is the
///   **unchecked door**. It hands the slot out and trusts that it is used only
///   with the handle's own module: a read through the handle's own module, or
///   a handle the same routine minted or already admitted through `slot_in`.
///
/// Modelled on `MetadataId::into_stored` / `MetadataId::from_stored` in
/// `metadata.rs`: the comparison is written once, here, so a boundary cannot
/// forget it one level up. Crate-private and blanket-implemented over
/// [`IsValue`], so every value handle has both doors under the same two names
/// and nothing outside the crate has either.
pub(crate) trait ValueSlotAccess<'ctx, B: ModuleBrand>: IsValue<'ctx, B> {
    /// The checked door: this handle's slot, if `owner` minted the handle;
    /// [`IrError::ForeignValueId`] otherwise.
    #[inline]
    fn slot_in(self, owner: ModuleId) -> IrResult<ValueSlot> {
        let value = self.as_erased();
        if value.module.id() != owner {
            return Err(IrError::ForeignValueId);
        }
        Ok(value.id)
    }

    /// The unchecked door: this handle's slot, trusting that the caller uses
    /// it only with the handle's own module.
    #[inline]
    fn slot_trusting_same_module(self) -> ValueSlot {
        self.as_erased().id
    }
}

impl<'ctx, B: ModuleBrand, T: IsValue<'ctx, B>> ValueSlotAccess<'ctx, B> for T {}

/// A rename `Value::setNameImpl` has admitted: every guard that can refuse
/// has run ([`Value::admit_rename`]), and what is left — the table update,
/// then `Value::setName`'s `Function::updateAfterNameChange` tail — cannot
/// fail. [`Self::apply`] performs it. The split is llvmkit's, not upstream's:
/// it lets a caller make its own refusals between the two, so that nothing it
/// does after renaming can fail.
#[must_use = "an admitted rename does nothing until it is applied"]
pub(crate) struct AdmittedRename<'ctx, B: ModuleBrand, C: Capability> {
    value: Value<'ctx, B, C>,
    step: AdmittedNameStep,
}

/// What `Value::setNameImpl` does once its guards have passed.
enum AdmittedNameStep {
    /// One of its early returns: an empty name for an unnamed value, or the
    /// name the value already has.
    Return,
    /// Its table update: to this name — uniqued against the table when it is
    /// applied, as `createValueName` does — or, for `None`, to no name.
    Update(Option<String>),
}

impl<'ctx, B: ModuleBrand + 'ctx, C: CanMutate> AdmittedRename<'ctx, B, C> {
    /// Perform the admitted rename: the rest of `Value::setNameImpl`, then
    /// `Value::setName`'s tail, which upstream runs however `setNameImpl`
    /// returned. Infallible.
    pub(crate) fn apply(self) {
        if let AdmittedNameStep::Update(requested) = self.step {
            self.value.update_symbol_table(requested.as_deref());
        }
        self.value.update_after_name_change();
    }
}

/// Sealed accessor trait: anything that has an IR type. Implemented by
/// every value handle; the type comes back at the handle's capability.
pub trait Typed<'ctx, B: ModuleBrand>: sealed::Sealed + CapabilityOf {
    fn ty(self) -> Type<'ctx, B, Self::Capability>;
}

/// Sealed accessor trait: anything that exposes an optional textual
/// name. Implemented by every value handle, at every capability.
pub trait HasName<'ctx, B: ModuleBrand>: sealed::Sealed {
    fn name(self) -> Option<String>;
}

/// Renaming a value — the mutating half of naming, implemented only for
/// handles that [`CanMutate`]. Split from [`HasName`] because a trait method
/// cannot carry a bound its trait does not declare, and reading a name must
/// stay available to a [`ReadOnly`](crate::ReadOnly) handle.
pub trait SetName<'ctx, B: ModuleBrand>: sealed::Sealed {
    /// [`Value::set_name`] on this value.
    ///
    /// # Errors
    ///
    /// [`IrError::InvalidValueName`] as [`Value::set_name`] documents.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module.
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>;
    /// [`Value::clear_name`] on this value.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module.
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>);
}

/// Sealed accessor trait: anything that carries an optional
/// debug-location. Implemented by every value handle.
pub trait HasDebugLoc: sealed::Sealed {
    fn debug_loc(self) -> Option<DebugLoc>;
}

impl<'ctx, B: ModuleBrand, C: Capability> sealed::Sealed for Value<'ctx, B, C> {}
impl<'ctx, B: ModuleBrand, C: Capability> IsValue<'ctx, B> for Value<'ctx, B, C> {
    #[inline]
    fn as_erased(self) -> Value<'ctx, B, C> {
        self
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B> for Value<'ctx, B, C> {
    #[inline]
    fn ty(self) -> Type<'ctx, B, C> {
        Value::ty(self)
    }
}
impl<'ctx, B: ModuleBrand, C: Capability> HasName<'ctx, B> for Value<'ctx, B, C> {
    #[inline]
    fn name(self) -> Option<String> {
        Value::name(self)
    }
}
impl<'ctx, B: ModuleBrand, C: CanMutate> SetName<'ctx, B> for Value<'ctx, B, C> {
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>,
    {
        Value::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        Value::clear_name(self, module_token);
    }
}
impl<B: ModuleBrand, C: Capability> HasDebugLoc for Value<'_, B, C> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Value::debug_loc(self)
    }
}

// --------------------------------------------------------------------------
// IntoErasedValue: operand input at an erased-by-design slot
// --------------------------------------------------------------------------

/// Inputs accepted where a builder wants an **erased** [`Value`] operand.
///
/// The erased sibling of [`IntoIntValue`](crate::IntoIntValue) /
/// [`IntoFloatValue`](crate::IntoFloatValue) / [`IntoPointerValue`]: it is the
/// bound on every builder operand whose declared parameter type is the erased
/// [`Value`] — `store`'s stored value, `freeze`'s operand, the
/// aggregate/vector element slots, the call-argument lists, and so on. Where
/// those three *narrow* to a pinned IR type, this one only widens, so it
/// accepts strictly more:
///
/// - every value **handle** — the whole [`IsValue`] family — which is refused
///   with [`IrError::ForeignValueId`] when minted by a module other than
///   `module`, exactly as an id is; and
/// - the storable **ids** ([`ValueId`], [`IntValueId`], [`FloatValueId`],
///   [`PointerValueId`], [`FunctionId`](crate::FunctionId) and
///   [`GlobalId`](crate::GlobalId)), which resolve against `module` and report
///   [`IrError::ForeignValueId`] for an id minted by a different module.
///
/// The *erased* [`ValueId`] is admitted here and **nowhere else**: an operand
/// slot bound by this trait is erased by design (its parameter type is
/// [`Value`]), so erased-in / erased-out is not the silent erased -> typed
/// narrowing that [`IntoIntValue`](crate::IntoIntValue) and friends forbid.
/// Narrowing an erased id stays spelled —
/// [`Module::try_view`](crate::Module::try_view) or `TryFrom`.
///
/// The trait is **sealed**, and deliberately carries *no* blanket impl over
/// [`IsValue`]: a blanket keyed on a trait bound would conflict with the
/// concrete id impls, because rustc has no negative reasoning with which to
/// prove `IntValueId: !IsValue`. Every implementor is therefore spelled out,
/// mostly by the handle-declaring macros via the crate-internal
/// `impl_into_erased_value_for_handle!`.
pub trait IntoErasedValue<'ctx, B: ModuleBrand>: Sized + into_erased_value_sealed::Sealed {
    #[doc(hidden)]
    fn into_erased_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}

/// Seals [`IntoErasedValue`] to the value handles plus the storable id family.
///
/// `pub(crate)` so each impl can live beside the type it is for — the
/// per-kind handles next to their handle, the id family in `value_id.rs` —
/// while the trait inside stays crate-private, so the seal holds.
pub(crate) mod into_erased_value_sealed {
    pub trait Sealed {}
}

/// Implement [`IntoErasedValue`] for one or more value **handles**, whose lift
/// is the [`IsValue::as_erased`] widen after the handle's module is checked
/// against `module` through [`ValueSlotAccess::slot_in`]. Optional
/// square-bracketed marker parameters are emitted ahead of
/// the brand `B`, matching how every handle orders its generics
/// (`IntValue<'ctx, W, B, C>`, `ArrayValue<'ctx, E, L, B, C>`, ...).
///
/// This exists because [`IntoErasedValue`] cannot be blanket-implemented over
/// [`IsValue`] without colliding with the id-family impls; see the trait docs.
macro_rules! impl_into_erased_value_for_handle {
    ($( $name:ident $([$($mk:ident : $mkb:path),+ $(,)?])? ),+ $(,)?) => { $(
        impl<
            'ctx,
            $($($mk: $mkb,)+)?
            B: $crate::module::ModuleBrand + 'ctx,
            Cap: $crate::capability::Capability,
        >
            $crate::value::into_erased_value_sealed::Sealed
            for $name<'ctx, $($($mk,)+)? B, Cap>
        {
        }
        impl<
            'ctx,
            $($($mk: $mkb,)+)?
            B: $crate::module::ModuleBrand + 'ctx,
            Cap: $crate::capability::Capability,
        >
            $crate::value::IntoErasedValue<'ctx, B>
            for $name<'ctx, $($($mk,)+)? B, Cap>
        {
            #[inline]
            fn into_erased_value(
                self,
                module: $crate::module::ModuleRef<'ctx, B>,
            ) -> $crate::error::IrResult<$crate::value::Value<'ctx, B>> {
                // Boundary: the caller's handle meets `module`. The checked
                // door refuses one minted elsewhere; a handle of any
                // capability is admitted and re-minted at `module`'s, so
                // reading a value as an operand is not mutating it.
                $crate::value::IsValue::as_erased(self).admitted_at(module)
            }
        }
    )+ };
}
pub(crate) use impl_into_erased_value_for_handle;

impl_into_erased_value_for_handle!(Value);

// --------------------------------------------------------------------------
// Per-kind value handles
// --------------------------------------------------------------------------

/// Internal helper: build a per-kind value handle that wraps a value
/// whose IR type matches a given predicate. Mirrors [`decl_type_handle!`]
/// (`derived_types.rs`) so the value-side trait surface stays parallel.
macro_rules! decl_value_handle {
    (
        $(#[$attr:meta])*
        $name:ident,
        $id:ident,
        $type_label:ident,
        $type_handle:ident,
        type_predicate $pred:expr
    ) => {
        $(#[$attr])*
        #[derive(Branded)]
        pub struct $name<'ctx, B: ModuleBrand, C: Capability = Mutable> {
            id: ValueSlot,
            pub(super) module: ModuleRef<'ctx, B, C>,
            ty: TypeSlot,
        }

        impl<B: ModuleBrand, C: Capability> CapabilityOf for $name<'_, B, C> {
            type Capability = C;
        }

        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> $name<'ctx, B, C> {
            /// Widen to the erased [`Value`] handle, at the same capability.
            #[inline]
            pub fn as_erased(self) -> Value<'ctx, B, C> {
                Value { id: self.id, module: self.module, ty: self.ty }
            }

            /// Storable, module-tagged id for this value (0.0.4),
            /// resolvable via [`Module::view`](crate::Module::view) /
            /// [`Module::try_view`](crate::Module::try_view).
            #[inline]
            pub fn id(self) -> $id<B> {
                $id::from_raw(self.module.id(), self.id)
            }

            /// Owning module reference.
            #[inline]
            pub fn module(self) -> ModuleView<'ctx, B> {
                ModuleView::new(self.module.module())
            }

            /// Refined IR-type handle for this value, at this value's
            /// capability.
            #[inline]
            pub fn ty(self) -> $type_handle<'ctx, B, C> {
                $type_handle::new(self.ty, self.module)
            }

            /// Optional textual name.
            pub fn name(self) -> Option<String> {
                self.as_erased().name()
            }

            /// Set the textual name. [`Value::set_name`] on this value.
            ///
            /// # Errors
            ///
            /// [`IrError::InvalidValueName`] for a name `Value::setNameImpl`
            /// asserts against; the value keeps its name.
            ///
            /// # Panics
            ///
            /// Panics if `module_token` is not this value's module.
            pub fn set_name<Name>(
                self,
                module_token: &'ctx Module<B, Unverified>,
                name: Name,
            ) -> IrResult<()>
            where
                Name: Into<String>,
                C: CanMutate,
            {
                self.as_erased().set_name(module_token, name)
            }

            /// Clear the textual name.
            pub fn clear_name(self, module_token: &'ctx Module<B, Unverified>)
            where
                C: CanMutate,
            {
                self.as_erased().clear_name(module_token);
            }

            /// Optional debug-location.
            #[inline]
            pub fn debug_loc(self) -> Option<DebugLoc> {
                self.as_erased().debug_loc()
            }
        }

        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> fmt::Display for $name<'ctx, B, C> {
            /// Print the operand form `<type> <ref>`, identical to what the
            /// erased [`Value`] handle from `as_erased` prints.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&Self::as_erased(*self), f)
            }
        }

        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed for $name<'ctx, B, C> {}
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> IsValue<'ctx, B> for $name<'ctx, B, C> {
            #[inline]
            fn as_erased(self) -> Value<'ctx, B, C> { Self::as_erased(self) }
        }
        impl_into_erased_value_for_handle!($name);
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B> for $name<'ctx, B, C> {
            #[inline]
            fn ty(self) -> Type<'ctx, B, C> {
                self.ty().as_type()
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> HasName<'ctx, B> for $name<'ctx, B, C> {
            #[inline]
            fn name(self) -> Option<String> { Self::name(self) }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: CanMutate> SetName<'ctx, B> for $name<'ctx, B, C> {
            #[inline]
            fn set_name<Name>(
                self,
                module_token: &'ctx Module<B, Unverified>,
                name: Name,
            ) -> IrResult<()>
            where
                Name: Into<String>,
            {
                Self::set_name(self, module_token, name)
            }
            #[inline]
            fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
                Self::clear_name(self, module_token)
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> HasDebugLoc for $name<'ctx, B, C> {
            #[inline]
            fn debug_loc(self) -> Option<DebugLoc> { Self::debug_loc(self) }
        }

        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> From<$name<'ctx, B, C>>
            for Value<'ctx, B, C>
        {
            #[inline]
            fn from(v: $name<'ctx, B, C>) -> Self { v.as_erased() }
        }

        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
            for $name<'ctx, B, C>
        {
            type Error = IrError;
            fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
                let pred: fn(&TypeData) -> bool = $pred;
                let ty = v.ty();
                if pred(ty.data()) {
                    Ok(Self { id: v.id, module: v.module, ty: v.ty })
                } else {
                    Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::$type_label,
                        got: ty.kind_label(),
                    })
                }
            }
        }

        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Argument<'ctx, B, C>>
            for $name<'ctx, B, C>
        {
            type Error = IrError;
            #[inline]
            fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
            }
        }

        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Constant<'ctx, B, C>>
            for $name<'ctx, B, C>
        {
            type Error = IrError;
            #[inline]
            fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
            }
        }

        // An attached `Instruction` is the linear lifecycle handle and is
        // always `Mutable` (rule R7), so this narrowing is too.
        impl<'ctx, B: ModuleBrand + 'ctx>
            TryFrom<Instruction<'ctx, Attached, B>>
            for $name<'ctx, B>
        {
            type Error = IrError;
            #[inline]
            fn try_from(
                i: Instruction<'ctx, Attached, B>,
            ) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(
                    Instruction::to_erased(&i),
                )
            }
        }
    };
}

// `IntValue<'ctx, W>` and `FloatValue<'ctx, K>` are hand-written below to
// carry their width / kind markers.
decl_value_handle!(
    /// Value whose type is a (opaque) pointer.
    PointerValue, PointerValueId, Pointer, PointerType,
    type_predicate |d| matches!(d, TypeData::Pointer { .. })
);
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> PointerValue<'ctx, B, C> {
    /// Crate-internal: wrap a [`Value`] **claimed** to have a pointer type,
    /// without checking that it does.
    ///
    /// # Callers must guarantee
    ///
    /// `v`'s runtime type is a pointer type. As with
    /// `IntValue::from_value_unchecked` (see it for the full account of the
    /// obligation), the builder is not the only caller: `ir_builder.rs` attaches
    /// the pointer marker to a freshly-appended instruction only through the
    /// `append_ptr` / `append_ptr_load` constructors (which append at a
    /// `PointerType`, proving pointer-ness by construction) — its other in-file
    /// callers are the fold seams (runtime-checked) and the select-arm re-wrap;
    /// `instructions.rs` re-wraps pointer operands read back out of an
    /// instruction payload, `function_signature.rs` lifts pointer arguments
    /// and block parameters, and `ssa_builder.rs` wraps arena reads against a
    /// pointer variable's pinned type.
    ///
    /// Carries no address-space marker to contradict — unlike the int/float
    /// handles, a `PointerValue` never statically pins one — so the claim
    /// forged here is only "this is a pointer". The checked path is
    /// `TryFrom<Value>`.
    #[inline]
    pub(super) fn from_value_unchecked(v: Value<'ctx, B, C>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
        }
    }
}

// --------------------------------------------------------------------------
// ArrayValue<'ctx, E, L> -- element + length-typed array value handle
// --------------------------------------------------------------------------

/// Value whose IR type is `[N x T]`. The `E: VecElem` marker (default
/// [`ElemDyn`]) pins the element type and `L: ArrayLen` (default
/// [`ArrLenDyn`]) pins the element count at the type level, mirroring
/// [`VectorValue`] (arrays differ only in the `u64` length and the
/// `ArrLen`/`ArrLenDyn` marker family). `ArrayValue<'ctx>` (both markers
/// erased) is the dynamic handle; `ArrayValue<'ctx, i32, ArrLen<4>>` is a
/// statically typed `[4 x i32]`.
pub struct ArrayValue<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand, C: Capability = Mutable> {
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B, C>,
    ty: TypeSlot,
    pub(super) _e: PhantomData<E>,
    pub(super) _l: PhantomData<L>,
}

impl<E: VecElem, L: ArrayLen, B: ModuleBrand, C: Capability> CapabilityOf
    for ArrayValue<'_, E, L, B, C>
{
    type Capability = C;
}

impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> Clone
    for ArrayValue<'ctx, E, L, B, C>
{
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> Copy
    for ArrayValue<'ctx, E, L, B, C>
{
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> PartialEq
    for ArrayValue<'ctx, E, L, B, C>
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> Eq
    for ArrayValue<'ctx, E, L, B, C>
{
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> Hash
    for ArrayValue<'ctx, E, L, B, C>
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> fmt::Debug
    for ArrayValue<'ctx, E, L, B, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArrayValue").field("id", &self.id).finish()
    }
}

impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> fmt::Display
    for ArrayValue<'ctx, E, L, B, C>
{
    /// Print the operand form `[N x T] <ref>`, identical to what the erased
    /// [`Value`] handle from [`ArrayValue::as_erased`] prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&Self::as_erased(*self), f)
    }
}

impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability>
    ArrayValue<'ctx, E, L, B, C>
{
    /// Widen to the erased [`Value`] handle, at the same capability.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }
    /// Refined IR-type handle for this value, at this value's capability.
    #[inline]
    pub fn ty(self) -> ArrayType<'ctx, E, L, B, C> {
        ArrayType::new(self.ty, self.module)
    }
    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_erased().name()
    }
    /// Set the textual name. [`Value::set_name`] on this value.
    ///
    /// # Errors
    ///
    /// [`IrError::InvalidValueName`] for a name `Value::setNameImpl` asserts
    /// against; the value keeps its name.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module.
    pub fn set_name<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        name: Name,
    ) -> IrResult<()>
    where
        Name: Into<String>,
        C: CanMutate,
    {
        self.as_erased().set_name(module_token, name)
    }
    /// Clear the textual name.
    pub fn clear_name(self, module_token: &'ctx Module<B, Unverified>)
    where
        C: CanMutate,
    {
        self.as_erased().clear_name(module_token);
    }
    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_erased().debug_loc()
    }
    /// Erase both markers; preserves the runtime element type / element count.
    #[inline]
    pub fn as_dyn(self) -> ArrayValue<'ctx, ElemDyn, ArrLenDyn, B, C> {
        ArrayValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _e: PhantomData,
            _l: PhantomData,
        }
    }

    /// Crate-internal: wrap a [`Value`] **claimed** to have an array type of
    /// the given element / length, without checking that it does. Mirrors
    /// [`VectorValue::from_value_unchecked`](VectorValue).
    ///
    /// # Callers must guarantee
    ///
    /// `v`'s runtime type is an array whose element type and length are
    /// exactly `E` and `L`. Two markers are forged here rather than one, so
    /// the obligation is correspondingly wider — see
    /// `IntValue::from_value_unchecked` for the full account of what it means
    /// to mint a marker rather than verify it.
    ///
    /// Callers: `ir_builder.rs` wraps array-result instructions
    /// (`insertvalue`) whose `E`/`L` are pinned by the statically-typed input
    /// array, `instructions.rs` re-wraps array operands read back out of a
    /// payload, and `function_signature.rs` lifts array arguments and block
    /// parameters. The checked path is `TryFrom<Value>`.
    #[inline]
    pub(super) fn from_value_unchecked(v: Value<'ctx, B, C>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _e: PhantomData,
            _l: PhantomData,
        }
    }
}

impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed
    for ArrayValue<'ctx, E, L, B, C>
{
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> IsValue<'ctx, B>
    for ArrayValue<'ctx, E, L, B, C>
{
    #[inline]
    fn as_erased(self) -> Value<'ctx, B, C> {
        Self::as_erased(self)
    }
}
impl_into_erased_value_for_handle!(ArrayValue[E: VecElem, L: ArrayLen]);
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B>
    for ArrayValue<'ctx, E, L, B, C>
{
    #[inline]
    fn ty(self) -> Type<'ctx, B, C> {
        self.ty().as_type()
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> HasName<'ctx, B>
    for ArrayValue<'ctx, E, L, B, C>
{
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: CanMutate> SetName<'ctx, B>
    for ArrayValue<'ctx, E, L, B, C>
{
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability> HasDebugLoc
    for ArrayValue<'ctx, E, L, B, C>
{
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx, C: Capability>
    From<ArrayValue<'ctx, E, L, B, C>> for Value<'ctx, B, C>
{
    #[inline]
    fn from(v: ArrayValue<'ctx, E, L, B, C>) -> Self {
        v.as_erased()
    }
}

// Erased narrowing: any array value lands in the fully dynamic form.
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B, C>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        let ty = v.ty();
        if matches!(ty.data(), TypeData::Array { .. }) {
            Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
                _e: PhantomData,
                _l: PhantomData,
            })
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Array,
                got: ty.kind_label(),
            })
        }
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Argument<'ctx, B, C>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Constant<'ctx, B, C>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::to_erased(&i))
    }
}

/// Typed narrowing: a `Value` accepts into `ArrayValue<'ctx, E, ArrLen<N>>`
/// only if it is an array whose element type is exactly `E`'s projection and
/// whose element count is exactly `N`. Element mismatch reports
/// [`IrError::TypeMismatch`] (the element kinds); length mismatch reports
/// [`IrError::ArrayLengthMismatch`] — a `u64`-shaped variant, since array
/// lengths do not fit the `u32` `OperandWidthMismatch` the sibling
/// `VectorValue` narrowing uses for its lane count.
impl<'ctx, E, const N: u64, B, C> TryFrom<Value<'ctx, B, C>>
    for ArrayValue<'ctx, E, ArrLen<N>, B, C>
where
    E: StaticVecElem<'ctx, B>,
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        let ty = v.ty();
        match ty.data() {
            TypeData::Array { elem, n } => {
                let expected_elem = E::element_ir_type(v.module);
                if *elem != expected_elem.slot_trusting_same_module() {
                    return Err(IrError::TypeIdentityMismatch {
                        expected: expected_elem.rendered(),
                        got: Type::new(*elem, v.module).rendered(),
                    });
                }
                if *n != N {
                    return Err(IrError::ArrayLengthMismatch {
                        expected: N,
                        got: *n,
                    });
                }
                Ok(Self {
                    id: v.id,
                    module: v.module,
                    ty: v.ty,
                    _e: PhantomData,
                    _l: PhantomData,
                })
            }
            _ => Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Array,
                got: ty.kind_label(),
            }),
        }
    }
}

/// Static -> `Dyn` widening (always succeeds). Restricted to the `ArrLen<N>`
/// typed form so it cannot overlap the reflexive `From<T> for T`.
impl<'ctx, E: VecElem, const N: u64, B: ModuleBrand + 'ctx, C: Capability>
    From<ArrayValue<'ctx, E, ArrLen<N>, B, C>> for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B, C>
{
    #[inline]
    fn from(v: ArrayValue<'ctx, E, ArrLen<N>, B, C>) -> Self {
        v.as_dyn()
    }
}

/// Value whose type is a struct.
#[derive(Branded)]
pub struct StructValue<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B, C>,
    ty: TypeSlot,
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for StructValue<'_, B, C> {
    type Capability = C;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> StructValue<'ctx, B, C> {
    /// Widen to the erased [`Value`] handle, at the same capability.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Crate-internal: wrap a [`Value`] **claimed** to have a struct type,
    /// without checking that it does.
    ///
    /// # Callers must guarantee
    ///
    /// `v`'s runtime type is a struct type. The body stays erased
    /// (`StructBodyDyn`), so — as with [`PointerValue`] and unlike the
    /// int/float handles — the only claim forged here is the kind itself; a
    /// schema-typed wrapper is minted separately against a
    /// `ValidatedStructValue` witness. See
    /// `IntValue::from_value_unchecked` for the full account of the
    /// obligation.
    ///
    /// Callers: `ir_builder.rs` wraps struct-result instructions it just
    /// produced, `instructions.rs` re-wraps struct operands read back out of
    /// a payload, and `struct_schema.rs` lifts schema-typed values and
    /// arguments. The checked path is `TryFrom<Value>`.
    #[inline]
    pub(crate) fn from_value_unchecked(v: Value<'ctx, B, C>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
        }
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    /// Refined IR-type handle for this value, at this value's capability.
    #[inline]
    pub fn ty(self) -> StructType<'ctx, StructBodyDyn, B, C> {
        StructType::new(self.ty, self.module)
    }

    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_erased().name()
    }

    /// Set the textual name. [`Value::set_name`] on this value.
    ///
    /// # Errors
    ///
    /// [`IrError::InvalidValueName`] for a name `Value::setNameImpl` asserts
    /// against; the value keeps its name.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module.
    pub fn set_name<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        name: Name,
    ) -> IrResult<()>
    where
        Name: Into<String>,
        C: CanMutate,
    {
        self.as_erased().set_name(module_token, name)
    }

    /// Clear the textual name.
    pub fn clear_name(self, module_token: &'ctx Module<B, Unverified>)
    where
        C: CanMutate,
    {
        self.as_erased().clear_name(module_token);
    }

    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_erased().debug_loc()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed for StructValue<'ctx, B, C> {}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> fmt::Display for StructValue<'ctx, B, C> {
    /// Print the operand form `{ ... } <ref>`, identical to what the erased
    /// [`Value`] handle from [`StructValue::as_erased`] prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&Self::as_erased(*self), f)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> IsValue<'ctx, B> for StructValue<'ctx, B, C> {
    #[inline]
    fn as_erased(self) -> Value<'ctx, B, C> {
        Self::as_erased(self)
    }
}
impl_into_erased_value_for_handle!(StructValue);
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B> for StructValue<'ctx, B, C> {
    #[inline]
    fn ty(self) -> Type<'ctx, B, C> {
        self.ty().as_type()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> HasName<'ctx, B> for StructValue<'ctx, B, C> {
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: CanMutate> SetName<'ctx, B> for StructValue<'ctx, B, C> {
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> HasDebugLoc for StructValue<'ctx, B, C> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> From<StructValue<'ctx, B, C>>
    for Value<'ctx, B, C>
{
    #[inline]
    fn from(v: StructValue<'ctx, B, C>) -> Self {
        v.as_erased()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
    for StructValue<'ctx, B, C>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        let ty = v.ty();
        if matches!(ty.data(), TypeData::Struct(_)) {
            Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
            })
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Struct,
                got: ty.kind_label(),
            })
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Argument<'ctx, B, C>>
    for StructValue<'ctx, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Constant<'ctx, B, C>>
    for StructValue<'ctx, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>> for StructValue<'ctx, B> {
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::to_erased(&i))
    }
}

// --------------------------------------------------------------------------
// VectorValue<'ctx, E, L> -- element + length-typed vector value handle
// --------------------------------------------------------------------------

/// Value whose IR type is a fixed or scalable vector. The `E: VecElem`
/// marker (default [`ElemDyn`]) pins the element type and `L: VecLen`
/// (default [`LenDyn`]) pins the lane count at the type level, mirroring
/// [`IntValue`]'s width marker. `VectorValue<'ctx>` (both markers erased)
/// is the dynamic handle; `VectorValue<'ctx, i32, Len<4>>` is a statically
/// typed `<4 x i32>`.
pub struct VectorValue<'ctx, E: VecElem, L: VecLen, B: ModuleBrand, C: Capability = Mutable> {
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B, C>,
    ty: TypeSlot,
    pub(super) _e: PhantomData<E>,
    pub(super) _l: PhantomData<L>,
}

impl<E: VecElem, L: VecLen, B: ModuleBrand, C: Capability> CapabilityOf
    for VectorValue<'_, E, L, B, C>
{
    type Capability = C;
}

impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> Clone
    for VectorValue<'ctx, E, L, B, C>
{
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> Copy
    for VectorValue<'ctx, E, L, B, C>
{
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> PartialEq
    for VectorValue<'ctx, E, L, B, C>
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> Eq
    for VectorValue<'ctx, E, L, B, C>
{
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> Hash
    for VectorValue<'ctx, E, L, B, C>
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> fmt::Debug
    for VectorValue<'ctx, E, L, B, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VectorValue").field("id", &self.id).finish()
    }
}

impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> fmt::Display
    for VectorValue<'ctx, E, L, B, C>
{
    /// Print the operand form `<N x T> <ref>`, identical to what the erased
    /// [`Value`] handle from [`VectorValue::as_erased`] prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&Self::as_erased(*self), f)
    }
}

impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability>
    VectorValue<'ctx, E, L, B, C>
{
    /// Crate-internal: wrap a [`Value`] **claimed** to have a vector type of
    /// the given element / length, without checking that it does.
    ///
    /// # Callers must guarantee
    ///
    /// `v`'s runtime type is a vector whose element type and length are
    /// exactly `E` and `L`. As on the array twin, two markers are forged at
    /// once — see `IntValue::from_value_unchecked` for the full account of
    /// the obligation.
    ///
    /// Callers: `ir_builder.rs` wraps vector-result instructions it just
    /// produced (insertelement / shufflevector / splat) — using the erased
    /// form where the shape is not statically known, and passing the pinned
    /// `E, L` where it is; `instructions.rs` re-wraps vector operands read
    /// back out of a payload; `function_signature.rs` lifts vector arguments
    /// and block parameters. `element.rs` gates its own raw wrap behind the
    /// unforgeable [`WrapWitness`](crate::element::WrapWitness) instead. The
    /// checked path here is `TryFrom<Value>`.
    #[inline]
    pub(super) fn from_value_unchecked(v: Value<'ctx, B, C>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _e: PhantomData,
            _l: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle, at the same capability.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }
    /// Refined IR-type handle for this value, at this value's capability.
    #[inline]
    pub fn ty(self) -> VectorType<'ctx, E, L, B, C> {
        VectorType::new(self.ty, self.module)
    }
    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_erased().name()
    }
    /// Set the textual name. [`Value::set_name`] on this value.
    ///
    /// # Errors
    ///
    /// [`IrError::InvalidValueName`] for a name `Value::setNameImpl` asserts
    /// against; the value keeps its name.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module.
    pub fn set_name<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        name: Name,
    ) -> IrResult<()>
    where
        Name: Into<String>,
        C: CanMutate,
    {
        self.as_erased().set_name(module_token, name)
    }
    /// Clear the textual name.
    pub fn clear_name(self, module_token: &'ctx Module<B, Unverified>)
    where
        C: CanMutate,
    {
        self.as_erased().clear_name(module_token);
    }
    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_erased().debug_loc()
    }
    /// Erase both markers; preserves the runtime element type / lane count.
    #[inline]
    pub fn as_dyn(self) -> VectorValue<'ctx, ElemDyn, LenDyn, B, C> {
        VectorValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _e: PhantomData,
            _l: PhantomData,
        }
    }
}

impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed
    for VectorValue<'ctx, E, L, B, C>
{
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> IsValue<'ctx, B>
    for VectorValue<'ctx, E, L, B, C>
{
    #[inline]
    fn as_erased(self) -> Value<'ctx, B, C> {
        Self::as_erased(self)
    }
}
impl_into_erased_value_for_handle!(VectorValue[E: VecElem, L: VecLen]);
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B>
    for VectorValue<'ctx, E, L, B, C>
{
    #[inline]
    fn ty(self) -> Type<'ctx, B, C> {
        self.ty().as_type()
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> HasName<'ctx, B>
    for VectorValue<'ctx, E, L, B, C>
{
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: CanMutate> SetName<'ctx, B>
    for VectorValue<'ctx, E, L, B, C>
{
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability> HasDebugLoc
    for VectorValue<'ctx, E, L, B, C>
{
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx, C: Capability>
    From<VectorValue<'ctx, E, L, B, C>> for Value<'ctx, B, C>
{
    #[inline]
    fn from(v: VectorValue<'ctx, E, L, B, C>) -> Self {
        v.as_erased()
    }
}

// Erased narrowing: any vector value lands in the fully dynamic form.
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B, C>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        let ty = v.ty();
        if matches!(
            ty.data(),
            TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }
        ) {
            Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
                _e: PhantomData,
                _l: PhantomData,
            })
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::FixedVector,
                got: ty.kind_label(),
            })
        }
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Argument<'ctx, B, C>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Constant<'ctx, B, C>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::to_erased(&i))
    }
}

/// Typed narrowing: a `Value` accepts into `VectorValue<'ctx, E, Len<N>>`
/// only if it is a **fixed** vector whose element type is exactly `E`'s
/// projection and whose lane count is exactly `N`. Element mismatch reports
/// [`IrError::TypeMismatch`] (the element kinds); lane-count mismatch
/// reports [`IrError::OperandWidthMismatch`] (the "vector length" arm of
/// that variant's doc). Scalable vectors — whose lane count is a runtime
/// multiple, not a fixed `N` — never narrow to `Len<N>`.
impl<'ctx, E, const N: u32, B, C> TryFrom<Value<'ctx, B, C>> for VectorValue<'ctx, E, Len<N>, B, C>
where
    E: StaticVecElem<'ctx, B>,
    B: ModuleBrand + 'ctx,
    C: Capability,
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        let ty = v.ty();
        match ty.data() {
            TypeData::FixedVector { elem, n } => {
                let expected_elem = E::element_ir_type(v.module);
                if *elem != expected_elem.slot_trusting_same_module() {
                    return Err(IrError::TypeIdentityMismatch {
                        expected: expected_elem.rendered(),
                        got: Type::new(*elem, v.module).rendered(),
                    });
                }
                if *n != N {
                    return Err(IrError::OperandWidthMismatch { lhs: N, rhs: *n });
                }
                Ok(Self {
                    id: v.id,
                    module: v.module,
                    ty: v.ty,
                    _e: PhantomData,
                    _l: PhantomData,
                })
            }
            _ => Err(IrError::TypeMismatch {
                expected: TypeKindLabel::FixedVector,
                got: ty.kind_label(),
            }),
        }
    }
}

/// Static -> `Dyn` widening (always succeeds). Restricted to the `Len<N>`
/// typed form so it cannot overlap the reflexive `From<T> for T`.
impl<'ctx, E: VecElem, const N: u32, B: ModuleBrand + 'ctx, C: Capability>
    From<VectorValue<'ctx, E, Len<N>, B, C>> for VectorValue<'ctx, ElemDyn, LenDyn, B, C>
{
    #[inline]
    fn from(v: VectorValue<'ctx, E, Len<N>, B, C>) -> Self {
        v.as_dyn()
    }
}

decl_value_handle!(
    /// Value whose type is a function signature. Mostly seen as a
    /// `FunctionValue` operand, but the concrete category is checked
    /// elsewhere; this handle only refines the type, not the category.
    ///
    /// Has no dedicated id family (it refines only the *type*, not the value
    /// category), so [`id`](FunctionTypedValue::id) mints the erased
    /// [`ValueId`].
    FunctionTypedValue, ValueId, Function, FunctionType,
    type_predicate |d| matches!(d, TypeData::Function { .. })
);

// --------------------------------------------------------------------------
// IntValue<'ctx, W> -- width-typed integer value handle
// --------------------------------------------------------------------------

// IntType / FloatType already imported at top of file.

/// Value whose IR type is `iN`. The `W: IntWidth` marker pins the
/// bit-width at the type level, so the IrBuilder can reject mismatched
/// widths at compile time.
pub struct IntValue<'ctx, W: IntWidth, B: ModuleBrand, C: Capability = Mutable> {
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B, C>,
    ty: TypeSlot,
    pub(super) _w: PhantomData<W>,
}

impl<W: IntWidth, B: ModuleBrand, C: Capability> CapabilityOf for IntValue<'_, W, B, C> {
    type Capability = C;
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> Clone for IntValue<'ctx, W, B, C> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> Copy for IntValue<'ctx, W, B, C> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> PartialEq
    for IntValue<'ctx, W, B, C>
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> Eq for IntValue<'ctx, W, B, C> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> Hash for IntValue<'ctx, W, B, C> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> fmt::Debug
    for IntValue<'ctx, W, B, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntValue")
            .field("id", &self.id)
            .field("width", &W::static_bits())
            .finish()
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> IntValue<'ctx, W, B, C> {
    /// Crate-internal: wrap a [`Value`] **claimed** to have type `iN` with
    /// width `W`, without checking that it does.
    ///
    /// This *mints* the claim `W` makes rather than verifying it, so `W` is
    /// only ever as honest as the caller. Crate-internal is the entire
    /// safety story: `pub(crate)` is what keeps external safe code from
    /// forging an `IntValue<W>` whose marker contradicts its runtime type.
    /// The checked paths are `TryFrom<Value>` (per concrete marker) and
    /// `IntWidth::narrow` (from marker-generic code); prefer them anywhere
    /// the runtime type is not already proven.
    ///
    /// # Callers must guarantee
    ///
    /// `v`'s runtime type is exactly `W`'s. This is an obligation to
    /// discharge, not a given — the in-crate callers are not equally safe,
    /// and they are not all the builder:
    ///
    /// - `ir_builder.rs` — as of the unforgeable-markers cycle, an int marker is
    ///   attached to a freshly-appended instruction *only* through the typed-append
    ///   constructor family (`append_int_like` / `append_int_at` / `append_int_load`),
    ///   each of which appends the instruction AT a typed `IntType<W>` (or a `W`-typed
    ///   operand) and re-wraps the result — so the marker matches the runtime type by
    ///   construction, not by a proof the reader must reconstruct. The other in-file
    ///   callers are the fold seams (below) and the `ptrtoaddr` `IntDyn` re-wrap (which
    ///   claims only integer-ness). This confinement is *audited*, not compiler-enforced:
    ///   `from_value_unchecked` stays `pub(crate)` — a hard seal is impossible, since
    ///   `value` and `ir_builder` are sibling modules and the constructors need
    ///   `ir_builder`-private helpers — so the fold re-checks remain the backstop.
    /// - `instructions.rs` — re-wraps an operand read back out of an
    ///   instruction's own payload, whose type the builder pinned going in.
    /// - `function_signature.rs` — argument and block-parameter (head-phi)
    ///   lifts, pinned by the marker the function or block was declared with.
    /// - `ssa_builder.rs` — arena reads (`use_int_var`) wrap the variable's
    ///   pinned `ty`; `def_int_var` is what makes that safe, by checking
    ///   every write against it.
    /// - `ir_builder/folder.rs` — fold results, but only *after*
    ///   `Type::require_match` has compared the two runtime types.
    /// - `int_width.rs` — the `IntoIntValue` lifts, on constants they just
    ///   built at `W`'s own type.
    ///
    /// The riskiest caller is the one this doc used to omit: a folder hook is
    /// an *extension point*, and an in-crate folder that wraps a wrong-width
    /// payload here is exactly the bug the builder's `accept_folded_int` /
    /// `narrow_folded_int` seams exist to catch — see
    /// `hostile_native_typed_override_wrong_width_rejected_at_static_width`
    /// (`ir_builder.rs`). Those seams check every marker, static ones
    /// included, precisely because this method makes a static `W` no more
    /// trustworthy than the code that wrote it.
    #[inline]
    pub(crate) fn from_value_unchecked(v: Value<'ctx, B, C>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _w: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle, at the same capability.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    /// Storable, module-tagged [`IntValueId<W>`] for this value (0.0.4),
    /// resolvable via [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view). Preserves the width
    /// marker `W`.
    #[inline]
    pub fn id(self) -> IntValueId<W, B> {
        IntValueId::from_raw(self.module.id(), self.id)
    }
    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }
    /// Refined IR-type handle for this value, at this value's capability.
    #[inline]
    pub fn ty(self) -> IntType<'ctx, W, B, C> {
        IntType::new(self.ty, self.module)
    }
    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_erased().name()
    }
    /// Set the textual name. [`Value::set_name`] on this value.
    ///
    /// # Errors
    ///
    /// [`IrError::InvalidValueName`] for a name `Value::setNameImpl` asserts
    /// against; the value keeps its name.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module.
    pub fn set_name<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        name: Name,
    ) -> IrResult<()>
    where
        Name: Into<String>,
        C: CanMutate,
    {
        self.as_erased().set_name(module_token, name)
    }
    /// Clear the textual name.
    pub fn clear_name(self, module_token: &'ctx Module<B, Unverified>)
    where
        C: CanMutate,
    {
        self.as_erased().clear_name(module_token);
    }
    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_erased().debug_loc()
    }
    /// Erase the width marker; preserves the runtime width.
    #[inline]
    pub fn as_dyn(self) -> IntValue<'ctx, IntDyn, B, C> {
        IntValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _w: PhantomData,
        }
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed
    for IntValue<'ctx, W, B, C>
{
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> fmt::Display
    for IntValue<'ctx, W, B, C>
{
    /// Print the operand form `i<N> <ref>`, identical to what the erased
    /// [`Value`] handle from [`IntValue::as_erased`] prints. A constant
    /// operand prints its signed-decimal literal in place of the `<ref>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&Self::as_erased(*self), f)
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> IsValue<'ctx, B>
    for IntValue<'ctx, W, B, C>
{
    #[inline]
    fn as_erased(self) -> Value<'ctx, B, C> {
        Self::as_erased(self)
    }
}
impl_into_erased_value_for_handle!(IntValue[W: IntWidth]);
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B>
    for IntValue<'ctx, W, B, C>
{
    #[inline]
    fn ty(self) -> Type<'ctx, B, C> {
        self.ty().as_type()
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> HasName<'ctx, B>
    for IntValue<'ctx, W, B, C>
{
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: CanMutate> SetName<'ctx, B>
    for IntValue<'ctx, W, B, C>
{
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> HasDebugLoc
    for IntValue<'ctx, W, B, C>
{
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx, C: Capability> From<IntValue<'ctx, W, B, C>>
    for Value<'ctx, B, C>
{
    #[inline]
    fn from(v: IntValue<'ctx, W, B, C>) -> Self {
        v.as_erased()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
    for IntValue<'ctx, IntDyn, B, C>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        let ty = v.ty();
        if matches!(ty.data(), TypeData::Integer { .. }) {
            Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
                _w: PhantomData,
            })
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: ty.kind_label(),
            })
        }
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Argument<'ctx, B, C>>
    for IntValue<'ctx, IntDyn, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Constant<'ctx, B, C>>
    for IntValue<'ctx, IntDyn, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
    for IntValue<'ctx, IntDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::to_erased(&i))
    }
}

/// Per-static-width narrowing.
macro_rules! impl_int_value_static_try_from {
    ($marker:ident, $bits:expr) => {
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
            for IntValue<'ctx, $marker, B, C>
        {
            type Error = IrError;
            fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
                let ty = v.ty();
                match ty.data() {
                    TypeData::Integer { bits } if *bits == $bits => Ok(Self {
                        id: v.id,
                        module: v.module,
                        ty: v.ty,
                        _w: PhantomData,
                    }),
                    TypeData::Integer { bits } => Err(IrError::OperandWidthMismatch {
                        lhs: $bits,
                        rhs: *bits,
                    }),
                    _ => Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::Integer,
                        got: ty.kind_label(),
                    }),
                }
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Argument<'ctx, B, C>>
            for IntValue<'ctx, $marker, B, C>
        {
            type Error = IrError;
            #[inline]
            fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Constant<'ctx, B, C>>
            for IntValue<'ctx, $marker, B, C>
        {
            type Error = IrError;
            #[inline]
            fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
            for IntValue<'ctx, $marker, B>
        {
            type Error = IrError;
            #[inline]
            fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::to_erased(&i))
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<IntValue<'ctx, IntDyn, B, C>>
            for IntValue<'ctx, $marker, B, C>
        {
            type Error = IrError;
            fn try_from(v: IntValue<'ctx, IntDyn, B, C>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B, C>>>::try_from(v.as_erased())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> From<IntValue<'ctx, $marker, B, C>>
            for IntValue<'ctx, IntDyn, B, C>
        {
            #[inline]
            fn from(v: IntValue<'ctx, $marker, B, C>) -> Self {
                v.as_dyn()
            }
        }
    };
}
impl_int_value_static_try_from!(bool, 1);
impl_int_value_static_try_from!(i8, 8);
impl_int_value_static_try_from!(i16, 16);
impl_int_value_static_try_from!(i32, 32);
impl_int_value_static_try_from!(i64, 64);
impl_int_value_static_try_from!(i128, 128);
// Const-generic narrowing: `Value` / `Argument` / `Constant` /
// `Instruction` / `IntValue<'ctx, IntDyn>` -> `IntValue<'ctx,
// Width<N>>`. Pattern matches `impl_int_value_static_try_from!` but
// the bit-count comes from the const generic `N` instead of a
// macro literal.
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability, const N: u32> TryFrom<Value<'ctx, B, C>>
    for IntValue<'ctx, Width<N>, B, C>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        let ty = v.ty();
        match ty.data() {
            TypeData::Integer { bits } if *bits == N => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
                _w: PhantomData,
            }),
            TypeData::Integer { bits } => Err(IrError::OperandWidthMismatch { lhs: N, rhs: *bits }),
            _ => Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: ty.kind_label(),
            }),
        }
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability, const N: u32> TryFrom<Argument<'ctx, B, C>>
    for IntValue<'ctx, Width<N>, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability, const N: u32> TryFrom<Constant<'ctx, B, C>>
    for IntValue<'ctx, Width<N>, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> TryFrom<Instruction<'ctx, Attached, B>>
    for IntValue<'ctx, Width<N>, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::to_erased(&i))
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability, const N: u32> TryFrom<IntValue<'ctx, IntDyn, B, C>>
    for IntValue<'ctx, Width<N>, B, C>
{
    type Error = IrError;
    #[inline]
    fn try_from(v: IntValue<'ctx, IntDyn, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(v.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability, const N: u32> From<IntValue<'ctx, Width<N>, B, C>>
    for IntValue<'ctx, IntDyn, B, C>
{
    #[inline]
    fn from(v: IntValue<'ctx, Width<N>, B, C>) -> Self {
        v.as_dyn()
    }
}

// --------------------------------------------------------------------------
// FloatValue<'ctx, K> -- kind-typed floating-point value handle
// --------------------------------------------------------------------------

/// Value whose IR type is an IEEE / non-IEEE float.
pub struct FloatValue<'ctx, K: FloatKind, B: ModuleBrand, C: Capability = Mutable> {
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B, C>,
    ty: TypeSlot,
    pub(super) _k: PhantomData<K>,
}

impl<K: FloatKind, B: ModuleBrand, C: Capability> CapabilityOf for FloatValue<'_, K, B, C> {
    type Capability = C;
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> Clone for FloatValue<'ctx, K, B, C> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> Copy for FloatValue<'ctx, K, B, C> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> PartialEq
    for FloatValue<'ctx, K, B, C>
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> Eq for FloatValue<'ctx, K, B, C> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> Hash for FloatValue<'ctx, K, B, C> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> fmt::Debug
    for FloatValue<'ctx, K, B, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FloatValue")
            .field("id", &self.id)
            .field("kind", &K::ieee_kind())
            .finish()
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> FloatValue<'ctx, K, B, C> {
    /// Crate-internal: wrap a [`Value`] **claimed** to have a float type of
    /// kind `K`, without checking that it does.
    ///
    /// The float twin of `IntValue::from_value_unchecked` in every respect,
    /// including its caller classes and the obligation they carry — see that
    /// method's doc for the full account (the float constructor family through
    /// which `ir_builder.rs` attaches the marker is `append_fp_like` /
    /// `append_fp_at` / `append_fp_load`). It forges a static `K` exactly as
    /// freely as the int side forges a static `W`, which is why the float
    /// acceptors (`accept_folded_fp`, `narrow_folded_fp`) and `def_float_var`
    /// check every marker rather than only the erased `FloatDyn` one.
    ///
    /// # Callers must guarantee
    ///
    /// `v`'s runtime type is exactly `K`'s. The checked paths are
    /// `TryFrom<Value>` (per concrete marker) and `FloatKind::narrow` (from
    /// kind-generic code).
    #[inline]
    pub(crate) fn from_value_unchecked(v: Value<'ctx, B, C>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _k: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle, at the same capability.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    /// Storable, module-tagged [`FloatValueId<K>`] for this value (llvmkit
    /// 2.0), resolvable via [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view). Preserves the
    /// float-kind marker `K`.
    #[inline]
    pub fn id(self) -> FloatValueId<K, B> {
        FloatValueId::from_raw(self.module.id(), self.id)
    }
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }
    /// Refined IR-type handle for this value, at this value's capability.
    #[inline]
    pub fn ty(self) -> FloatType<'ctx, K, B, C> {
        FloatType::new(self.ty, self.module)
    }
    pub fn name(self) -> Option<String> {
        self.as_erased().name()
    }
    /// Set the textual name. [`Value::set_name`] on this value.
    ///
    /// # Errors
    ///
    /// [`IrError::InvalidValueName`] for a name `Value::setNameImpl` asserts
    /// against; the value keeps its name.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this value's module.
    pub fn set_name<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        name: Name,
    ) -> IrResult<()>
    where
        Name: Into<String>,
        C: CanMutate,
    {
        self.as_erased().set_name(module_token, name)
    }
    pub fn clear_name(self, module_token: &'ctx Module<B, Unverified>)
    where
        C: CanMutate,
    {
        self.as_erased().clear_name(module_token);
    }
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_erased().debug_loc()
    }
    #[inline]
    pub fn as_dyn(self) -> FloatValue<'ctx, FloatDyn, B, C> {
        FloatValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _k: PhantomData,
        }
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> sealed::Sealed
    for FloatValue<'ctx, K, B, C>
{
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> fmt::Display
    for FloatValue<'ctx, K, B, C>
{
    /// Print the operand form `<float-type> <ref>`, identical to what the
    /// erased [`Value`] handle from [`FloatValue::as_erased`] prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&Self::as_erased(*self), f)
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> IsValue<'ctx, B>
    for FloatValue<'ctx, K, B, C>
{
    #[inline]
    fn as_erased(self) -> Value<'ctx, B, C> {
        Self::as_erased(self)
    }
}
impl_into_erased_value_for_handle!(FloatValue[K: FloatKind]);
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B>
    for FloatValue<'ctx, K, B, C>
{
    #[inline]
    fn ty(self) -> Type<'ctx, B, C> {
        self.ty().as_type()
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> HasName<'ctx, B>
    for FloatValue<'ctx, K, B, C>
{
    fn name(self) -> Option<String> {
        Self::name(self)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: CanMutate> SetName<'ctx, B>
    for FloatValue<'ctx, K, B, C>
{
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> HasDebugLoc
    for FloatValue<'ctx, K, B, C>
{
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx, C: Capability> From<FloatValue<'ctx, K, B, C>>
    for Value<'ctx, B, C>
{
    #[inline]
    fn from(v: FloatValue<'ctx, K, B, C>) -> Self {
        v.as_erased()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
    for FloatValue<'ctx, FloatDyn, B, C>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        let ty = v.ty();
        if matches!(
            ty.data(),
            TypeData::Half
                | TypeData::Bfloat
                | TypeData::Float
                | TypeData::Double
                | TypeData::X86Fp80
                | TypeData::Fp128
                | TypeData::PpcFp128
        ) {
            Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
                _k: PhantomData,
            })
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Float,
                got: ty.kind_label(),
            })
        }
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Argument<'ctx, B, C>>
    for FloatValue<'ctx, FloatDyn, B, C>
{
    type Error = IrError;
    fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Constant<'ctx, B, C>>
    for FloatValue<'ctx, FloatDyn, B, C>
{
    type Error = IrError;
    fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
    for FloatValue<'ctx, FloatDyn, B>
{
    type Error = IrError;
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::to_erased(&i))
    }
}

macro_rules! impl_float_value_static_try_from {
    ($marker:ident, $variant:ident, $label:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
            for FloatValue<'ctx, $marker, B, C>
        {
            type Error = IrError;
            fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
                let ty = v.ty();
                match ty.data() {
                    TypeData::$variant => Ok(Self {
                        id: v.id,
                        module: v.module,
                        ty: v.ty,
                        _k: PhantomData,
                    }),
                    _ => Err(IrError::TypeMismatch {
                        expected: TypeKindLabel::$label,
                        got: ty.kind_label(),
                    }),
                }
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Argument<'ctx, B, C>>
            for FloatValue<'ctx, $marker, B, C>
        {
            type Error = IrError;
            fn try_from(a: Argument<'ctx, B, C>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B, C>>>::try_from(a.as_erased())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Constant<'ctx, B, C>>
            for FloatValue<'ctx, $marker, B, C>
        {
            type Error = IrError;
            fn try_from(c: Constant<'ctx, B, C>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B, C>>>::try_from(c.as_erased())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
            for FloatValue<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::to_erased(&i))
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<FloatValue<'ctx, FloatDyn, B, C>>
            for FloatValue<'ctx, $marker, B, C>
        {
            type Error = IrError;
            fn try_from(v: FloatValue<'ctx, FloatDyn, B, C>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B, C>>>::try_from(v.as_erased())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> From<FloatValue<'ctx, $marker, B, C>>
            for FloatValue<'ctx, FloatDyn, B, C>
        {
            #[inline]
            fn from(v: FloatValue<'ctx, $marker, B, C>) -> Self {
                v.as_dyn()
            }
        }
    };
}
impl_float_value_static_try_from!(Half, Half, Half);
impl_float_value_static_try_from!(Bfloat, Bfloat, Bfloat);
impl_float_value_static_try_from!(f32, Float, Float);
impl_float_value_static_try_from!(f64, Double, Double);
impl_float_value_static_try_from!(Fp128, Fp128, Fp128);
impl_float_value_static_try_from!(X86Fp80, X86Fp80, X86Fp80);
impl_float_value_static_try_from!(PpcFp128, PpcFp128, PpcFp128);

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> fmt::Display for Value<'ctx, B, C> {
    /// Print as `<type> <ref>`. Mirrors LLVM's `Value::print`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::asm_writer::fmt_operand(f, *self, None)
    }
}

// --------------------------------------------------------------------------
// IntoPointerValue: ergonomic operand input for the IrBuilder
// --------------------------------------------------------------------------

/// Inputs that can be lifted into a [`PointerValue<'ctx, B>`] operand
/// for the IR builder. Mirrors the int-side
/// [`crate::IntoIntValue`] for the pointer family.
///
/// Implemented by:
/// - [`PointerValue<'ctx, B>`] (identity).
/// - [`crate::ConstantPointerNull<'ctx, B>`] (lift via `null`).
/// - [`crate::TypedPointerValue<'ctx, T, B>`] (drops the schema, identity lift).
///
/// The trait is **sealed**. An erased [`Value`] / `Argument` /
/// `Instruction` no longer lifts silently: narrow it explicitly with
/// [`PointerValue::try_from`] (or [`IsValue`]-erased `_dyn` builders).
pub trait IntoPointerValue<'ctx, B: ModuleBrand>:
    Sized + into_pointer_value_sealed::Sealed
{
    fn into_pointer_value(self, module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>>;
}

/// Seals [`IntoPointerValue`] to the pointer-value handles below.
/// [`TypedPointerValue`](crate::TypedPointerValue) also implements it
/// (its `Sealed` impl lives beside its lift impl).
pub(crate) mod into_pointer_value_sealed {
    pub trait Sealed {}
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> into_pointer_value_sealed::Sealed
    for PointerValue<'ctx, B, C>
{
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> into_pointer_value_sealed::Sealed
    for ConstantPointerNull<'ctx, B, C>
{
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> IntoPointerValue<'ctx, B>
    for PointerValue<'ctx, B, C>
{
    #[inline]
    fn into_pointer_value(self, module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>> {
        // Boundary: refuse a handle minted by another module; one of any
        // capability is admitted and re-minted at `module`'s.
        Ok(PointerValue::from_value_unchecked(
            self.as_erased().admitted_at(module)?,
        ))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> IntoPointerValue<'ctx, B>
    for ConstantPointerNull<'ctx, B, C>
{
    #[inline]
    fn into_pointer_value(self, module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>> {
        // Boundary: refuse a handle minted by another module; one of any
        // capability is admitted and re-minted at `module`'s.
        Ok(PointerValue::from_value_unchecked(
            crate::value::IsValue::as_erased(self).admitted_at(module)?,
        ))
    }
}
