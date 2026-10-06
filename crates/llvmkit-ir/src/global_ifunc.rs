//! Module-level indirect function. Mirrors `llvm/include/llvm/IR/GlobalIFunc.h`.

use crate::Branded;
use core::cell::{Cell, RefCell};

use super::DebugLoc;
use super::capability::{CanMutate, Capability, CapabilityOf, Mutable, ReadOnly};
use super::constant::{Constant, IsConstant};
use super::derived_types::PointerType;
use super::error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use super::global_value::{DllStorageClass, DsoLocality, Linkage, ThreadLocalMode, Visibility};
use super::metadata::MetadataAttachmentSet;
use super::metadata::{MetadataAttachmentKind, MetadataId, StoredBrand};
use super::module::{Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::r#type::{IrType, Type, TypeKind, TypeSlot, TypeSlotAccess};
use super::unnamed_addr::UnnamedAddr;
use super::value::{
    GlobalFieldKind, HasDebugLoc, HasName, IsValue, SetName, Typed, Value, ValueKindData,
    ValueSlot, ValueSlotAccess, sealed,
};
use super::value_id::GlobalIfuncId;

#[derive(Debug)]
pub(super) struct GlobalIfuncData {
    pub(super) value_type: TypeSlot,
    pub(super) address_space: u32,
    pub(super) resolver: Cell<ValueSlot>,
    pub(super) linkage: Cell<Linkage>,
    pub(super) dso_locality: Cell<DsoLocality>,
    pub(super) visibility: Cell<Visibility>,
    /// `parseAliasOrIFunc` reads these three before it knows whether it is
    /// looking at an alias or an ifunc, so an ifunc carries them too — the
    /// grammar shares the whole prefix.
    pub(super) dll_storage_class: Cell<DllStorageClass>,
    pub(super) thread_local_mode: Cell<ThreadLocalMode>,
    pub(super) unnamed_addr: Cell<UnnamedAddr>,
    pub(super) partition: RefCell<Option<String>>,
    pub(super) metadata: RefCell<MetadataAttachmentSet<StoredBrand>>,
}

/// Module-level indirect function handle. Mirrors `GlobalIFunc *`.
///
/// `C` is the handle's [`Capability`] (D8): an ifunc viewed through an
/// unverified module is [`Mutable`]; one viewed through a verified module is
/// [`ReadOnly`] and has no setters.
#[derive(Branded)]
pub struct GlobalIfunc<'ctx, B: ModuleBrand, C: Capability = Mutable> {
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B, C>,
    ty: TypeSlot,
}

impl<B: ModuleBrand, C: Capability> CapabilityOf for GlobalIfunc<'_, B, C> {
    type Capability = C;
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> GlobalIfunc<'ctx, B, C> {
    #[inline]
    pub(super) fn from_parts_unchecked<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B, C>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
        }
    }

    /// Widen to the erased [`Value`] handle, at the same capability.
    #[inline]
    pub fn as_erased(self) -> Value<'ctx, B, C> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// Storable, module-tagged [`GlobalIfuncId`] for this `ifunc` (llvmkit
    /// 2.0), resolvable via [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view).
    #[inline]
    pub fn id(self) -> GlobalIfuncId<B> {
        GlobalIfuncId::from_raw(self.module.id(), self.id)
    }

    #[inline]
    pub fn as_constant(self) -> Constant<'ctx, B, C> {
        Constant::from_parts(Value::from_parts(self.id, self.module, self.ty))
    }

    #[inline]
    pub fn as_global_constant_ptr(self) -> Constant<'ctx, B, C> {
        self.as_constant()
    }

    fn data(self) -> &'ctx GlobalIfuncData {
        match &self.module.value_data(self.id).kind {
            ValueKindData::GlobalIfunc(i) => i,
            _ => unreachable!("GlobalIfunc handle invariant: ValueKindData::GlobalIfunc"),
        }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B, C> {
        crate::PointerType::new(self.ty, self.module)
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx, B, C> {
        Type::new(self.data().value_type, self.module)
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.data().address_space
    }

    /// Symbol name (without the leading `@`); [`None`] for an unnamed ifunc,
    /// which the printer numbers. Mirrors `Value::getName`, which answers the
    /// empty string there.
    #[inline]
    pub fn name(self) -> Option<String> {
        self.as_erased().name()
    }

    /// Rename this ifunc through its module's symbol table. Mirrors
    /// `Value::setName` on a `GlobalIFunc`: a name another global value holds
    /// is uniqued (`name.1`, or `name1` on an NVPTX module).
    ///
    /// # Errors
    ///
    /// [`IrError::InvalidValueName`] for a name containing a NUL byte, which
    /// `Value::setNameImpl` asserts against; the ifunc keeps its name.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this ifunc's module — reachable with
    /// two modules of one brand, such as two [`Module::dynamic`] values.
    pub fn set_name<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        name: Name,
    ) -> IrResult<()>
    where
        Name: Into<String>,
        C: CanMutate,
    {
        self.as_erased().set_name(module_token, name)
    }

    /// Leave this ifunc unnamed. Mirrors `Value::setName("")`.
    ///
    /// # Panics
    ///
    /// Panics if `module_token` is not this ifunc's module — reachable with
    /// two modules of one brand, such as two [`Module::dynamic`] values.
    pub fn clear_name(self, module_token: &'ctx Module<B, Unverified>)
    where
        C: CanMutate,
    {
        self.as_erased().clear_name(module_token);
    }

    pub fn resolver(self) -> Constant<'ctx, B, C> {
        let id = self.data().resolver.get();
        let value_data = self.module.value_data(id);
        Constant::from_parts(Value::from_parts(id, self.module, value_data.ty))
    }

    pub fn set_resolver<Resolver: IsConstant<'ctx, B>>(
        self,
        _module: &'ctx Module<B, Unverified>,
        resolver: Resolver,
    ) -> IrResult<()>
    where
        C: CanMutate,
    {
        let constant = resolver.as_constant();
        // Only the slot is stored, so a constant from another module sharing
        // this brand would silently name a different value here.
        let resolver = constant.slot_in(self.module.id())?;
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
        self.module.module().context().retarget_global_field_use(
            self.id,
            GlobalFieldKind::IfuncResolver,
            Some(self.data().resolver.get()),
            Some(resolver),
        );
        self.data().resolver.set(resolver);
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
    pub fn set_dso_locality(self, _module: &'ctx Module<B, Unverified>, dso: DsoLocality)
    where
        C: CanMutate,
    {
        self.data().dso_locality.set(dso);
    }

    #[inline]
    pub fn set_linkage(self, _module: &'ctx Module<B, Unverified>, linkage: Linkage)
    where
        C: CanMutate,
    {
        self.data().linkage.set(linkage);
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.data().visibility.get()
    }

    #[inline]
    pub fn set_visibility(self, _module: &'ctx Module<B, Unverified>, visibility: Visibility)
    where
        C: CanMutate,
    {
        self.data().visibility.set(visibility);
    }

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.data().dll_storage_class.get()
    }

    #[inline]
    pub fn set_dll_storage_class(self, _module: &'ctx Module<B, Unverified>, cls: DllStorageClass)
    where
        C: CanMutate,
    {
        self.data().dll_storage_class.set(cls);
    }

    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.data().thread_local_mode.get()
    }

    #[inline]
    pub fn set_thread_local_mode(self, _module: &'ctx Module<B, Unverified>, tlm: ThreadLocalMode)
    where
        C: CanMutate,
    {
        self.data().thread_local_mode.set(tlm);
    }

    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.data().unnamed_addr.get()
    }

    #[inline]
    pub fn set_unnamed_addr(self, _module: &'ctx Module<B, Unverified>, value: UnnamedAddr)
    where
        C: CanMutate,
    {
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
    ) -> IrResult<()>
    where
        C: CanMutate,
    {
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
        C: CanMutate,
    {
        *self.data().partition.borrow_mut() = Some(partition.into());
    }

    pub fn clear_partition(self, _module: &'ctx Module<B, Unverified>)
    where
        C: CanMutate,
    {
        *self.data().partition.borrow_mut() = None;
    }
}

impl<'ctx, B: ModuleBrand, C: Capability> sealed::Sealed for GlobalIfunc<'ctx, B, C> {}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> IsValue<'ctx, B> for GlobalIfunc<'ctx, B, C> {
    #[inline]
    fn as_erased(self) -> Value<'ctx, B, C> {
        GlobalIfunc::as_erased(self)
    }
}
crate::value::impl_into_erased_value_for_handle!(GlobalIfunc);
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> IsConstant<'ctx, B> for GlobalIfunc<'ctx, B, C> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B, C> {
        GlobalIfunc::as_constant(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> Typed<'ctx, B> for GlobalIfunc<'ctx, B, C> {
    #[inline]
    fn ty(self) -> Type<'ctx, B, C> {
        Type::new(self.ty, self.module)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> HasName<'ctx, B> for GlobalIfunc<'ctx, B, C> {
    fn name(self) -> Option<String> {
        self.as_erased().name()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: CanMutate> SetName<'ctx, B> for GlobalIfunc<'ctx, B, C> {
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name) -> IrResult<()>
    where
        Name: Into<String>,
    {
        GlobalIfunc::set_name(self, module_token, name)
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        GlobalIfunc::clear_name(self, module_token);
    }
}
impl<B: ModuleBrand + 'static, C: Capability> HasDebugLoc for GlobalIfunc<'_, B, C> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> From<GlobalIfunc<'ctx, B, C>>
    for Value<'ctx, B, C>
{
    #[inline]
    fn from(i: GlobalIfunc<'ctx, B, C>) -> Self {
        i.as_erased()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> From<GlobalIfunc<'ctx, B, C>>
    for Constant<'ctx, B, C>
{
    #[inline]
    fn from(i: GlobalIfunc<'ctx, B, C>) -> Self {
        i.as_constant()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> TryFrom<Value<'ctx, B, C>>
    for GlobalIfunc<'ctx, B, C>
{
    type Error = IrError;

    fn try_from(v: Value<'ctx, B, C>) -> IrResult<Self> {
        match &v.data().kind {
            ValueKindData::GlobalIfunc(_) => Ok(Self {
                // Internal: a re-wrap that keeps `v`'s own module.
                id: v.slot_trusting_same_module(),
                module: v.module,
                ty: v.ty().slot_trusting_same_module(),
            }),
            other => Err(IrError::ValueCategoryMismatch {
                expected: ValueCategoryLabel::GlobalIfunc,
                got: crate::value::category_label_for_kind(other),
            }),
        }
    }
}

#[derive(Branded)]
#[branded(Debug)]
pub struct GlobalIfuncBuilder<'ctx, B: ModuleBrand> {
    module: ModuleRef<'ctx, B>,
    name: String,
    /// Kept as the caller's handle, not its slot, at `ReadOnly`: a type of
    /// any capability is accepted, `build` admits it through the checked
    /// door, and only then does its slot enter this module. Re-minting it at
    /// this module before admission would skip the foreign-module check.
    value_type: Type<'ctx, B, ReadOnly>,
    /// Kept as the caller's handle for the same reason, at `ReadOnly`: a
    /// resolver of any capability is accepted, and `build` admits it.
    resolver: Constant<'ctx, B, ReadOnly>,
    address_space: u32,
    linkage: Linkage,
    dso_locality: DsoLocality,
    visibility: Visibility,
    dll_storage_class: DllStorageClass,
    thread_local_mode: ThreadLocalMode,
    unnamed_addr: UnnamedAddr,
    partition: Option<String>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalIfuncBuilder<'ctx, B> {
    pub(super) fn new<M, T, R, N>(module: M, name: N, value_type: T, resolver: R) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
        T: IrType<'ctx, B>,
        R: IsConstant<'ctx, B>,
        N: Into<String>,
    {
        let module = module.into();
        let resolver = resolver.as_constant().read_only();
        let address_space = pointer_address_space(resolver.ty()).unwrap_or(0);
        Self {
            module,
            name: name.into(),
            value_type: value_type.as_type().read_only(),
            resolver,
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

    /// The three clauses `parseAliasOrIFunc` reads before it knows whether it
    /// is looking at an alias or an ifunc, so an ifunc accepts them too.
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

    /// Materialise the `ifunc`, returning its storable [`GlobalIfuncId`].
    /// Resolve the id back into a borrowing [`GlobalIfunc`] with
    /// [`Module::view`](crate::Module::view).
    ///
    /// Errors with [`IrError::ForeignType`] when the value type, and with
    /// [`IrError::ForeignValueId`] when the resolver, was minted by another
    /// module sharing this brand.
    pub fn build(self) -> IrResult<GlobalIfuncId<B>> {
        // The linkage is deliberately *not* checked here. Upstream rejects a
        // bad ifunc linkage in `Verifier::visitGlobalIFunc`, not at
        // construction and not in `parseAliasOrIFunc` — whose `isValidLinkage`
        // guard is `if (IsAlias && ...)`. Checking it here made that
        // diagnostic unreachable; it now lives in
        // `VerifierRule::IfuncInvalidLinkage`.
        // Each handle's slot names a different type or value — or nothing — in
        // another module's arena, so both are admitted before this arena is
        // read at either.
        let owner = self.module.id();
        let value_type = self.value_type.slot_in(owner)?;
        let resolver = self.resolver.slot_in(owner)?;
        // The resolver was admitted just above, so its type handle is this
        // module's too.
        let resolver_type = self.resolver.ty();
        if self.module.module().context().value_data(resolver).ty
            != resolver_type.slot_trusting_same_module()
        {
            return Err(IrError::IfuncResolverTypeChangedBeforeBuild);
        }
        if !matches!(resolver_type.kind(), TypeKind::Pointer { .. }) {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Pointer,
                got: resolver_type.kind_label(),
            });
        }
        let module = self.module;
        let (name, data, address_space) = self.into_data(value_type, resolver);
        module
            .module()
            .install_global_ifunc::<B>(name, data, address_space)
            .map(|f| f.id())
    }

    /// Lower the builder to its storage payload, holding the slots `build`
    /// admitted rather than anything read off the handles again.
    fn into_data(
        self,
        value_type: TypeSlot,
        resolver: ValueSlot,
    ) -> (String, GlobalIfuncData, u32) {
        let GlobalIfuncBuilder {
            module: _,
            name,
            value_type: _,
            resolver: _,
            address_space,
            linkage,
            dso_locality,
            visibility,
            dll_storage_class,
            thread_local_mode,
            unnamed_addr,
            partition,
        } = self;
        let data = GlobalIfuncData {
            value_type,
            address_space,
            resolver: Cell::new(resolver),
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
fn pointer_address_space<B: ModuleBrand, C: Capability>(ty: Type<'_, B, C>) -> Option<u32> {
    match ty.kind() {
        TypeKind::Pointer { addr_space } => Some(addr_space),
        _ => None,
    }
}

/// Ports `GlobalIFunc::isValidLinkage` (`IR/GlobalIFunc.h`):
/// `isExternalLinkage(L) || isLocalLinkage(L) || isWeakLinkage(L) ||
/// isLinkOnceLinkage(L)`. `isWeakLinkage` is `weak` / `weak_odr` only, so
/// `extern_weak` is not valid for an ifunc (`test/Verifier/ifunc.ll`'s
/// `@inval_linkage`).
#[inline]
pub const fn is_valid_ifunc_linkage(linkage: Linkage) -> bool {
    // `isExternalLinkage(L)`
    matches!(linkage, Linkage::External)
        // `isLocalLinkage(L)` — internal or private.
        || matches!(linkage, Linkage::Internal | Linkage::Private)
        // `isWeakLinkage(L)` — `isWeakAnyLinkage` or `isWeakODRLinkage`.
        || matches!(linkage, Linkage::WeakAny | Linkage::WeakOdr)
        // `isLinkOnceLinkage(L)` — `isLinkOnceAnyLinkage` or `isLinkOnceODRLinkage`.
        || matches!(linkage, Linkage::LinkOnceAny | Linkage::LinkOnceOdr)
}

impl<'ctx, B: ModuleBrand + 'ctx, C: Capability> core::fmt::Display for GlobalIfunc<'ctx, B, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_ifunc(f, *self)
    }
}
