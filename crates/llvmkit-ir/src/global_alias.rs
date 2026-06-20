//! Module-level global alias. Mirrors `llvm/include/llvm/IR/GlobalAlias.h`.

use core::cell::{Cell, RefCell};

use crate::DebugLoc;
use crate::constant::{Constant, IsConstant};
use crate::error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use crate::global_value::{DllStorageClass, Linkage, ThreadLocalMode, Visibility};
use crate::module::{Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use crate::r#type::{Type, TypeId, TypeKind};
use crate::unnamed_addr::UnnamedAddr;
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, ValueKindData, sealed};

#[derive(Debug)]
pub(crate) struct GlobalAliasData {
    pub(crate) name: String,
    pub(crate) value_type: TypeId,
    pub(crate) address_space: u32,
    pub(crate) aliasee: Cell<ValueId>,
    pub(crate) linkage: Cell<Linkage>,
    pub(crate) visibility: Cell<Visibility>,
    pub(crate) dll_storage_class: Cell<DllStorageClass>,
    pub(crate) thread_local_mode: Cell<ThreadLocalMode>,
    pub(crate) unnamed_addr: Cell<UnnamedAddr>,
    pub(crate) partition: RefCell<Option<String>>,
    pub(crate) metadata: RefCell<crate::metadata::MetadataAttachmentSet>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalAlias<'ctx, B: crate::module::ModuleBrand = crate::module::Brand<'ctx>> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx, B>,
    pub(crate) ty: TypeId,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalAlias<'ctx, B> {
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

    fn data(self) -> &'ctx GlobalAliasData {
        match &self.module.value_data(self.id).kind {
            ValueKindData::GlobalAlias(a) => a,
            _ => unreachable!("GlobalAlias handle invariant: ValueKindData::GlobalAlias"),
        }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    #[inline]
    pub fn ty(self) -> crate::PointerType<'ctx, B> {
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

    pub fn aliasee(self) -> Constant<'ctx, B> {
        let id = self.data().aliasee.get();
        let value_data = self.module.value_data(id);
        Constant {
            id,
            module: self.module,
            ty: value_data.ty,
        }
    }

    pub fn set_aliasee<C: IsConstant<'ctx, B>>(
        self,
        _module: &Module<'ctx, B, Unverified>,
        aliasee: C,
    ) -> IrResult<()> {
        let constant = aliasee.as_constant();
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
        self.data().aliasee.set(constant.id);
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

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.data().dll_storage_class.get()
    }

    #[inline]
    pub fn set_dll_storage_class(
        self,
        _module: &Module<'ctx, B, Unverified>,
        cls: DllStorageClass,
    ) {
        self.data().dll_storage_class.set(cls);
    }

    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.data().thread_local_mode.get()
    }

    #[inline]
    pub fn set_thread_local_mode(
        self,
        _module: &Module<'ctx, B, Unverified>,
        tlm: ThreadLocalMode,
    ) {
        self.data().thread_local_mode.set(tlm);
    }

    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.data().unnamed_addr.get()
    }

    #[inline]
    pub fn set_unnamed_addr(self, _module: &Module<'ctx, B, Unverified>, value: UnnamedAddr) {
        self.data().unnamed_addr.set(value);
    }

    pub fn metadata(self) -> core::cell::Ref<'ctx, crate::metadata::MetadataAttachmentSet> {
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

    pub fn set_partition(
        self,
        _module: &Module<'ctx, B, Unverified>,
        partition: Option<impl Into<String>>,
    ) {
        *self.data().partition.borrow_mut() = partition.map(Into::into);
    }
}

impl<'ctx, B: ModuleBrand> sealed::Sealed for GlobalAlias<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for GlobalAlias<'ctx, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        GlobalAlias::as_value(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> IsConstant<'ctx, B> for GlobalAlias<'ctx, B> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B> {
        GlobalAlias::as_constant(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for GlobalAlias<'ctx, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasName<'ctx, B> for GlobalAlias<'ctx, B> {
    fn name(self) -> Option<String> {
        Some(self.data().name.clone())
    }
    fn set_name(self, _module_token: &Module<'ctx, B, Unverified>, _name: &str) {}
    fn clear_name(self, _module_token: &Module<'ctx, B, Unverified>) {}
}
impl<B: ModuleBrand + 'static> HasDebugLoc for GlobalAlias<'_, B> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalAlias<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(a: GlobalAlias<'ctx, B>) -> Self {
        a.as_value()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalAlias<'ctx, B>> for Constant<'ctx, B> {
    #[inline]
    fn from(a: GlobalAlias<'ctx, B>) -> Self {
        a.as_constant()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for GlobalAlias<'ctx, B> {
    type Error = IrError;

    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        match &v.data().kind {
            ValueKindData::GlobalAlias(_) => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
            }),
            other => Err(IrError::ValueCategoryMismatch {
                expected: ValueCategoryLabel::GlobalAlias,
                got: crate::value::category_label_for_kind(other),
            }),
        }
    }
}

pub struct GlobalAliasBuilder<'ctx, B: ModuleBrand = crate::module::Brand<'ctx>> {
    module: ModuleRef<'ctx, B>,
    name: String,
    value_type: TypeId,
    aliasee: ValueId,
    aliasee_type: TypeId,
    address_space: u32,
    linkage: Linkage,
    visibility: Visibility,
    dll_storage_class: DllStorageClass,
    thread_local_mode: ThreadLocalMode,
    unnamed_addr: UnnamedAddr,
    partition: Option<String>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalAliasBuilder<'ctx, B> {
    pub(crate) fn new<M, C>(
        module: M,
        name: impl Into<String>,
        value_type: Type<'ctx, B>,
        aliasee: C,
    ) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
        C: IsConstant<'ctx, B>,
    {
        let module = module.into();
        let aliasee = aliasee.as_constant();
        let address_space = pointer_address_space(aliasee.ty()).unwrap_or(0);
        Self {
            module,
            name: name.into(),
            value_type: value_type.id(),
            aliasee: aliasee.id,
            aliasee_type: aliasee.ty,
            address_space,
            linkage: Linkage::External,
            visibility: Visibility::Default,
            dll_storage_class: DllStorageClass::Default,
            thread_local_mode: ThreadLocalMode::NotThreadLocal,
            unnamed_addr: UnnamedAddr::None,
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

    pub fn dll_storage_class(mut self, cls: DllStorageClass) -> Self {
        self.dll_storage_class = cls;
        self
    }

    pub fn thread_local_mode(mut self, tlm: ThreadLocalMode) -> Self {
        self.thread_local_mode = tlm;
        self
    }

    pub fn unnamed_addr(mut self, value: UnnamedAddr) -> Self {
        self.unnamed_addr = value;
        self
    }

    pub fn partition(mut self, partition: impl Into<String>) -> Self {
        self.partition = Some(partition.into());
        self
    }

    pub fn build(self) -> IrResult<GlobalAlias<'ctx, B>> {
        if !is_valid_alias_linkage(self.linkage) {
            return Err(IrError::InvalidOperation {
                message: "invalid linkage type for alias",
            });
        }
        if self.module.module().context().value_data(self.aliasee).ty != self.aliasee_type {
            return Err(IrError::InvalidOperation {
                message: "alias aliasee type changed before build",
            });
        }
        if !matches!(
            Type::new(self.aliasee_type, self.module).kind(),
            TypeKind::Pointer { .. }
        ) {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Pointer,
                got: Type::new(self.aliasee_type, self.module).kind_label(),
            });
        }
        self.module.module().install_global_alias::<B>(self)
    }

    pub(crate) fn into_data(self) -> (String, GlobalAliasData, u32) {
        let GlobalAliasBuilder {
            module: _,
            name,
            value_type,
            aliasee,
            aliasee_type: _,
            address_space,
            linkage,
            visibility,
            dll_storage_class,
            thread_local_mode,
            unnamed_addr,
            partition,
        } = self;
        let data = GlobalAliasData {
            name: name.clone(),
            value_type,
            address_space,
            aliasee: Cell::new(aliasee),
            linkage: Cell::new(linkage),
            visibility: Cell::new(visibility),
            dll_storage_class: Cell::new(dll_storage_class),
            thread_local_mode: Cell::new(thread_local_mode),
            unnamed_addr: Cell::new(unnamed_addr),
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
pub const fn is_valid_alias_linkage(linkage: Linkage) -> bool {
    matches!(
        linkage,
        Linkage::External
            | Linkage::AvailableExternally
            | Linkage::LinkOnceAny
            | Linkage::LinkOnceODR
            | Linkage::WeakAny
            | Linkage::WeakODR
            | Linkage::Internal
            | Linkage::Private
            | Linkage::ExternalWeak
    )
}

impl<'ctx, B: ModuleBrand + 'ctx> core::fmt::Display for GlobalAlias<'ctx, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_alias(f, *self)
    }
}
