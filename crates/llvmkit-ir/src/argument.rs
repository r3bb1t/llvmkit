//! Function parameter handle. Mirrors
//! `llvm/include/llvm/IR/Argument.h`.
//!
//! An [`Argument`] is a [`Value`] whose underlying value-arena entry
//! carries the function-parameter category.
//! [`Value`]: crate::Value
//! The handle caches the parent-function id and parameter slot so the
//! common accessors do not round-trip through the value arena.

use super::function::FunctionValue;
use super::marker::Dyn;
use super::module::{Module, ModuleBrand, ModuleRef, Unverified};
use super::r#type::{Type, TypeId};
use super::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, ValueKindData, sealed};
use super::{DebugLoc, IrError, IrResult};

/// Typed handle for a function parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Argument<'ctx, B: crate::module::ModuleBrand = crate::module::Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    pub(super) parent_fn: ValueId,
    pub(super) slot: u32,
}

impl<'ctx, B: ModuleBrand + 'ctx> Argument<'ctx, B> {
    #[inline]
    pub(super) fn from_parts<M>(
        id: ValueId,
        module: M,
        ty: TypeId,
        parent_fn: ValueId,
        slot: u32,
    ) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            parent_fn,
            slot,
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

    /// Owning function as a runtime-checked [`FunctionValue<Dyn>`].
    /// Narrow with [`TryFrom`] when a typed handle is needed.
    #[inline]
    pub fn parent_function(self) -> FunctionValue<'ctx, Dyn, B> {
        FunctionValue::from_parts_unchecked(self.parent_fn, self.module)
    }

    /// 0-based parameter index.
    #[inline]
    pub fn slot(self) -> u32 {
        self.slot
    }

    /// Parameter type.
    #[inline]
    pub fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }

    /// Optional textual name.
    #[inline]
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }

    /// Set the textual name.
    #[inline]
    pub fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.as_value().set_name(module_token, name);
    }

    /// Clear the textual name.
    #[inline]
    pub fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> core::fmt::Display for Argument<'ctx, B> {
    /// Print the operand form `<type> %name`, identical to what the erased
    /// [`Argument::as_value`] handle prints. An unnamed parameter has no
    /// slot number outside a function-wide numbering pass, so it prints as
    /// `%<unnumbered>` here.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&Argument::as_value(*self), f)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> sealed::Sealed for Argument<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for Argument<'ctx, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        Argument::as_value(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for Argument<'ctx, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Argument::ty(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasName<'ctx, B> for Argument<'ctx, B> {
    #[inline]
    fn name(self) -> Option<String> {
        Argument::name(self)
    }
    #[inline]
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        Argument::set_name(self, module_token, name);
    }
    #[inline]
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        Argument::clear_name(self, module_token);
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasDebugLoc for Argument<'ctx, B> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<Argument<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(a: Argument<'ctx, B>) -> Self {
        a.as_value()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for Argument<'ctx, B> {
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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
