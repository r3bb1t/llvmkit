//! Module-level global variable. Mirrors
//! `llvm/include/llvm/IR/GlobalVariable.h` and the relevant slice of
//! `llvm/lib/IR/Globals.cpp`.
//!
//! A global variable is the IR-level equivalent of a top-level `static`
//! / `extern` symbol in C. Its *value type* (the data behind the
//! pointer) and its *handle type* (`ptr addrspace(N)`) are distinct:
//! callers reference `@g` as a pointer, but the initializer is typed
//! as the value type.
//!
//! Materialise a global through
//! [`Module::add_global`](crate::Module::add_global) /
//! [`Module::add_global_constant`](crate::Module::add_global_constant) /
//! [`Module::global_builder`](crate::Module::global_builder).

use crate::DebugLoc;
use crate::align::MaybeAlign;
use crate::comdat::ComdatRef;
use crate::constant::{Constant, IsConstant};
use crate::derived_types::PointerType;
use crate::error::{IrError, IrResult, ValueCategoryLabel};
use crate::global_value::{DllStorageClass, Linkage, ThreadLocalMode, Visibility};
use crate::module::{Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use crate::r#type::{Type, TypeId};
use crate::unnamed_addr::UnnamedAddr;
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, ValueKindData, sealed};

use crate::metadata::MetadataAttachmentSet;
use core::cell::{Cell, RefCell};

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload for a global variable. Stored in the value
/// arena under `ValueKindData::GlobalVariable`. Mirrors the data
/// portion of `class GlobalVariable` in `IR/GlobalVariable.h`.
#[derive(Debug)]
pub(crate) struct GlobalVariableData {
    pub(crate) name: String,
    /// Type of the data the global holds (the *pointee* type). The
    /// outer [`crate::value::ValueData::ty`] is the *pointer* type
    /// (`ptr addrspace(N)`).
    pub(crate) value_type: TypeId,
    pub(crate) address_space: u32,
    pub(crate) is_constant: bool,
    pub(crate) externally_initialized: Cell<bool>,
    pub(crate) initializer: Cell<Option<ValueId>>,
    pub(crate) linkage: Cell<Linkage>,
    pub(crate) visibility: Cell<Visibility>,
    pub(crate) dll_storage_class: Cell<DllStorageClass>,
    pub(crate) thread_local_mode: Cell<ThreadLocalMode>,
    pub(crate) unnamed_addr: Cell<UnnamedAddr>,
    pub(crate) align: Cell<MaybeAlign>,
    pub(crate) section: RefCell<Option<String>>,
    pub(crate) partition: RefCell<Option<String>>,
    /// Comdat name (no leading `$`). The actual `ComdatData` lives in
    /// the owning module's comdat storage.
    pub(crate) comdat: RefCell<Option<String>>,
    pub(crate) metadata: RefCell<crate::metadata::MetadataAttachmentSet>,
}

// Construction goes through `GlobalBuilder::into_data`.

// --------------------------------------------------------------------------
// Public handle
// --------------------------------------------------------------------------

/// Module-level global variable handle. Mirrors `GlobalVariable *`
/// in upstream LLVM.
///
/// The handle's [`Typed`] / [`Value::ty`] is the *pointer* type
/// (`ptr addrspace(N)`). Use [`Self::value_type`] to obtain the type
/// of the stored data, and [`Self::initializer`] to read the
/// initializer when one is present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalVariable<'ctx, B: crate::module::ModuleBrand = crate::module::Brand<'ctx>> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx, B>,
    /// Cached pointer type id (`ptr addrspace(N)`).
    pub(crate) ty: TypeId,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalVariable<'ctx, B> {
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

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Widen to the erased [`Constant`] handle. Globals are
    /// constants in the LLVM sense (their *address* is the constant);
    /// this lets them appear in initializers of other globals or as
    /// `ConstantExpr` operands.
    #[inline]
    pub fn as_constant(self) -> Constant<'ctx, B> {
        Constant {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// View this global as a pointer-typed constant reference. Mirrors
    /// `GlobalValue::getType`: the global's stored value type is separate from
    /// the `ptr addrspace(N)` type used when its address appears as a constant.
    #[inline]
    pub fn as_global_constant_ptr(self) -> Constant<'ctx, B> {
        let module = self.module.module();
        let ptr_ty = module.ptr_type(self.address_space()).as_type().id();
        let id = module
            .context()
            .intern_constant_global_value_ref(ptr_ty, self.id);
        Constant {
            id,
            module: self.module,
            ty: ptr_ty,
        }
    }

    /// A `ptr`-typed constant pointing `off` bytes into this global, printed as
    /// `getelementptr inbounds (i8, ptr @<self>, i64 off)`.
    ///
    /// llvmkit has no general `ConstantExpr`, so a pointer into the *middle* of
    /// a global cannot be spelled directly; this materialises the one offset
    /// form needed for symbol-relative initializers (a relocated pointer slot
    /// inside an embedded data section that targets another section's interior).
    /// `off == 0` is equivalent to [`Self::as_constant`] but always prints the
    /// gep form; prefer `as_constant` for the zero case.
    pub fn as_global_constant_ptr_offset(self, off: i64, addr_space: u32) -> Constant<'ctx, B> {
        let module = self.module.module();
        let ptr_ty = module.ptr_type(addr_space).as_type().id();
        let id = module
            .context()
            .intern_constant_gep_offset(ptr_ty, self.id, off);
        Constant {
            id,
            module: self.module,
            ty: ptr_ty,
        }
    }

    /// A `ptr`-typed constant pointing `off` bytes into this global, preserving
    /// this global's address space in both the GEP result and pointer operand.
    pub fn ptr_offset(self, off: i64) -> Constant<'ctx, B> {
        let module = self.module.module();
        let ptr_ty = module.ptr_type(self.address_space()).as_type().id();
        let id = module
            .context()
            .intern_constant_gep_offset(ptr_ty, self.id, off);
        Constant {
            id,
            module: self.module,
            ty: ptr_ty,
        }
    }

    /// An `i64` constant equal to `self_addr - other_addr`, printed as the
    /// const-expr `sub (i64 ptrtoint (ptr @self to i64), i64 ptrtoint (ptr
    /// @other to i64))`.
    ///
    /// llvmkit has no general `ConstantExpr`; this materialises the one
    /// two-symbol difference form needed to bake a link-time delta into a
    /// global initializer. lld resolves the subtraction at link time, so the
    /// delta is a real constant in the image without either absolute address
    /// being known at emit time. Use it to express an address as
    /// store `real.try_delta_from(anchor)?` in a data global and add it to
    /// `ptrtoint @anchor` at the use site. Both globals must be defined symbols
    /// in the final image.
    pub fn try_delta_from(
        self,
        other: GlobalVariable<'ctx, B>,
    ) -> IrResult<crate::ConstantIntValue<'ctx, i64, B>> {
        if self.module != other.module {
            return Err(IrError::ForeignValue);
        }
        let module = self.module.module();
        let i64_ty = module.i64_type().as_type().id();
        let id = module
            .context()
            .intern_constant_symbol_delta(i64_ty, self.id, other.id);
        Ok(crate::ConstantIntValue::<i64, B>::from_parts_typed(
            Constant {
                id,
                module: self.module,
                ty: i64_ty,
            },
        ))
    }

    /// An `i64` constant equal to `(self_addr - other_addr) + addend`, printed
    /// as `add (i64 sub (i64 ptrtoint(@real), i64 ptrtoint(@anchor)),
    /// i64 K)` -- the encrypted-delta form.
    pub fn try_delta_from_plus(
        self,
        other: GlobalVariable<'ctx, B>,
        addend: i64,
    ) -> IrResult<crate::ConstantIntValue<'ctx, i64, B>> {
        if self.module != other.module {
            return Err(IrError::ForeignValue);
        }
        let module = self.module.module();
        let i64_ty = module.i64_type().as_type().id();
        let id = module
            .context()
            .intern_constant_symbol_delta_plus(i64_ty, self.id, other.id, addend);
        Ok(crate::ConstantIntValue::<i64, B>::from_parts_typed(
            Constant {
                id,
                module: self.module,
                ty: i64_ty,
            },
        ))
    }

    fn data(self) -> &'ctx GlobalVariableData {
        match &self.module.value_data(self.id).kind {
            ValueKindData::GlobalVariable(g) => g,
            _ => unreachable!("GlobalVariable handle invariant: ValueKindData::GlobalVariable"),
        }
    }

    /// Owning module (borrowed for the lifetime of the handle).
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    /// Pointer type (`ptr addrspace(N)`) of `@<name>`.
    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B> {
        PointerType::new(self.ty, self.module)
    }

    /// Type of the stored data (the *pointee* type). Mirrors
    /// `GlobalValue::getValueType`.
    #[inline]
    pub fn value_type(self) -> Type<'ctx, B> {
        Type::new(self.data().value_type, self.module)
    }

    /// Address space of the global. Mirrors
    /// `GlobalValue::getAddressSpace`.
    #[inline]
    pub fn address_space(self) -> u32 {
        self.data().address_space
    }

    /// Symbol name (without the leading `@`).
    #[inline]
    pub fn name(self) -> &'ctx str {
        &self.data().name
    }

    /// `true` if this was declared with `constant` (vs `global`).
    /// Mirrors `GlobalVariable::isConstant`.
    #[inline]
    pub fn is_constant(self) -> bool {
        self.data().is_constant
    }

    /// Whether the global has an `externally_initialized` marker.
    /// Mirrors `GlobalVariable::isExternallyInitialized`.
    #[inline]
    pub fn is_externally_initialized(self) -> bool {
        self.data().externally_initialized.get()
    }

    /// `true` if the global has an initializer attached. Globals
    /// without an initializer are *declarations*; with one they are
    /// *definitions*. Mirrors `GlobalVariable::hasInitializer`.
    #[inline]
    pub fn has_initializer(self) -> bool {
        self.data().initializer.get().is_some()
    }

    /// Initializer constant, if any. Mirrors
    /// `GlobalVariable::getInitializer`.
    pub fn initializer(self) -> Option<Constant<'ctx, B>> {
        let id = self.data().initializer.get()?;
        let value_data = self.module.value_data(id);
        Some(Constant {
            id,
            module: self.module,
            ty: value_data.ty,
        })
    }

    /// Set the initializer. Mirrors
    /// `GlobalVariable::setInitializer`. Errors with
    /// [`IrError::TypeMismatch`] when the initializer's type does not
    /// match the global's value type, with [`IrError::ForeignValue`]
    /// when the constant belongs to a different module.
    pub fn set_initializer<C>(self, _module: &Module<'ctx, B, Unverified>, init: C) -> IrResult<()>
    where
        C: IsConstant<'ctx, B>,
    {
        let constant = init.as_constant();
        if constant.module != self.module {
            return Err(IrError::ForeignValue);
        }
        if constant.ty != self.data().value_type {
            let value_ty = self.value_type();
            return Err(IrError::TypeMismatch {
                expected: value_ty.kind_label(),
                got: constant.ty().kind_label(),
            });
        }
        self.data().initializer.set(Some(constant.id));
        Ok(())
    }

    /// Clear the initializer.
    pub fn clear_initializer(self, _module: &Module<'ctx, B, Unverified>) {
        self.data().initializer.set(None);
    }

    /// Linkage. Mirrors `GlobalValue::getLinkage`.
    #[inline]
    pub fn linkage(self) -> Linkage {
        self.data().linkage.get()
    }

    /// Update the linkage. Mirrors `GlobalValue::setLinkage`.
    #[inline]
    pub fn set_linkage(self, _module: &Module<'ctx, B, Unverified>, linkage: Linkage) {
        self.data().linkage.set(linkage);
    }

    /// Visibility. Mirrors `GlobalValue::getVisibility`.
    #[inline]
    pub fn visibility(self) -> Visibility {
        self.data().visibility.get()
    }

    /// Update visibility. Mirrors `GlobalValue::setVisibility`.
    #[inline]
    pub fn set_visibility(self, _module: &Module<'ctx, B, Unverified>, vis: Visibility) {
        self.data().visibility.set(vis);
    }

    /// DLL storage class. Mirrors `GlobalValue::getDLLStorageClass`.
    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.data().dll_storage_class.get()
    }

    /// Update DLL storage class. Mirrors
    /// `GlobalValue::setDLLStorageClass`.
    #[inline]
    pub fn set_dll_storage_class(
        self,
        _module: &Module<'ctx, B, Unverified>,
        cls: DllStorageClass,
    ) {
        self.data().dll_storage_class.set(cls);
    }

    /// Thread-local mode. Mirrors `GlobalValue::getThreadLocalMode`.
    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.data().thread_local_mode.get()
    }

    /// Update thread-local mode. Mirrors
    /// `GlobalValue::setThreadLocalMode`.
    #[inline]
    pub fn set_thread_local_mode(
        self,
        _module: &Module<'ctx, B, Unverified>,
        tlm: ThreadLocalMode,
    ) {
        self.data().thread_local_mode.set(tlm);
    }

    /// Unnamed-addr marker. Mirrors `GlobalValue::getUnnamedAddr`.
    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.data().unnamed_addr.get()
    }

    /// Update unnamed-addr marker. Mirrors
    /// `GlobalValue::setUnnamedAddr`.
    #[inline]
    pub fn set_unnamed_addr(self, _module: &Module<'ctx, B, Unverified>, value: UnnamedAddr) {
        self.data().unnamed_addr.set(value);
    }

    /// Required alignment, if any. Mirrors `GlobalValue::getAlign`.
    #[inline]
    pub fn align(self) -> MaybeAlign {
        self.data().align.get()
    }

    /// Set or clear the alignment. Mirrors `GlobalValue::setAlignment`.
    #[inline]
    pub fn set_align(self, _module: &Module<'ctx, B, Unverified>, align: MaybeAlign) {
        self.data().align.set(align);
    }

    /// `true` if the global has been pinned to a custom section.
    /// Mirrors `GlobalValue::hasSection`.
    #[inline]
    pub fn has_section(self) -> bool {
        self.data().section.borrow().is_some()
    }

    /// Section name, if set. Mirrors `GlobalValue::getSection`.
    pub fn section(self) -> Option<String> {
        self.data().section.borrow().clone()
    }

    /// Set the section. Mirrors `GlobalValue::setSection`.
    pub fn set_section<S>(self, _module: &Module<'ctx, B, Unverified>, section: S)
    where
        S: Into<String>,
    {
        *self.data().section.borrow_mut() = Some(section.into());
    }

    /// Clear the section.
    pub fn clear_section(self, _module: &Module<'ctx, B, Unverified>) {
        *self.data().section.borrow_mut() = None;
    }

    /// Partition name, if set. Mirrors
    /// `GlobalValue::getPartition`.
    pub fn partition(self) -> Option<String> {
        self.data().partition.borrow().clone()
    }

    /// Set the partition. Mirrors
    /// `GlobalValue::setPartition`.
    pub fn set_partition<P>(self, _module: &Module<'ctx, B, Unverified>, partition: P)
    where
        P: Into<String>,
    {
        *self.data().partition.borrow_mut() = Some(partition.into());
    }

    /// Clear the partition.
    pub fn clear_partition(self, _module: &Module<'ctx, B, Unverified>) {
        *self.data().partition.borrow_mut() = None;
    }

    /// Toggle the `externally_initialized` marker. Mirrors
    /// `GlobalVariable::setExternallyInitialized`.
    #[inline]
    pub fn set_externally_initialized(self, _module: &Module<'ctx, B, Unverified>, value: bool) {
        self.data().externally_initialized.set(value);
    }

    /// Comdat reference, if attached. Mirrors `GlobalValue::getComdat`.
    pub fn comdat(self) -> Option<ComdatRef<'ctx, B>> {
        let name = self.data().comdat.borrow().clone()?;
        self.module.module().get_comdat::<B>(&name)
    }

    /// Attach a comdat. The comdat must already exist in
    /// the owning module (use
    /// [`Module::get_or_insert_comdat`](crate::Module::get_or_insert_comdat)
    /// to materialise one). Errors with
    /// [`IrError::InvalidOperation`] if the comdat does not belong
    /// to this module.
    pub fn set_comdat(
        self,
        _module: &Module<'ctx, B, Unverified>,
        comdat: ComdatRef<'ctx, B>,
    ) -> IrResult<()> {
        if comdat.module != self.module {
            return Err(IrError::InvalidOperation {
                message: "comdat does not belong to this module",
            });
        }
        *self.data().comdat.borrow_mut() = Some(comdat.name().to_owned());
        Ok(())
    }

    /// Clear the attached comdat.
    pub fn clear_comdat(self, _module: &Module<'ctx, B, Unverified>) {
        *self.data().comdat.borrow_mut() = None;
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
}

impl<'ctx, B: ModuleBrand> sealed::Sealed for GlobalVariable<'ctx, B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for GlobalVariable<'ctx, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        GlobalVariable::as_value(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> IsConstant<'ctx, B> for GlobalVariable<'ctx, B> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx, B> {
        GlobalVariable::as_constant(self)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> Typed<'ctx, B> for GlobalVariable<'ctx, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Type::new(self.ty, self.module)
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> HasName<'ctx, B> for GlobalVariable<'ctx, B> {
    fn name(self) -> Option<String> {
        self.as_value().name()
    }
    fn set_name<Name>(self, _module_token: &Module<'ctx, B, Unverified>, _name: Name)
    where
        Name: Into<String>,
    {
        // GlobalVariable names are immutable through this interface
        // -- they participate in the module's name table. Renaming
        // requires a dedicated path that keeps the table consistent.
    }
    fn clear_name(self, _module_token: &Module<'ctx, B, Unverified>) {}
}
impl<B: ModuleBrand + 'static> HasDebugLoc for GlobalVariable<'_, B> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalVariable<'ctx, B>> for Value<'ctx, B> {
    #[inline]
    fn from(g: GlobalVariable<'ctx, B>) -> Self {
        g.as_value()
    }
}
impl<'ctx, B: ModuleBrand + 'ctx> From<GlobalVariable<'ctx, B>> for Constant<'ctx, B> {
    #[inline]
    fn from(g: GlobalVariable<'ctx, B>) -> Self {
        g.as_constant()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for GlobalVariable<'ctx, B> {
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        match &v.data().kind {
            ValueKindData::GlobalVariable(_) => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
            }),
            other => {
                let got = crate::value::category_label_for_kind(other);
                Err(IrError::ValueCategoryMismatch {
                    expected: ValueCategoryLabel::GlobalVariable,
                    got,
                })
            }
        }
    }
}

// --------------------------------------------------------------------------
// Builder
// --------------------------------------------------------------------------

/// Chainable builder for adding a [`GlobalVariable`] to a module.
/// Mirrors the configurable form of `GlobalVariable::GlobalVariable`
/// in `lib/IR/Globals.cpp`. Materialise via [`Self::build`].
///
/// Constructed by
/// [`Module::global_builder`](crate::Module::global_builder).
pub struct GlobalBuilder<'ctx, B: ModuleBrand = crate::module::Brand<'ctx>> {
    module: ModuleRef<'ctx, B>,
    name: String,
    value_type: TypeId,
    address_space: u32,
    is_constant: bool,
    externally_initialized: bool,
    initializer: Option<ValueId>,
    initializer_type: Option<TypeId>,
    linkage: Linkage,
    visibility: Visibility,
    dll_storage_class: DllStorageClass,
    thread_local_mode: ThreadLocalMode,
    unnamed_addr: UnnamedAddr,
    align: MaybeAlign,
    section: Option<String>,
    partition: Option<String>,
    comdat: Option<String>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalBuilder<'ctx, B> {
    pub(crate) fn new<M>(module: M, name: impl Into<String>, value_type: Type<'ctx, B>) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            module: module.into(),
            name: name.into(),
            value_type: value_type.id(),
            address_space: 0,
            is_constant: false,
            externally_initialized: false,
            initializer: None,
            initializer_type: None,
            linkage: Linkage::External,
            visibility: Visibility::Default,
            dll_storage_class: DllStorageClass::Default,
            thread_local_mode: ThreadLocalMode::NotThreadLocal,
            unnamed_addr: UnnamedAddr::None,
            align: MaybeAlign::default(),
            section: None,
            partition: None,
            comdat: None,
        }
    }

    /// Mark as `constant` (vs `global`). Mirrors
    /// `GlobalVariable::setConstant`.
    pub fn constant(mut self, value: bool) -> Self {
        self.is_constant = value;
        self
    }

    /// Address space. Mirrors the `AddressSpace` ctor argument.
    pub fn address_space(mut self, addrspace: u32) -> Self {
        self.address_space = addrspace;
        self
    }

    /// Linkage. Mirrors the `Linkage` ctor argument.
    pub fn linkage(mut self, linkage: Linkage) -> Self {
        self.linkage = linkage;
        self
    }

    /// Visibility. Mirrors `GlobalValue::setVisibility`.
    pub fn visibility(mut self, vis: Visibility) -> Self {
        self.visibility = vis;
        self
    }

    /// DLL storage class. Mirrors
    /// `GlobalValue::setDLLStorageClass`.
    pub fn dll_storage_class(mut self, cls: DllStorageClass) -> Self {
        self.dll_storage_class = cls;
        self
    }

    /// Thread-local mode. Mirrors
    /// `GlobalVariable::setThreadLocalMode`.
    pub fn thread_local_mode(mut self, tlm: ThreadLocalMode) -> Self {
        self.thread_local_mode = tlm;
        self
    }

    /// Unnamed-addr marker. Mirrors
    /// `GlobalValue::setUnnamedAddr`.
    pub fn unnamed_addr(mut self, value: UnnamedAddr) -> Self {
        self.unnamed_addr = value;
        self
    }

    /// Alignment. Mirrors `GlobalValue::setAlignment`.
    pub fn align(mut self, align: MaybeAlign) -> Self {
        self.align = align;
        self
    }

    /// Section name. Mirrors `GlobalValue::setSection`.
    pub fn section<Section>(mut self, section: Section) -> Self
    where
        Section: Into<String>,
    {
        self.section = Some(section.into());
        self
    }

    /// Partition name. Mirrors `GlobalValue::setPartition`.
    pub fn partition<Partition>(mut self, partition: Partition) -> Self
    where
        Partition: Into<String>,
    {
        self.partition = Some(partition.into());
        self
    }

    /// Attach a comdat. Errors at build time if the comdat does not
    /// belong to this module.
    pub fn comdat(mut self, comdat: ComdatRef<'ctx, B>) -> Self {
        // Defer the cross-module check to `build` so the builder
        // remains infallible per call.
        self.comdat = Some(comdat.name().to_owned());
        self
    }

    /// Mark as `externally_initialized`. Mirrors
    /// `GlobalVariable::setExternallyInitialized`.
    pub fn externally_initialized(mut self, value: bool) -> Self {
        self.externally_initialized = value;
        self
    }

    /// Attach an initializer. Errors at build time if the
    /// initializer's type does not match the value type.
    pub fn initializer<C: IsConstant<'ctx, B>>(mut self, init: C) -> Self {
        let constant = init.as_constant();
        self.initializer = Some(constant.id);
        self.initializer_type = Some(constant.ty);
        self
    }

    /// Materialise the global. Mirrors the second
    /// `GlobalVariable::GlobalVariable(Module &M, ...)` ctor.
    pub fn build(self) -> IrResult<GlobalVariable<'ctx, B>> {
        if let Some(init_ty) = self.initializer_type
            && init_ty != self.value_type
        {
            let module = self.module;
            let want = Type::new(self.value_type, module).kind_label();
            let got = Type::new(init_ty, module).kind_label();
            return Err(IrError::TypeMismatch {
                expected: want,
                got,
            });
        }
        if let Some(name) = &self.comdat
            && self.module.module().get_comdat::<B>(name).is_none()
        {
            return Err(IrError::InvalidOperation {
                message: "comdat does not belong to this module",
            });
        }
        self.module.module().install_global_variable::<B>(self)
    }

    pub(crate) fn into_data(self) -> (String, GlobalVariableData, Option<ValueId>, u32, TypeId) {
        let GlobalBuilder {
            module: _,
            name,
            value_type,
            address_space,
            is_constant,
            externally_initialized,
            initializer,
            initializer_type: _,
            linkage,
            visibility,
            dll_storage_class,
            thread_local_mode,
            unnamed_addr,
            align,
            section,
            partition,
            comdat,
        } = self;
        let data = GlobalVariableData {
            name: name.clone(),
            value_type,
            address_space,
            is_constant,
            externally_initialized: Cell::new(externally_initialized),
            initializer: Cell::new(initializer),
            linkage: Cell::new(linkage),
            visibility: Cell::new(visibility),
            dll_storage_class: Cell::new(dll_storage_class),
            thread_local_mode: Cell::new(thread_local_mode),
            unnamed_addr: Cell::new(unnamed_addr),
            align: Cell::new(align),
            section: RefCell::new(section),
            partition: RefCell::new(partition),
            comdat: RefCell::new(comdat),
            metadata: RefCell::new(crate::metadata::MetadataAttachmentSet::new()),
        };
        (name, data, initializer, address_space, value_type)
    }
}
