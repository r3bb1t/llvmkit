//! Module-level indirect function. Mirrors `llvm/include/llvm/IR/GlobalIFunc.h`.

use core::cell::{Cell, RefCell};

use crate::DebugLoc;
use crate::constant::{Constant, IsConstant};
use crate::error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use crate::global_value::{Linkage, Visibility};
use crate::module::{Module, ModuleRef};
use crate::r#type::{Type, TypeId, TypeKind};
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, ValueKindData, sealed};

#[derive(Debug)]
pub(crate) struct GlobalIFuncData {
    pub(crate) name: String,
    pub(crate) value_type: TypeId,
    pub(crate) address_space: u32,
    pub(crate) resolver: Cell<ValueId>,
    pub(crate) linkage: Cell<Linkage>,
    pub(crate) visibility: Cell<Visibility>,
    pub(crate) partition: RefCell<Option<String>>,
    pub(crate) metadata: RefCell<crate::metadata::MetadataAttachmentSet>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalIFunc<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

impl<'ctx> GlobalIFunc<'ctx> {
    #[inline]
    pub(crate) fn from_parts_unchecked(
        id: ValueId,
        module: &'ctx Module<'ctx>,
        ty: TypeId,
    ) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
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
    pub fn as_constant(self) -> Constant<'ctx> {
        Constant {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    #[inline]
    pub fn as_global_constant_ptr(self) -> Constant<'ctx> {
        self.as_constant()
    }

    fn data(self) -> &'ctx GlobalIFuncData {
        match &self.module.value_data(self.id).kind {
            ValueKindData::GlobalIFunc(i) => i,
            _ => unreachable!("GlobalIFunc handle invariant: ValueKindData::GlobalIFunc"),
        }
    }

    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    #[inline]
    pub fn ty(self) -> crate::PointerType<'ctx> {
        crate::PointerType::new(self.ty, self.module.module())
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx> {
        Type::new(self.data().value_type, self.module.module())
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.data().address_space
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        &self.data().name
    }

    pub fn resolver(self) -> Constant<'ctx> {
        let id = self.data().resolver.get();
        let value_data = self.module.value_data(id);
        Constant {
            id,
            module: self.module,
            ty: value_data.ty,
        }
    }

    pub fn set_resolver<C: IsConstant<'ctx>>(self, resolver: C) -> IrResult<()> {
        let constant = resolver.as_constant();
        if constant.module != self.module {
            return Err(IrError::ForeignValue);
        }
        let Some(addr_space) = pointer_address_space(constant.ty()) else {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Pointer,
                got: constant.ty().kind_label(),
            });
        };
        if addr_space != self.address_space() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Pointer,
                got: constant.ty().kind_label(),
            });
        }
        self.data().resolver.set(constant.id);
        Ok(())
    }

    #[inline]
    pub fn linkage(self) -> Linkage {
        self.data().linkage.get()
    }

    #[inline]
    pub fn set_linkage(self, linkage: Linkage) {
        self.data().linkage.set(linkage);
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.data().visibility.get()
    }

    #[inline]
    pub fn set_visibility(self, visibility: Visibility) {
        self.data().visibility.set(visibility);
    }

    pub fn metadata(self) -> core::cell::Ref<'ctx, crate::metadata::MetadataAttachmentSet> {
        self.data().metadata.borrow()
    }

    pub fn set_metadata(
        self,
        kind: crate::metadata::MetadataAttachmentKind,
        id: crate::metadata::MetadataId,
    ) {
        self.data().metadata.borrow_mut().insert(kind, id);
    }

    pub fn partition(self) -> Option<String> {
        self.data().partition.borrow().clone()
    }

    pub fn set_partition(self, partition: Option<impl Into<String>>) {
        *self.data().partition.borrow_mut() = partition.map(Into::into);
    }
}

impl<'ctx> sealed::Sealed for GlobalIFunc<'ctx> {}
impl<'ctx> IsValue<'ctx> for GlobalIFunc<'ctx> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        GlobalIFunc::as_value(self)
    }
}
impl<'ctx> IsConstant<'ctx> for GlobalIFunc<'ctx> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx> {
        GlobalIFunc::as_constant(self)
    }
}
impl<'ctx> Typed<'ctx> for GlobalIFunc<'ctx> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }
}
impl<'ctx> HasName<'ctx> for GlobalIFunc<'ctx> {
    fn name(self) -> Option<String> {
        Some(self.data().name.clone())
    }
    fn set_name(self, _name: Option<&str>) {}
}
impl HasDebugLoc for GlobalIFunc<'_> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx> From<GlobalIFunc<'ctx>> for Value<'ctx> {
    #[inline]
    fn from(i: GlobalIFunc<'ctx>) -> Self {
        i.as_value()
    }
}
impl<'ctx> From<GlobalIFunc<'ctx>> for Constant<'ctx> {
    #[inline]
    fn from(i: GlobalIFunc<'ctx>) -> Self {
        i.as_constant()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for GlobalIFunc<'ctx> {
    type Error = IrError;

    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
        match &v.data().kind {
            ValueKindData::GlobalIFunc(_) => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
            }),
            other => Err(IrError::ValueCategoryMismatch {
                expected: ValueCategoryLabel::GlobalIFunc,
                got: crate::value::category_label_for_kind(other),
            }),
        }
    }
}

pub struct GlobalIFuncBuilder<'ctx> {
    module: &'ctx Module<'ctx>,
    name: String,
    value_type: TypeId,
    resolver: ValueId,
    resolver_type: TypeId,
    address_space: u32,
    linkage: Linkage,
    visibility: Visibility,
    partition: Option<String>,
}

impl<'ctx> GlobalIFuncBuilder<'ctx> {
    pub(crate) fn new<C: IsConstant<'ctx>>(
        module: &'ctx Module<'ctx>,
        name: impl Into<String>,
        value_type: Type<'ctx>,
        resolver: C,
    ) -> Self {
        let resolver = resolver.as_constant();
        let address_space = pointer_address_space(resolver.ty()).unwrap_or(0);
        Self {
            module,
            name: name.into(),
            value_type: value_type.id(),
            resolver: resolver.id,
            resolver_type: resolver.ty,
            address_space,
            linkage: Linkage::External,
            visibility: Visibility::Default,
            partition: None,
        }
    }

    pub fn linkage(mut self, linkage: Linkage) -> Self {
        self.linkage = linkage;
        self
    }

    pub fn visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    pub fn partition(mut self, partition: impl Into<String>) -> Self {
        self.partition = Some(partition.into());
        self
    }

    pub fn build(self) -> IrResult<GlobalIFunc<'ctx>> {
        if !is_valid_ifunc_linkage(self.linkage) {
            return Err(IrError::InvalidOperation {
                message: "invalid linkage type for ifunc",
            });
        }
        if self.module.context().value_data(self.resolver).ty != self.resolver_type {
            return Err(IrError::InvalidOperation {
                message: "ifunc resolver type changed before build",
            });
        }
        if !matches!(
            Type::new(self.resolver_type, self.module).kind(),
            TypeKind::Pointer { .. }
        ) {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Pointer,
                got: Type::new(self.resolver_type, self.module).kind_label(),
            });
        }
        self.module.install_global_ifunc(self)
    }

    pub(crate) fn into_data(self) -> (String, GlobalIFuncData, u32) {
        let GlobalIFuncBuilder {
            module: _,
            name,
            value_type,
            resolver,
            resolver_type: _,
            address_space,
            linkage,
            visibility,
            partition,
        } = self;
        let data = GlobalIFuncData {
            name: name.clone(),
            value_type,
            address_space,
            resolver: Cell::new(resolver),
            linkage: Cell::new(linkage),
            visibility: Cell::new(visibility),
            partition: RefCell::new(partition),
            metadata: RefCell::new(crate::metadata::MetadataAttachmentSet::new()),
        };
        (name, data, address_space)
    }
}

#[inline]
fn pointer_address_space(ty: Type<'_>) -> Option<u32> {
    match ty.kind() {
        TypeKind::Pointer { addr_space } => Some(addr_space),
        _ => None,
    }
}

#[inline]
pub const fn is_valid_ifunc_linkage(linkage: Linkage) -> bool {
    matches!(
        linkage,
        Linkage::External
            | Linkage::LinkOnceAny
            | Linkage::LinkOnceODR
            | Linkage::WeakAny
            | Linkage::WeakODR
            | Linkage::Internal
            | Linkage::Private
            | Linkage::ExternalWeak
    )
}

impl<'ctx> core::fmt::Display for GlobalIFunc<'ctx> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_ifunc(f, *self)
    }
}
