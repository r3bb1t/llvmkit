//! Module-level indirect function. Mirrors `llvm/include/llvm/IR/GlobalIFunc.h`.

use crate::Branded;
use core::cell::{Cell, RefCell};

use super::DebugLoc;
use super::constant::{Constant, IsConstant};
use super::derived_types::PointerType;
use super::error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use super::global_value::{DsoLocality, Linkage, Visibility};
use super::metadata::MetadataAttachmentSet;
use super::metadata::{MetadataAttachmentKind, MetadataId, StoredBrand};
use super::module::{Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::r#type::{Type, TypeKind, TypeSlot};
use super::value::{
    GlobalFieldKind, HasDebugLoc, HasName, IsValue, Typed, Value, ValueKindData, ValueSlot, sealed,
};
use super::value_id::GlobalIfuncId;

#[derive(Debug)]
pub(super) struct GlobalIfuncData {
    pub(super) name: String,
    pub(super) value_type: TypeSlot,
    pub(super) address_space: u32,
    pub(super) resolver: Cell<ValueSlot>,
    pub(super) linkage: Cell<Linkage>,
    pub(super) dso_locality: Cell<DsoLocality>,
    pub(super) visibility: Cell<Visibility>,
    pub(super) partition: RefCell<Option<String>>,
    pub(super) metadata: RefCell<MetadataAttachmentSet<StoredBrand>>,
}

#[derive(Branded)]
pub struct GlobalIfunc<'ctx, B: ModuleBrand> {
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalIfunc<'ctx, B> {
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

    /// Storable, module-tagged [`GlobalIfuncId`] for this `ifunc` (llvmkit
    /// 2.0), resolvable via [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view).
    #[inline]
    pub fn id(self) -> GlobalIfuncId<B> {
        GlobalIfuncId::from_raw(self.module.id(), self.id)
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
        _module: &'ctx Module<B, Unverified>,
        resolver: C,
    ) -> IrResult<()> {
        let constant = resolver.as_constant();
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
            Some(constant.id),
        );
        self.data().resolver.set(constant.id);
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

impl<'ctx, B: ModuleBrand> sealed::Sealed for GlobalIfunc<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for GlobalIfunc<'ctx, B> {
    #[inline]
    fn as_erased(self) -> Value<'ctx, B> {
        GlobalIfunc::as_erased(self)
    }
}
crate::value::impl_into_erased_value_for_handle!(GlobalIfunc);
impl<'ctx, B: ModuleBrand + 'ctx> IsConstant<'ctx, B> for GlobalIfunc<'ctx, B> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B> {
        GlobalIfunc::as_constant(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for GlobalIfunc<'ctx, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasName<'ctx, B> for GlobalIfunc<'ctx, B> {
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
impl<B: ModuleBrand + 'static> HasDebugLoc for GlobalIfunc<'_, B> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalIfunc<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(i: GlobalIfunc<'ctx, B>) -> Self {
        i.as_erased()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalIfunc<'ctx, B>> for Constant<'ctx, B> {
    #[inline]
    fn from(i: GlobalIfunc<'ctx, B>) -> Self {
        i.as_constant()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for GlobalIfunc<'ctx, B> {
    type Error = IrError;

    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        match &v.data().kind {
            ValueKindData::GlobalIfunc(_) => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
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
    value_type: TypeSlot,
    resolver: ValueSlot,
    resolver_type: TypeSlot,
    address_space: u32,
    linkage: Linkage,
    dso_locality: DsoLocality,
    visibility: Visibility,
    partition: Option<String>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalIfuncBuilder<'ctx, B> {
    pub(super) fn new<M, C, N>(module: M, name: N, value_type: Type<'ctx, B>, resolver: C) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
        C: IsConstant<'ctx, B>,
        N: Into<String>,
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
            dso_locality: DsoLocality::Default,
            visibility: Visibility::Default,
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
    pub fn build(self) -> IrResult<GlobalIfuncId<B>> {
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
        self.module
            .module()
            .install_global_ifunc::<B>(self)
            .map(|f| f.id())
    }

    pub(super) fn into_data(self) -> (String, GlobalIfuncData, u32) {
        let GlobalIfuncBuilder {
            module: _,
            name,
            value_type,
            resolver,
            resolver_type: _,
            address_space,
            linkage,
            dso_locality,
            visibility,
            partition,
        } = self;
        let data = GlobalIfuncData {
            name: name.clone(),
            value_type,
            address_space,
            resolver: Cell::new(resolver),
            linkage: Cell::new(linkage),
            dso_locality: Cell::new(dso_locality),
            visibility: Cell::new(visibility),
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
pub const fn is_valid_ifunc_linkage(linkage: Linkage) -> bool {
    matches!(
        linkage,
        Linkage::External
            | Linkage::LinkOnceAny
            | Linkage::LinkOnceOdr
            | Linkage::WeakAny
            | Linkage::WeakOdr
            | Linkage::Internal
            | Linkage::Private
            | Linkage::ExternalWeak
    )
}

impl<'ctx, B: ModuleBrand + 'ctx> core::fmt::Display for GlobalIfunc<'ctx, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_ifunc(f, *self)
    }
}
