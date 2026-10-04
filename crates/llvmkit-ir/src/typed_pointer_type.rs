//! Typed-pointer type. Mirrors `llvm/include/llvm/IR/TypedPointerType.h`.
//!
//! LLVM 17+ made all `PointerType` opaque. A handful of GPU targets
//! still use *typed* pointers to express address-space conventions;
//! LLVM exposes those as the separate `TypedPointerType` kind.
//!
//! Shape mirrors the per-kind handles in [`crate::derived_types`]:
//! `(TypeSlot, ModuleRef<'ctx>)` with full derive on identity, accessors
//! routed through the internal `TypeData::as_typed_pointer` projection,
//! `From` / `TryFrom` against the erased [`Type`] handle.
//!
//! Unrelated to [`crate::TypedPointerValue`], which is a Rust-side-only
//! pointee-schema overlay on top of a plain *opaque* pointer *value*
//! (compile-time bookkeeping, no IR-level change, prints identically to
//! the erased path). This module's [`TypedPointerType`] is the actual
//! LLVM IR *type* kind above -- it changes printed IR and is the GPU-only
//! typed-pointer story, not the general-purpose ergonomics overlay.

use core::fmt;

use crate::Branded;
use crate::capability::{Capability, CapabilityOf, Mutable};
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::module::{ModuleBrand, ModuleRef};
use crate::r#type::{Type, TypeData, TypeSlot, TypeSlotAccess};

/// Typed pointer (`<elem>*`, `<elem> addrspace(N)*`).
#[derive(Branded)]
pub struct TypedPointerType<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    id: TypeSlot,
    pub(crate) module: ModuleRef<'ctx, B, C>,
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for TypedPointerType<'_, B, C> {
    type Capability = C;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TypedPointerType<'ctx, B, C> {
    #[inline]
    pub(crate) fn new<M>(id: TypeSlot, module: M) -> Self
    where
        M: Into<ModuleRef<'ctx, B, C>>,
    {
        Self {
            id,
            module: module.into(),
        }
    }

    #[inline]
    pub(crate) fn data(self) -> &'ctx TypeData {
        self.module.type_data(self.id)
    }

    #[inline]
    pub fn as_type(self) -> Type<'ctx, B, C> {
        Type::new(self.id, self.module)
    }

    /// Pointee type. Mirrors `TypedPointerType::getElementType`.
    pub fn pointee(self) -> Type<'ctx, B, C> {
        let (pointee, _) = self
            .data()
            .as_typed_pointer()
            .unwrap_or_else(|| unreachable!("TypedPointerType invariant: wraps TypedPointer"));
        Type::new(pointee, self.module)
    }

    /// Address space. Mirrors `TypedPointerType::getAddressSpace`.
    pub fn address_space(self) -> u32 {
        let (_, addr_space) = self
            .data()
            .as_typed_pointer()
            .unwrap_or_else(|| unreachable!("TypedPointerType invariant: wraps TypedPointer"));
        addr_space
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> crate::r#type::sealed::Sealed
    for TypedPointerType<'ctx, B, C>
{
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> crate::r#type::IrType<'ctx, B>
    for TypedPointerType<'ctx, B, C>
{
    #[inline]
    fn as_type(self) -> Type<'ctx, B, C> {
        self.as_type()
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> fmt::Display for TypedPointerType<'ctx, B, C> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_type().fmt(f)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> From<TypedPointerType<'ctx, B, C>>
    for Type<'ctx, B, C>
{
    #[inline]
    fn from(t: TypedPointerType<'ctx, B, C>) -> Self {
        t.as_type()
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> TryFrom<Type<'ctx, B, C>>
    for TypedPointerType<'ctx, B, C>
{
    type Error = IrError;
    fn try_from(t: Type<'ctx, B, C>) -> IrResult<Self> {
        if t.data().as_typed_pointer().is_some() {
            Ok(Self {
                // Internal: a re-wrap that keeps `t`'s own module.
                id: t.slot_trusting_same_module(),
                module: t.module,
            })
        } else {
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::TypedPointer,
                got: t.kind_label(),
            })
        }
    }
}
