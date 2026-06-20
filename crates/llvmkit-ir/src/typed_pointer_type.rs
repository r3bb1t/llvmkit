//! Typed-pointer type. Mirrors `llvm/include/llvm/IR/TypedPointerType.h`.
//!
//! LLVM 17+ made all `PointerType` opaque. A handful of GPU targets
//! still use *typed* pointers to express address-space conventions;
//! LLVM exposes those as the separate `TypedPointerType` kind.
//!
//! Shape mirrors the per-kind handles in [`crate::derived_types`]:
//! `(TypeId, ModuleRef<'ctx>)` with full derive on identity, accessors
//! routed through the internal `TypeData::as_typed_pointer` projection,
//! `From` / `TryFrom` against the erased [`Type`] handle.

use core::fmt;

use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::module::{ModuleBrand, ModuleRef};
use crate::r#type::{Type, TypeData, TypeId};

/// Typed pointer (`<elem>*`, `<elem> addrspace(N)*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypedPointerType<'ctx, B: crate::module::ModuleBrand = crate::module::Brand<'ctx>> {
    pub(crate) id: TypeId,
    pub(crate) module: ModuleRef<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> TypedPointerType<'ctx, B> {
    #[inline]
    pub(crate) fn new<M>(id: TypeId, module: M) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
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
    pub fn as_type(self) -> Type<'ctx, B> {
        Type::new(self.id, self.module)
    }

    /// Pointee type. Mirrors `TypedPointerType::getElementType`.
    pub fn pointee(self) -> Type<'ctx, B> {
        let (pointee, _) = self
            .data()
            .as_typed_pointer()
            .expect("TypedPointerType invariant: wraps TypedPointer");
        Type::new(pointee, self.module)
    }

    /// Address space. Mirrors `TypedPointerType::getAddressSpace`.
    pub fn address_space(self) -> u32 {
        let (_, addr_space) = self
            .data()
            .as_typed_pointer()
            .expect("TypedPointerType invariant: wraps TypedPointer");
        addr_space
    }
}

impl<'ctx, B: ModuleBrand> crate::r#type::sealed::Sealed for TypedPointerType<'ctx, B> {}

impl<'ctx, B: ModuleBrand + 'ctx> crate::r#type::IrType<'ctx, B> for TypedPointerType<'ctx, B> {
    #[inline]
    fn as_type(self) -> Type<'ctx, B> {
        self.as_type()
    }
}

impl<'ctx, B: ModuleBrand> fmt::Display for TypedPointerType<'ctx, B> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_type().fmt(f)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> From<TypedPointerType<'ctx, B>> for Type<'ctx, B> {
    #[inline]
    fn from(t: TypedPointerType<'ctx, B>) -> Self {
        t.as_type()
    }
}

impl<'ctx, B: ModuleBrand> TryFrom<Type<'ctx, B>> for TypedPointerType<'ctx, B> {
    type Error = IrError;
    fn try_from(t: Type<'ctx, B>) -> IrResult<Self> {
        if t.data().as_typed_pointer().is_some() {
            Ok(Self {
                id: t.id(),
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
