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
//! - **Public handle:** [`Value<'ctx>`] is `(ValueId, ModuleRef<'ctx>,
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

use crate::basic_block::BasicBlockData;
use crate::constant::ConstantData;
use crate::debug_loc::DebugLoc;
use crate::derived_types::{
    ArrayType, FloatType, FunctionType, IntType, PointerType, StructType, VectorType,
};
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::function::FunctionData;
use crate::instruction::InstructionData;
use crate::module::{Module, ModuleRef};
use crate::r#type::{Type, TypeData, TypeId};
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use crate::float_kind::{BFloat, FloatDyn, FloatKind, Fp128, Half, PpcFp128, X86Fp80};
use crate::int_width::{IntDyn, IntWidth};

// --------------------------------------------------------------------------
// ValueId
// --------------------------------------------------------------------------

/// Crate-internal index into the value arena. `NonZeroUsize` so
/// `Option<ValueId>` stays the same size and so `idx + 1` is
/// overflow-free for every realistic input — the only way to wrap is
/// allocating `usize::MAX` values, which exceeds addressable memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ValueId(NonZeroUsize);

impl ValueId {
    /// Build from a 0-based arena index.
    #[inline]
    pub(crate) fn from_index(index: usize) -> Self {
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
    pub(crate) fn arena_index(self) -> usize {
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
pub(crate) struct ValueData {
    pub(crate) ty: TypeId,
    pub(crate) name: RefCell<Option<String>>,
    pub(crate) debug_loc: Option<DebugLoc>,
    pub(crate) kind: ValueKindData,
    /// Reverse-direction use-list: every user (instruction id) that
    /// references this value as one of its SSA operands. Mirrors
    /// LLVM's `Value::use_list_` (`Value.h`). Maintained eagerly:
    /// instruction creation registers entries here for each operand,
    /// `replace_all_uses_with` walks and rewrites them, and
    /// `erase_from_parent` deregisters them.
    ///
    /// User ids may appear more than once if the same instruction
    /// references this value in multiple slots (e.g. `add %x, %x`).
    /// Order is registration-order for determinism.
    pub(crate) use_list: RefCell<Vec<ValueId>>,
}

/// Discriminator over the closed value-category set.
///
/// Variants are populated as the per-kind layers land. Keeping the enum
/// closed (no `#[non_exhaustive]`) is intentional: every IR pass needs
/// to know it has covered every category. New categories require
/// thinking through every handler.
#[derive(Debug)]
pub(crate) enum ValueKindData {
    Constant(ConstantData),
    Argument { parent_fn: ValueId, slot: u32 },
    BasicBlock(BasicBlockData),
    Function(FunctionData),
    Instruction(InstructionData),
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
/// The handle derives `Copy + Clone + PartialEq + Eq + Hash + Debug`
/// without any hand-rolled impls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Value<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

impl<'ctx> Value<'ctx> {
    /// Construct from raw parts. Crate-internal: only the value-arena
    /// constructors hand these out.
    #[inline]
    pub(crate) fn from_parts(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
        }
    }

    /// Borrow the underlying payload via the module's value arena.
    #[inline]
    pub(crate) fn data(self) -> &'ctx ValueData {
        self.module.value_data(self.id)
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    /// Cached IR type of this value.
    #[inline]
    pub fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }

    /// Optional textual name. `None` for slot-numbered (`%0`, `%1`)
    /// values.
    pub fn name(self) -> Option<String> {
        self.data().name.borrow().clone()
    }

    /// Set or clear the textual name. Mirrors `Value::setName`.
    pub fn set_name(self, name: Option<impl Into<String>>) {
        *self.data().name.borrow_mut() = name.map(Into::into);
    }

    /// Read the optional debug-location attached to this value.
    /// Currently always `None` (Phase F wires this in).
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
        }
    }

    /// Snapshot the reverse use-list for this value. Each returned
    /// id is an instruction that references this value as an SSA
    /// operand. Mirrors `Value::users` in `llvm/include/llvm/IR/Value.h`.
    ///
    /// The list is a snapshot, not a live view: callers may mutate
    /// the IR (erase, RAUW) without invalidating the iterator. Order
    /// is registration-order; user ids may appear more than once if
    /// the same instruction references this value in multiple slots.
    pub fn users(
        self,
    ) -> impl ExactSizeIterator<Item = crate::instruction::Instruction<'ctx>> + 'ctx {
        let module = self.module.module();
        let snapshot: Vec<ValueId> = self.data().use_list.borrow().clone();
        snapshot
            .into_iter()
            .map(move |id| crate::instruction::Instruction::from_parts(id, module))
    }

    /// `true` when at least one instruction references this value.
    /// Mirrors `Value::hasUses`. Cheaper than [`Self::users`] for the
    /// common "is this dead?" check.
    #[inline]
    pub fn has_uses(self) -> bool {
        !self.data().use_list.borrow().is_empty()
    }

    /// Number of currently-registered uses. Mirrors `Value::getNumUses`.
    #[inline]
    pub fn num_uses(self) -> usize {
        self.data().use_list.borrow().len()
    }
}

/// Read-only discriminator over the closed value-category set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueCategory {
    Constant,
    Argument,
    BasicBlock,
    Function,
    Instruction,
}

impl From<ValueCategory> for crate::error::ValueCategoryLabel {
    fn from(c: ValueCategory) -> Self {
        match c {
            ValueCategory::Constant => Self::Constant,
            ValueCategory::Argument => Self::Argument,
            ValueCategory::BasicBlock => Self::BasicBlock,
            ValueCategory::Function => Self::Function,
            ValueCategory::Instruction => Self::Instruction,
        }
    }
}

// --------------------------------------------------------------------------
// Sealed marker traits
// --------------------------------------------------------------------------

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// Marker trait implemented by every typed value-handle plus the
/// erased [`Value`] itself.
///
/// Sealed: the closed set of LLVM value categories is part of the IR
/// spec, not an extension point.
pub trait IsValue<'ctx>: sealed::Sealed + Copy + Sized + core::fmt::Debug {
    /// Widen to the erased [`Value`] handle.
    fn as_value(self) -> Value<'ctx>;
}

/// Sealed accessor trait: anything that has an IR type. Implemented by
/// every value handle and every type handle.
pub trait Typed<'ctx>: sealed::Sealed {
    fn ty(self) -> Type<'ctx>;
}

/// Sealed accessor trait: anything that exposes an optional textual
/// name. Implemented by every value handle.
pub trait HasName<'ctx>: sealed::Sealed {
    fn name(self) -> Option<String>;
    fn set_name(self, name: Option<&str>);
}

/// Sealed accessor trait: anything that carries an optional
/// debug-location. Implemented by every value handle.
pub trait HasDebugLoc: sealed::Sealed {
    fn debug_loc(self) -> Option<DebugLoc>;
}

impl<'ctx> sealed::Sealed for Value<'ctx> {}
impl<'ctx> IsValue<'ctx> for Value<'ctx> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        self
    }
}
impl<'ctx> Typed<'ctx> for Value<'ctx> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Value::ty(self)
    }
}
impl<'ctx> HasName<'ctx> for Value<'ctx> {
    #[inline]
    fn name(self) -> Option<String> {
        Value::name(self)
    }
    #[inline]
    fn set_name(self, name: Option<&str>) {
        Value::set_name(self, name);
    }
}
impl HasDebugLoc for Value<'_> {
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
        pub struct $name<'ctx> {
            pub(crate) id: ValueId,
            pub(crate) module: ModuleRef<'ctx>,
            pub(crate) ty: TypeId,
        }

        impl<'ctx> $name<'ctx> {
            /// Widen to the erased [`Value`] handle.
            #[inline]
            pub fn as_value(self) -> Value<'ctx> {
                Value { id: self.id, module: self.module, ty: self.ty }
            }

            /// Owning module reference.
            #[inline]
            pub fn module(self) -> &'ctx Module<'ctx> {
                self.module.module()
            }

            /// Refined IR-type handle for this value.
            #[inline]
            pub fn ty(self) -> $type_handle<'ctx> {
                $type_handle::new(self.ty, self.module.module())
            }

            /// Optional textual name.
            pub fn name(self) -> Option<String> {
                self.as_value().name()
            }

            /// Set or clear the textual name.
            pub fn set_name(self, name: Option<&str>) {
                self.as_value().set_name(name);
            }

            /// Optional debug-location.
            #[inline]
            pub fn debug_loc(self) -> Option<DebugLoc> {
                self.as_value().debug_loc()
            }
        }

        impl<'ctx> sealed::Sealed for $name<'ctx> {}
        impl<'ctx> IsValue<'ctx> for $name<'ctx> {
            #[inline]
            fn as_value(self) -> Value<'ctx> { Self::as_value(self) }
        }
        impl<'ctx> Typed<'ctx> for $name<'ctx> {
            #[inline]
            fn ty(self) -> Type<'ctx> {
                Type::new(self.ty, self.module.module())
            }
        }
        impl<'ctx> HasName<'ctx> for $name<'ctx> {
            #[inline]
            fn name(self) -> Option<String> { Self::name(self) }
            #[inline]
            fn set_name(self, name: Option<&str>) { Self::set_name(self, name) }
        }
        impl<'ctx> HasDebugLoc for $name<'ctx> {
            #[inline]
            fn debug_loc(self) -> Option<DebugLoc> { Self::debug_loc(self) }
        }

        impl<'ctx> From<$name<'ctx>> for Value<'ctx> {
            #[inline]
            fn from(v: $name<'ctx>) -> Self { v.as_value() }
        }

        impl<'ctx> TryFrom<Value<'ctx>> for $name<'ctx> {
            type Error = IrError;
            fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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

        impl<'ctx> TryFrom<crate::argument::Argument<'ctx>> for $name<'ctx> {
            type Error = IrError;
            #[inline]
            fn try_from(a: crate::argument::Argument<'ctx>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(a.as_value())
            }
        }

        impl<'ctx> TryFrom<crate::constant::Constant<'ctx>> for $name<'ctx> {
            type Error = IrError;
            #[inline]
            fn try_from(c: crate::constant::Constant<'ctx>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(c.as_value())
            }
        }

        impl<'ctx> TryFrom<crate::instruction::Instruction<'ctx>> for $name<'ctx> {
            type Error = IrError;
            #[inline]
            fn try_from(i: crate::instruction::Instruction<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(crate::instruction::Instruction::as_value(&i))
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
impl<'ctx> PointerValue<'ctx> {
    /// Crate-internal: wrap a [`Value`] known to have a pointer type.
    /// The IR builder uses this when it just produced a pointer-result
    /// instruction (cast / GEP / alloca / load).
    #[inline]
    pub(crate) fn from_value_unchecked(v: Value<'ctx>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
        }
    }
}

decl_value_handle!(
    /// Value whose type is `[N x T]`.
    ArrayValue, Array, ArrayType,
    type_predicate |d| matches!(d, TypeData::Array { .. })
);
decl_value_handle!(
    /// Value whose type is a struct.
    StructValue, Struct, StructType,
    type_predicate |d| matches!(d, TypeData::Struct(_))
);
decl_value_handle!(
    /// Value whose type is a fixed or scalable vector.
    VectorValue, FixedVector, VectorType,
    type_predicate |d| matches!(
        d,
        TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }
    )
);
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
pub struct IntValue<'ctx, W: IntWidth> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    pub(crate) _w: PhantomData<W>,
}

impl<'ctx, W: IntWidth> Clone for IntValue<'ctx, W> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: IntWidth> Copy for IntValue<'ctx, W> {}
impl<'ctx, W: IntWidth> PartialEq for IntValue<'ctx, W> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, W: IntWidth> Eq for IntValue<'ctx, W> {}
impl<'ctx, W: IntWidth> Hash for IntValue<'ctx, W> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, W: IntWidth> fmt::Debug for IntValue<'ctx, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntValue")
            .field("id", &self.id)
            .field("width", &W::static_bits())
            .finish()
    }
}

impl<'ctx, W: IntWidth> IntValue<'ctx, W> {
    /// Crate-internal: wrap a [`Value`] known to have type `iN` with width `W`.
    /// The IRBuilder uses this when it just produced an instruction whose
    /// type matches the operand widths it validated.
    #[inline]
    pub(crate) fn from_value_unchecked(v: Value<'ctx>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _w: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    /// Owning module reference.
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }
    /// Refined IR-type handle for this value.
    #[inline]
    pub fn ty(self) -> IntType<'ctx, W> {
        IntType::new(self.ty, self.module.module())
    }
    /// Optional textual name.
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }
    /// Set or clear the textual name.
    pub fn set_name(self, name: Option<&str>) {
        self.as_value().set_name(name);
    }
    /// Optional debug-location.
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
    /// Erase the width marker; preserves the runtime width.
    #[inline]
    pub fn as_dyn(self) -> IntValue<'ctx, IntDyn> {
        IntValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _w: PhantomData,
        }
    }
}

impl<'ctx, W: IntWidth> sealed::Sealed for IntValue<'ctx, W> {}
impl<'ctx, W: IntWidth> IsValue<'ctx> for IntValue<'ctx, W> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        Self::as_value(self)
    }
}
impl<'ctx, W: IntWidth> Typed<'ctx> for IntValue<'ctx, W> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }
}
impl<'ctx, W: IntWidth> HasName<'ctx> for IntValue<'ctx, W> {
    #[inline]
    fn name(self) -> Option<String> {
        Self::name(self)
    }
    #[inline]
    fn set_name(self, name: Option<&str>) {
        Self::set_name(self, name)
    }
}
impl<'ctx, W: IntWidth> HasDebugLoc for IntValue<'ctx, W> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, W: IntWidth> From<IntValue<'ctx, W>> for Value<'ctx> {
    #[inline]
    fn from(v: IntValue<'ctx, W>) -> Self {
        v.as_value()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for IntValue<'ctx, IntDyn> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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
impl<'ctx> TryFrom<crate::argument::Argument<'ctx>> for IntValue<'ctx, IntDyn> {
    type Error = IrError;
    #[inline]
    fn try_from(a: crate::argument::Argument<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(a.as_value())
    }
}
impl<'ctx> TryFrom<crate::constant::Constant<'ctx>> for IntValue<'ctx, IntDyn> {
    type Error = IrError;
    #[inline]
    fn try_from(c: crate::constant::Constant<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(c.as_value())
    }
}
impl<'ctx> TryFrom<crate::instruction::Instruction<'ctx>> for IntValue<'ctx, IntDyn> {
    type Error = IrError;
    #[inline]
    fn try_from(i: crate::instruction::Instruction<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(crate::instruction::Instruction::as_value(&i))
    }
}

/// Per-static-width narrowing.
macro_rules! impl_int_value_static_try_from {
    ($marker:ident, $bits:expr) => {
        impl<'ctx> TryFrom<Value<'ctx>> for IntValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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
        impl<'ctx> TryFrom<crate::argument::Argument<'ctx>> for IntValue<'ctx, $marker> {
            type Error = IrError;
            #[inline]
            fn try_from(a: crate::argument::Argument<'ctx>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(a.as_value())
            }
        }
        impl<'ctx> TryFrom<crate::constant::Constant<'ctx>> for IntValue<'ctx, $marker> {
            type Error = IrError;
            #[inline]
            fn try_from(c: crate::constant::Constant<'ctx>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(c.as_value())
            }
        }
        impl<'ctx> TryFrom<crate::instruction::Instruction<'ctx>> for IntValue<'ctx, $marker> {
            type Error = IrError;
            #[inline]
            fn try_from(i: crate::instruction::Instruction<'ctx>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(crate::instruction::Instruction::as_value(
                    &i,
                ))
            }
        }
        impl<'ctx> TryFrom<IntValue<'ctx, IntDyn>> for IntValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(v: IntValue<'ctx, IntDyn>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(v.as_value())
            }
        }
        impl<'ctx> From<IntValue<'ctx, $marker>> for IntValue<'ctx, IntDyn> {
            #[inline]
            fn from(v: IntValue<'ctx, $marker>) -> Self {
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
impl<'ctx, const N: u32> TryFrom<Value<'ctx>> for IntValue<'ctx, crate::int_width::Width<N>> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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
impl<'ctx, const N: u32> TryFrom<crate::argument::Argument<'ctx>>
    for IntValue<'ctx, crate::int_width::Width<N>>
{
    type Error = IrError;
    #[inline]
    fn try_from(a: crate::argument::Argument<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(a.as_value())
    }
}
impl<'ctx, const N: u32> TryFrom<crate::constant::Constant<'ctx>>
    for IntValue<'ctx, crate::int_width::Width<N>>
{
    type Error = IrError;
    #[inline]
    fn try_from(c: crate::constant::Constant<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(c.as_value())
    }
}
impl<'ctx, const N: u32> TryFrom<crate::instruction::Instruction<'ctx>>
    for IntValue<'ctx, crate::int_width::Width<N>>
{
    type Error = IrError;
    #[inline]
    fn try_from(i: crate::instruction::Instruction<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(crate::instruction::Instruction::as_value(&i))
    }
}
impl<'ctx, const N: u32> TryFrom<IntValue<'ctx, IntDyn>>
    for IntValue<'ctx, crate::int_width::Width<N>>
{
    type Error = IrError;
    #[inline]
    fn try_from(v: IntValue<'ctx, IntDyn>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(v.as_value())
    }
}
impl<'ctx, const N: u32> From<IntValue<'ctx, crate::int_width::Width<N>>>
    for IntValue<'ctx, IntDyn>
{
    #[inline]
    fn from(v: IntValue<'ctx, crate::int_width::Width<N>>) -> Self {
        v.as_dyn()
    }
}

// --------------------------------------------------------------------------
// FloatValue<'ctx, K> -- kind-typed floating-point value handle
// --------------------------------------------------------------------------

/// Value whose IR type is an IEEE / non-IEEE float.
pub struct FloatValue<'ctx, K: FloatKind> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    pub(crate) _k: PhantomData<K>,
}

impl<'ctx, K: FloatKind> Clone for FloatValue<'ctx, K> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, K: FloatKind> Copy for FloatValue<'ctx, K> {}
impl<'ctx, K: FloatKind> PartialEq for FloatValue<'ctx, K> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, K: FloatKind> Eq for FloatValue<'ctx, K> {}
impl<'ctx, K: FloatKind> Hash for FloatValue<'ctx, K> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, K: FloatKind> fmt::Debug for FloatValue<'ctx, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FloatValue")
            .field("id", &self.id)
            .field("kind", &K::ieee_label())
            .finish()
    }
}

impl<'ctx, K: FloatKind> FloatValue<'ctx, K> {
    /// Crate-internal: wrap a [`Value`] known to have a float type of kind `K`.
    #[inline]
    pub(crate) fn from_value_unchecked(v: Value<'ctx>) -> Self {
        Self {
            id: v.id,
            module: v.module,
            ty: v.ty,
            _k: PhantomData,
        }
    }

    #[inline]
    pub fn as_value(self) -> Value<'ctx> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }
    #[inline]
    pub fn ty(self) -> FloatType<'ctx, K> {
        FloatType::new(self.ty, self.module.module())
    }
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }
    pub fn set_name(self, name: Option<&str>) {
        self.as_value().set_name(name);
    }
    #[inline]
    pub fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
    #[inline]
    pub fn as_dyn(self) -> FloatValue<'ctx, FloatDyn> {
        FloatValue {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _k: PhantomData,
        }
    }
}

impl<'ctx, K: FloatKind> sealed::Sealed for FloatValue<'ctx, K> {}
impl<'ctx, K: FloatKind> IsValue<'ctx> for FloatValue<'ctx, K> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        Self::as_value(self)
    }
}
impl<'ctx, K: FloatKind> Typed<'ctx> for FloatValue<'ctx, K> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }
}
impl<'ctx, K: FloatKind> HasName<'ctx> for FloatValue<'ctx, K> {
    fn name(self) -> Option<String> {
        Self::name(self)
    }
    fn set_name(self, name: Option<&str>) {
        Self::set_name(self, name)
    }
}
impl<'ctx, K: FloatKind> HasDebugLoc for FloatValue<'ctx, K> {
    fn debug_loc(self) -> Option<DebugLoc> {
        Self::debug_loc(self)
    }
}
impl<'ctx, K: FloatKind> From<FloatValue<'ctx, K>> for Value<'ctx> {
    #[inline]
    fn from(v: FloatValue<'ctx, K>) -> Self {
        v.as_value()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for FloatValue<'ctx, FloatDyn> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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
impl<'ctx> TryFrom<crate::argument::Argument<'ctx>> for FloatValue<'ctx, FloatDyn> {
    type Error = IrError;
    fn try_from(a: crate::argument::Argument<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(a.as_value())
    }
}
impl<'ctx> TryFrom<crate::constant::Constant<'ctx>> for FloatValue<'ctx, FloatDyn> {
    type Error = IrError;
    fn try_from(c: crate::constant::Constant<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(c.as_value())
    }
}
impl<'ctx> TryFrom<crate::instruction::Instruction<'ctx>> for FloatValue<'ctx, FloatDyn> {
    type Error = IrError;
    fn try_from(i: crate::instruction::Instruction<'ctx>) -> IrResult<Self> {
        <Self as TryFrom<Value<'ctx>>>::try_from(crate::instruction::Instruction::as_value(&i))
    }
}

macro_rules! impl_float_value_static_try_from {
    ($marker:ident, $variant:ident, $label:ident) => {
        impl<'ctx> TryFrom<Value<'ctx>> for FloatValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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
        impl<'ctx> TryFrom<crate::argument::Argument<'ctx>> for FloatValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(a: crate::argument::Argument<'ctx>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(a.as_value())
            }
        }
        impl<'ctx> TryFrom<crate::constant::Constant<'ctx>> for FloatValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(c: crate::constant::Constant<'ctx>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(c.as_value())
            }
        }
        impl<'ctx> TryFrom<crate::instruction::Instruction<'ctx>> for FloatValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(i: crate::instruction::Instruction<'ctx>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(crate::instruction::Instruction::as_value(
                    &i,
                ))
            }
        }
        impl<'ctx> TryFrom<FloatValue<'ctx, FloatDyn>> for FloatValue<'ctx, $marker> {
            type Error = IrError;
            fn try_from(v: FloatValue<'ctx, FloatDyn>) -> IrResult<Self> {
                <Self as TryFrom<Value<'ctx>>>::try_from(v.as_value())
            }
        }
        impl<'ctx> From<FloatValue<'ctx, $marker>> for FloatValue<'ctx, FloatDyn> {
            #[inline]
            fn from(v: FloatValue<'ctx, $marker>) -> Self {
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

impl<'ctx> fmt::Display for Value<'ctx> {
    /// Print as `<type> <ref>`. Mirrors LLVM's `Value::print`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::asm_writer::fmt_operand(f, *self, None)
    }
}

// --------------------------------------------------------------------------
// IntoPointerValue: ergonomic operand input for the IRBuilder
// --------------------------------------------------------------------------

/// Inputs that can be lifted into a [`PointerValue<'ctx>`] operand
/// for the IR builder. Mirrors the int-side
/// [`crate::IntoIntValue`] for the pointer family.
///
/// Implemented by:
/// - [`PointerValue<'ctx>`] (identity).
/// - [`crate::ConstantPointerNull<'ctx>`] (lift via `null`).
/// - [`crate::argument::Argument<'ctx>`] (runtime-checked narrow).
/// - [`Value<'ctx>`] (runtime-checked narrow).
/// - [`crate::instruction::Instruction<'ctx>`] (runtime-checked narrow).
pub trait IntoPointerValue<'ctx>: Sized {
    fn into_pointer_value(
        self,
        module: &'ctx crate::module::Module<'ctx>,
    ) -> crate::IrResult<PointerValue<'ctx>>;
}

impl<'ctx> IntoPointerValue<'ctx> for PointerValue<'ctx> {
    #[inline]
    fn into_pointer_value(
        self,
        _module: &'ctx crate::module::Module<'ctx>,
    ) -> crate::IrResult<PointerValue<'ctx>> {
        Ok(self)
    }
}

impl<'ctx> IntoPointerValue<'ctx> for crate::constants::ConstantPointerNull<'ctx> {
    #[inline]
    fn into_pointer_value(
        self,
        _module: &'ctx crate::module::Module<'ctx>,
    ) -> crate::IrResult<PointerValue<'ctx>> {
        Ok(PointerValue::from_value_unchecked(
            crate::value::IsValue::as_value(self),
        ))
    }
}

impl<'ctx> IntoPointerValue<'ctx> for crate::argument::Argument<'ctx> {
    #[inline]
    fn into_pointer_value(
        self,
        _module: &'ctx crate::module::Module<'ctx>,
    ) -> crate::IrResult<PointerValue<'ctx>> {
        PointerValue::try_from(self.as_value())
    }
}

impl<'ctx> IntoPointerValue<'ctx> for Value<'ctx> {
    #[inline]
    fn into_pointer_value(
        self,
        _module: &'ctx crate::module::Module<'ctx>,
    ) -> crate::IrResult<PointerValue<'ctx>> {
        PointerValue::try_from(self)
    }
}

impl<'ctx> IntoPointerValue<'ctx> for crate::instruction::Instruction<'ctx> {
    #[inline]
    fn into_pointer_value(
        self,
        _module: &'ctx crate::module::Module<'ctx>,
    ) -> crate::IrResult<PointerValue<'ctx>> {
        PointerValue::try_from(self.as_value())
    }
}
