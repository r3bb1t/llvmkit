//! Module-level indirect function. Mirrors `llvm/include/llvm/IR/GlobalIFunc.h`.

use core::cell::{Cell, RefCell};

use crate::DebugLoc;
use crate::constant::{Constant, IsConstant};
use crate::derived_types::PointerType;
use crate::error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use crate::global_value::{Linkage, Visibility};
use crate::metadata::MetadataAttachmentSet;
use crate::module::{Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
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
pub struct GlobalIFunc<'ctx, B: crate::module::ModuleBrand = crate::module::Brand<'ctx>> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx, B>,
    pub(crate) ty: TypeId,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalIFunc<'ctx, B> {
    #[inline]
    pub(crate) fn from_parts_unchecked<M>(id: ValueId, module: M, ty: TypeId) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
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
    pub fn as_constant(self) -> Constant<'ctx, B> {
        Constant {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    #[inline]
    pub fn as_global_constant_ptr(self) -> Constant<'ctx, B> {
        self.as_constant()
    }

    fn data(self) -> &'ctx GlobalIFuncData {
        match &self.module.value_data(self.id).kind {
            ValueKindData::GlobalIFunc(i) => i,
            _ => unreachable!("GlobalIFunc handle invariant: ValueKindData::GlobalIFunc"),
        }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B> {
        crate::PointerType::new(self.ty, self.module)
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx, B> {
        Type::new(self.data().value_type, self.module)
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.data().address_space
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        &self.data().name
    }

    pub fn resolver(self) -> Constant<'ctx, B> {
        let id = self.data().resolver.get();
        let value_data = self.module.value_data(id);
        Constant {
            id,
            module: self.module,
            ty: value_data.ty,
        }
    }

    pub fn set_resolver<C: IsConstant<'ctx, B>>(
        self,
        _module: &Module<'ctx, B, Unverified>,
        resolver: C,
    ) -> IrResult<()> {
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
    pub fn set_linkage(self, _module: &Module<'ctx, B, Unverified>, linkage: Linkage) {
        self.data().linkage.set(linkage);
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.data().visibility.get()
    }

    #[inline]
    pub fn set_visibility(self, _module: &Module<'ctx, B, Unverified>, visibility: Visibility) {
        self.data().visibility.set(visibility);
    }

    pub fn metadata(self) -> core::cell::Ref<'ctx, MetadataAttachmentSet> {
        self.data().metadata.borrow()
    }

    pub fn set_metadata(
        self,
        _module: &Module<'ctx, B, Unverified>,
        kind: crate::metadata::MetadataAttachmentKind,
        id: crate::metadata::MetadataId,
    ) {
        self.data().metadata.borrow_mut().insert(kind, id);
    }

    pub fn partition(self) -> Option<String> {
        self.data().partition.borrow().clone()
    }

    pub fn set_partition<P>(self, _module: &Module<'ctx, B, Unverified>, partition: P)
    where
        P: Into<String>,
    {
        *self.data().partition.borrow_mut() = Some(partition.into());
    }

    pub fn clear_partition(self, _module: &Module<'ctx, B, Unverified>) {
        *self.data().partition.borrow_mut() = None;
    }
}

impl<'ctx, B: ModuleBrand> sealed::Sealed for GlobalIFunc<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for GlobalIFunc<'ctx, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        GlobalIFunc::as_value(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> IsConstant<'ctx, B> for GlobalIFunc<'ctx, B> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B> {
        GlobalIFunc::as_constant(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for GlobalIFunc<'ctx, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasName<'ctx, B> for GlobalIFunc<'ctx, B> {
    fn name(self) -> Option<String> {
        self.as_value().name()
    }
    fn set_name<Name>(self, _module_token: &Module<'ctx, B, Unverified>, _name: Name)
    where
        Name: Into<String>,
    {
    }
    fn clear_name(self, _module_token: &Module<'ctx, B, Unverified>) {}
}
impl<B: ModuleBrand + 'static> HasDebugLoc for GlobalIFunc<'_, B> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalIFunc<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(i: GlobalIFunc<'ctx, B>) -> Self {
        i.as_value()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalIFunc<'ctx, B>> for Constant<'ctx, B> {
    #[inline]
    fn from(i: GlobalIFunc<'ctx, B>) -> Self {
        i.as_constant()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for GlobalIFunc<'ctx, B> {
    type Error = IrError;

    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
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

pub struct GlobalIFuncBuilder<'ctx, B: ModuleBrand = crate::module::Brand<'ctx>> {
    module: ModuleRef<'ctx, B>,
    name: String,
    value_type: TypeId,
    resolver: ValueId,
    resolver_type: TypeId,
    address_space: u32,
    linkage: Linkage,
    visibility: Visibility,
    partition: Option<String>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalIFuncBuilder<'ctx, B> {
    pub(crate) fn new<M, C>(
        module: M,
        name: impl Into<String>,
        value_type: Type<'ctx, B>,
        resolver: C,
    ) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
        C: IsConstant<'ctx, B>,
    {
        let module = module.into();
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

    pub fn partition<Partition>(mut self, partition: Partition) -> Self
    where
        Partition: Into<String>,
    {
        self.partition = Some(partition.into());
        self
    }

    pub fn build(self) -> IrResult<GlobalIFunc<'ctx, B>> {
        if !is_valid_ifunc_linkage(self.linkage) {
            return Err(IrError::InvalidOperation {
                message: "invalid linkage type for ifunc",
            });
        }
        if self.module.module().context().value_data(self.resolver).ty != self.resolver_type {
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
        self.module.module().install_global_ifunc::<B>(self)
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
fn pointer_address_space<B: ModuleBrand>(ty: Type<'_, B>) -> Option<u32> {
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

impl<'ctx, B: ModuleBrand + 'ctx> core::fmt::Display for GlobalIFunc<'ctx, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_ifunc(f, *self)
    }
}
