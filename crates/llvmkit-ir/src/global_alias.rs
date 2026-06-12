//! Module-level global alias. Mirrors `llvm/include/llvm/IR/GlobalAlias.h`.

use core::cell::{Cell, RefCell};

use crate::DebugLoc;
use crate::constant::{Constant, IsConstant};
use crate::error::{IrError, IrResult, TypeKindLabel, ValueCategoryLabel};
use crate::global_value::{DllStorageClass, Linkage, ThreadLocalMode, Visibility};
use crate::module::{Module, ModuleRef};
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalAlias<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

impl<'ctx> GlobalAlias<'ctx> {
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

    fn data(self) -> &'ctx GlobalAliasData {
        match &self.module.value_data(self.id).kind {
            ValueKindData::GlobalAlias(a) => a,
            _ => unreachable!("GlobalAlias handle invariant: ValueKindData::GlobalAlias"),
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

    pub fn aliasee(self) -> Constant<'ctx> {
        let id = self.data().aliasee.get();
        let value_data = self.module.value_data(id);
        Constant {
            id,
            module: self.module,
            ty: value_data.ty,
        }
    }

    pub fn set_aliasee<C: IsConstant<'ctx>>(self, aliasee: C) -> IrResult<()> {
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

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.data().dll_storage_class.get()
    }

    #[inline]
    pub fn set_dll_storage_class(self, cls: DllStorageClass) {
        self.data().dll_storage_class.set(cls);
    }

    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.data().thread_local_mode.get()
    }

    #[inline]
    pub fn set_thread_local_mode(self, tlm: ThreadLocalMode) {
        self.data().thread_local_mode.set(tlm);
    }

    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.data().unnamed_addr.get()
    }

    #[inline]
    pub fn set_unnamed_addr(self, value: UnnamedAddr) {
        self.data().unnamed_addr.set(value);
    }

    pub fn partition(self) -> Option<String> {
        self.data().partition.borrow().clone()
    }

    pub fn set_partition(self, partition: Option<impl Into<String>>) {
        *self.data().partition.borrow_mut() = partition.map(Into::into);
    }
}

impl<'ctx> sealed::Sealed for GlobalAlias<'ctx> {}
impl<'ctx> IsValue<'ctx> for GlobalAlias<'ctx> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        GlobalAlias::as_value(self)
    }
}
impl<'ctx> IsConstant<'ctx> for GlobalAlias<'ctx> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx> {
        GlobalAlias::as_constant(self)
    }
}
impl<'ctx> Typed<'ctx> for GlobalAlias<'ctx> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }
}
impl<'ctx> HasName<'ctx> for GlobalAlias<'ctx> {
    fn name(self) -> Option<String> {
        Some(self.data().name.clone())
    }
    fn set_name(self, _name: Option<&str>) {}
}
impl HasDebugLoc for GlobalAlias<'_> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx> From<GlobalAlias<'ctx>> for Value<'ctx> {
    #[inline]
    fn from(a: GlobalAlias<'ctx>) -> Self {
        a.as_value()
    }
}
impl<'ctx> From<GlobalAlias<'ctx>> for Constant<'ctx> {
    #[inline]
    fn from(a: GlobalAlias<'ctx>) -> Self {
        a.as_constant()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for GlobalAlias<'ctx> {
    type Error = IrError;

    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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

pub struct GlobalAliasBuilder<'ctx> {
    module: &'ctx Module<'ctx>,
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

impl<'ctx> GlobalAliasBuilder<'ctx> {
    pub(crate) fn new<C: IsConstant<'ctx>>(
        module: &'ctx Module<'ctx>,
        name: impl Into<String>,
        value_type: Type<'ctx>,
        aliasee: C,
    ) -> Self {
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

    pub fn build(self) -> IrResult<GlobalAlias<'ctx>> {
        if !is_valid_alias_linkage(self.linkage) {
            return Err(IrError::InvalidOperation {
                message: "invalid linkage type for alias",
            });
        }
        if self.module.context().value_data(self.aliasee).ty != self.aliasee_type {
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
        self.module.install_global_alias(self)
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

impl<'ctx> core::fmt::Display for GlobalAlias<'ctx> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_alias(f, *self)
    }
}
