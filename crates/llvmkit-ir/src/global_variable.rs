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
use crate::module::{Module, ModuleRef};
use crate::r#type::{Type, TypeId};
use crate::unnamed_addr::UnnamedAddr;
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, ValueKindData, sealed};

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
pub struct GlobalVariable<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    /// Cached pointer type id (`ptr addrspace(N)`).
    pub(crate) ty: TypeId,
}

impl<'ctx> GlobalVariable<'ctx> {
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

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(self) -> Value<'ctx> {
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
    pub fn as_constant(self) -> Constant<'ctx> {
        Constant {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    fn data(self) -> &'ctx GlobalVariableData {
        match &self.module.value_data(self.id).kind {
            ValueKindData::GlobalVariable(g) => g,
            _ => unreachable!("GlobalVariable handle invariant: ValueKindData::GlobalVariable"),
        }
    }

    /// Owning module (borrowed for the lifetime of the handle).
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    /// Pointer type (`ptr addrspace(N)`) of `@<name>`.
    #[inline]
    pub fn ty(self) -> PointerType<'ctx> {
        PointerType::new(self.ty, self.module.module())
    }

    /// Type of the stored data (the *pointee* type). Mirrors
    /// `GlobalValue::getValueType`.
    #[inline]
    pub fn value_type(self) -> Type<'ctx> {
        Type::new(self.data().value_type, self.module.module())
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
    pub fn initializer(self) -> Option<Constant<'ctx>> {
        let id = self.data().initializer.get()?;
        let value_data = self.module.value_data(id);
        Some(Constant {
            id,
            module: self.module,
            ty: value_data.ty,
        })
    }

    /// Set or clear the initializer. Mirrors
    /// `GlobalVariable::setInitializer`. Errors with
    /// [`IrError::TypeMismatch`] when the initializer's type does not
    /// match the global's value type, with [`IrError::ForeignValue`]
    /// when the constant belongs to a different module.
    pub fn set_initializer(self, init: Option<impl IsConstant<'ctx>>) -> IrResult<()> {
        match init {
            None => {
                self.data().initializer.set(None);
                Ok(())
            }
            Some(c) => {
                let constant = c.as_constant();
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
        }
    }

    /// Linkage. Mirrors `GlobalValue::getLinkage`.
    #[inline]
    pub fn linkage(self) -> Linkage {
        self.data().linkage.get()
    }

    /// Update the linkage. Mirrors `GlobalValue::setLinkage`.
    #[inline]
    pub fn set_linkage(self, linkage: Linkage) {
        self.data().linkage.set(linkage);
    }

    /// Visibility. Mirrors `GlobalValue::getVisibility`.
    #[inline]
    pub fn visibility(self) -> Visibility {
        self.data().visibility.get()
    }

    /// Update visibility. Mirrors `GlobalValue::setVisibility`.
    #[inline]
    pub fn set_visibility(self, vis: Visibility) {
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
    pub fn set_dll_storage_class(self, cls: DllStorageClass) {
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
    pub fn set_thread_local_mode(self, tlm: ThreadLocalMode) {
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
    pub fn set_unnamed_addr(self, value: UnnamedAddr) {
        self.data().unnamed_addr.set(value);
    }

    /// Required alignment, if any. Mirrors `GlobalValue::getAlign`.
    #[inline]
    pub fn align(self) -> MaybeAlign {
        self.data().align.get()
    }

    /// Set or clear the alignment. Mirrors `GlobalValue::setAlignment`.
    #[inline]
    pub fn set_align(self, align: MaybeAlign) {
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

    /// Set or clear the section. Mirrors `GlobalValue::setSection`.
    pub fn set_section(self, section: Option<impl Into<String>>) {
        *self.data().section.borrow_mut() = section.map(Into::into);
    }

    /// Partition name, if set. Mirrors
    /// `GlobalValue::getPartition`.
    pub fn partition(self) -> Option<String> {
        self.data().partition.borrow().clone()
    }

    /// Set or clear the partition. Mirrors
    /// `GlobalValue::setPartition`.
    pub fn set_partition(self, partition: Option<impl Into<String>>) {
        *self.data().partition.borrow_mut() = partition.map(Into::into);
    }

    /// Toggle the `externally_initialized` marker. Mirrors
    /// `GlobalVariable::setExternallyInitialized`.
    #[inline]
    pub fn set_externally_initialized(self, value: bool) {
        self.data().externally_initialized.set(value);
    }

    /// Comdat reference, if attached. Mirrors `GlobalValue::getComdat`.
    pub fn comdat(self) -> Option<ComdatRef<'ctx>> {
        let name = self.data().comdat.borrow().clone()?;
        self.module.module().get_comdat(&name)
    }

    /// Attach (or clear) a comdat. The comdat must already exist in
    /// the owning module (use
    /// [`Module::get_or_insert_comdat`](crate::Module::get_or_insert_comdat)
    /// to materialise one). Errors with
    /// [`IrError::InvalidOperation`] if the comdat does not belong
    /// to this module.
    pub fn set_comdat(self, comdat: Option<ComdatRef<'ctx>>) -> IrResult<()> {
        match comdat {
            None => {
                *self.data().comdat.borrow_mut() = None;
                Ok(())
            }
            Some(c) => {
                if c.module != self.module {
                    return Err(IrError::InvalidOperation {
                        message: "comdat does not belong to this module",
                    });
                }
                *self.data().comdat.borrow_mut() = Some(c.name().to_owned());
                Ok(())
            }
        }
    }
}

impl<'ctx> sealed::Sealed for GlobalVariable<'ctx> {}
impl<'ctx> IsValue<'ctx> for GlobalVariable<'ctx> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        GlobalVariable::as_value(self)
    }
}
impl<'ctx> IsConstant<'ctx> for GlobalVariable<'ctx> {
    #[inline]
    fn as_constant(self) -> Constant<'ctx> {
        GlobalVariable::as_constant(self)
    }
}
impl<'ctx> Typed<'ctx> for GlobalVariable<'ctx> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }
}
impl<'ctx> HasName<'ctx> for GlobalVariable<'ctx> {
    fn name(self) -> Option<String> {
        Some(self.data().name.clone())
    }
    fn set_name(self, _name: Option<&str>) {
        // GlobalVariable names are immutable through this interface
        // -- they participate in the module's name table. Renaming
        // requires a dedicated path that keeps the table consistent.
    }
}
impl HasDebugLoc for GlobalVariable<'_> {
    fn debug_loc(self) -> Option<DebugLoc> {
        None
    }
}

impl<'ctx> From<GlobalVariable<'ctx>> for Value<'ctx> {
    #[inline]
    fn from(g: GlobalVariable<'ctx>) -> Self {
        g.as_value()
    }
}
impl<'ctx> From<GlobalVariable<'ctx>> for Constant<'ctx> {
    #[inline]
    fn from(g: GlobalVariable<'ctx>) -> Self {
        g.as_constant()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for GlobalVariable<'ctx> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
        match &v.data().kind {
            ValueKindData::GlobalVariable(_) => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
            }),
            other => {
                let got = match other {
                    ValueKindData::Constant(_) => ValueCategoryLabel::Constant,
                    ValueKindData::Argument { .. } => ValueCategoryLabel::Argument,
                    ValueKindData::BasicBlock(_) => ValueCategoryLabel::BasicBlock,
                    ValueKindData::Function(_) => ValueCategoryLabel::Function,
                    ValueKindData::Instruction(_) => ValueCategoryLabel::Instruction,
                    ValueKindData::GlobalVariable(_) => ValueCategoryLabel::GlobalVariable,
                };
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
pub struct GlobalBuilder<'ctx> {
    module: &'ctx Module<'ctx>,
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

impl<'ctx> GlobalBuilder<'ctx> {
    pub(crate) fn new(
        module: &'ctx Module<'ctx>,
        name: impl Into<String>,
        value_type: Type<'ctx>,
    ) -> Self {
        Self {
            module,
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
    pub fn section(mut self, section: impl Into<String>) -> Self {
        self.section = Some(section.into());
        self
    }

    /// Partition name. Mirrors `GlobalValue::setPartition`.
    pub fn partition(mut self, partition: impl Into<String>) -> Self {
        self.partition = Some(partition.into());
        self
    }

    /// Attach a comdat. Errors at build time if the comdat does not
    /// belong to this module.
    pub fn comdat(mut self, comdat: ComdatRef<'ctx>) -> Self {
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
    pub fn initializer<C: IsConstant<'ctx>>(mut self, init: C) -> Self {
        let constant = init.as_constant();
        self.initializer = Some(constant.id);
        self.initializer_type = Some(constant.ty);
        self
    }

    /// Materialise the global. Mirrors the second
    /// `GlobalVariable::GlobalVariable(Module &M, ...)` ctor.
    pub fn build(self) -> IrResult<GlobalVariable<'ctx>> {
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
            && self.module.get_comdat(name).is_none()
        {
            return Err(IrError::InvalidOperation {
                message: "comdat does not belong to this module",
            });
        }
        self.module.install_global_variable(self)
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
        };
        (name, data, initializer, address_space, value_type)
    }
}
