//! Top-level IR container. Mirrors `llvm/include/llvm/IR/Module.h` and
//! `llvm/lib/IR/Module.cpp`.
//!
//! Phase A surface: name + every type constructor required by
//! `IRBuilder` and the `.ll` parser. Functions, globals, named metadata,
//! and the data-layout subsystem land in Phase D.
//!
//! ## Identity and verification model
//!
//! A [`Module`] is a linear token over crate-private `ModuleCore` storage.
//! The token carries a generative [`ModuleBrand`] and a verification state:
//! [`Unverified`] while IR is still being built and [`Verified`] after
//! structural verification succeeds. Handles store a state-erased
//! [`ModuleRef`] with the same brand, so same-brand APIs reject cross-module
//! values statically and erased/parser paths can still fall back to
//! [`ModuleId`] checks.
//!
//! Public handle accessors expose [`ModuleView`], a read-only branded view of
//! the storage. Construction and mutation require the unverified [`Module`]
//! token instead of a raw storage reference.

use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::align::MaybeAlign;
use crate::attributes::AttributeStorage;
use crate::comdat::{ComdatRef, SelectionKind};
use crate::constant::{Constant, IsConstant};
use crate::data_layout::DataLayout;
use crate::derived_types::{
    ArrayType, FloatType, FunctionType, IntType, LabelType, MetadataType, PointerType, StructType,
    TargetExtType, TokenType, VectorType, VoidType,
};
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::float_kind::{BFloat, Fp128, Half, PpcFp128, X86Fp80};
use crate::global_alias::GlobalAlias;
use crate::global_ifunc::GlobalIFunc;
use crate::global_value::{DllStorageClass, Linkage, ThreadLocalMode, Visibility};
use crate::int_width::{IntDyn, Width};
use crate::llvm_context::Context;
use crate::metadata::{
    MetadataAttachmentSet, MetadataId, MetadataKind, MetadataRef, SpecializedMetadataNode,
};
use crate::named_md_node::NamedMDNode;
use crate::r#type::{MAX_INT_BITS, MIN_INT_BITS, StructBody, Type, TypeId};
use crate::typed_pointer_type::TypedPointerType;
use crate::unnamed_addr::UnnamedAddr;
use crate::value::{ValueId, ValueUse};

// --------------------------------------------------------------------------
// ModuleId
// --------------------------------------------------------------------------

/// Globally-unique module identifier. Assigned at construction by an
/// atomic counter; never reused within a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(NonZeroU32);

impl ModuleId {
    /// Allocate the next unused id. The counter starts at 1 so the
    /// underlying `NonZeroU32` always has its niche populated.
    fn fresh() -> Self {
        // `Relaxed` is fine: the counter only needs uniqueness, not
        // happens-before ordering with any other memory operation.
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let raw = NEXT.fetch_add(1, Ordering::Relaxed);
        let nz = NonZeroU32::new(raw).expect("ModuleId counter overflow (>u32::MAX modules)");
        Self(nz)
    }

    /// Raw integer value. Useful for diagnostics.
    #[inline]
    pub fn as_u32(self) -> u32 {
        self.0.get()
    }
}

// --------------------------------------------------------------------------
// Module brands and verification state
// --------------------------------------------------------------------------

pub(crate) mod brand_sealed {
    pub trait Sealed {}
}

/// Sealed marker for a generative module identity brand.
pub trait ModuleBrand: brand_sealed::Sealed + Copy + core::fmt::Debug + Eq + Hash {}

/// Concrete lifetime-generated module brand used by [`Module::with_new`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Brand<'id>(PhantomData<fn(&'id ()) -> &'id ()>);
impl<'id> brand_sealed::Sealed for Brand<'id> {}
impl<'id> ModuleBrand for Brand<'id> {}

/// Module state before successful structural verification.
#[derive(Debug)]
pub enum Unverified {}

/// Module state after successful structural verification.
#[derive(Debug)]
pub enum Verified {}

pub(crate) type Invariant<T> = PhantomData<fn(T) -> T>;

// --------------------------------------------------------------------------
// ModuleRef helper
// --------------------------------------------------------------------------

/// State-erased reference to a module's storage.
///
/// The reference carries the invariant module brand `B`, but it points at
/// crate-private `ModuleCore` storage rather than a `Module<..., State>` token,
/// so handles do not borrow the verification state.
pub struct ModuleRef<'ctx, B: ModuleBrand = Brand<'ctx>> {
    core: &'ctx ModuleCore,
    _brand: Invariant<B>,
}

impl<B: ModuleBrand> Clone for ModuleRef<'_, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: ModuleBrand> Copy for ModuleRef<'_, B> {}

impl<'ctx, B: ModuleBrand> ModuleRef<'ctx, B> {
    #[inline]
    pub(crate) fn new(core: &'ctx ModuleCore) -> Self {
        Self {
            core,
            _brand: PhantomData,
        }
    }

    /// Borrow the underlying state-erased module storage.
    pub(crate) fn module(self) -> &'ctx ModuleCore {
        self.core
    }

    /// Owning module's [`ModuleId`].
    #[inline]
    pub fn id(self) -> ModuleId {
        self.core.id
    }

    /// Crate-internal: resolve a [`TypeId`] to its payload via the
    /// owning module's context.
    #[inline]
    pub(crate) fn type_data(self, id: crate::r#type::TypeId) -> &'ctx crate::r#type::TypeData {
        self.core.context().type_data(id)
    }

    /// Crate-internal: resolve a [`ValueId`](crate::value::ValueId) to its
    /// payload via the owning module's context.
    #[inline]
    pub(crate) fn value_data(self, id: crate::value::ValueId) -> &'ctx crate::value::ValueData {
        self.core.context().value_data(id)
    }
}

impl<'ctx> From<&'ctx ModuleCore> for ModuleRef<'ctx, Brand<'ctx>> {
    #[inline]
    fn from(core: &'ctx ModuleCore) -> Self {
        ModuleRef::new(core)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, S> From<&'ctx Module<'ctx, B, S>> for ModuleRef<'ctx, B> {
    #[inline]
    fn from(module: &'ctx Module<'ctx, B, S>) -> Self {
        ModuleRef::new(module.core)
    }
}

impl<B: ModuleBrand> PartialEq for ModuleRef<'_, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.core.id == other.core.id
    }
}
impl<B: ModuleBrand> Eq for ModuleRef<'_, B> {}
impl<B: ModuleBrand> Hash for ModuleRef<'_, B> {
    #[inline]
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.core.id.hash(h);
    }
}
impl<B: ModuleBrand> core::fmt::Debug for ModuleRef<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ModuleRef").field(&self.core.id).finish()
    }
}

// --------------------------------------------------------------------------
// ModuleView helper
// --------------------------------------------------------------------------

/// Read-only branded view of a module.
///
/// `ModuleView` lets handles report their owning module without exposing the
/// crate-private storage or the linear verification-state token.
#[derive(Clone, Copy)]
pub struct ModuleView<'ctx, B: ModuleBrand> {
    core: &'ctx ModuleCore,
    _brand: Invariant<B>,
}

/// Read-only branded view of a global variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalVariableView<'ctx, B: ModuleBrand> {
    global: crate::global_variable::GlobalVariable<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalVariableView<'ctx, B> {
    #[inline]
    pub(crate) fn new(global: crate::global_variable::GlobalVariable<'ctx, B>) -> Self {
        Self { global }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        self.global.module()
    }

    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B> {
        self.global.ty()
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx, B> {
        self.global.value_type()
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.global.address_space()
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        self.global.name()
    }

    #[inline]
    pub fn is_constant(self) -> bool {
        self.global.is_constant()
    }

    #[inline]
    pub fn is_externally_initialized(self) -> bool {
        self.global.is_externally_initialized()
    }

    #[inline]
    pub fn has_initializer(self) -> bool {
        self.global.has_initializer()
    }

    #[inline]
    pub fn initializer(self) -> Option<Constant<'ctx, B>> {
        self.global.initializer()
    }

    #[inline]
    pub fn linkage(self) -> Linkage {
        self.global.linkage()
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.global.visibility()
    }

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.global.dll_storage_class()
    }

    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.global.thread_local_mode()
    }

    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.global.unnamed_addr()
    }

    #[inline]
    pub fn align(self) -> MaybeAlign {
        self.global.align()
    }

    #[inline]
    pub fn has_section(self) -> bool {
        self.global.has_section()
    }

    #[inline]
    pub fn section(self) -> Option<String> {
        self.global.section()
    }

    #[inline]
    pub fn partition(self) -> Option<String> {
        self.global.partition()
    }

    #[inline]
    pub fn comdat(self) -> Option<ComdatView<'ctx, B>> {
        self.global.comdat().map(ComdatView::new)
    }

    #[inline]
    pub fn metadata(self) -> core::cell::Ref<'ctx, MetadataAttachmentSet> {
        self.global.metadata()
    }
}

/// Read-only branded view of a global alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalAliasView<'ctx, B: ModuleBrand> {
    alias: crate::global_alias::GlobalAlias<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalAliasView<'ctx, B> {
    #[inline]
    pub(crate) fn new(alias: crate::global_alias::GlobalAlias<'ctx, B>) -> Self {
        Self { alias }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        self.alias.module()
    }

    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B> {
        self.alias.ty()
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx, B> {
        self.alias.value_type()
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.alias.address_space()
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        self.alias.name()
    }

    #[inline]
    pub fn aliasee(self) -> Constant<'ctx, B> {
        self.alias.aliasee()
    }

    #[inline]
    pub fn linkage(self) -> Linkage {
        self.alias.linkage()
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.alias.visibility()
    }

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.alias.dll_storage_class()
    }

    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.alias.thread_local_mode()
    }

    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.alias.unnamed_addr()
    }

    #[inline]
    pub fn metadata(self) -> core::cell::Ref<'ctx, MetadataAttachmentSet> {
        self.alias.metadata()
    }

    #[inline]
    pub fn partition(self) -> Option<String> {
        self.alias.partition()
    }
}

/// Read-only branded view of a global ifunc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalIFuncView<'ctx, B: ModuleBrand> {
    ifunc: crate::global_ifunc::GlobalIFunc<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalIFuncView<'ctx, B> {
    #[inline]
    pub(crate) fn new(ifunc: crate::global_ifunc::GlobalIFunc<'ctx, B>) -> Self {
        Self { ifunc }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        self.ifunc.module()
    }

    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B> {
        self.ifunc.ty()
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx, B> {
        self.ifunc.value_type()
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.ifunc.address_space()
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        self.ifunc.name()
    }

    #[inline]
    pub fn resolver(self) -> Constant<'ctx, B> {
        self.ifunc.resolver()
    }

    #[inline]
    pub fn linkage(self) -> Linkage {
        self.ifunc.linkage()
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.ifunc.visibility()
    }

    #[inline]
    pub fn metadata(self) -> core::cell::Ref<'ctx, MetadataAttachmentSet> {
        self.ifunc.metadata()
    }

    #[inline]
    pub fn partition(self) -> Option<String> {
        self.ifunc.partition()
    }
}

/// Read-only branded view of a COMDAT.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ComdatView<'ctx, B: ModuleBrand> {
    comdat: crate::comdat::ComdatRef<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ComdatView<'ctx, B> {
    #[inline]
    pub(crate) fn new(comdat: crate::comdat::ComdatRef<'ctx, B>) -> Self {
        Self { comdat }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.comdat.module.module())
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        self.comdat.name()
    }

    #[inline]
    pub fn selection_kind(self) -> SelectionKind {
        self.comdat.selection_kind()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleView<'ctx, B> {
    #[inline]
    pub(crate) fn new(core: &'ctx ModuleCore) -> Self {
        Self {
            core,
            _brand: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn core_ref(self) -> &'ctx ModuleCore {
        self.core
    }

    #[inline]
    pub(crate) fn context(self) -> &'ctx Context {
        self.core.context()
    }

    /// Owning module's [`ModuleId`].
    #[inline]
    pub fn id(self) -> ModuleId {
        self.core.id()
    }

    /// Module identifier.
    #[inline]
    pub fn name(self) -> &'ctx str {
        self.core.name()
    }

    /// `source_filename = "..."` directive.
    #[inline]
    pub fn source_filename(self) -> Option<core::cell::Ref<'ctx, str>> {
        self.core.source_filename()
    }

    /// Parsed data layout.
    #[inline]
    pub fn data_layout(self) -> core::cell::Ref<'ctx, DataLayout> {
        self.core.data_layout()
    }

    /// Target triple directive.
    #[inline]
    pub fn target_triple(self) -> Option<String> {
        self.core.target_triple()
    }

    /// Module-level inline assembly.
    #[inline]
    pub fn module_asm(self) -> String {
        self.core.module_asm()
    }

    #[inline]
    pub(crate) fn metadata_store(self) -> core::cell::Ref<'ctx, crate::metadata::MetadataStore> {
        self.core.metadata_store()
    }

    #[inline]
    pub(crate) fn ptr_type(self, addr_space: u32) -> PointerType<'ctx, B> {
        PointerType::new(
            self.core.ptr_type(addr_space).as_type().id(),
            ModuleRef::new(self.core),
        )
    }
    #[inline]
    pub(crate) fn vector_type<T>(self, elem: T, n: u32, scalable: bool) -> VectorType<'ctx, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let elem_id = elem.into().id();
        let id = if scalable {
            self.core.context().scalable_vector_type(elem_id, n)
        } else {
            self.core.context().fixed_vector_type(elem_id, n)
        };
        VectorType::new(id, ModuleRef::new(self.core))
    }

    #[inline]
    pub(crate) fn label_type(self) -> LabelType<'ctx, B> {
        LabelType::new(
            self.core.label_type().as_type().id(),
            ModuleRef::new(self.core),
        )
    }

    /// Iterate functions in declaration order.
    #[inline]
    pub fn iter_functions(
        self,
    ) -> impl ExactSizeIterator<Item = crate::pass_context::FunctionView<'ctx, B>> + 'ctx {
        self.core
            .iter_functions::<B>()
            .map(crate::pass_context::FunctionView::new)
    }

    /// Iterate globals in declaration order.
    #[inline]
    pub fn iter_globals(self) -> impl ExactSizeIterator<Item = GlobalVariableView<'ctx, B>> + 'ctx {
        self.core.iter_globals::<B>().map(GlobalVariableView::new)
    }

    /// Iterate aliases in declaration order.
    #[inline]
    pub fn iter_aliases(self) -> impl ExactSizeIterator<Item = GlobalAliasView<'ctx, B>> + 'ctx {
        self.core.iter_aliases::<B>().map(GlobalAliasView::new)
    }

    /// Iterate ifuncs in declaration order.
    #[inline]
    pub fn iter_ifuncs(self) -> impl ExactSizeIterator<Item = GlobalIFuncView<'ctx, B>> + 'ctx {
        self.core.iter_ifuncs::<B>().map(GlobalIFuncView::new)
    }

    /// Iterate COMDATs in insertion order.
    #[inline]
    pub fn iter_comdats(self) -> impl ExactSizeIterator<Item = ComdatView<'ctx, B>> + 'ctx {
        self.core.iter_comdats::<B>().map(ComdatView::new)
    }
}

impl<'ctx, B: ModuleBrand> From<ModuleView<'ctx, B>> for ModuleRef<'ctx, B> {
    #[inline]
    fn from(view: ModuleView<'ctx, B>) -> Self {
        ModuleRef::new(view.core)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, S> From<&Module<'ctx, B, S>> for ModuleView<'ctx, B> {
    #[inline]
    fn from(module: &Module<'ctx, B, S>) -> Self {
        module.as_view()
    }
}

impl<B: ModuleBrand> PartialEq for ModuleView<'_, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.core.id == other.core.id
    }
}

impl<B: ModuleBrand> Eq for ModuleView<'_, B> {}

impl<B: ModuleBrand> Hash for ModuleView<'_, B> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.core.id.hash(state);
    }
}

impl<B: ModuleBrand> core::fmt::Debug for ModuleView<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ModuleView")
            .field("id", &self.core.id)
            .field("name", &self.core.name)
            .finish()
    }
}

impl<B: ModuleBrand> core::fmt::Display for ModuleView<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_module(f, self.core)
    }
}

// --------------------------------------------------------------------------
// Module
// --------------------------------------------------------------------------

/// Structured `uselistorder Type Value, { ... }` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseListOrderRecord {
    value: ValueId,
    value_ty: TypeId,
    indexes: Box<[u32]>,
}

impl UseListOrderRecord {
    pub fn new<Indexes>(value: ValueId, value_ty: TypeId, indexes: Indexes) -> IrResult<Self>
    where
        Indexes: Into<Box<[u32]>>,
    {
        let indexes = indexes.into();
        validate_use_list_order_indexes(&indexes)?;
        Ok(Self {
            value,
            value_ty,
            indexes,
        })
    }

    pub fn value(&self) -> ValueId {
        self.value
    }

    pub fn value_type(&self) -> TypeId {
        self.value_ty
    }

    pub fn indexes(&self) -> &[u32] {
        &self.indexes
    }
}

/// Structured `uselistorder_bb @function, %block, { ... }` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseListOrderBBRecord {
    function: ValueId,
    block: ValueId,
    indexes: Box<[u32]>,
}

impl UseListOrderBBRecord {
    pub fn new<Indexes>(function: ValueId, block: ValueId, indexes: Indexes) -> IrResult<Self>
    where
        Indexes: Into<Box<[u32]>>,
    {
        let indexes = indexes.into();
        validate_use_list_order_indexes(&indexes)?;
        Ok(Self {
            function,
            block,
            indexes,
        })
    }

    pub fn function(&self) -> ValueId {
        self.function
    }

    pub fn block(&self) -> ValueId {
        self.block
    }

    pub fn indexes(&self) -> &[u32] {
        &self.indexes
    }
}

pub(crate) fn validate_use_list_order_indexes(indexes: &[u32]) -> IrResult<()> {
    let identity = indexes
        .iter()
        .enumerate()
        .all(|(i, idx)| u32::try_from(i).is_ok_and(|i| i == *idx));
    if identity {
        return Err(IrError::InvalidOperation {
            message: "expected uselistorder indexes to change the order",
        });
    }
    Ok(())
}

/// Top-level IR container.
pub(crate) struct ModuleCore {
    id: ModuleId,
    name: String,
    /// `source_filename = "..."` directive. Optional; upstream stores an
    /// empty string for absence, but `Option` keeps the missing directive
    /// explicit on the Rust side.
    source_filename: core::cell::RefCell<Option<String>>,
    ctx: Context,
    /// Functions defined in this module, in declaration order.
    /// Stored as a `RefCell<Vec<ValueId>>` so `add_function` can mutate
    /// while the same `&'ctx self` borrow is held by call sites.
    functions: core::cell::RefCell<Vec<crate::value::ValueId>>,
    /// Module-level name -> function value-id table.
    function_by_name: core::cell::RefCell<std::collections::HashMap<String, crate::value::ValueId>>,
    /// Globals defined in this module, in declaration order.
    /// Mirrors `Module::GlobalList`. Stored under the same shape as
    /// `functions` so the AsmWriter can iterate in source order.
    globals: core::cell::RefCell<Vec<crate::value::ValueId>>,
    /// Module-level name -> global value-id table.
    global_by_name: core::cell::RefCell<std::collections::HashMap<String, crate::value::ValueId>>,
    aliases: core::cell::RefCell<Vec<crate::value::ValueId>>,
    alias_by_name: core::cell::RefCell<std::collections::HashMap<String, crate::value::ValueId>>,
    ifuncs: core::cell::RefCell<Vec<crate::value::ValueId>>,
    ifunc_by_name: core::cell::RefCell<std::collections::HashMap<String, crate::value::ValueId>>,
    /// Module-level COMDAT entries. Mirrors `Module::ComdatSymTab`.
    /// Stored in a `boxcar::Vec` for stable `&ComdatData` references
    /// under `&self`, so [`ComdatRef`](crate::comdat::ComdatRef) can
    /// hand out borrows without runtime cell juggling.
    comdats: boxcar::Vec<crate::comdat::ComdatData>,
    /// Name -> comdat-id table. Mirrors
    /// `Module::ComdatSymTab` lookup.
    comdat_by_name: core::cell::RefCell<std::collections::HashMap<String, crate::comdat::ComdatId>>,
    /// Parsed `target datalayout = "..."` directive. Default
    /// (empty string) when the module has no directive. Mirrors
    /// `Module::DL` in `IR/Module.h`.
    data_layout: core::cell::RefCell<crate::data_layout::DataLayout>,
    /// `target triple = "..."` directive. Optional.
    target_triple: core::cell::RefCell<Option<String>>,
    /// Module-level inline assembly. Mirrors `Module::ModuleAsm`.
    /// Stored as a single `String` joined by newlines (one entry
    /// per `module asm "..."` directive).
    module_asm: core::cell::RefCell<String>,
    use_list_orders: core::cell::RefCell<Vec<UseListOrderRecord>>,
    attribute_groups: core::cell::RefCell<Vec<(u32, crate::attributes::AttributeStorage)>>,
    use_list_order_bbs: core::cell::RefCell<Vec<UseListOrderBBRecord>>,
    /// Module-level metadata node arena. Mirrors `LLVMContextImpl`'s
    /// metadata store (scoped to the module for simplicity).
    metadata: core::cell::RefCell<crate::metadata::MetadataStore>,
    /// Named metadata nodes (`!llvm.module.flags`, `!llvm.ident`, ...).
    /// Mirrors `Module::NamedMDList`. Insertion order is preserved.
    named_metadata: core::cell::RefCell<Vec<NamedMDNode>>,
    /// Uniquing cache for [`metadata_as_value`](Self::metadata_as_value):
    /// maps a metadata node to its wrapping value so repeated wraps of the
    /// same node return the identical `Value`. Mirrors LLVM's uniqued
    /// `MetadataAsValue::get`.
    metadata_as_value_cache: core::cell::RefCell<
        std::collections::HashMap<crate::metadata::MetadataId, crate::value::ValueId>,
    >,
}

/// Linear module token carrying a generative brand `B` and verification state `S`.
pub struct Module<'ctx, B: ModuleBrand = Brand<'ctx>, S = Unverified> {
    core: &'ctx ModuleCore,
    _brand: Invariant<B>,
    _state: PhantomData<S>,
}

impl<'ctx> ModuleCore {
    /// Construct a fresh, empty module with a freshly-allocated
    /// [`ModuleId`].
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            id: ModuleId::fresh(),
            name: name.into(),
            source_filename: core::cell::RefCell::new(None),
            ctx: Context::new(),
            functions: core::cell::RefCell::new(Vec::new()),
            function_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            globals: core::cell::RefCell::new(Vec::new()),
            global_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            aliases: core::cell::RefCell::new(Vec::new()),
            alias_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            ifuncs: core::cell::RefCell::new(Vec::new()),
            ifunc_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            comdats: boxcar::Vec::new(),
            comdat_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            data_layout: core::cell::RefCell::new(crate::data_layout::DataLayout::default()),
            target_triple: core::cell::RefCell::new(None),
            module_asm: core::cell::RefCell::new(String::new()),
            use_list_orders: core::cell::RefCell::new(Vec::new()),
            use_list_order_bbs: core::cell::RefCell::new(Vec::new()),
            attribute_groups: core::cell::RefCell::new(Vec::new()),
            metadata: core::cell::RefCell::new(crate::metadata::MetadataStore::default()),
            named_metadata: core::cell::RefCell::new(Vec::new()),
            metadata_as_value_cache: core::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// Module identifier (the human-readable name).
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }
    /// `source_filename = "..."` directive. Mirrors
    /// `Module::getSourceFileName`.
    pub fn source_filename(&self) -> Option<core::cell::Ref<'_, str>> {
        core::cell::Ref::filter_map(self.source_filename.borrow(), Option::as_deref).ok()
    }

    /// Set the `source_filename` directive. Mirrors
    /// `Module::setSourceFileName`.
    pub fn set_source_filename<Filename>(&self, filename: Filename)
    where
        Filename: Into<String>,
    {
        *self.source_filename.borrow_mut() = Some(filename.into());
    }

    /// Clear the `source_filename` directive.
    pub fn clear_source_filename(&self) {
        *self.source_filename.borrow_mut() = None;
    }

    /// This module's globally-unique id.
    #[inline]
    pub fn id(&self) -> ModuleId {
        self.id
    }

    /// Crate-internal access to the interning context.
    #[inline]
    pub(crate) fn context(&self) -> &Context {
        &self.ctx
    }

    /// Named-struct type ids in declaration order. The printer turns each
    /// into a [`Type`](crate::r#type::Type) via `Type::new(id, self)` to emit
    /// the `%Name = type {...}` identity block.
    #[inline]
    pub(crate) fn iter_named_struct_ids(&self) -> Vec<crate::r#type::TypeId> {
        self.ctx.iter_named_structs()
    }

    // ---- Primitive type constructors ----

    /// `void`.
    pub fn void_type(&'ctx self) -> VoidType<'ctx> {
        VoidType::new(self.ctx.void(), self)
    }

    /// `label`.
    pub fn label_type(&'ctx self) -> LabelType<'ctx> {
        LabelType::new(self.ctx.label(), self)
    }

    /// `token`.
    pub fn token_type(&'ctx self) -> TokenType<'ctx> {
        TokenType::new(self.ctx.token(), self)
    }

    /// `half`.
    pub fn half_type(&'ctx self) -> FloatType<'ctx, Half> {
        FloatType::new(self.ctx.half(), self)
    }

    /// `bfloat`.
    pub fn bfloat_type(&'ctx self) -> FloatType<'ctx, BFloat> {
        FloatType::new(self.ctx.bfloat(), self)
    }

    /// `float` (32-bit IEEE 754).
    pub fn f32_type(&'ctx self) -> FloatType<'ctx, f32> {
        FloatType::new(self.ctx.float(), self)
    }

    /// `double` (64-bit IEEE 754).
    pub fn f64_type(&'ctx self) -> FloatType<'ctx, f64> {
        FloatType::new(self.ctx.double(), self)
    }

    /// `fp128` (128-bit IEEE 754 binary128).
    pub fn fp128_type(&'ctx self) -> FloatType<'ctx, Fp128> {
        FloatType::new(self.ctx.fp128(), self)
    }

    /// `x86_fp80` (80-bit X87 extended precision).
    pub fn x86_fp80_type(&'ctx self) -> FloatType<'ctx, X86Fp80> {
        FloatType::new(self.ctx.x86_fp80(), self)
    }

    /// `ppc_fp128` (PowerPC double-double).
    pub fn ppc_fp128_type(&'ctx self) -> FloatType<'ctx, PpcFp128> {
        FloatType::new(self.ctx.ppc_fp128(), self)
    }

    // ---- Integer types ----

    /// `i1`. Convenience for [`Self::custom_width_int_type`] with `bits = 1`.
    pub fn bool_type(&'ctx self) -> IntType<'ctx, bool> {
        IntType::new(self.ctx.int_type(1), self)
    }
    pub fn i8_type(&'ctx self) -> IntType<'ctx, i8> {
        IntType::new(self.ctx.int_type(8), self)
    }
    pub fn i16_type(&'ctx self) -> IntType<'ctx, i16> {
        IntType::new(self.ctx.int_type(16), self)
    }
    pub fn i32_type(&'ctx self) -> IntType<'ctx, i32> {
        IntType::new(self.ctx.int_type(32), self)
    }
    pub fn i64_type(&'ctx self) -> IntType<'ctx, i64> {
        IntType::new(self.ctx.int_type(64), self)
    }
    pub fn i128_type(&'ctx self) -> IntType<'ctx, i128> {
        IntType::new(self.ctx.int_type(128), self)
    }

    /// Const-generic integer type. Returns [`IntType<'ctx, Width<N>>`](
    /// crate::Width). Const-evaluated range check at monomorphisation:
    /// `N` outside `MIN_INT_BITS..=MAX_INT_BITS` is a compile error.
    /// Mirrors `Type::getIntNTy(C, N)`.
    pub fn int_type_n<const N: u32>(&'ctx self) -> IntType<'ctx, Width<N>> {
        const {
            assert!(
                N >= MIN_INT_BITS && N <= MAX_INT_BITS,
                "integer width N outside [MIN_INT_BITS, MAX_INT_BITS]",
            );
        }
        IntType::new(self.ctx.int_type(N), self)
    }

    // ---- Pointer / typed-pointer ----

    /// Opaque pointer in address space `addr_space` (`0` = default).
    pub fn ptr_type(&'ctx self, addr_space: u32) -> PointerType<'ctx> {
        PointerType::new(self.ctx.ptr_type(addr_space), self)
    }

    // ---- Array / vector ----

    /// Fixed `<N x T>` or scalable `<vscale x N x T>` vector.
    pub fn vector_type(
        &'ctx self,
        elem: impl Into<Type<'ctx>>,
        n: u32,
        scalable: bool,
    ) -> VectorType<'ctx> {
        let elem_id = elem.into().id();
        let id = if scalable {
            self.ctx.scalable_vector_type(elem_id, n)
        } else {
            self.ctx.fixed_vector_type(elem_id, n)
        };
        VectorType::new(id, self)
    }

    // ---- Struct ----

    /// Literal struct.
    pub fn struct_type<I, T>(&'ctx self, elements: I, packed: bool) -> StructType<'ctx>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx>>,
    {
        let elems: Box<[TypeId]> = elements.into_iter().map(|t| t.into().id()).collect();
        StructType::new(self.ctx.literal_struct_type(elems, packed), self)
    }

    // ---- Function creation ----

    /// Add a function to this module. Mirrors `Function::Create`.
    /// Returns `Err(IrError::DuplicateFunctionName)` if a function
    /// of the same name already exists, or
    /// [`IrError::ReturnTypeMismatch`] if the signature's return
    /// type does not match the chosen [`ReturnMarker`](crate::marker::ReturnMarker).
    pub fn add_function<R, Name>(
        &'ctx self,
        name: Name,
        signature: FunctionType<'ctx>,
        linkage: crate::global_value::Linkage,
    ) -> IrResult<crate::function::FunctionValue<'ctx, R>>
    where
        R: crate::marker::ReturnMarker,
        Name: AsRef<str>,
    {
        let name = name.as_ref();
        if !name.is_empty() && self.global_name_exists(name) {
            return Err(IrError::DuplicateFunctionName {
                name: name.to_owned(),
            });
        }
        // Reject the static-marker / signature mismatch up front.
        let ret_data = self.ctx.type_data(signature.return_type().id());
        if !crate::function::signature_matches_marker::<R>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: signature.return_type().kind_label(),
                got: signature.return_type().kind_label(),
            });
        }

        let signature_id = signature.id;

        // Push the function value first so each argument's
        // `parent_fn` can already point at the real id. Initial
        // `args` is empty; we patch it via `RefCell` once every
        // parameter is in the arena.
        let fn_data = crate::function::FunctionData::new(
            name.to_owned(),
            signature_id,
            linkage,
            crate::CallingConv::default(),
        );
        let fn_id = self.ctx.push_value(crate::value::ValueData {
            ty: signature_id,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.to_owned())),
            debug_loc: None,
            kind: crate::value::ValueKindData::Function(Box::new(fn_data)),
            use_list: core::cell::RefCell::new(Vec::new()),
        });

        // Push each parameter as its own value-arena entry.
        let param_types: Vec<TypeId> = signature.params().map(|t| t.id()).collect();
        let mut arg_ids = Vec::with_capacity(param_types.len());
        for (slot, &ty) in param_types.iter().enumerate() {
            let slot_u32 = u32::try_from(slot)
                .unwrap_or_else(|_| unreachable!("function parameter slot exceeds u32::MAX"));
            let id = self.ctx.push_value(crate::value::ValueData {
                ty,
                name: core::cell::RefCell::new(None),
                debug_loc: None,
                kind: crate::value::ValueKindData::Argument {
                    parent_fn: fn_id,
                    slot: slot_u32,
                },
                use_list: core::cell::RefCell::new(Vec::new()),
            });
            arg_ids.push(id);
        }

        // Patch the function's args list.
        let fn_value_data = self.ctx.value_data(fn_id);
        let fn_inner = match &fn_value_data.kind {
            crate::value::ValueKindData::Function(f) => f,
            _ => unreachable!("just pushed Function variant"),
        };
        *fn_inner.args.borrow_mut() = arg_ids.into_boxed_slice();

        self.functions.borrow_mut().push(fn_id);
        if !name.is_empty() {
            self.function_by_name
                .borrow_mut()
                .insert(name.to_owned(), fn_id);
        }
        Ok(crate::function::FunctionValue::<'ctx, R>::from_parts_unchecked(fn_id, self))
    }

    /// Iterate the module's functions in declaration order, widened
    /// to [`Dyn`](crate::marker::Dyn). Mirrors `Module::functions`.
    pub fn iter_functions<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = crate::function::FunctionValue<'ctx, crate::marker::Dyn, B>> + 'ctx
    {
        let ids: Vec<crate::value::ValueId> = self.functions.borrow().clone();
        ids.into_iter().map(move |id| {
            crate::function::FunctionValue::<'ctx, crate::marker::Dyn, B>::from_parts_unchecked(
                id,
                ModuleRef::<B>::new(self),
            )
        })
    }

    /// Start a [`FunctionBuilder`](crate::function::FunctionBuilder)
    /// for incremental setup of linkage, calling convention,
    /// `unnamed_addr`, parameter names, and attributes before
    /// materialising the function.
    pub fn function_builder<R, Name>(
        &'ctx self,
        name: Name,
        signature: FunctionType<'ctx>,
    ) -> crate::function::FunctionBuilder<'ctx, R>
    where
        R: crate::marker::ReturnMarker,
        Name: Into<String>,
    {
        crate::function::FunctionBuilder::new(self, name, signature)
    }

    // ---- Verification (Phase F) ----

    /// Iterate the module's globals in declaration order. Mirrors
    /// `Module::globals`.
    pub fn iter_globals<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = crate::global_variable::GlobalVariable<'ctx, B>> + 'ctx {
        let ids: Vec<crate::value::ValueId> = self.globals.borrow().clone();
        ids.into_iter().map(move |id| {
            let value_data = self.ctx.value_data(id);
            crate::global_variable::GlobalVariable::from_parts_unchecked(
                id,
                ModuleRef::<B>::new(self),
                value_data.ty,
            )
        })
    }

    pub fn iter_aliases<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = crate::global_alias::GlobalAlias<'ctx, B>> + 'ctx {
        let ids: Vec<crate::value::ValueId> = self.aliases.borrow().clone();
        ids.into_iter().map(move |id| {
            let value_data = self.ctx.value_data(id);
            crate::global_alias::GlobalAlias::from_parts_unchecked(
                id,
                ModuleRef::<B>::new(self),
                value_data.ty,
            )
        })
    }

    pub fn alias_empty(&self) -> bool {
        self.aliases.borrow().is_empty()
    }

    pub fn iter_ifuncs<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = crate::global_ifunc::GlobalIFunc<'ctx, B>> + 'ctx {
        let ids: Vec<crate::value::ValueId> = self.ifuncs.borrow().clone();
        ids.into_iter().map(move |id| {
            let value_data = self.ctx.value_data(id);
            crate::global_ifunc::GlobalIFunc::from_parts_unchecked(
                id,
                ModuleRef::<B>::new(self),
                value_data.ty,
            )
        })
    }

    pub fn ifunc_empty(&self) -> bool {
        self.ifuncs.borrow().is_empty()
    }

    pub fn global_empty(&self) -> bool {
        self.globals.borrow().is_empty()
    }

    /// Crate-internal: install a built [`GlobalBuilder`] into the
    /// module. Performs the duplicate-name check and the comdat
    /// existence check, then pushes to the value arena.
    pub(crate) fn install_global_variable<B: ModuleBrand + 'ctx>(
        &'ctx self,
        builder: crate::global_variable::GlobalBuilder<'ctx, B>,
    ) -> IrResult<crate::global_variable::GlobalVariable<'ctx, B>> {
        let (name, data, _initializer, address_space, value_type) = builder.into_data();
        if !name.is_empty() && self.global_name_exists(&name) {
            return Err(IrError::DuplicateFunctionName { name });
        }
        let pointer_ty = self.ctx.ptr_type(address_space);
        // Sanity: value_type must already be in the same context. Use
        // the cached id directly. (Construction APIs only hand out
        // typed ids belonging to this module.)
        let _ = value_type;
        let value_id = self.ctx.push_value(crate::value::ValueData {
            ty: pointer_ty,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.clone())),
            debug_loc: None,
            kind: crate::value::ValueKindData::GlobalVariable(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.globals.borrow_mut().push(value_id);
        if !name.is_empty() {
            self.global_by_name.borrow_mut().insert(name, value_id);
        }
        Ok(
            crate::global_variable::GlobalVariable::from_parts_unchecked(
                value_id,
                ModuleRef::<B>::new(self),
                pointer_ty,
            ),
        )
    }

    pub(crate) fn install_global_alias<B: ModuleBrand + 'ctx>(
        &'ctx self,
        builder: crate::global_alias::GlobalAliasBuilder<'ctx, B>,
    ) -> IrResult<crate::global_alias::GlobalAlias<'ctx, B>> {
        let (name, data, address_space) = builder.into_data();
        if !name.is_empty() && self.global_name_exists(&name) {
            return Err(IrError::DuplicateFunctionName { name });
        }
        let pointer_ty = self.ctx.ptr_type(address_space);
        let value_id = self.ctx.push_value(crate::value::ValueData {
            ty: pointer_ty,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.clone())),
            debug_loc: None,
            kind: crate::value::ValueKindData::GlobalAlias(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.aliases.borrow_mut().push(value_id);
        if !name.is_empty() {
            self.alias_by_name.borrow_mut().insert(name, value_id);
        }
        Ok(crate::global_alias::GlobalAlias::from_parts_unchecked(
            value_id,
            ModuleRef::<B>::new(self),
            pointer_ty,
        ))
    }

    pub(crate) fn install_global_ifunc<B: ModuleBrand + 'ctx>(
        &'ctx self,
        builder: crate::global_ifunc::GlobalIFuncBuilder<'ctx, B>,
    ) -> IrResult<crate::global_ifunc::GlobalIFunc<'ctx, B>> {
        let (name, data, address_space) = builder.into_data();
        if !name.is_empty() && self.global_name_exists(&name) {
            return Err(IrError::DuplicateFunctionName { name });
        }
        let pointer_ty = self.ctx.ptr_type(address_space);
        let value_id = self.ctx.push_value(crate::value::ValueData {
            ty: pointer_ty,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.clone())),
            debug_loc: None,
            kind: crate::value::ValueKindData::GlobalIFunc(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.ifuncs.borrow_mut().push(value_id);
        if !name.is_empty() {
            self.ifunc_by_name.borrow_mut().insert(name, value_id);
        }
        Ok(crate::global_ifunc::GlobalIFunc::from_parts_unchecked(
            value_id,
            ModuleRef::<B>::new(self),
            pointer_ty,
        ))
    }

    fn global_name_exists(&self, name: &str) -> bool {
        self.function_by_name.borrow().contains_key(name)
            || self.global_by_name.borrow().contains_key(name)
            || self.alias_by_name.borrow().contains_key(name)
            || self.ifunc_by_name.borrow().contains_key(name)
    }

    // ---- DataLayout / target triple / module asm ----

    /// Borrow the parsed [`DataLayout`](crate::data_layout::DataLayout).
    /// Mirrors `Module::getDataLayout`. Returns the default (empty)
    /// layout when no directive has been set.
    pub fn data_layout(&self) -> core::cell::Ref<'_, DataLayout> {
        self.data_layout.borrow()
    }

    /// Replace the data layout with a parsed copy of the given
    /// string. Mirrors `Module::setDataLayout(StringRef)`.
    pub fn set_data_layout<Layout>(&self, layout: Layout) -> IrResult<()>
    where
        Layout: AsRef<str>,
    {
        let parsed = crate::data_layout::DataLayout::parse(layout.as_ref())?;
        *self.data_layout.borrow_mut() = parsed;
        Ok(())
    }

    /// Replace the data layout with an already-parsed
    /// [`DataLayout`](crate::data_layout::DataLayout). Mirrors
    /// `Module::setDataLayout(const DataLayout &)`.
    pub fn set_data_layout_value(&self, layout: DataLayout) {
        *self.data_layout.borrow_mut() = layout;
    }

    /// `target triple = "..."` directive. Mirrors
    /// `Module::getTargetTriple` (post-Triple-class API: returns the
    /// stored string).
    pub fn target_triple(&self) -> Option<String> {
        self.target_triple.borrow().clone()
    }

    /// Set the `target triple` directive. Mirrors
    /// `Module::setTargetTriple`.
    pub fn set_target_triple<T>(&self, triple: T)
    where
        T: Into<String>,
    {
        *self.target_triple.borrow_mut() = Some(triple.into());
    }

    /// Clear the `target triple` directive.
    pub fn clear_target_triple(&self) {
        *self.target_triple.borrow_mut() = None;
    }

    /// Module-level inline assembly. Mirrors
    /// `Module::getModuleInlineAsm`.
    pub fn module_asm(&self) -> String {
        self.module_asm.borrow().clone()
    }

    /// Replace the module-level inline assembly. Mirrors
    /// `Module::setModuleInlineAsm`. Pass an empty string to clear.
    pub fn set_module_asm<Asm>(&self, asm: Asm)
    where
        Asm: Into<String>,
    {
        *self.module_asm.borrow_mut() = asm.into();
    }

    /// Append one line of module-level inline assembly. Mirrors
    /// `Module::appendModuleInlineAsm`.
    pub fn append_module_asm<Line>(&self, line: Line)
    where
        Line: AsRef<str>,
    {
        let mut buf = self.module_asm.borrow_mut();
        if !buf.is_empty() && !buf.ends_with('\n') {
            buf.push('\n');
        }
        buf.push_str(line.as_ref());
    }
    pub fn append_use_list_order(&self, record: UseListOrderRecord) -> IrResult<()> {
        validate_use_list_order_indexes(record.indexes())?;
        self.use_list_orders.borrow_mut().push(record);
        Ok(())
    }

    pub fn append_use_list_order_bb(&self, record: UseListOrderBBRecord) -> IrResult<()> {
        validate_use_list_order_indexes(record.indexes())?;
        self.use_list_order_bbs.borrow_mut().push(record);
        Ok(())
    }

    pub fn iter_use_list_orders(&self) -> impl ExactSizeIterator<Item = UseListOrderRecord> {
        self.use_list_orders.borrow().clone().into_iter()
    }

    pub fn iter_use_list_order_bbs(&self) -> impl ExactSizeIterator<Item = UseListOrderBBRecord> {
        self.use_list_order_bbs.borrow().clone().into_iter()
    }

    pub fn set_attribute_group(&self, id: u32, storage: AttributeStorage) {
        let mut groups = self.attribute_groups.borrow_mut();
        if let Some((_, existing)) = groups.iter_mut().find(|(slot, _)| *slot == id) {
            *existing = storage;
            return;
        }
        groups.push((id, storage));
        groups.sort_by_key(|(slot, _)| *slot);
    }

    pub fn attribute_groups(&self) -> Vec<(u32, AttributeStorage)> {
        self.attribute_groups.borrow().clone()
    }

    // ---- Metadata ----

    /// Intern a metadata string node. Returns an existing id if an
    /// identical string was already interned. Mirrors `MDString::get`.
    pub fn metadata_string<S>(&self, s: S) -> MetadataId
    where
        S: Into<String>,
    {
        self.metadata.borrow_mut().get_string(s)
    }

    /// Create a metadata tuple node. Mirrors `MDTuple::get` (distinct).
    ///
    /// Accepts anything that borrows as a slice of
    /// [`MetadataRef`](crate::metadata::MetadataRef) — both an owned
    /// `Vec` and a borrowed `&[..]` work.
    pub fn metadata_tuple<Ops>(&self, operands: Ops) -> MetadataId
    where
        Ops: AsRef<[crate::metadata::MetadataRef]>,
    {
        self.metadata
            .borrow_mut()
            .get_tuple(operands.as_ref().to_vec())
    }
    /// Create a tuple node with explicit distinctness.
    pub fn metadata_tuple_with_distinct<Ops>(&self, distinct: bool, operands: Ops) -> MetadataId
    where
        Ops: AsRef<[MetadataRef]>,
    {
        self.metadata
            .borrow_mut()
            .get_tuple_with_distinct(distinct, operands.as_ref().to_vec())
    }

    /// Create a specialized debug metadata node.
    pub fn metadata_specialized(&self, node: SpecializedMetadataNode) -> MetadataId {
        self.metadata.borrow_mut().get_specialized(node)
    }
    /// Store an already-parsed metadata node and return its id.
    pub fn metadata_node(&self, kind: MetadataKind) -> MetadataId {
        let (id, value_use) = {
            let mut store = self.metadata.borrow_mut();
            match kind {
                MetadataKind::String(s) => (store.get_string(s), None),
                MetadataKind::Tuple { distinct, operands } => {
                    (store.get_tuple_with_distinct(distinct, operands), None)
                }
                MetadataKind::Specialized(node) => (store.get_specialized(node), None),
                MetadataKind::Constant(value_id) => {
                    let id = store.get_constant(value_id);
                    (id, Some(value_id))
                }
                MetadataKind::Ref(id) => (id, None),
                MetadataKind::Null => {
                    let id = store.reserve();
                    store.set(id, MetadataKind::Null);
                    (id, None)
                }
            }
        };
        if let Some(value_id) = value_use {
            self.register_metadata_value_use(id, value_id);
        }
        id
    }

    /// Reserve a fresh metadata node id with placeholder content, to be
    /// filled via [`metadata_set`](Self::metadata_set). Used by the parser
    /// to resolve forward references without assuming textual `!N` slots
    /// equal arena indices.
    pub fn metadata_reserve(&self) -> MetadataId {
        self.metadata.borrow_mut().reserve()
    }

    /// Overwrite a reserved metadata node with concrete content. Pairs
    /// with [`metadata_reserve`](Self::metadata_reserve).
    pub fn metadata_set(&self, id: MetadataId, kind: MetadataKind) {
        if let Some(MetadataKind::Constant(value_id)) = self.metadata.borrow().get(id).cloned() {
            self.deregister_metadata_value_use(id, value_id);
        }
        let value_use = match kind {
            MetadataKind::Constant(value_id) => Some(value_id),
            MetadataKind::Null
            | MetadataKind::String(_)
            | MetadataKind::Tuple { .. }
            | MetadataKind::Ref(_)
            | MetadataKind::Specialized(_) => None,
        };
        self.metadata.borrow_mut().set(id, kind);
        if let Some(value_id) = value_use {
            self.register_metadata_value_use(id, value_id);
        }
    }

    pub(crate) fn metadata_constant_value(&self, value_id: ValueId) -> MetadataId {
        let id = self.metadata.borrow_mut().get_constant(value_id);
        self.register_metadata_value_use(id, value_id);
        id
    }

    pub(crate) fn rewrite_metadata_value(&self, id: MetadataId, from: ValueId, to: ValueId) {
        let mut store = self.metadata.borrow_mut();
        if let Some(MetadataKind::Constant(value_id)) = store.get_mut(id)
            && *value_id == from
        {
            *value_id = to;
        }
    }

    fn register_metadata_value_use(&self, metadata_id: MetadataId, value_id: ValueId) {
        self.ctx
            .value_data(value_id)
            .use_list
            .borrow_mut()
            .push(ValueUse::Metadata(metadata_id));
    }

    fn deregister_metadata_value_use(&self, metadata_id: MetadataId, value_id: ValueId) {
        let mut uses = self.ctx.value_data(value_id).use_list.borrow_mut();
        if let Some(pos) = uses
            .iter()
            .position(|edge| *edge == ValueUse::Metadata(metadata_id))
        {
            uses.remove(pos);
        }
    }

    /// Look up a metadata node by id.
    pub fn metadata_get(
        &self,
        id: crate::metadata::MetadataId,
    ) -> Option<crate::metadata::MetadataKind> {
        self.metadata.borrow().get(id).cloned()
    }

    /// Number of numbered metadata nodes. `MDString`s are uniqued metadata
    /// operands, but LLVM does not assign them standalone `!N` slots.
    pub fn metadata_count(&self) -> usize {
        self.metadata
            .borrow()
            .nodes()
            .iter()
            .filter(|node| !matches!(node, crate::metadata::MetadataKind::String(_)))
            .count()
    }

    /// Crate-internal: borrow the metadata store.
    pub(crate) fn metadata_store(&self) -> core::cell::Ref<'_, crate::metadata::MetadataStore> {
        self.metadata.borrow()
    }

    /// Get or create a named metadata node with the given name.
    /// Mirrors `Module::getOrInsertNamedMetadata`.
    pub fn get_or_insert_named_metadata<Name>(&self, name: Name) -> usize
    where
        Name: Into<String>,
    {
        let name = name.into();
        let mut nmd = self.named_metadata.borrow_mut();
        for (i, node) in nmd.iter().enumerate() {
            if node.name() == name {
                return i;
            }
        }
        let idx = nmd.len();
        nmd.push(NamedMDNode::new(name));
        idx
    }

    /// Append an operand to a named metadata node (by index).
    pub fn named_metadata_add_operand(&self, index: usize, op: MetadataRef) {
        self.named_metadata.borrow_mut()[index].add_operand(op);
    }

    /// Number of named metadata nodes.
    pub fn named_metadata_count(&self) -> usize {
        self.named_metadata.borrow().len()
    }

    /// Crate-internal: borrow named metadata list for printing.
    pub(crate) fn named_metadata_list(&self) -> core::cell::Ref<'_, Vec<NamedMDNode>> {
        self.named_metadata.borrow()
    }

    // ---- Comdats ----

    /// Get or create a [`ComdatRef`](crate::comdat::ComdatRef) of
    /// the given name. Mirrors `Module::getOrInsertComdat`.
    ///
    /// On first lookup the selection kind defaults to
    /// [`SelectionKind::Any`](crate::comdat::SelectionKind::Any);
    /// callers can refine via
    /// [`ComdatRef::set_selection_kind`](crate::comdat::ComdatRef::set_selection_kind).
    pub fn get_or_insert_comdat<B, Name>(
        &'ctx self,
        name: Name,
    ) -> crate::comdat::ComdatRef<'ctx, B>
    where
        B: ModuleBrand,
        Name: AsRef<str>,
    {
        let name = name.as_ref();
        if let Some(&id) = self.comdat_by_name.borrow().get(name) {
            return crate::comdat::ComdatRef {
                module: ModuleRef::new(self),
                id,
            };
        }
        let index = self.comdats.push(crate::comdat::ComdatData::new(
            name.to_owned(),
            crate::comdat::SelectionKind::Any,
        ));
        let id = crate::comdat::ComdatId::from_index(index);
        self.comdat_by_name.borrow_mut().insert(name.to_owned(), id);
        crate::comdat::ComdatRef {
            module: ModuleRef::new(self),
            id,
        }
    }

    /// Look up an existing comdat by name. Returns `None` when not
    /// present.
    pub fn get_comdat<B: ModuleBrand>(
        &'ctx self,
        name: &str,
    ) -> Option<crate::comdat::ComdatRef<'ctx, B>> {
        let id = *self.comdat_by_name.borrow().get(name)?;
        Some(crate::comdat::ComdatRef {
            module: ModuleRef::new(self),
            id,
        })
    }

    /// Crate-internal: borrow the underlying [`ComdatData`] by id.
    /// Mirrors `Module::comdat_at`.
    pub(crate) fn comdat_at(&self, id: crate::comdat::ComdatId) -> &crate::comdat::ComdatData {
        self.comdats
            .get(id.arena_index())
            .unwrap_or_else(|| unreachable!("ComdatId is always valid for the owning module"))
    }

    /// Iterate comdat refs in insertion order. Mirrors
    /// `Module::getComdatSymbolTable` (insertion-order traversal).
    pub fn iter_comdats<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = crate::comdat::ComdatRef<'ctx, B>> + 'ctx {
        let count = self.comdats.count();
        (0..count).map(move |i| crate::comdat::ComdatRef {
            module: ModuleRef::new(self),
            id: crate::comdat::ComdatId::from_index(i),
        })
    }
}

impl Module<'static, Brand<'static>, Unverified> {
    /// Construct a fresh module under a generative brand closure.
    pub fn with_new<N, R, F>(name: N, f: F) -> R
    where
        N: Into<String>,
        F: for<'brand> FnOnce(Module<'brand, Brand<'brand>, Unverified>) -> R,
    {
        let core = ModuleCore::new(name);
        let module = Module {
            core: &core,
            _brand: PhantomData,
            _state: PhantomData,
        };
        f(module)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, S> Module<'ctx, B, S> {
    /// Owning module's [`ModuleId`].
    #[inline]
    pub fn id(&self) -> ModuleId {
        self.core.id()
    }

    /// Module identifier.
    #[inline]
    pub fn name(&self) -> &str {
        self.core.name()
    }

    /// Read-only branded view.
    #[inline]
    pub fn as_view(&self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.core)
    }

    /// Crate-internal borrow of the state-erased module storage.
    #[inline]
    pub(crate) fn core_ref(&self) -> &'ctx ModuleCore {
        self.core
    }

    /// Crate-internal state-erased module handle with this token's brand.
    #[inline]
    pub(crate) fn module_ref(&self) -> ModuleRef<'ctx, B> {
        ModuleRef::new(self.core)
    }

    /// `source_filename = "..."` directive.
    #[inline]
    pub fn source_filename(&self) -> Option<core::cell::Ref<'_, str>> {
        self.core.source_filename()
    }

    /// Borrow the parsed data layout.
    #[inline]
    pub fn data_layout(&self) -> core::cell::Ref<'_, DataLayout> {
        self.core.data_layout()
    }

    /// Target triple directive.
    #[inline]
    pub fn target_triple(&self) -> Option<String> {
        self.core.target_triple()
    }

    /// Module-level inline assembly.
    #[inline]
    pub fn module_asm(&self) -> String {
        self.core.module_asm()
    }

    pub fn attribute_groups(&self) -> Vec<(u32, AttributeStorage)> {
        self.core.attribute_groups()
    }

    /// Iterate globals in declaration order with this module token's brand.
    pub fn iter_globals(
        &self,
    ) -> impl ExactSizeIterator<Item = crate::global_variable::GlobalVariable<'ctx, B>> + 'ctx {
        self.core.iter_globals::<B>()
    }

    /// Look up a function by name with this module token's brand.
    pub fn function_by_name(
        &self,
        name: &str,
    ) -> Option<crate::function::FunctionValue<'ctx, crate::marker::Dyn, B>> {
        self.core
            .function_by_name
            .borrow()
            .get(name)
            .copied()
            .map(|id| {
                crate::function::FunctionValue::<'ctx, crate::marker::Dyn, B>::from_parts_unchecked(
                    id,
                    ModuleRef::<B>::new(self.core),
                )
            })
    }

    /// Look up a function by name and narrow to a specific return marker.
    pub fn function_by_name_typed<R>(
        &self,
        name: &str,
    ) -> IrResult<Option<crate::function::FunctionValue<'ctx, R, B>>>
    where
        R: crate::marker::ReturnMarker,
    {
        let Some(id) = self.core.function_by_name.borrow().get(name).copied() else {
            return Ok(None);
        };
        let value_data = self.core.ctx.value_data(id);
        let signature_id = match &value_data.kind {
            crate::value::ValueKindData::Function(f) => f.signature,
            _ => unreachable!("function_by_name table only stores function ids"),
        };
        let ret_id = self
            .core
            .ctx
            .type_data(signature_id)
            .as_function()
            .unwrap_or_else(|| unreachable!("function value carries a function signature"))
            .0;
        let ret_data = self.core.ctx.type_data(ret_id);
        if !crate::function::signature_matches_marker::<R>(ret_data) {
            let label =
                crate::r#type::Type::new(ret_id, ModuleRef::<B>::new(self.core)).kind_label();
            return Err(IrError::ReturnTypeMismatch {
                expected: label,
                got: label,
            });
        }
        Ok(Some(
            crate::function::FunctionValue::<'ctx, R, B>::from_parts_unchecked(
                id,
                ModuleRef::<B>::new(self.core),
            ),
        ))
    }
    /// Verify the module's structural invariants without consuming it.
    pub fn verify_borrowed(&self) -> IrResult<()> {
        crate::verifier::Verifier::new(self.as_view()).run()
    }
}

impl<'ctx> Module<'ctx, Brand<'ctx>, Unverified> {
    pub fn function_builder<R, Name>(
        &self,
        name: Name,
        signature: FunctionType<'ctx>,
    ) -> crate::function::FunctionBuilder<'ctx, R>
    where
        R: crate::marker::ReturnMarker,
        Name: Into<String>,
    {
        self.core.function_builder(name, signature)
    }

    pub fn constant_expr<Operands, Indices, Mask>(
        &self,
        result_ty: Type<'ctx>,
        opcode: crate::ConstantExprOpcode,
        operands: Operands,
        indices: Indices,
        mask: Mask,
        flags: crate::ConstantExprFlags,
    ) -> IrResult<crate::Constant<'ctx>>
    where
        Operands: IntoIterator<Item = crate::Value<'ctx>>,
        Indices: IntoIterator<Item = u32>,
        Mask: IntoIterator<Item = i32>,
    {
        self.core
            .constant_expr(result_ty, opcode, operands, indices, mask, flags)
    }

    pub fn constant_expr_with_options<Operands, Indices, Mask>(
        &self,
        result_ty: Type<'ctx>,
        opcode: crate::ConstantExprOpcode,
        operands: Operands,
        indices: Indices,
        mask: Mask,
        options: crate::ConstantExprOptions<'ctx>,
    ) -> IrResult<crate::Constant<'ctx>>
    where
        Operands: IntoIterator<Item = crate::Value<'ctx>>,
        Indices: IntoIterator<Item = u32>,
        Mask: IntoIterator<Item = i32>,
    {
        self.core
            .constant_expr_with_options(result_ty, opcode, operands, indices, mask, options)
    }

    pub fn block_address<R, S>(
        &self,
        function: crate::FunctionValue<'ctx, R>,
        block: crate::BasicBlock<'ctx, R, S>,
    ) -> IrResult<crate::Constant<'ctx>>
    where
        R: crate::ReturnMarker,
        S: crate::BlockSealState,
    {
        self.core.block_address(function, block)
    }

    pub fn block_address_placeholder(
        &self,
        ty: Type<'ctx>,
    ) -> IrResult<crate::BlockAddressPlaceholder<'ctx>> {
        self.core.block_address_placeholder(ty)
    }

    pub fn dso_local_equivalent(
        &self,
        function: crate::FunctionValue<'ctx, crate::Dyn>,
    ) -> crate::Constant<'ctx> {
        self.core.dso_local_equivalent(function)
    }

    pub fn dso_local_equivalent_global(
        &self,
        global: crate::Constant<'ctx>,
    ) -> IrResult<crate::Constant<'ctx>> {
        self.core.dso_local_equivalent_global(global)
    }

    pub fn no_cfi(
        &self,
        function: crate::FunctionValue<'ctx, crate::Dyn>,
    ) -> crate::Constant<'ctx> {
        self.core.no_cfi(function)
    }

    pub fn no_cfi_global(&self, global: Constant<'ctx>) -> IrResult<Constant<'ctx>> {
        self.core.no_cfi_global(global)
    }

    pub fn ptr_auth(
        &self,
        pointer: impl crate::IsConstant<'ctx>,
        key: impl crate::IsConstant<'ctx>,
        discriminator: impl crate::IsConstant<'ctx>,
        addr_discriminator: impl crate::IsConstant<'ctx>,
        deactivation_symbol: impl crate::IsConstant<'ctx>,
    ) -> IrResult<crate::Constant<'ctx>> {
        self.core.ptr_auth(
            pointer,
            key,
            discriminator,
            addr_discriminator,
            deactivation_symbol,
        )
    }

    pub fn token_none(&self) -> Constant<'ctx> {
        self.core.token_none()
    }

    pub fn target_ext_none(&self, ty: Type<'ctx>) -> IrResult<Constant<'ctx>> {
        self.core.target_ext_none(ty)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Module<'ctx, B, Unverified> {
    /// `void`.
    #[inline]
    pub fn void_type(&self) -> VoidType<'ctx, B> {
        VoidType::new(self.core.ctx.void(), self.module_ref())
    }

    /// `label`.
    #[inline]
    pub fn label_type(&self) -> LabelType<'ctx, B> {
        LabelType::new(self.core.ctx.label(), self.module_ref())
    }

    /// `metadata`.
    #[inline]
    pub fn metadata_type(&self) -> MetadataType<'ctx, B> {
        MetadataType::new(self.core.ctx.metadata(), self.module_ref())
    }

    /// `token`.
    #[inline]
    pub fn token_type(&self) -> TokenType<'ctx, B> {
        TokenType::new(self.core.ctx.token(), self.module_ref())
    }

    /// `half`.
    #[inline]
    pub fn half_type(&self) -> FloatType<'ctx, Half, B> {
        FloatType::new(self.core.ctx.half(), self.module_ref())
    }

    /// `bfloat`.
    #[inline]
    pub fn bfloat_type(&self) -> FloatType<'ctx, BFloat, B> {
        FloatType::new(self.core.ctx.bfloat(), self.module_ref())
    }

    /// `float` (32-bit IEEE 754).
    #[inline]
    pub fn f32_type(&self) -> FloatType<'ctx, f32, B> {
        FloatType::new(self.core.ctx.float(), self.module_ref())
    }

    /// `double` (64-bit IEEE 754).
    #[inline]
    pub fn f64_type(&self) -> FloatType<'ctx, f64, B> {
        FloatType::new(self.core.ctx.double(), self.module_ref())
    }

    /// `fp128`.
    #[inline]
    pub fn fp128_type(&self) -> FloatType<'ctx, Fp128, B> {
        FloatType::new(self.core.ctx.fp128(), self.module_ref())
    }

    /// `x86_fp80`.
    #[inline]
    pub fn x86_fp80_type(&self) -> FloatType<'ctx, X86Fp80, B> {
        FloatType::new(self.core.ctx.x86_fp80(), self.module_ref())
    }

    /// `ppc_fp128`.
    #[inline]
    pub fn ppc_fp128_type(&self) -> FloatType<'ctx, PpcFp128, B> {
        FloatType::new(self.core.ctx.ppc_fp128(), self.module_ref())
    }

    /// `x86_amx`.
    #[inline]
    pub fn x86_amx_type(&self) -> Type<'ctx, B> {
        Type::new(self.core.ctx.x86_amx(), self.module_ref())
    }

    /// `i1`.
    #[inline]
    pub fn bool_type(&self) -> IntType<'ctx, bool, B> {
        IntType::new(self.core.ctx.int_type(1), self.module_ref())
    }

    /// Alias for [`Self::bool_type`].
    #[inline]
    pub fn i1_type(&self) -> IntType<'ctx, bool, B> {
        self.bool_type()
    }

    #[inline]
    pub fn i8_type(&self) -> IntType<'ctx, i8, B> {
        IntType::new(self.core.ctx.int_type(8), self.module_ref())
    }

    #[inline]
    pub fn i16_type(&self) -> IntType<'ctx, i16, B> {
        IntType::new(self.core.ctx.int_type(16), self.module_ref())
    }

    #[inline]
    pub fn i32_type(&self) -> IntType<'ctx, i32, B> {
        IntType::new(self.core.ctx.int_type(32), self.module_ref())
    }

    #[inline]
    pub fn i64_type(&self) -> IntType<'ctx, i64, B> {
        IntType::new(self.core.ctx.int_type(64), self.module_ref())
    }

    #[inline]
    pub fn i128_type(&self) -> IntType<'ctx, i128, B> {
        IntType::new(self.core.ctx.int_type(128), self.module_ref())
    }

    pub fn custom_width_int_type(&self, bits: u32) -> IrResult<IntType<'ctx, IntDyn, B>> {
        if !(MIN_INT_BITS..=MAX_INT_BITS).contains(&bits) {
            return Err(IrError::InvalidIntegerWidth { bits });
        }
        Ok(IntType::new(
            self.core.ctx.int_type(bits),
            self.module_ref(),
        ))
    }

    pub fn int_type_n<const N: u32>(&self) -> IntType<'ctx, Width<N>, B> {
        const {
            assert!(
                N >= MIN_INT_BITS && N <= MAX_INT_BITS,
                "integer width N outside [MIN_INT_BITS, MAX_INT_BITS]",
            );
        }
        IntType::new(self.core.ctx.int_type(N), self.module_ref())
    }

    pub fn ptr_type(&self, addr_space: u32) -> PointerType<'ctx, B> {
        PointerType::new(self.core.ctx.ptr_type(addr_space), self.module_ref())
    }

    pub fn typed_pointer_type<T>(&self, pointee: T, addr_space: u32) -> TypedPointerType<'ctx, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let pointee_id = pointee.into().id();
        TypedPointerType::new(
            self.core.ctx.typed_pointer_type(pointee_id, addr_space),
            self.module_ref(),
        )
    }

    pub fn array_type<T>(&self, elem: T, n: u64) -> ArrayType<'ctx, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let elem_id = elem.into().id();
        ArrayType::new(self.core.ctx.array_type(elem_id, n), self.module_ref())
    }

    pub fn vector_type<T>(&self, elem: T, n: u32, scalable: bool) -> VectorType<'ctx, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let elem_id = elem.into().id();
        let id = if scalable {
            self.core.ctx.scalable_vector_type(elem_id, n)
        } else {
            self.core.ctx.fixed_vector_type(elem_id, n)
        };
        VectorType::new(id, self.module_ref())
    }

    pub fn struct_type<I, T>(
        &self,
        elements: I,
        packed: bool,
    ) -> StructType<'ctx, crate::struct_body_state::StructBodyDyn, B>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        let elems: Box<[TypeId]> = elements.into_iter().map(|t| t.into().id()).collect();
        StructType::new(
            self.core.ctx.literal_struct_type(elems, packed),
            self.module_ref(),
        )
    }

    pub fn named_struct(
        &self,
        name: &str,
    ) -> StructType<'ctx, crate::struct_body_state::StructBodyDyn, B> {
        let (id, _existed) = self.core.ctx.get_or_create_named_struct(name);
        StructType::new(id, self.module_ref())
    }

    pub fn opaque_struct(
        &self,
        name: &str,
    ) -> IrResult<StructType<'ctx, crate::struct_body_state::Opaque, B>> {
        let (id, existed) = self.core.ctx.get_or_create_named_struct(name);
        if existed {
            let s = self
                .core
                .ctx
                .type_data(id)
                .as_struct()
                .unwrap_or_else(|| unreachable!("named struct id stores struct data"));
            if s.body.borrow().is_some() {
                return Err(IrError::StructBodyAlreadySet {
                    name: name.to_owned(),
                });
            }
        }
        Ok(StructType::new(id, self.module_ref()))
    }

    pub fn get_named_struct(
        &self,
        name: &str,
    ) -> Option<StructType<'ctx, crate::struct_body_state::StructBodyDyn, B>> {
        self.core
            .ctx
            .get_named_struct(name)
            .map(|id| StructType::new(id, self.module_ref()))
    }

    pub fn set_struct_body<I, T>(
        &self,
        st: StructType<'ctx, crate::struct_body_state::StructBodyDyn, B>,
        elements: I,
        packed: bool,
    ) -> IrResult<()>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        let elems: Box<[TypeId]> = elements.into_iter().map(|t| t.into().id()).collect();
        let body = StructBody {
            elements: elems,
            packed,
        };
        let s = self
            .core
            .ctx
            .type_data(st.id)
            .as_struct()
            .unwrap_or_else(|| unreachable!("StructType wraps struct data"));
        if s.name.is_none() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Struct,
                got: TypeKindLabel::Struct,
            });
        }
        self.core.ctx.set_named_struct_body(st.id, body)
    }

    pub fn set_struct_body_typed<I, T>(
        &self,
        opaque: StructType<'ctx, crate::struct_body_state::Opaque, B>,
        elements: I,
        packed: bool,
    ) -> IrResult<StructType<'ctx, crate::struct_body_state::BodySet, B>>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        let elems: Box<[TypeId]> = elements.into_iter().map(|t| t.into().id()).collect();
        let body = StructBody {
            elements: elems,
            packed,
        };
        self.core.ctx.set_named_struct_body(opaque.id, body)?;
        Ok(opaque.retag::<crate::struct_body_state::BodySet>())
    }

    pub fn fn_type<I, R, T>(&self, ret: R, params: I, is_var_arg: bool) -> FunctionType<'ctx, B>
    where
        I: IntoIterator<Item = T>,
        R: Into<Type<'ctx, B>>,
        T: Into<Type<'ctx, B>>,
    {
        let ret = ret.into();
        let params: Box<[TypeId]> = params.into_iter().map(|t| t.into().id()).collect();
        FunctionType::new(
            self.core.ctx.function_type(ret.id(), params, is_var_arg),
            self.module_ref(),
        )
    }

    pub fn target_ext_type<Name, I, T, J>(
        &self,
        name: Name,
        type_params: I,
        int_params: J,
    ) -> TargetExtType<'ctx, B>
    where
        Name: Into<String>,
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
        J: IntoIterator<Item = u32>,
    {
        let name: String = name.into();
        let type_params: Box<[TypeId]> = type_params.into_iter().map(|t| t.into().id()).collect();
        let int_params: Box<[u32]> = int_params.into_iter().collect();
        TargetExtType::new(
            self.core.ctx.target_ext_type(name, type_params, int_params),
            self.module_ref(),
        )
    }

    pub fn add_function<R, Name>(
        &self,
        name: Name,
        signature: FunctionType<'ctx, B>,
        linkage: crate::global_value::Linkage,
    ) -> IrResult<crate::function::FunctionValue<'ctx, R, B>>
    where
        R: crate::marker::ReturnMarker,
        Name: AsRef<str>,
    {
        let name = name.as_ref();
        if !name.is_empty() && self.core.global_name_exists(name) {
            return Err(IrError::DuplicateFunctionName {
                name: name.to_owned(),
            });
        }
        let ret_data = self.core.ctx.type_data(signature.return_type().id());
        if !crate::function::signature_matches_marker::<R>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: signature.return_type().kind_label(),
                got: signature.return_type().kind_label(),
            });
        }
        let signature_id = signature.id;
        let fn_data = crate::function::FunctionData::new(
            name.to_owned(),
            signature_id,
            linkage,
            crate::CallingConv::default(),
        );
        let fn_id = self.core.ctx.push_value(crate::value::ValueData {
            ty: signature_id,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.to_owned())),
            debug_loc: None,
            kind: crate::value::ValueKindData::Function(Box::new(fn_data)),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        let param_types: Vec<TypeId> = signature.params().map(|t| t.id()).collect();
        let mut arg_ids = Vec::with_capacity(param_types.len());
        for (slot, &ty) in param_types.iter().enumerate() {
            let slot_u32 = u32::try_from(slot)
                .unwrap_or_else(|_| unreachable!("function parameter slot exceeds u32::MAX"));
            let id = self.core.ctx.push_value(crate::value::ValueData {
                ty,
                name: core::cell::RefCell::new(None),
                debug_loc: None,
                kind: crate::value::ValueKindData::Argument {
                    parent_fn: fn_id,
                    slot: slot_u32,
                },
                use_list: core::cell::RefCell::new(Vec::new()),
            });
            arg_ids.push(id);
        }
        let fn_value_data = self.core.ctx.value_data(fn_id);
        let fn_inner = match &fn_value_data.kind {
            crate::value::ValueKindData::Function(f) => f,
            _ => unreachable!("just pushed Function variant"),
        };
        *fn_inner.args.borrow_mut() = arg_ids.into_boxed_slice();
        self.core.functions.borrow_mut().push(fn_id);
        if !name.is_empty() {
            self.core
                .function_by_name
                .borrow_mut()
                .insert(name.to_owned(), fn_id);
        }
        Ok(
            crate::function::FunctionValue::<'ctx, R, B>::from_parts_unchecked(
                fn_id,
                self.module_ref(),
            ),
        )
    }

    pub fn add_global<N, C>(
        &self,
        name: N,
        value_type: Type<'ctx, B>,
        initializer: C,
    ) -> IrResult<crate::global_variable::GlobalVariable<'ctx, B>>
    where
        N: AsRef<str>,
        C: crate::constant::IsConstant<'ctx, B>,
    {
        crate::global_variable::GlobalBuilder::<B>::new(
            self.module_ref(),
            name.as_ref().to_owned(),
            value_type,
        )
        .initializer(initializer)
        .build()
    }

    pub fn add_global_constant<N, C>(
        &self,
        name: N,
        value_type: Type<'ctx, B>,
        initializer: C,
    ) -> IrResult<crate::global_variable::GlobalVariable<'ctx, B>>
    where
        N: AsRef<str>,
        C: crate::constant::IsConstant<'ctx, B>,
    {
        crate::global_variable::GlobalBuilder::<B>::new(
            self.module_ref(),
            name.as_ref().to_owned(),
            value_type,
        )
        .constant(true)
        .initializer(initializer)
        .build()
    }

    pub fn add_external_global<N>(
        &self,
        name: N,
        value_type: Type<'ctx, B>,
    ) -> IrResult<crate::global_variable::GlobalVariable<'ctx, B>>
    where
        N: AsRef<str>,
    {
        crate::global_variable::GlobalBuilder::<B>::new(
            self.module_ref(),
            name.as_ref().to_owned(),
            value_type,
        )
        .linkage(crate::global_value::Linkage::External)
        .build()
    }

    pub fn global_builder<N>(
        &self,
        name: N,
        value_type: Type<'ctx, B>,
    ) -> crate::global_variable::GlobalBuilder<'ctx, B>
    where
        N: Into<String>,
    {
        crate::global_variable::GlobalBuilder::new(self.module_ref(), name, value_type)
    }

    pub fn get_global(
        &self,
        name: &str,
    ) -> Option<crate::global_variable::GlobalVariable<'ctx, B>> {
        let id = self.core.global_by_name.borrow().get(name).copied()?;
        let value_data = self.core.ctx.value_data(id);
        Some(
            crate::global_variable::GlobalVariable::from_parts_unchecked(
                id,
                self.module_ref(),
                value_data.ty,
            ),
        )
    }

    pub fn alias_builder<C, Name>(
        &self,
        name: Name,
        value_type: Type<'ctx, B>,
        aliasee: C,
    ) -> crate::global_alias::GlobalAliasBuilder<'ctx, B>
    where
        C: crate::constant::IsConstant<'ctx, B>,
        Name: Into<String>,
    {
        crate::global_alias::GlobalAliasBuilder::new(self.module_ref(), name, value_type, aliasee)
    }

    pub fn get_alias(&self, name: &str) -> Option<GlobalAlias<'ctx, B>> {
        let id = self.core.alias_by_name.borrow().get(name).copied()?;
        let value_data = self.core.ctx.value_data(id);
        Some(crate::global_alias::GlobalAlias::from_parts_unchecked(
            id,
            self.module_ref(),
            value_data.ty,
        ))
    }

    pub fn alias_empty(&self) -> bool {
        self.core.alias_empty()
    }

    pub fn ifunc_builder<C, Name>(
        &self,
        name: Name,
        value_type: Type<'ctx, B>,
        resolver: C,
    ) -> crate::global_ifunc::GlobalIFuncBuilder<'ctx, B>
    where
        C: crate::constant::IsConstant<'ctx, B>,
        Name: Into<String>,
    {
        crate::global_ifunc::GlobalIFuncBuilder::new(self.module_ref(), name, value_type, resolver)
    }

    pub fn get_ifunc(&self, name: &str) -> Option<GlobalIFunc<'ctx, B>> {
        let id = self.core.ifunc_by_name.borrow().get(name).copied()?;
        let value_data = self.core.ctx.value_data(id);
        Some(crate::global_ifunc::GlobalIFunc::from_parts_unchecked(
            id,
            self.module_ref(),
            value_data.ty,
        ))
    }

    pub fn ifunc_empty(&self) -> bool {
        self.core.ifunc_empty()
    }

    pub fn global_empty(&self) -> bool {
        self.core.global_empty()
    }

    pub fn set_source_filename<N>(&self, filename: N)
    where
        N: Into<String>,
    {
        self.core.set_source_filename(filename);
    }

    pub fn clear_source_filename(&self) {
        self.core.clear_source_filename();
    }

    pub fn set_data_layout<L>(&self, layout: L) -> IrResult<()>
    where
        L: AsRef<str>,
    {
        self.core.set_data_layout(layout)
    }

    pub fn set_data_layout_value(&self, layout: DataLayout) {
        self.core.set_data_layout_value(layout);
    }

    pub fn set_target_triple<T>(&self, triple: T)
    where
        T: Into<String>,
    {
        self.core.set_target_triple(triple);
    }

    pub fn clear_target_triple(&self) {
        self.core.clear_target_triple();
    }

    pub fn set_module_asm<A>(&self, asm: A)
    where
        A: Into<String>,
    {
        self.core.set_module_asm(asm);
    }

    pub fn append_module_asm<A>(&self, line: A)
    where
        A: AsRef<str>,
    {
        self.core.append_module_asm(line);
    }

    pub fn get_or_insert_comdat(&self, name: &str) -> ComdatRef<'ctx, B> {
        self.core.get_or_insert_comdat::<B, _>(name)
    }

    pub fn get_comdat(&self, name: &str) -> Option<ComdatRef<'ctx, B>> {
        self.core.get_comdat::<B>(name)
    }

    pub fn inline_asm<Asm, Constraints>(
        &self,
        fn_ty: FunctionType<'ctx, B>,
        asm: Asm,
        constraints: Constraints,
        options: crate::inline_asm::InlineAsmOptions,
    ) -> crate::inline_asm::InlineAsm<'ctx, B>
    where
        Asm: Into<String>,
        Constraints: Into<String>,
    {
        let ptr_ty = self.ptr_type(0).as_type().id();
        let data = crate::inline_asm::InlineAsmData {
            asm_string: asm.into(),
            constraint_string: constraints.into(),
            fn_ty: fn_ty.as_type().id(),
            has_side_effects: options.has_side_effects(),
            is_align_stack: options.is_align_stack(),
            can_unwind: options.can_unwind(),
            dialect: options.dialect(),
        };
        let id = self.core.ctx.push_value(crate::value::ValueData {
            ty: ptr_ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: crate::value::ValueKindData::InlineAsm(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        crate::inline_asm::InlineAsm::from_parts(id, self.module_ref(), ptr_ty)
    }

    pub fn metadata_string<S>(&self, s: S) -> MetadataId
    where
        S: Into<String>,
    {
        self.core.metadata_string(s)
    }

    pub fn metadata_tuple<Ops>(&self, operands: Ops) -> MetadataId
    where
        Ops: AsRef<[MetadataRef]>,
    {
        self.core.metadata_tuple(operands)
    }

    pub fn metadata_tuple_with_distinct<Ops>(&self, distinct: bool, operands: Ops) -> MetadataId
    where
        Ops: AsRef<[MetadataRef]>,
    {
        self.core.metadata_tuple_with_distinct(distinct, operands)
    }

    pub fn metadata_constant<C>(&self, c: C) -> MetadataId
    where
        C: IsConstant<'ctx, B>,
    {
        let id = c.as_constant().id;
        self.core.metadata_constant_value(id)
    }

    pub fn metadata_specialized(&self, node: SpecializedMetadataNode) -> MetadataId {
        self.core.metadata_specialized(node)
    }

    pub fn metadata_node(&self, kind: MetadataKind) -> MetadataId {
        self.core.metadata_node(kind)
    }

    pub fn metadata_as_value(
        &self,
        md: crate::metadata::MetadataId,
    ) -> crate::value::Value<'ctx, B> {
        let ty = self.core.ctx.metadata();
        if let Some(&id) = self.core.metadata_as_value_cache.borrow().get(&md) {
            return crate::value::Value::from_parts(id, self.module_ref(), ty);
        }
        let id = self.core.ctx.push_value(crate::value::ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: crate::value::ValueKindData::MetadataAsValue(md),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.core
            .metadata_as_value_cache
            .borrow_mut()
            .insert(md, id);
        crate::value::Value::from_parts(id, self.module_ref(), ty)
    }

    pub fn metadata_reserve(&self) -> MetadataId {
        self.core.metadata_reserve()
    }

    pub fn metadata_set(
        &self,
        id: crate::metadata::MetadataId,
        kind: crate::metadata::MetadataKind,
    ) {
        self.core.metadata_set(id, kind);
    }

    pub fn metadata_get(
        &self,
        id: crate::metadata::MetadataId,
    ) -> Option<crate::metadata::MetadataKind> {
        self.core.metadata_get(id)
    }

    pub fn metadata_count(&self) -> usize {
        self.core.metadata_count()
    }

    pub fn get_or_insert_named_metadata<Name>(&self, name: Name) -> usize
    where
        Name: Into<String>,
    {
        self.core.get_or_insert_named_metadata(name)
    }

    pub fn named_metadata_add_operand(&self, index: usize, op: MetadataRef) {
        self.core.named_metadata_add_operand(index, op);
    }

    pub fn named_metadata_count(&self) -> usize {
        self.core.named_metadata_count()
    }

    pub fn append_use_list_order(&self, record: UseListOrderRecord) -> IrResult<()> {
        self.core.append_use_list_order(record)
    }

    pub fn append_use_list_order_bb(&self, record: UseListOrderBBRecord) -> IrResult<()> {
        self.core.append_use_list_order_bb(record)
    }

    pub fn set_attribute_group(&self, id: u32, storage: AttributeStorage) {
        self.core.set_attribute_group(id, storage);
    }

    /// Verify the module and consume it into the `Verified` state.
    pub fn verify(self) -> IrResult<Module<'ctx, B, Verified>> {
        crate::verifier::Verifier::new(self.as_view()).run()?;
        Ok(Module {
            core: self.core,
            _brand: PhantomData,
            _state: PhantomData,
        })
    }
}

impl<'ctx, B: ModuleBrand> Module<'ctx, B, Verified> {
    /// Strip the verified state after mutation is required.
    pub fn unverify(self) -> Module<'ctx, B, Unverified> {
        Module {
            core: self.core,
            _brand: PhantomData,
            _state: PhantomData,
        }
    }
}

// `&'ctx TypeData` borrows are *not* mutated; they point into a
// `boxcar::Vec` that only ever appends. The `RefCell`s inside `Context`
// guard hashmap mutation, never the arena, so iteration / accessor
// borrows of payload data are safe even while construction proceeds.
//
// `Module<'ctx>: !Sync` falls out of the `RefCell` fields. `Send` is
// blocked by `&'ctx` references in handles transitively, which is fine
// for a "one context per thread" model.

impl<'ctx, B: ModuleBrand, S> core::fmt::Display for Module<'ctx, B, S> {
    /// Print the module as textual `.ll`. Mirrors `Module::print` from
    /// `llvm/lib/IR/AsmWriter.cpp`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_module(f, self.core)
    }
}
