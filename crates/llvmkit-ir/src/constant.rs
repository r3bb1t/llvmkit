//! Generic [`Constant`] handle plus the storage payload for every
//! constant kind. Mirrors `llvm/include/llvm/IR/Constant.h` and the
//! constant-data subset of `llvm/include/llvm/IR/Constants.h`.
//!
//! ## Storage shape
//!
//! Like the type-data layer (`crate::Type`'s storage), the constant
//! storage payload is lifetime-free: every cross-reference is a value-
//! arena index into the same
//! module's value arena. Per-kind refinement handles
//! ([`ConstantIntValue`], [`ConstantFloatValue`], ...) live in
//! [`crate::constants`] and follow the same `(ValueId, ModuleRef, ty:
//! TypeId)` layout as the value handles.
//!
//! ## What's shipped
//!
//! Phase B continued:
//! - `Int(magnitude_words)` — arbitrary-precision integer.
//! - `Float(bit_pattern)` — IEEE bit pattern.
//! - `PointerNull` — `ptr null` / `null` for typed pointers.
//! - `Aggregate(elements)` — `ConstantArray`, `ConstantStruct`,
//!   `ConstantVector` element list.
//! - `Undef` / `Poison` — kind-erased markers.
//!
//! The richer shapes (`ConstantExpr`, `BlockAddress`, `TokenNone`,
//! `PtrAuth`, `DSOLocalEquivalent`, `NoCFIValue`, target-extension
//! `none`) land in their own focused sessions per the foundation plan.
//!

//! [`ConstantIntValue`]: crate::constants::ConstantIntValue
//! [`ConstantFloatValue`]: crate::constants::ConstantFloatValue

use crate::module::ModuleRef;
use crate::r#type::{Type, TypeId};
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, sealed};
use crate::{DebugLoc, IrError, IrResult};

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload stored in the value arena under
/// [`ValueKindData::Constant`](crate::value::ValueKindData::Constant).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ConstantData {
    /// Arbitrary-precision integer. Magnitude words are little-endian
    /// (`words[0]` is the least significant 64-bit limb), normalised so
    /// trailing zero limbs are stripped. The sign is encoded by the
    /// owning [`IntType`](crate::IntType): two's-complement
    /// representation in `bit_width` bits is materialised via
    /// `ConstantIntValue::value_zext_u128` / `value_sext_i128`.
    Int(Box<[u64]>),
    /// IEEE bit pattern. Width is determined by the value's
    /// `FloatType`. Stored as a `u128` so every IEEE width up to
    /// `fp128` fits without a discriminant tag.
    Float(u128),
    /// `null` of a pointer or typed-pointer type.
    PointerNull,
    /// Aggregate constant — `ConstantArray`, `ConstantStruct`, or
    /// `ConstantVector`. Element categorisation is determined by the
    /// owning aggregate type.
    Aggregate(Box<[ValueId]>),
    /// `undef` of any first-class type.
    Undef,
    /// `poison` of any first-class type. Distinct from `undef` per
    /// LangRef.
    Poison,
}

// --------------------------------------------------------------------------
// Public erased handle
// --------------------------------------------------------------------------

/// Type-erased constant handle. Mirrors the role of `Constant *` in
/// LLVM C++ — every concrete constant ([`ConstantIntValue`], ...)
/// widens to this handle for storage in operand lists or for analysis
/// passes.
///
/// [`ConstantIntValue`]: crate::constants::ConstantIntValue
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Constant<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

impl<'ctx> Constant<'ctx> {
    /// Construct from raw parts. Crate-internal: only the constant
    /// constructors hand these out.
    #[inline]
    pub(crate) fn from_parts(value: Value<'ctx>) -> Self {
        Self {
            id: value.id,
            module: value.module,
            ty: value.ty,
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

    /// IR type of the constant.
    #[inline]
    pub fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }
}

impl<'ctx> sealed::Sealed for Constant<'ctx> {}
impl<'ctx> IsValue<'ctx> for Constant<'ctx> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        Constant::as_value(self)
    }
}
impl<'ctx> Typed<'ctx> for Constant<'ctx> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Constant::ty(self)
    }
}
impl<'ctx> HasName<'ctx> for Constant<'ctx> {
    #[inline]
    fn name(self) -> Option<String> {
        self.as_value().name()
    }
    #[inline]
    fn set_name(self, name: Option<&str>) {
        self.as_value().set_name(name);
    }
}
impl HasDebugLoc for Constant<'_> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx> From<Constant<'ctx>> for Value<'ctx> {
    #[inline]
    fn from(c: Constant<'ctx>) -> Self {
        c.as_value()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for Constant<'ctx> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
        if let crate::value::ValueKindData::Constant(_) = v.data().kind {
            Ok(Self::from_parts(v))
        } else {
            Err(IrError::ValueCategoryMismatch {
                expected: crate::error::ValueCategoryLabel::Constant,
                got: v.category().into(),
            })
        }
    }
}

// --------------------------------------------------------------------------
// Sealed marker
// --------------------------------------------------------------------------

/// Sealed marker implemented by every per-kind constant refinement
/// (`ConstantIntValue`, `ConstantFloatValue`, ...) plus the erased
/// [`Constant`] itself. Bound generic code with this trait when a
/// function should accept any constant.
pub trait IsConstant<'ctx>: sealed::Sealed + IsValue<'ctx> {
    /// Widen to the erased [`Constant`] handle.
    fn as_constant(self) -> Constant<'ctx>;
}

impl<'ctx> IsConstant<'ctx> for Constant<'ctx> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx> {
        self
    }
}
