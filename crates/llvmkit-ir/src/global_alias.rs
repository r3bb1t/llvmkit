//! Module-level global alias. Mirrors `llvm/include/llvm/IR/GlobalAlias.h`.

use crate::Branded;
use core::cell::{Cell, RefCell};

use super::DebugLoc;
use super::constant::{Constant, IsConstant};
use super::derived_types::PointerType;
use super::error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use super::global_value::{DllStorageClass, DsoLocality, Linkage, ThreadLocalMode, Visibility};
use super::metadata::MetadataAttachmentSet;
use super::metadata::{MetadataAttachmentKind, MetadataId, StoredBrand};
use super::module::{Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::r#type::{Type, TypeKind, TypeSlot};
use super::unnamed_addr::UnnamedAddr;
use super::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueKindData, ValueSlot, sealed};
use super::value_id::GlobalAliasId;

#[derive(Debug)]
pub(super) struct GlobalAliasData {
    pub(super) name: String,
    pub(super) value_type: TypeSlot,
    pub(super) address_space: u32,
    pub(super) aliasee: Cell<ValueSlot>,
    pub(super) linkage: Cell<Linkage>,
    pub(super) dso_locality: Cell<DsoLocality>,
    pub(super) visibility: Cell<Visibility>,
    pub(super) dll_storage_class: Cell<DllStorageClass>,
    pub(super) thread_local_mode: Cell<ThreadLocalMode>,
    pub(super) unnamed_addr: Cell<UnnamedAddr>,
    pub(super) partition: RefCell<Option<String>>,
    pub(super) metadata: RefCell<MetadataAttachmentSet<StoredBrand>>,
}

#[derive(Branded)]
pub struct GlobalAlias<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalAlias<'ctx, B> {
    #[inline]
    pub(super) fn from_parts_unchecked<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
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
    pub fn as_erased(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Storable, module-tagged [`GlobalAliasId`] for this alias (0.0.4),
    /// resolvable via [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view).
    #[inline]
    pub fn id(self) -> GlobalAliasId<B> {
        GlobalAliasId::from_raw(self.module.id(), self.id)
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
        _module: &'ctx Module<B, Unverified>,
        aliasee: C,
    ) -> IrResult<()> {
        let constant = aliasee.as_constant();
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

    /// DSO locality (`dso_local` / `dso_preemptable`). Mirrors
    /// `GlobalValue::isDSOLocal`.
    pub fn dso_locality(self) -> DsoLocality {
        self.data().dso_locality.get()
    }

    /// Set the DSO locality. Mirrors `GlobalValue::setDSOLocal`.
    pub fn set_dso_locality(self, _module: &'ctx Module<B, Unverified>, dso: DsoLocality) {
        self.data().dso_locality.set(dso);
    }

    #[inline]
    pub fn set_linkage(self, _module: &'ctx Module<B, Unverified>, linkage: Linkage) {
        self.data().linkage.set(linkage);
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.data().visibility.get()
    }

    #[inline]
    pub fn set_visibility(self, _module: &'ctx Module<B, Unverified>, visibility: Visibility) {
        self.data().visibility.set(visibility);
    }

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.data().dll_storage_class.get()
    }

    #[inline]
    pub fn set_dll_storage_class(self, _module: &'ctx Module<B, Unverified>, cls: DllStorageClass) {
        self.data().dll_storage_class.set(cls);
    }

    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.data().thread_local_mode.get()
    }

    #[inline]
    pub fn set_thread_local_mode(self, _module: &'ctx Module<B, Unverified>, tlm: ThreadLocalMode) {
        self.data().thread_local_mode.set(tlm);
    }

    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.data().unnamed_addr.get()
    }

    #[inline]
    pub fn set_unnamed_addr(self, _module: &'ctx Module<B, Unverified>, value: UnnamedAddr) {
        self.data().unnamed_addr.set(value);
    }

    pub fn metadata(self) -> MetadataAttachmentSet<B> {
        MetadataAttachmentSet::from_stored(&self.data().metadata.borrow())
    }

    /// Crate-internal: the stored attachment set, for the printer and the
    /// verifier, which already work inside the owning module.
    pub(crate) fn metadata_stored(
        self,
    ) -> core::cell::Ref<'ctx, MetadataAttachmentSet<StoredBrand>> {
        self.data().metadata.borrow()
    }

    /// Set or replace one metadata attachment.
    ///
    /// `Err(IrError::ForeignMetadataId)` when `id` was minted by another
    /// module — the module token proves *which* module may be mutated, and the
    /// id's tag is what proves the node belongs to it.
    pub fn set_metadata(
        self,
        module: &'ctx Module<B, Unverified>,
        kind: MetadataAttachmentKind,
        id: MetadataId<B>,
    ) -> IrResult<()> {
        let id = id.into_stored(module.id())?;
        self.data().metadata.borrow_mut().insert(kind, id);
        Ok(())
    }

    pub fn partition(self) -> Option<String> {
        self.data().partition.borrow().clone()
    }

    pub fn set_partition<P>(self, _module: &'ctx Module<B, Unverified>, partition: P)
    where
        P: Into<String>,
    {
        *self.data().partition.borrow_mut() = Some(partition.into());
    }

    pub fn clear_partition(self, _module: &'ctx Module<B, Unverified>) {
        *self.data().partition.borrow_mut() = None;
    }
}

impl<'ctx, B: ModuleBrand> sealed::Sealed for GlobalAlias<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for GlobalAlias<'ctx, B> {
    #[inline]
    fn as_erased(self) -> Value<'ctx, B> {
        GlobalAlias::as_erased(self)
    }
}
crate::value::impl_into_erased_value_for_handle!(GlobalAlias);
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
        self.as_erased().name()
    }
    fn set_name<Name>(self, _module_token: &'ctx Module<B, Unverified>, _name: Name)
    where
        Name: Into<String>,
    {
    }
    fn clear_name(self, _module_token: &'ctx Module<B, Unverified>) {}
}
impl<B: ModuleBrand + 'static> HasDebugLoc for GlobalAlias<'_, B> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalAlias<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(a: GlobalAlias<'ctx, B>) -> Self {
        a.as_erased()
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

#[derive(Branded)]
#[branded(Debug)]
pub struct GlobalAliasBuilder<'ctx, B: ModuleBrand> {
    module: ModuleRef<'ctx, B>,
    name: String,
    value_type: TypeSlot,
    aliasee: ValueSlot,
    aliasee_type: TypeSlot,
    address_space: u32,
    linkage: Linkage,
    dso_locality: DsoLocality,
    visibility: Visibility,
    dll_storage_class: DllStorageClass,
    thread_local_mode: ThreadLocalMode,
    unnamed_addr: UnnamedAddr,
    partition: Option<String>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalAliasBuilder<'ctx, B> {
    pub(super) fn new<M, C, N>(module: M, name: N, value_type: Type<'ctx, B>, aliasee: C) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
        C: IsConstant<'ctx, B>,
        N: Into<String>,
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
            dso_locality: DsoLocality::Default,
            visibility: Visibility::Default,
            dll_storage_class: DllStorageClass::Default,
            thread_local_mode: ThreadLocalMode::NotThreadLocal,
            unnamed_addr: UnnamedAddr::None,
            partition: None,
        }
    }

    #[must_use]
    pub fn linkage(mut self, linkage: Linkage) -> Self {
        self.linkage = linkage;
        self
    }

    /// DSO locality (`dso_local` / `dso_preemptable`). Mirrors
    /// `GlobalValue::setDSOLocal`.
    #[must_use]
    pub fn dso_locality(mut self, dso: DsoLocality) -> Self {
        self.dso_locality = dso;
        self
    }

    #[must_use]
    pub fn visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    #[must_use]
    pub fn dll_storage_class(mut self, cls: DllStorageClass) -> Self {
        self.dll_storage_class = cls;
        self
    }

    #[must_use]
    pub fn thread_local_mode(mut self, tlm: ThreadLocalMode) -> Self {
        self.thread_local_mode = tlm;
        self
    }

    #[must_use]
    pub fn unnamed_addr(mut self, value: UnnamedAddr) -> Self {
        self.unnamed_addr = value;
        self
    }

    pub fn partition<Partition>(mut self, partition: Partition) -> Self
    where
        Partition: Into<String>,
    {
        self.partition = Some(partition.into());
        self
    }

    /// Materialise the alias, returning its storable [`GlobalAliasId`].
    /// Resolve the id back into a borrowing [`GlobalAlias`] with
    /// [`Module::view`](crate::Module::view).
    pub fn build(self) -> IrResult<GlobalAliasId<B>> {
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
        self.module
            .module()
            .install_global_alias::<B>(self)
            .map(|a| a.id())
    }

    pub(super) fn into_data(self) -> (String, GlobalAliasData, u32) {
        let GlobalAliasBuilder {
            module: _,
            name,
            value_type,
            aliasee,
            aliasee_type: _,
            address_space,
            linkage,
            dso_locality,
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
            dso_locality: Cell::new(dso_locality),
            visibility: Cell::new(visibility),
            dll_storage_class: Cell::new(dll_storage_class),
            thread_local_mode: Cell::new(thread_local_mode),
            unnamed_addr: Cell::new(unnamed_addr),
            partition: RefCell::new(partition),
            metadata: RefCell::new(MetadataAttachmentSet::new()),
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
