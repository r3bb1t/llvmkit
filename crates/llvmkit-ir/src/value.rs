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
//! - **Public handle:** [`Value<'ctx, B>`] is `(ValueId, ModuleRef<'ctx, B>,
//!   ty: TypeId)`. `ty` is cached so `value.ty()` is a thin wrapper
//!   instead of an arena round-trip — the type of a value is an
//!   immutable property by construction.
//! - **Per-kind handles:** [`IntValue`], [`FloatValue`],
//!   [`PointerValue`], etc. carry the same triple but with the
//!   additional invariant that the wrapped value's *type* belongs to
//!   the matching kind. Bound generic code with the sealed
//!   [`IsValue`] / [`Typed`] / [`HasName`] / [`HasDebugLoc`] traits.

use core::cell::RefCell;
use core::num::NonZeroUsize;

use super::argument::Argument;
use super::basic_block::BasicBlockData;
use super::constant::{Constant, ConstantData};
use super::constants::ConstantPointerNull;
use super::debug_loc::DebugLoc;
use super::derived_types::{
    ArrayType, FloatType, FunctionType, IntType, PointerType, StructType, VectorType,
};
use super::error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use super::function::FunctionData;
use super::instruction::{Instruction, InstructionData, InstructionView, state::Attached};
use super::module::{Brand, Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::struct_body_state::StructBodyDyn;
use super::r#type::{Type, TypeData, TypeId};
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use super::array_len::{ArrLen, ArrLenDyn, ArrayLen};
use super::element::{ElemDyn, StaticVecElem, VecElem};
use super::float_kind::{BFloat, FloatDyn, FloatKind, Fp128, Half, PpcFp128, X86Fp80};
use super::int_width::{IntDyn, IntWidth, Width};
use super::vec_len::{Len, LenDyn, VecLen};

// --------------------------------------------------------------------------
// ValueId
// --------------------------------------------------------------------------

/// Stable index into the value arena. The numeric contents are opaque; callers
/// may store and pass the handle back to this crate, but cannot construct one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(NonZeroUsize);

impl ValueId {
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
                "ValueId arena exhausted: usize::MAX values allocated, exceeds addressable memory"
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
    pub(super) ty: TypeId,
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
    /// in multiple slots (e.g. `add %x, %x`). Order is registration-order for
    /// determinism.
    pub(super) use_list: RefCell<Vec<ValueUse>>,
}

/// One reverse use-list edge for a value. Instruction operands, constants,
/// metadata nodes, and debug records have different mutation paths, so the
/// edge kind is part of the stored fact rather than inferred from a `ValueId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ValueUse {
    Instruction(ValueId),
    Constant(ValueId),
    Metadata(crate::metadata::MetadataId),
    DebugRecord { inst: ValueId, record: usize },
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
        parent_fn: ValueId,
        slot: u32,
    },
    BasicBlock(BasicBlockData),
    Function(Box<FunctionData>),
    Instruction(InstructionData),
    GlobalAlias(crate::global_alias::GlobalAliasData),
    GlobalIFunc(crate::global_ifunc::GlobalIFuncData),
    GlobalVariable(crate::global_variable::GlobalVariableData),
    /// A metadata node used in a value context. Mirrors LLVM's
    /// `MetadataAsValue` (`llvm/include/llvm/IR/Metadata.h`): it lets a
    /// metadata node (e.g. `!0`) appear where a `Value` is expected,
    /// such as a `call` argument of `metadata` type. Like a constant,
    /// it is context-global — it has no function-local SSA definition
    /// and is never assigned a `%N` slot.
    MetadataAsValue(crate::metadata::MetadataId),
    /// An inline-assembly value used as a `call` callee. Mirrors LLVM's
    /// `InlineAsm` (`llvm/include/llvm/IR/InlineAsm.h`). Like a
    /// `Function` or `Constant`, it is context-global — it has no
    /// function-local SSA definition and is never assigned a `%N` slot;
    /// a `call` whose callee is one of these prints the `asm ...` form
    /// instead of an `@name` operand.
    InlineAsm(crate::inline_asm::InlineAsmData),
}

// --------------------------------------------------------------------------
// Public erased handle
// --------------------------------------------------------------------------

/// Erased public handle for any IR value.
///
/// Three-field record:
/// - `id: ValueId` — arena index.
/// - `module: ModuleRef<'ctx>` — brand carrier; equality routes through
///   the process-global [`ModuleId`](crate::ModuleId).
/// - `ty: TypeId` — cached type. Values do not change type, so caching
///   here saves an arena lookup on every `value.ty()` access.
///
/// Equality and hashing compare the branded module reference by `ModuleId`,
/// so the handle remains cheap to copy and store in maps.
pub struct Value<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

impl<B: ModuleBrand> Clone for Value<'_, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: ModuleBrand> Copy for Value<'_, B> {}

impl<B: ModuleBrand> PartialEq for Value<'_, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module
    }
}

impl<B: ModuleBrand> Eq for Value<'_, B> {}

impl<B: ModuleBrand> Hash for Value<'_, B> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.module.hash(state);
        self.ty.hash(state);
    }
}

impl<B: ModuleBrand> fmt::Debug for Value<'_, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Value")
            .field("id", &self.id)
            .field("ty", &self.ty)
            .finish()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Value<'ctx, B> {
    /// Construct from raw parts. Crate-internal: only the value-arena
    /// constructors hand these out.
    #[inline]
    pub(super) fn from_parts<M>(id: ValueId, module: M, ty: TypeId) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
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

    /// Opaque arena id for structured side tables such as use-list order
    /// records.
    #[inline]
    pub fn id(self) -> ValueId {
        self.id
    }

    /// Cached IR type of this value.
    #[inline]
    pub fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }

    /// Optional textual name. `None` for slot-numbered (`%0`, `%1`)
    /// values.
    pub fn name(self) -> Option<String> {
        self.data().name.borrow().clone()
    }

    /// Set the textual name. Mirrors `Value::setName`.
    pub fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        if module_token.id() != self.module.id() {
            return;
        }
        let requested = name.into();
        if self.ty().is_void() {
            self.set_name_internal(None);
            return;
        }
        if let Some(parent_fn_id) = self.local_parent_function_id() {
            let parent_fn =
                crate::function::FunctionValue::<crate::marker::Dyn, B>::from_parts_unchecked(
                    parent_fn_id,
                    self.module,
                );
            parent_fn.set_local_value_name(self.id, Some(requested.as_str()));
            return;
        }
        if self.is_parentless_local_nameable() {
            self.set_name_internal((!requested.is_empty()).then_some(requested));
        }
    }

    /// Clear the textual name.
    pub fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        if module_token.id() != self.module.id() {
            return;
        }
        if self.ty().is_void() {
            self.set_name_internal(None);
            return;
        }
        if let Some(parent_fn_id) = self.local_parent_function_id() {
            let parent_fn =
                crate::function::FunctionValue::<crate::marker::Dyn, B>::from_parts_unchecked(
                    parent_fn_id,
                    self.module,
                );
            parent_fn.set_local_value_name(self.id, None);
            return;
        }
        if self.is_parentless_local_nameable() {
            self.set_name_internal(None);
        }
    }

    /// Raw assignment for already-uniqued names and parentless fabrication.
    /// Do not use this for attached local values until their owning
    /// `ValueSymbolTable` has returned the final unique name.
    pub(super) fn set_name_internal(self, name: Option<String>) {
        *self.data().name.borrow_mut() = name;
    }

    pub(super) fn local_parent_function_id(self) -> Option<ValueId> {
        match &self.data().kind {
            ValueKindData::Argument { parent_fn, .. } => Some(*parent_fn),
            ValueKindData::BasicBlock(data) => *data.parent.borrow(),
            ValueKindData::Instruction(data) => {
                let parent_block_id = data.parent.get();
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
            | ValueKindData::GlobalIFunc(_)
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
            ValueKindData::GlobalIFunc(_) => ValueCategory::GlobalIFunc,
            ValueKindData::MetadataAsValue(_) => ValueCategory::MetadataAsValue,
            ValueKindData::InlineAsm(_) => ValueCategory::InlineAsm,
        }
    }

    /// Snapshot the read-only instruction views that use this value. Metadata
    /// and debug-record uses are tracked structurally and counted by
    /// [`Self::num_uses`], but are intentionally omitted here because callers of
    /// `users()` expect concrete instruction views.
    ///
    /// The list is a snapshot, not a live view: callers may mutate the IR
    /// (erase, RAUW) without invalidating the iterator. Order is
    /// registration-order; user ids may appear more than once if the same
    /// instruction references this value in multiple slots.
    pub fn users(self) -> impl ExactSizeIterator<Item = InstructionView<'ctx, B>> + 'ctx {
        let module = self.module;
        let snapshot: Vec<ValueId> = self
            .data()
            .use_list
            .borrow()
            .iter()
            .filter_map(|edge| match edge {
                ValueUse::Instruction(id) => Some(*id),
                ValueUse::Constant(_) | ValueUse::Metadata(_) | ValueUse::DebugRecord { .. } => {
                    None
                }
            })
            .collect();
        snapshot
            .into_iter()
            .map(move |id| InstructionView::from_parts(id, module))
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

    /// If this value is a constant integer, its arbitrary-precision value.
    /// Mirrors reading a `ConstantInt`'s `getValue()`; backs the matcher
    /// constant predicates (`m_zero`, `m_all_ones`, `m_ap_int`, ...).
    /// Scalar only — vector splats are not unwrapped here.
    pub fn as_const_int(self) -> Option<crate::ap_int::ApInt> {
        let constant = Constant::try_from(self).ok()?;
        let int: crate::constants::ConstantIntValue<'_, crate::int_width::IntDyn, B> =
            crate::constants::ConstantIntValue::try_from(constant).ok()?;
        Some(int.ap_int())
    }
}

pub enum ValueCategory {
    Constant,
    Argument,
    BasicBlock,
    Function,
    Instruction,
    GlobalVariable,
    GlobalAlias,
    GlobalIFunc,
    MetadataAsValue,
    InlineAsm,
}

impl From<ValueCategory> for crate::error::ValueCategoryLabel {
    fn from(c: ValueCategory) -> Self {
        match c {
            ValueCategory::Constant => Self::Constant,
            ValueCategory::Argument => Self::Argument,
            ValueCategory::BasicBlock => Self::BasicBlock,
            ValueCategory::Function => Self::Function,
            ValueCategory::Instruction => Self::Instruction,
            ValueCategory::GlobalVariable => Self::GlobalVariable,
            ValueCategory::GlobalAlias => Self::GlobalAlias,
            ValueCategory::GlobalIFunc => Self::GlobalIFunc,
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
        ValueKindData::GlobalIFunc(_) => ValueCategoryLabel::GlobalIFunc,
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
pub trait IsValue<'ctx, B: ModuleBrand = Brand<'ctx>>:
    sealed::Sealed + Copy + Sized + core::fmt::Debug
{
    /// Widen to the erased [`Value`] handle.
    fn as_value(self) -> Value<'ctx, B>;
}

/// Sealed accessor trait: anything that has an IR type. Implemented by
/// every value handle and every type handle.
pub trait Typed<'ctx, B: ModuleBrand = Brand<'ctx>>: sealed::Sealed {
    fn ty(self) -> Type<'ctx, B>;
}

/// Sealed accessor trait: anything that exposes an optional textual
/// name. Implemented by every value handle.
pub trait HasName<'ctx, B: ModuleBrand = Brand<'ctx>>: sealed::Sealed {
    fn name(self) -> Option<String>;
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>;
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>);
}

/// Sealed accessor trait: anything that carries an optional
/// debug-location. Implemented by every value handle.
pub trait HasDebugLoc: sealed::Sealed {
    fn debug_loc(self) -> Option<DebugLoc>;
}

impl<'ctx, B: ModuleBrand> sealed::Sealed for Value<'ctx, B> {}
impl<'ctx, B: ModuleBrand> IsValue<'ctx, B> for Value<'ctx, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        self
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for Value<'ctx, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Value::ty(self)
    }
}
impl<'ctx, B: ModuleBrand> HasName<'ctx, B> for Value<'ctx, B> {
    #[inline]
    fn name(self) -> Option<String> {
        Value::name(self)
    }
    #[inline]
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        Value::set_name(self, module_token, name);
    }
    #[inline]
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        Value::clear_name(self, module_token);
    }
}
impl<B: ModuleBrand> HasDebugLoc for Value<'_, B> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Value::debug_loc(self)
    }
}

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
        $type_label:ident,
        $type_handle:ident,
        type_predicate $pred:expr
    ) => {
        $(#[$attr])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name<'ctx, B: ModuleBrand = Brand<'ctx>> {
            pub(super) id: ValueId,
            pub(super) module: ModuleRef<'ctx, B>,
            pub(super) ty: TypeId,
        }

        impl<'ctx, B: ModuleBrand + 'ctx> $name<'ctx, B> {
            /// Widen to the erased [`Value`] handle.
            #[inline]
            pub fn as_value(self) -> Value<'ctx, B> {
                Value { id: self.id, module: self.module, ty: self.ty }
            }

            /// Owning module reference.
            #[inline]
            pub fn module(self) -> ModuleView<'ctx, B> {
                ModuleView::new(self.module.module())
            }

            /// Refined IR-type handle for this value.
            #[inline]
            pub fn ty(self) -> $type_handle<'ctx, B> {
                $type_handle::new(self.ty, self.module)
            }

            /// Optional textual name.
            pub fn name(self) -> Option<String> {
                self.as_value().name()
            }

            /// Set the textual name.
            pub fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
            where
                Name: Into<String>,
            {
                self.as_value().set_name(module_token, name);
            }

            /// Clear the textual name.
            pub fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
                self.as_value().clear_name(module_token);
            }

            /// Optional debug-location.
            #[inline]
            pub fn debug_loc(self) -> Option<DebugLoc> {
                self.as_value().debug_loc()
            }
        }

        impl<'ctx, B: ModuleBrand + 'ctx> sealed::Sealed for $name<'ctx, B> {}
        impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for $name<'ctx, B> {
            #[inline]
            fn as_value(self) -> Value<'ctx, B> { Self::as_value(self) }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for $name<'ctx, B> {
            #[inline]
            fn ty(self) -> Type<'ctx, B> {
                self.ty().as_type()
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> HasName<'ctx, B> for $name<'ctx, B> {
            #[inline]
            fn name(self) -> Option<String> { Self::name(self) }
            #[inline]
            fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
            where
                Name: Into<String>,
            {
                Self::set_name(self, module_token, name)
            }
            #[inline]
            fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
                Self::clear_name(self, module_token)
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> HasDebugLoc for $name<'ctx, B> {
            #[inline]
            fn debug_loc(self) -> Option<DebugLoc> { Self::debug_loc(self) }
        }

        impl<'ctx, B: ModuleBrand + 'ctx> From<$name<'ctx, B>> for Value<'ctx, B> {
            #[inline]
            fn from(v: $name<'ctx, B>) -> Self { v.as_value() }
        }

        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for $name<'ctx, B> {
            type Error = IrError;
            fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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

        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Argument<'ctx, B>>
            for $name<'ctx, B>
        {
            type Error = IrError;
            #[inline]
            fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
            }
        }

        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>>
            for $name<'ctx, B>
        {
            type Error = IrError;
            #[inline]
            fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
            }
        }

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
                    Instruction::as_value(&i),
                )
            }
        }
    };
}

// `IntValue<'ctx, W>` and `FloatValue<'ctx, K>` are hand-written below to
// carry their width / kind markers.
decl_value_handle!(
    /// Value whose type is a (opaque) pointer.
    PointerValue, Pointer, PointerType,
    type_predicate |d| matches!(d, TypeData::Pointer { .. })
);
impl<'ctx, B: ModuleBrand + 'ctx> PointerValue<'ctx, B> {
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
    pub(super) fn from_value_unchecked(v: Value<'ctx, B>) -> Self {
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
pub struct ArrayValue<
    'ctx,
    E: VecElem = ElemDyn,
    L: ArrayLen = ArrLenDyn,
    B: ModuleBrand = Brand<'ctx>,
> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    pub(super) _e: PhantomData<E>,
    pub(super) _l: PhantomData<L>,
}

impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> Clone for ArrayValue<'ctx, E, L, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> Copy for ArrayValue<'ctx, E, L, B> {}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> PartialEq for ArrayValue<'ctx, E, L, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> Eq for ArrayValue<'ctx, E, L, B> {}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> Hash for ArrayValue<'ctx, E, L, B> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> fmt::Debug
    for ArrayValue<'ctx, E, L, B>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArrayValue").field("id", &self.id).finish()
    }
}

impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> ArrayValue<'ctx, E, L, B> {
    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
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
    /// Refined IR-type handle for this value.
    #[inline]
    pub fn ty(self) -> ArrayType<'ctx, E, L, B> {
        ArrayType::new(self.ty, self.module)
    }
    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }
    /// Set the textual name.
    pub fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.as_value().set_name(module_token, name);
    }
    /// Clear the textual name.
    pub fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
    }
    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
    /// Erase both markers; preserves the runtime element type / element count.
    #[inline]
    pub fn as_dyn(self) -> ArrayValue<'ctx, ElemDyn, ArrLenDyn, B> {
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
    pub(super) fn from_value_unchecked(v: Value<'ctx, B>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _e: PhantomData,
            _l: PhantomData,
        }
    }
}

impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> sealed::Sealed
    for ArrayValue<'ctx, E, L, B>
{
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> IsValue<'ctx, B>
    for ArrayValue<'ctx, E, L, B>
{
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        Self::as_value(self)
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> Typed<'ctx, B>
    for ArrayValue<'ctx, E, L, B>
{
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        self.ty().as_type()
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> HasName<'ctx, B>
    for ArrayValue<'ctx, E, L, B>
{
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
    #[inline]
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> HasDebugLoc
    for ArrayValue<'ctx, E, L, B>
{
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, E: VecElem, L: ArrayLen, B: ModuleBrand + 'ctx> From<ArrayValue<'ctx, E, L, B>>
    for Value<'ctx, B>
{
    #[inline]
    fn from(v: ArrayValue<'ctx, E, L, B>) -> Self {
        v.as_value()
    }
}

// Erased narrowing: any array value lands in the fully dynamic form.
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Argument<'ctx, B>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::as_value(&i))
    }
}

/// Typed narrowing: a `Value` accepts into `ArrayValue<'ctx, E, ArrLen<N>>`
/// only if it is an array whose element type is exactly `E`'s projection and
/// whose element count is exactly `N`. Element mismatch reports
/// [`IrError::TypeMismatch`] (the element kinds); length mismatch reports
/// [`IrError::ArrayLengthMismatch`] — a `u64`-shaped variant, since array
/// lengths do not fit the `u32` `OperandWidthMismatch` the sibling
/// `VectorValue` narrowing uses for its lane count.
impl<'ctx, E, const N: u64, B> TryFrom<Value<'ctx, B>> for ArrayValue<'ctx, E, ArrLen<N>, B>
where
    E: StaticVecElem<'ctx, B>,
    B: ModuleBrand + 'ctx,
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        let ty = v.ty();
        match ty.data() {
            TypeData::Array { elem, n } => {
                let expected_elem = E::element_ir_type(v.module);
                if *elem != expected_elem.id() {
                    return Err(IrError::TypeMismatch {
                        expected: expected_elem.kind_label(),
                        got: Type::new(*elem, v.module).kind_label(),
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
impl<'ctx, E: VecElem, const N: u64, B: ModuleBrand + 'ctx> From<ArrayValue<'ctx, E, ArrLen<N>, B>>
    for ArrayValue<'ctx, ElemDyn, ArrLenDyn, B>
{
    #[inline]
    fn from(v: ArrayValue<'ctx, E, ArrLen<N>, B>) -> Self {
        v.as_dyn()
    }
}

/// Value whose type is a struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StructValue<'ctx, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
}

impl<'ctx, B: ModuleBrand + 'ctx> StructValue<'ctx, B> {
    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
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
    pub(crate) fn from_value_unchecked(v: Value<'ctx, B>) -> Self {
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

    /// Refined IR-type handle for this value.
    #[inline]
    pub fn ty(self) -> StructType<'ctx, StructBodyDyn, B> {
        StructType::new(self.ty, self.module)
    }

    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }

    /// Set the textual name.
    pub fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.as_value().set_name(module_token, name);
    }

    /// Clear the textual name.
    pub fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
    }

    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> sealed::Sealed for StructValue<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for StructValue<'ctx, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        Self::as_value(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for StructValue<'ctx, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        self.ty().as_type()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasName<'ctx, B> for StructValue<'ctx, B> {
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
    #[inline]
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasDebugLoc for StructValue<'ctx, B> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<StructValue<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(v: StructValue<'ctx, B>) -> Self {
        v.as_value()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for StructValue<'ctx, B> {
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Argument<'ctx, B>> for StructValue<'ctx, B> {
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>> for StructValue<'ctx, B> {
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>> for StructValue<'ctx, B> {
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::as_value(&i))
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
pub struct VectorValue<'ctx, E: VecElem = ElemDyn, L: VecLen = LenDyn, B: ModuleBrand = Brand<'ctx>>
{
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    pub(super) _e: PhantomData<E>,
    pub(super) _l: PhantomData<L>,
}

impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> Clone for VectorValue<'ctx, E, L, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> Copy for VectorValue<'ctx, E, L, B> {}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> PartialEq for VectorValue<'ctx, E, L, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> Eq for VectorValue<'ctx, E, L, B> {}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> Hash for VectorValue<'ctx, E, L, B> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> fmt::Debug for VectorValue<'ctx, E, L, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VectorValue").field("id", &self.id).finish()
    }
}

impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> VectorValue<'ctx, E, L, B> {
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
    pub(super) fn from_value_unchecked(v: Value<'ctx, B>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _e: PhantomData,
            _l: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
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
    /// Refined IR-type handle for this value.
    #[inline]
    pub fn ty(self) -> VectorType<'ctx, E, L, B> {
        VectorType::new(self.ty, self.module)
    }
    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }
    /// Set the textual name.
    pub fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.as_value().set_name(module_token, name);
    }
    /// Clear the textual name.
    pub fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
    }
    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
    /// Erase both markers; preserves the runtime element type / lane count.
    #[inline]
    pub fn as_dyn(self) -> VectorValue<'ctx, ElemDyn, LenDyn, B> {
        VectorValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _e: PhantomData,
            _l: PhantomData,
        }
    }
}

impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> sealed::Sealed
    for VectorValue<'ctx, E, L, B>
{
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> IsValue<'ctx, B>
    for VectorValue<'ctx, E, L, B>
{
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        Self::as_value(self)
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> Typed<'ctx, B>
    for VectorValue<'ctx, E, L, B>
{
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        self.ty().as_type()
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> HasName<'ctx, B>
    for VectorValue<'ctx, E, L, B>
{
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
    #[inline]
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> HasDebugLoc
    for VectorValue<'ctx, E, L, B>
{
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, E: VecElem, L: VecLen, B: ModuleBrand + 'ctx> From<VectorValue<'ctx, E, L, B>>
    for Value<'ctx, B>
{
    #[inline]
    fn from(v: VectorValue<'ctx, E, L, B>) -> Self {
        v.as_value()
    }
}

// Erased narrowing: any vector value lands in the fully dynamic form.
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Argument<'ctx, B>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::as_value(&i))
    }
}

/// Typed narrowing: a `Value` accepts into `VectorValue<'ctx, E, Len<N>>`
/// only if it is a **fixed** vector whose element type is exactly `E`'s
/// projection and whose lane count is exactly `N`. Element mismatch reports
/// [`IrError::TypeMismatch`] (the element kinds); lane-count mismatch
/// reports [`IrError::OperandWidthMismatch`] (the "vector length" arm of
/// that variant's doc). Scalable vectors — whose lane count is a runtime
/// multiple, not a fixed `N` — never narrow to `Len<N>`.
impl<'ctx, E, const N: u32, B> TryFrom<Value<'ctx, B>> for VectorValue<'ctx, E, Len<N>, B>
where
    E: StaticVecElem<'ctx, B>,
    B: ModuleBrand + 'ctx,
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        let ty = v.ty();
        match ty.data() {
            TypeData::FixedVector { elem, n } => {
                let expected_elem = E::element_ir_type(v.module);
                if *elem != expected_elem.id() {
                    return Err(IrError::TypeMismatch {
                        expected: expected_elem.kind_label(),
                        got: Type::new(*elem, v.module).kind_label(),
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
impl<'ctx, E: VecElem, const N: u32, B: ModuleBrand + 'ctx> From<VectorValue<'ctx, E, Len<N>, B>>
    for VectorValue<'ctx, ElemDyn, LenDyn, B>
{
    #[inline]
    fn from(v: VectorValue<'ctx, E, Len<N>, B>) -> Self {
        v.as_dyn()
    }
}

decl_value_handle!(
    /// Value whose type is a function signature. Mostly seen as a
    /// `FunctionValue` operand, but the concrete category is checked
    /// elsewhere; this handle only refines the type, not the category.
    FunctionTypedValue, Function, FunctionType,
    type_predicate |d| matches!(d, TypeData::Function { .. })
);

// --------------------------------------------------------------------------
// IntValue<'ctx, W> -- width-typed integer value handle
// --------------------------------------------------------------------------

// IntType / FloatType already imported at top of file.

/// Value whose IR type is `iN`. The `W: IntWidth` marker pins the
/// bit-width at the type level, so the IRBuilder can reject mismatched
/// widths at compile time.
pub struct IntValue<'ctx, W: IntWidth, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    pub(super) _w: PhantomData<W>,
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Clone for IntValue<'ctx, W, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Copy for IntValue<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> PartialEq for IntValue<'ctx, W, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Eq for IntValue<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Hash for IntValue<'ctx, W, B> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> fmt::Debug for IntValue<'ctx, W, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntValue")
            .field("id", &self.id)
            .field("width", &W::static_bits())
            .finish()
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IntValue<'ctx, W, B> {
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
    pub(crate) fn from_value_unchecked(v: Value<'ctx, B>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _w: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
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
    /// Refined IR-type handle for this value.
    #[inline]
    pub fn ty(self) -> IntType<'ctx, W, B> {
        IntType::new(self.ty, self.module)
    }
    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }
    /// Set the textual name.
    pub fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.as_value().set_name(module_token, name);
    }
    /// Clear the textual name.
    pub fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
    }
    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
    /// Erase the width marker; preserves the runtime width.
    #[inline]
    pub fn as_dyn(self) -> IntValue<'ctx, IntDyn, B> {
        IntValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _w: PhantomData,
        }
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> sealed::Sealed for IntValue<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for IntValue<'ctx, W, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        Self::as_value(self)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Typed<'ctx, B> for IntValue<'ctx, W, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        self.ty().as_type()
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> HasName<'ctx, B> for IntValue<'ctx, W, B> {
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
    #[inline]
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> HasDebugLoc for IntValue<'ctx, W, B> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> From<IntValue<'ctx, W, B>> for Value<'ctx, B> {
    #[inline]
    fn from(v: IntValue<'ctx, W, B>) -> Self {
        v.as_value()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for IntValue<'ctx, IntDyn, B> {
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Argument<'ctx, B>> for IntValue<'ctx, IntDyn, B> {
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>> for IntValue<'ctx, IntDyn, B> {
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
    for IntValue<'ctx, IntDyn, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::as_value(&i))
    }
}

/// Per-static-width narrowing.
macro_rules! impl_int_value_static_try_from {
    ($marker:ident, $bits:expr) => {
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for IntValue<'ctx, $marker, B> {
            type Error = IrError;
            fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Argument<'ctx, B>>
            for IntValue<'ctx, $marker, B>
        {
            type Error = IrError;
            #[inline]
            fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>>
            for IntValue<'ctx, $marker, B>
        {
            type Error = IrError;
            #[inline]
            fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
            for IntValue<'ctx, $marker, B>
        {
            type Error = IrError;
            #[inline]
            fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::as_value(&i))
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<IntValue<'ctx, IntDyn, B>>
            for IntValue<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(v: IntValue<'ctx, IntDyn, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(v.as_value())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> From<IntValue<'ctx, $marker, B>>
            for IntValue<'ctx, IntDyn, B>
        {
            #[inline]
            fn from(v: IntValue<'ctx, $marker, B>) -> Self {
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
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> TryFrom<Value<'ctx, B>>
    for IntValue<'ctx, Width<N>, B>
{
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> TryFrom<Argument<'ctx, B>>
    for IntValue<'ctx, Width<N>, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> TryFrom<Constant<'ctx, B>>
    for IntValue<'ctx, Width<N>, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> TryFrom<Instruction<'ctx, Attached, B>>
    for IntValue<'ctx, Width<N>, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::as_value(&i))
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> TryFrom<IntValue<'ctx, IntDyn, B>>
    for IntValue<'ctx, Width<N>, B>
{
    type Error = IrError;
    #[inline]
    fn try_from(v: IntValue<'ctx, IntDyn, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(v.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, const N: u32> From<IntValue<'ctx, Width<N>, B>>
    for IntValue<'ctx, IntDyn, B>
{
    #[inline]
    fn from(v: IntValue<'ctx, Width<N>, B>) -> Self {
        v.as_dyn()
    }
}

// --------------------------------------------------------------------------
// FloatValue<'ctx, K> -- kind-typed floating-point value handle
// --------------------------------------------------------------------------

/// Value whose IR type is an IEEE / non-IEEE float.
pub struct FloatValue<'ctx, K: FloatKind, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    pub(super) _k: PhantomData<K>,
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Clone for FloatValue<'ctx, K, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Copy for FloatValue<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> PartialEq for FloatValue<'ctx, K, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Eq for FloatValue<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Hash for FloatValue<'ctx, K, B> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> fmt::Debug for FloatValue<'ctx, K, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FloatValue")
            .field("id", &self.id)
            .field("kind", &K::ieee_label())
            .finish()
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> FloatValue<'ctx, K, B> {
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
    pub(crate) fn from_value_unchecked(v: Value<'ctx, B>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _k: PhantomData,
        }
    }

    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }
    #[inline]
    pub fn ty(self) -> FloatType<'ctx, K, B> {
        FloatType::new(self.ty, self.module)
    }
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }
    pub fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.as_value().set_name(module_token, name);
    }
    pub fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
    }
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
    #[inline]
    pub fn as_dyn(self) -> FloatValue<'ctx, FloatDyn, B> {
        FloatValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _k: PhantomData,
        }
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> sealed::Sealed for FloatValue<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for FloatValue<'ctx, K, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        Self::as_value(self)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Typed<'ctx, B> for FloatValue<'ctx, K, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        self.ty().as_type()
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> HasName<'ctx, B> for FloatValue<'ctx, K, B> {
    fn name(self) -> Option<String> {
        Self::name(self)
    }
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        Self::set_name(self, module_token, name)
    }
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        Self::clear_name(self, module_token)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> HasDebugLoc for FloatValue<'ctx, K, B> {
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> From<FloatValue<'ctx, K, B>> for Value<'ctx, B> {
    #[inline]
    fn from(v: FloatValue<'ctx, K, B>) -> Self {
        v.as_value()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for FloatValue<'ctx, FloatDyn, B> {
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        let ty = v.ty();
        if matches!(
            ty.data(),
            TypeData::Half
                | TypeData::BFloat
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
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Argument<'ctx, B>> for FloatValue<'ctx, FloatDyn, B> {
    type Error = IrError;
    fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>> for FloatValue<'ctx, FloatDyn, B> {
    type Error = IrError;
    fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
    for FloatValue<'ctx, FloatDyn, B>
{
    type Error = IrError;
    fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::as_value(&i))
    }
}

macro_rules! impl_float_value_static_try_from {
    ($marker:ident, $variant:ident, $label:ident) => {
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for FloatValue<'ctx, $marker, B> {
            type Error = IrError;
            fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Argument<'ctx, B>>
            for FloatValue<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(a: Argument<'ctx, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(a.as_value())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Constant<'ctx, B>>
            for FloatValue<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(c: Constant<'ctx, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(c.as_value())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Instruction<'ctx, Attached, B>>
            for FloatValue<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(i: Instruction<'ctx, Attached, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(Instruction::as_value(&i))
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<FloatValue<'ctx, FloatDyn, B>>
            for FloatValue<'ctx, $marker, B>
        {
            type Error = IrError;
            fn try_from(v: FloatValue<'ctx, FloatDyn, B>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx, B>>>::try_from(v.as_value())
            }
        }
        impl<'ctx, B: ModuleBrand + 'ctx> From<FloatValue<'ctx, $marker, B>>
            for FloatValue<'ctx, FloatDyn, B>
        {
            #[inline]
            fn from(v: FloatValue<'ctx, $marker, B>) -> Self {
                v.as_dyn()
            }
        }
    };
}
impl_float_value_static_try_from!(Half, Half, Half);
impl_float_value_static_try_from!(BFloat, BFloat, BFloat);
impl_float_value_static_try_from!(f32, Float, Float);
impl_float_value_static_try_from!(f64, Double, Double);
impl_float_value_static_try_from!(Fp128, Fp128, Fp128);
impl_float_value_static_try_from!(X86Fp80, X86Fp80, X86Fp80);
impl_float_value_static_try_from!(PpcFp128, PpcFp128, PpcFp128);

impl<'ctx, B: ModuleBrand + 'ctx> fmt::Display for Value<'ctx, B> {
    /// Print as `<type> <ref>`. Mirrors LLVM's `Value::print`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::asm_writer::fmt_operand(f, *self, None)
    }
}

// --------------------------------------------------------------------------
// IntoPointerValue: ergonomic operand input for the IRBuilder
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
/// The trait is **sealed**. An erased [`Value`] / [`Argument`] /
/// [`Instruction`] no longer lifts silently: narrow it explicitly with
/// [`PointerValue::try_from`] (or [`IsValue`]-erased `_dyn` builders).
pub trait IntoPointerValue<'ctx, B: ModuleBrand = Brand<'ctx>>:
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

impl<'ctx, B: ModuleBrand + 'ctx> into_pointer_value_sealed::Sealed for PointerValue<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> into_pointer_value_sealed::Sealed
    for ConstantPointerNull<'ctx, B>
{
}

impl<'ctx, B: ModuleBrand + 'ctx> IntoPointerValue<'ctx, B> for PointerValue<'ctx, B> {
    #[inline]
    fn into_pointer_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>> {
        Ok(self)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> IntoPointerValue<'ctx, B> for ConstantPointerNull<'ctx, B> {
    #[inline]
    fn into_pointer_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>> {
        Ok(PointerValue::from_value_unchecked(
            crate::value::IsValue::as_value(self),
        ))
    }
}
