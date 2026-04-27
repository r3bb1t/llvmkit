//! Function parameter handle. Mirrors
//! `llvm/include/llvm/IR/Argument.h`.
//!
//! An [`Argument`] is a [`Value`] whose underlying value-arena entry
//! carries the function-parameter category.
//! [`Value`]: crate::Value
//! The handle caches the parent-function id and parameter slot so the
//! common accessors do not round-trip through the value arena.

use crate::function::FunctionValue;
use crate::module::{Module, ModuleRef};
use crate::r#type::{Type, TypeId};
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, ValueKindData, sealed};
use crate::{DebugLoc, IrError, IrResult};

/// Typed handle for a function parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Argument<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    pub(crate) parent_fn: ValueId,
    pub(crate) slot: u32,
}

impl<'ctx> Argument<'ctx> {
    #[inline]
    pub(crate) fn from_parts(
        id: ValueId,
        module: &'ctx Module<'ctx>,
        ty: TypeId,
        parent_fn: ValueId,
        slot: u32,
    ) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            parent_fn,
            slot,
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

    /// Owning function as a runtime-checked [`FunctionValue<Dyn>`].
    /// Narrow with [`TryFrom`] when a typed handle is needed.
    #[inline]
    pub fn parent_function(self) -> FunctionValue<'ctx, crate::marker::Dyn> {
        FunctionValue::<'_, crate::marker::Dyn>::from_parts_unchecked(
            self.parent_fn,
            self.module.module(),
        )
    }

    /// 0-based parameter index.
    #[inline]
    pub fn slot(self) -> u32 {
        self.slot
    }

    /// Parameter type.
    #[inline]
    pub fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }

    /// Optional textual name.
    #[inline]
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }

    /// Set or clear the textual name.
    #[inline]
    pub fn set_name(self, name: Option<&str>) {
        self.as_value().set_name(name);
    }
}

impl<'ctx> sealed::Sealed for Argument<'ctx> {}
impl<'ctx> IsValue<'ctx> for Argument<'ctx> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        Argument::as_value(self)
    }
}
impl<'ctx> Typed<'ctx> for Argument<'ctx> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Argument::ty(self)
    }
}
impl<'ctx> HasName<'ctx> for Argument<'ctx> {
    #[inline]
    fn name(self) -> Option<String> {
        Argument::name(self)
    }
    #[inline]
    fn set_name(self, name: Option<&str>) {
        Argument::set_name(self, name);
    }
}
impl HasDebugLoc for Argument<'_> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx> From<Argument<'ctx>> for Value<'ctx> {
    #[inline]
    fn from(a: Argument<'ctx>) -> Self {
        a.as_value()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for Argument<'ctx> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
        match v.data().kind {
            ValueKindData::Argument { parent_fn, slot } => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
                parent_fn,
                slot,
            }),
            _ => Err(IrError::ValueCategoryMismatch {
                expected: crate::error::ValueCategoryLabel::Argument,
                got: v.category().into(),
            }),
        }
    }
}
