//! Function value. Mirrors `llvm/include/llvm/IR/Function.h` and
//! `llvm/lib/IR/Function.cpp`.
//!
//! ## What's shipped
//!
//! Phase D minimum: enough to model `define <ret> @name(<params>) { ... }`
//! with `Linkage`, visibility, DLL storage class, DSO locality, calling
//! convention, [`UnnamedAddr`], address space, section/partition, comdat,
//! alignment, GC, prefix/prologue/personality data, metadata attachments, and
//! per-index attribute slots.
//!
//! ## Storage shape
//!
//! - The function value lives at one value-arena entry.
//! - Each parameter is its own value-arena entry (one argument-category
//!   record per slot), so an `Argument<'ctx>` can be `Copy` and
//!   round-trip through the user/use machinery exactly like any other
//!   value.
//! - Basic blocks live in a `RefCell<Vec<ValueId>>` so the IRBuilder
//!   can append while holding a `&'ctx ModuleCore` borrow.
//!
//! ## Return-type safety (Phase A3)
//!
//! [`FunctionValue<'ctx, R>`] carries a [`ReturnMarker`] generic. A
//! function known to return `i32` is spelled
//! `FunctionValue<'ctx, i32>`; one that returns `void` is
//! `FunctionValue<'ctx, ()>`; parsed / runtime IR uses
//! `FunctionValue<'ctx, Dyn>`. The marker propagates to the function's
//! basic blocks and to any [`IRBuilder`](crate::IRBuilder) positioned
//! inside them, so the builder's `build_ret` can be statically typed.

use core::cell::{Cell, RefCell};
use core::marker::PhantomData;

use crate::AttrIndex;
use crate::DebugLoc;
use crate::align::MaybeAlign;
use crate::argument::Argument;
use crate::attributes::AttributeStorage;
use crate::basic_block::{BasicBlock, BasicBlockData};
use crate::calling_conv::CallingConv;
use crate::comdat::ComdatRef;
use crate::constant::{Constant, IsConstant};
use crate::derived_types::FunctionType;
use crate::error::{IrError, IrResult};
use crate::float_kind::FloatKind;
use crate::global_value::{DllStorageClass, DsoLocality, Linkage, Visibility};
use crate::int_width::IntWidth;
use crate::marker::{Dyn, ReturnMarker};
use crate::module::{
    Module, ModuleBrand, ModuleCore, ModuleRef, ModuleView, Unverified, UseListOrderRecord,
    validate_use_list_order_indexes,
};
use crate::r#type::{Type, TypeData, TypeId};
use crate::unnamed_addr::UnnamedAddr;
use crate::value::{
    HasDebugLoc, HasName, IsValue, Typed, Value, ValueData, ValueId, ValueKindData, sealed,
};
use crate::value_symbol_table::ValueSymbolTable;

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload stored under
/// [`ValueKindData::Function`](crate::value::ValueKindData::Function).
#[derive(Debug)]
pub(crate) struct FunctionData {
    pub(crate) name: String,
    pub(crate) signature: TypeId,
    pub(crate) linkage: RefCell<Linkage>,
    pub(crate) visibility: RefCell<Visibility>,
    pub(crate) dll_storage_class: RefCell<DllStorageClass>,
    pub(crate) dso_locality: RefCell<DsoLocality>,
    pub(crate) calling_conv: RefCell<CallingConv>,
    pub(crate) unnamed_addr: RefCell<UnnamedAddr>,
    pub(crate) address_space: RefCell<u32>,
    pub(crate) section: RefCell<Option<String>>,
    pub(crate) partition: RefCell<Option<String>>,
    pub(crate) align: RefCell<MaybeAlign>,
    pub(crate) gc: RefCell<Option<String>>,
    pub(crate) prefix_data: Cell<Option<ValueId>>,
    pub(crate) prologue_data: Cell<Option<ValueId>>,
    pub(crate) personality_fn: Cell<Option<ValueId>>,
    pub(crate) comdat: RefCell<Option<String>>,
    /// One value-id per parameter, in declaration order. Set once at
    /// function-creation time after every argument value-id is known;
    /// LLVM does not allow adding parameters in place afterwards, so
    /// this stays effectively immutable past the constructor.
    pub(crate) args: RefCell<Box<[ValueId]>>,
    pub(crate) basic_blocks: RefCell<Vec<ValueId>>,
    pub(crate) attributes: RefCell<AttributeStorage>,
    pub(crate) function_attr_groups: RefCell<Vec<u32>>,
    pub(crate) use_list_orders: RefCell<Vec<UseListOrderRecord>>,
    pub(crate) metadata: RefCell<crate::metadata::MetadataAttachmentSet>,
    pub(crate) symbol_table: ValueSymbolTable,
}

impl FunctionData {
    pub(crate) fn new(
        name: String,
        signature: TypeId,
        linkage: Linkage,
        calling_conv: CallingConv,
    ) -> Self {
        Self {
            name,
            signature,
            linkage: RefCell::new(linkage),
            visibility: RefCell::new(Visibility::Default),
            dll_storage_class: RefCell::new(DllStorageClass::Default),
            dso_locality: RefCell::new(DsoLocality::Default),
            calling_conv: RefCell::new(calling_conv),
            unnamed_addr: RefCell::new(UnnamedAddr::None),
            address_space: RefCell::new(0),
            section: RefCell::new(None),
            partition: RefCell::new(None),
            align: RefCell::new(MaybeAlign::NONE),
            gc: RefCell::new(None),
            prefix_data: Cell::new(None),
            prologue_data: Cell::new(None),
            personality_fn: Cell::new(None),
            comdat: RefCell::new(None),
            args: RefCell::new(Box::new([])),
            basic_blocks: RefCell::new(Vec::new()),
            attributes: RefCell::new(AttributeStorage::new()),
            function_attr_groups: RefCell::new(Vec::new()),
            use_list_orders: RefCell::new(Vec::new()),
            metadata: RefCell::new(crate::metadata::MetadataAttachmentSet::new()),
            symbol_table: ValueSymbolTable::new(),
        }
    }
}

// --------------------------------------------------------------------------
// Public handle
// --------------------------------------------------------------------------

/// Typed handle to a function value parametrised by its return shape.
///
/// The `R: ReturnMarker` parameter encodes the return type at compile
/// time (see [`crate::marker`]). Use [`FunctionValue::as_dyn`]
/// to widen to the runtime-checked [`Dyn`] form.
pub struct FunctionValue<
    'ctx,
    R: ReturnMarker,
    B: crate::module::ModuleBrand = crate::module::Brand<'ctx>,
> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx, B>,
    /// Cached signature type id. The value's value-arena type is the
    /// pointer-to-function on real LLVM; here we cache the function-
    /// type id directly so `signature()` is a thin lookup.
    pub(crate) signature: TypeId,
    pub(crate) _r: PhantomData<R>,
}

// Manual derives — `derive` would propagate `R: Trait` bounds that
// callers should not have to spell. The fields themselves are all
// trivially `Copy`/`Hash`/`Eq`.
impl<'ctx, R: ReturnMarker, B: ModuleBrand> Clone for FunctionValue<'ctx, R, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> Copy for FunctionValue<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> PartialEq for FunctionValue<'ctx, R, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.signature == other.signature
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> Eq for FunctionValue<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> core::hash::Hash for FunctionValue<'ctx, R, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.signature.hash(h);
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> core::fmt::Debug for FunctionValue<'ctx, R, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FunctionValue")
            .field("id", &self.id)
            .field("signature", &self.signature)
            .finish()
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> FunctionValue<'ctx, R, B> {
    /// Construct from raw parts. Crate-internal: only the
    /// function-creation paths hand these out, after they've
    /// validated that the signature's return type matches `R`.
    #[inline]
    pub(crate) fn from_parts_unchecked<M>(id: ValueId, module: M) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        let module = module.into();
        let data = module.value_data(id);
        let signature = match &data.kind {
            ValueKindData::Function(f) => f.signature,
            _ => unreachable!("FunctionValue handle invariant: kind is Function"),
        };
        Self {
            id,
            module,
            signature,
            _r: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle. The value's IR type is
    /// the function-pointer type; we use the cached signature here
    /// because the slice does not yet need the pointer wrapper, and
    /// LLVM 17+ pointers are opaque (so the signature is the only
    /// useful per-value type-side information).
    #[inline]
    pub fn as_value(self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.signature,
        }
    }

    /// Erase the return-shape marker, producing a runtime-checked
    /// [`Dyn`] handle.
    #[inline]
    pub fn as_dyn(self) -> FunctionValue<'ctx, Dyn, B> {
        FunctionValue {
            id: self.id,
            module: self.module,
            signature: self.signature,
            _r: PhantomData,
        }
    }

    /// Read-only function view for analysis and pass contexts.
    #[inline]
    pub fn as_view(self) -> crate::pass_context::FunctionView<'ctx, B> {
        crate::pass_context::FunctionView::from(self)
    }

    /// Borrow the storage payload.
    pub(crate) fn data(self) -> &'ctx FunctionData {
        match &self.as_value().data().kind {
            ValueKindData::Function(f) => f,
            _ => unreachable!("FunctionValue handle invariant: kind is Function"),
        }
    }

    /// Function name.
    #[inline]
    pub fn name(self) -> &'ctx str {
        &self.data().name
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    /// Function signature.
    #[inline]
    pub fn signature(self) -> FunctionType<'ctx, B> {
        FunctionType::new(self.signature, self.module)
    }

    /// Function return type.
    #[inline]
    pub fn return_type(self) -> Type<'ctx, B> {
        self.signature().return_type()
    }

    /// Linkage of this function.
    #[inline]
    pub fn linkage(self) -> Linkage {
        *self.data().linkage.borrow()
    }

    /// Update linkage.
    #[inline]
    pub fn set_linkage(self, _module: &Module<'ctx, B, Unverified>, linkage: Linkage) {
        *self.data().linkage.borrow_mut() = linkage;
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        *self.data().visibility.borrow()
    }

    #[inline]
    pub fn set_visibility(self, _module: &Module<'ctx, B, Unverified>, visibility: Visibility) {
        *self.data().visibility.borrow_mut() = visibility;
    }

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        *self.data().dll_storage_class.borrow()
    }

    #[inline]
    pub fn set_dll_storage_class(
        self,
        _module: &Module<'ctx, B, Unverified>,
        cls: DllStorageClass,
    ) {
        *self.data().dll_storage_class.borrow_mut() = cls;
    }

    #[inline]
    pub fn dso_locality(self) -> DsoLocality {
        *self.data().dso_locality.borrow()
    }

    #[inline]
    pub fn set_dso_locality(self, _module: &Module<'ctx, B, Unverified>, locality: DsoLocality) {
        *self.data().dso_locality.borrow_mut() = locality;
    }

    /// Calling convention.
    #[inline]
    pub fn calling_conv(self) -> CallingConv {
        *self.data().calling_conv.borrow()
    }

    /// Update calling convention.
    #[inline]
    pub fn set_calling_conv(self, _module: &Module<'ctx, B, Unverified>, cc: CallingConv) {
        *self.data().calling_conv.borrow_mut() = cc;
    }

    /// Unnamed-address marker. Mirrors `GlobalValue::getUnnamedAddr`.
    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        *self.data().unnamed_addr.borrow()
    }

    /// Update the unnamed-address marker. Mirrors
    /// `GlobalValue::setUnnamedAddr`.
    #[inline]
    pub fn set_unnamed_addr(self, _module: &Module<'ctx, B, Unverified>, value: UnnamedAddr) {
        *self.data().unnamed_addr.borrow_mut() = value;
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        *self.data().address_space.borrow()
    }

    #[inline]
    pub fn set_address_space(self, _module: &Module<'ctx, B, Unverified>, address_space: u32) {
        *self.data().address_space.borrow_mut() = address_space;
    }

    pub fn section(self) -> Option<String> {
        self.data().section.borrow().clone()
    }

    pub fn set_section(
        self,
        _module: &Module<'ctx, B, Unverified>,
        section: Option<impl Into<String>>,
    ) {
        *self.data().section.borrow_mut() = section.map(Into::into);
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

    #[inline]
    pub fn align(self) -> MaybeAlign {
        *self.data().align.borrow()
    }

    #[inline]
    pub fn set_align(self, _module: &Module<'ctx, B, Unverified>, align: MaybeAlign) {
        *self.data().align.borrow_mut() = align;
    }

    pub fn gc(self) -> Option<String> {
        self.data().gc.borrow().clone()
    }

    pub fn set_gc(self, _module: &Module<'ctx, B, Unverified>, gc: Option<impl Into<String>>) {
        *self.data().gc.borrow_mut() = gc.map(Into::into);
    }
    pub fn prefix_data(self) -> Option<Constant<'ctx, B>> {
        self.data().prefix_data.get().map(|id| {
            let data = self.module.module().context().value_data(id);
            Constant {
                id,
                module: self.module,
                ty: data.ty,
            }
        })
    }

    pub fn set_prefix_data(
        self,
        _module: &Module<'ctx, B, Unverified>,
        data: Option<impl IsConstant<'ctx, B>>,
    ) -> IrResult<()> {
        let id = self.checked_constant_id(data)?;
        self.data().prefix_data.set(id);
        Ok(())
    }

    pub fn prologue_data(self) -> Option<Constant<'ctx, B>> {
        self.data().prologue_data.get().map(|id| {
            let data = self.module.module().context().value_data(id);
            Constant {
                id,
                module: self.module,
                ty: data.ty,
            }
        })
    }

    pub fn set_prologue_data(
        self,
        _module: &Module<'ctx, B, Unverified>,
        data: Option<impl IsConstant<'ctx, B>>,
    ) -> IrResult<()> {
        let id = self.checked_constant_id(data)?;
        self.data().prologue_data.set(id);
        Ok(())
    }

    pub fn personality_fn(self) -> Option<Constant<'ctx, B>> {
        self.data().personality_fn.get().map(|id| {
            let data = self.module.module().context().value_data(id);
            Constant {
                id,
                module: self.module,
                ty: data.ty,
            }
        })
    }

    pub fn set_personality_fn(
        self,
        _module: &Module<'ctx, B, Unverified>,
        data: Option<impl IsConstant<'ctx, B>>,
    ) -> IrResult<()> {
        let id = self.checked_constant_id(data)?;
        self.data().personality_fn.set(id);
        Ok(())
    }

    fn checked_constant_id(
        self,
        data: Option<impl IsConstant<'ctx, B>>,
    ) -> IrResult<Option<ValueId>> {
        match data {
            None => Ok(None),
            Some(value) => {
                let constant = value.as_constant();
                if constant.module != self.module {
                    return Err(IrError::ForeignValue);
                }
                Ok(Some(constant.as_value().id))
            }
        }
    }

    pub fn comdat(self) -> Option<ComdatRef<'ctx, B>> {
        let name = self.data().comdat.borrow().clone()?;
        self.module.module().get_comdat::<B>(&name)
    }

    pub fn set_comdat(
        self,
        _module: &Module<'ctx, B, Unverified>,
        comdat: Option<ComdatRef<'ctx, B>>,
    ) -> IrResult<()> {
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

    /// Add an attribute at `index` to an already-created function.
    /// Mirrors `Function::addAttributeAtIndex`. Complements the
    /// build-time [`function_builder().attribute`](crate::function::FunctionBuilder::attribute)
    /// path for the common case where a function is forward-declared
    /// with [`add_function`](crate::Module::add_function) and gains
    /// attributes only once its body is being emitted. De-duplicates by
    /// structural equality.
    #[inline]
    pub fn add_attribute(
        self,
        _module: &Module<'ctx, B, Unverified>,
        index: AttrIndex,
        attr: crate::Attribute<'ctx>,
    ) {
        self.data().attributes.borrow_mut().add(index, attr);
    }

    pub fn add_function_attr_group(self, _module: &Module<'ctx, B, Unverified>, group: u32) {
        let mut groups = self.data().function_attr_groups.borrow_mut();
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    pub fn set_attributes(
        self,
        _module: &Module<'ctx, B, Unverified>,
        attributes: AttributeStorage,
    ) {
        *self.data().attributes.borrow_mut() = attributes;
    }

    pub(crate) fn function_attr_groups(self) -> Vec<u32> {
        self.data().function_attr_groups.borrow().clone()
    }

    /// Convenience: add a string-valued attribute (`"key"="value"`) at
    /// `index`. Mirrors `Function::addAttributeAtIndex` with a string
    /// attribute, e.g. `"frame-pointer"="all"`.
    #[inline]
    pub fn set_string_attribute(
        self,
        module: &Module<'ctx, B, Unverified>,
        index: AttrIndex,
        key: impl Into<String>,
        value: impl Into<String>,
    ) {
        self.add_attribute(module, index, crate::Attribute::string(key, value));
    }

    /// Number of parameters.
    #[inline]
    pub fn arg_count(self) -> u32 {
        u32::try_from(self.data().args.borrow().len())
            .unwrap_or_else(|_| unreachable!("function has more than u32::MAX params"))
    }

    /// Parameter at slot `index`. Mirrors `Function::getArg`.
    pub fn param(self, index: u32) -> IrResult<Argument<'ctx>> {
        let count = self.arg_count();
        let slot = usize::try_from(index)
            .unwrap_or_else(|_| unreachable!("u32 fits in usize on supported targets"));
        let id = self
            .data()
            .args
            .borrow()
            .get(slot)
            .copied()
            .ok_or(IrError::ArgumentIndexOutOfRange { index, count })?;
        let arg_ty = self
            .signature()
            .params()
            .nth(slot)
            .unwrap_or_else(|| unreachable!("argument count matches signature"))
            .id();
        Ok(Argument::from_parts(
            id,
            self.module.module(),
            arg_ty,
            self.id,
            index,
        ))
    }

    /// Iterate the function parameters in declaration order.
    pub fn params(self) -> impl ExactSizeIterator<Item = Argument<'ctx>> + 'ctx {
        let module = self.module.module();
        let parent = self.id;
        let signature = self.signature;
        let args: Box<[ValueId]> = self.data().args.borrow().clone();
        let param_types: Vec<TypeId> = FunctionType::new(signature, module)
            .params()
            .map(|t| t.id())
            .collect();
        args.into_vec()
            .into_iter()
            .zip(param_types)
            .enumerate()
            .map(move |(slot, (id, ty))| {
                let slot = u32::try_from(slot)
                    .unwrap_or_else(|_| unreachable!("function parameter slot exceeds u32::MAX"));
                Argument::from_parts(id, module, ty, parent, slot)
            })
    }

    // ---- Basic blocks ----

    /// Append a fresh basic block to this function. Mirrors
    /// `BasicBlock::Create(ctx, name, parent)`. The block inherits
    /// the function's [`ReturnMarker`], so positioned IRBuilders see
    /// the typed return shape transitively.
    pub fn append_basic_block(
        self,
        _module: &Module<'ctx, B, Unverified>,
        name: impl Into<String>,
    ) -> BasicBlock<'ctx, R> {
        let name = name.into();
        let label_ty = self.module.module().label_type().as_type().id();
        let bb_id = self.module.module().context().push_value(ValueData {
            ty: label_ty,
            name: RefCell::new((!name.is_empty()).then(|| name.clone())),
            debug_loc: None,
            kind: ValueKindData::BasicBlock(BasicBlockData::new(Some(self.id))),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        // Register the block under its name in the per-function symbol table.
        if !name.is_empty() {
            self.data().symbol_table.insert(&name, bb_id);
        }
        self.data().basic_blocks.borrow_mut().push(bb_id);
        BasicBlock::from_parts(bb_id, self.module.module(), label_ty)
    }

    /// Iterate the basic blocks in insertion order.
    pub fn basic_blocks(self) -> impl ExactSizeIterator<Item = BasicBlock<'ctx, R>> + 'ctx {
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        let ids: Vec<ValueId> = self.data().basic_blocks.borrow().clone();
        ids.into_iter()
            .map(move |id| BasicBlock::from_parts(id, module, label_ty))
    }

    /// Append a function-local `uselistorder` record.
    pub fn append_use_list_order(self, record: UseListOrderRecord) -> IrResult<()> {
        validate_use_list_order_indexes(&record.indexes)?;
        self.data().use_list_orders.borrow_mut().push(record);
        Ok(())
    }

    pub(crate) fn use_list_orders(self) -> Vec<UseListOrderRecord> {
        self.data().use_list_orders.borrow().clone()
    }

    pub fn entry_block(self) -> Option<BasicBlock<'ctx, R>> {
        let id = *self.data().basic_blocks.borrow().first()?;
        let module = self.module.module();
        Some(BasicBlock::from_parts(
            id,
            module,
            module.label_type().as_type().id(),
        ))
    }

    /// Bind a name to a value id in the symbol table. Mirrors the
    /// behavior of `ValueSymbolTable::reinsertWithName` on
    /// well-formed first inserts; conflicts return `false`.
    pub(crate) fn register_value_name(self, name: &str, id: ValueId) -> bool {
        self.data().symbol_table.insert(name, id)
    }

    /// View this function as a `ptr`-typed [`Constant`] referencing it by
    /// name — i.e. the constant `ptr @<name>`, suitable as a global
    /// initializer.
    ///
    /// Mirrors LLVM's `GlobalValue`: the function's value type remains its
    /// signature, while the constant's type is the default-address-space
    /// pointer returned by `GlobalValue::getType`.
    #[inline]
    pub fn as_global_constant_ptr(self) -> crate::constant::Constant<'ctx, B> {
        let module = self.module.module();
        let ptr_ty = module.ptr_type(0).as_type().id();
        let id = module
            .context()
            .intern_constant_global_value_ref(ptr_ty, self.id);
        crate::constant::Constant {
            id,
            module: self.module,
            ty: ptr_ty,
        }
    }

    /// A `ptr`-typed constant referencing this function, as a *distinct* arena
    /// node (`getelementptr inbounds (i8, ptr @<self>, i64 0)`).
    ///
    /// Unlike [`Self::as_global_constant_ptr`] (which reuses the function's own
    /// value-id, so its arena type is the function *signature*), this interns a
    /// separate `ptr`-typed constant. Needed inside an **aggregate** initializer,
    /// where the assembly writer prints each element's type from the element
    /// value's arena type: a bare function reference would print as `void () @f`
    /// (invalid — "functions are not values"), whereas this prints as a proper
    /// `ptr getelementptr (...)` element. The byte offset is always 0 (the
    /// function entry).
    pub fn as_aggregate_ptr(self, addr_space: u32) -> crate::constant::Constant<'ctx, B> {
        let module = self.module.module();
        let ptr_ty = module.ptr_type(addr_space).as_type().id();
        let id = module
            .context()
            .intern_constant_gep_offset(ptr_ty, self.id, 0);
        crate::constant::Constant {
            id,
            module: self.module,
            ty: ptr_ty,
        }
    }
}

// --------------------------------------------------------------------------
// Marker-narrowing helpers
// --------------------------------------------------------------------------

/// Crate-internal: validate that a function's signature matches the
/// caller's chosen [`ReturnMarker`].
///
/// Mirrors the runtime side of LLVM's `Function::Create` invariant
/// (the `RetTy` template parameter on the C++ IRBuilder enforces this
/// at the type level; we do the same here for static markers, with
/// a runtime fallback for [`Dyn`] / aggregate return types).
pub(crate) fn signature_matches_marker<R: ReturnMarker>(ret: &TypeData) -> bool {
    use crate::marker::ExpectedRetKind;
    match R::expected_kind() {
        ExpectedRetKind::Dyn => true,
        ExpectedRetKind::Void => matches!(ret, TypeData::Void),
        ExpectedRetKind::Ptr => matches!(ret, TypeData::Pointer { .. }),
        ExpectedRetKind::IntStatic(b) => matches!(ret, TypeData::Integer { bits } if *bits == b),
        ExpectedRetKind::IntDyn => matches!(ret, TypeData::Integer { .. }),
        ExpectedRetKind::FloatStatic(label) => match label {
            "half" => matches!(ret, TypeData::Half),
            "bfloat" => matches!(ret, TypeData::BFloat),
            "float" => matches!(ret, TypeData::Float),
            "double" => matches!(ret, TypeData::Double),
            "fp128" => matches!(ret, TypeData::Fp128),
            "x86_fp80" => matches!(ret, TypeData::X86Fp80),
            "ppc_fp128" => matches!(ret, TypeData::PpcFp128),
            _ => unreachable!("FloatKind::ieee_label() returned unrecognised tag"),
        },
        ExpectedRetKind::FloatDyn => matches!(
            ret,
            TypeData::Half
                | TypeData::BFloat
                | TypeData::Float
                | TypeData::Double
                | TypeData::Fp128
                | TypeData::X86Fp80
                | TypeData::PpcFp128
        ),
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand> sealed::Sealed for FunctionValue<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> IsValue<'ctx, B> for FunctionValue<'ctx, R, B> {
    #[inline]
    fn as_value(self) -> Value<'ctx, B> {
        FunctionValue::as_value(self)
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> Typed<'ctx, B> for FunctionValue<'ctx, R, B> {
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        Type::new(self.signature, self.module)
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> HasName<'ctx, B> for FunctionValue<'ctx, R, B> {
    #[inline]
    fn name(self) -> Option<String> {
        Some(FunctionValue::name(self).to_owned())
    }
    #[inline]
    fn set_name(self, _module_token: &Module<'ctx, B, Unverified>, _name: &str) {
        // Renaming a function in place is its own diff: the symbol
        // table needs updating, and external linkers care. Phase D
        // adds the proper path; today this is a no-op.
    }
    #[inline]
    fn clear_name(self, _module_token: &Module<'ctx, B, Unverified>) {}
}
impl<R: ReturnMarker, B: ModuleBrand> HasDebugLoc for FunctionValue<'_, R, B> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> From<FunctionValue<'ctx, R, B>>
    for Value<'ctx, B>
{
    #[inline]
    fn from(f: FunctionValue<'ctx, R, B>) -> Self {
        f.as_value()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for FunctionValue<'ctx, Dyn, B> {
    type Error = IrError;
    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        match &v.data().kind {
            ValueKindData::Function(f) => Ok(Self {
                id: v.id,
                module: v.module,
                signature: f.signature,
                _r: PhantomData,
            }),
            _ => Err(IrError::ValueCategoryMismatch {
                expected: crate::error::ValueCategoryLabel::Function,
                got: v.category().into(),
            }),
        }
    }
}

// --------------------------------------------------------------------------
// FunctionBuilder
// --------------------------------------------------------------------------

/// Builder for [`Module::function_builder`]. Use this when you want
/// to set linkage, calling convention, attributes, parameter names,
/// or `unnamed_addr` at creation time without the call site growing
/// past three arguments.
///
/// The `R: ReturnMarker` parameter selects the typed
/// [`FunctionValue<'ctx, R>`] surface; the [`build`](Self::build)
/// step validates that the signature's return type matches `R` and
/// returns [`IrError::ReturnTypeMismatch`] otherwise.
///
/// ```ignore
/// let f = m.function_builder::<i32, _>("worker", fn_ty)
///     .linkage(Linkage::Internal)
///     .calling_conv(CallingConv::FAST)
///     .unnamed_addr(UnnamedAddr::Local)
///     .param_name(0, "x")
///     .return_attribute(AttrKind::NoUndef)
///     .build()?;
/// ```
pub struct FunctionBuilder<'ctx, R: ReturnMarker> {
    module: &'ctx ModuleCore,
    name: String,
    signature: FunctionType<'ctx>,
    linkage: Linkage,
    visibility: Visibility,
    dll_storage_class: DllStorageClass,
    dso_locality: DsoLocality,
    calling_conv: CallingConv,
    unnamed_addr: UnnamedAddr,
    address_space: u32,
    section: Option<String>,
    partition: Option<String>,
    align: MaybeAlign,
    gc: Option<String>,
    prefix_data: Option<Constant<'ctx>>,
    prologue_data: Option<Constant<'ctx>>,
    personality_fn: Option<Constant<'ctx>>,
    comdat: Option<ComdatRef<'ctx>>,
    attributes: AttributeStorage,
    function_attr_groups: Vec<u32>,
    /// Pending `(slot, name)` pairs to apply after the function value
    /// exists. Slots out of range error at `build()` time.
    param_names: Vec<(u32, String)>,
    _r: PhantomData<R>,
}

impl<'ctx, R: ReturnMarker> FunctionBuilder<'ctx, R> {
    /// Crate-internal constructor; users start through
    /// [`Module::function_builder`].
    pub(crate) fn new(
        module: &'ctx ModuleCore,
        name: impl Into<String>,
        signature: FunctionType<'ctx>,
    ) -> Self {
        Self {
            module,
            name: name.into(),
            signature,
            linkage: Linkage::default(),
            visibility: Visibility::Default,
            dll_storage_class: DllStorageClass::Default,
            dso_locality: DsoLocality::Default,
            calling_conv: CallingConv::default(),
            unnamed_addr: UnnamedAddr::None,
            address_space: 0,
            section: None,
            partition: None,
            align: MaybeAlign::NONE,
            gc: None,
            prefix_data: None,
            prologue_data: None,
            personality_fn: None,
            comdat: None,
            attributes: AttributeStorage::new(),
            function_attr_groups: Vec::new(),
            param_names: Vec::new(),
            _r: PhantomData,
        }
    }

    /// Override the linkage.
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

    pub fn dso_locality(mut self, locality: DsoLocality) -> Self {
        self.dso_locality = locality;
        self
    }

    /// Override the calling convention.
    pub fn calling_conv(mut self, cc: CallingConv) -> Self {
        self.calling_conv = cc;
        self
    }

    /// Set the unnamed-address marker. Default is [`UnnamedAddr::None`].
    pub fn unnamed_addr(mut self, value: UnnamedAddr) -> Self {
        self.unnamed_addr = value;
        self
    }

    pub fn address_space(mut self, address_space: u32) -> Self {
        self.address_space = address_space;
        self
    }

    pub fn section(mut self, section: impl Into<String>) -> Self {
        self.section = Some(section.into());
        self
    }

    pub fn partition(mut self, partition: impl Into<String>) -> Self {
        self.partition = Some(partition.into());
        self
    }

    pub fn align(mut self, align: MaybeAlign) -> Self {
        self.align = align;
        self
    }

    pub fn gc(mut self, gc: impl Into<String>) -> Self {
        self.gc = Some(gc.into());
        self
    }
    pub fn prefix_data(mut self, data: impl IsConstant<'ctx>) -> Self {
        self.prefix_data = Some(data.as_constant());
        self
    }

    pub fn prologue_data(mut self, data: impl IsConstant<'ctx>) -> Self {
        self.prologue_data = Some(data.as_constant());
        self
    }

    pub fn personality_fn(mut self, data: impl IsConstant<'ctx>) -> Self {
        self.personality_fn = Some(data.as_constant());
        self
    }

    pub fn comdat(mut self, comdat: ComdatRef<'ctx>) -> Self {
        self.comdat = Some(comdat);
        self
    }

    pub fn attribute(mut self, index: AttrIndex, attr: crate::Attribute<'ctx>) -> Self {
        self.attributes.add(index, attr);
        self
    }

    pub fn attribute_storage(mut self, attributes: AttributeStorage) -> Self {
        self.attributes = attributes;
        self
    }

    pub fn function_attr_group(mut self, group: u32) -> Self {
        if !self.function_attr_groups.contains(&group) {
            self.function_attr_groups.push(group);
        }
        self
    }

    /// Convenience: add an enum-flavored attribute on the function's
    /// return slot. Mirrors `Function::addRetAttr(AttrKind)`.
    pub fn return_attribute(self, kind: crate::AttrKind) -> Self {
        let attr = crate::Attribute::enum_attr(kind)
            .unwrap_or_else(|| unreachable!("return_attribute called with non-enum kind"));
        self.attribute(AttrIndex::Return, attr)
    }

    /// Convenience: add an enum-flavored attribute on parameter
    /// `slot`. Mirrors `Function::addParamAttr(slot, AttrKind)`.
    pub fn param_attribute(self, slot: u32, kind: crate::AttrKind) -> Self {
        let attr = crate::Attribute::enum_attr(kind)
            .unwrap_or_else(|| unreachable!("param_attribute called with non-enum kind"));
        self.attribute(AttrIndex::Param(slot), attr)
    }

    /// Bind a textual name to parameter `slot`. The name is applied
    /// at [`build`](Self::build) time, after the function value's
    /// argument records exist in the value arena.
    pub fn param_name(mut self, slot: u32, name: impl Into<String>) -> Self {
        self.param_names.push((slot, name.into()));
        self
    }

    /// Materialize the function. Mirrors `Function::Create`.
    ///
    /// Returns [`IrError::ReturnTypeMismatch`] if the signature's
    /// return type does not match the chosen [`ReturnMarker`].
    pub fn build(self) -> IrResult<FunctionValue<'ctx, R>> {
        let f = self
            .module
            .add_function::<R>(&self.name, self.signature, self.linkage)?;
        *f.data().visibility.borrow_mut() = self.visibility;
        *f.data().dll_storage_class.borrow_mut() = self.dll_storage_class;
        *f.data().dso_locality.borrow_mut() = self.dso_locality;
        *f.data().calling_conv.borrow_mut() = self.calling_conv;
        *f.data().unnamed_addr.borrow_mut() = self.unnamed_addr;
        *f.data().address_space.borrow_mut() = self.address_space;
        if let Some(section) = self.section {
            *f.data().section.borrow_mut() = Some(section);
        }
        if let Some(partition) = self.partition {
            *f.data().partition.borrow_mut() = Some(partition);
        }
        *f.data().align.borrow_mut() = self.align;
        if let Some(gc) = self.gc {
            *f.data().gc.borrow_mut() = Some(gc);
        }
        if let Some(prefix_data) = self.prefix_data {
            if prefix_data.module != f.module {
                return Err(IrError::ForeignValue);
            }
            f.data().prefix_data.set(Some(prefix_data.as_value().id));
        }
        if let Some(prologue_data) = self.prologue_data {
            if prologue_data.module != f.module {
                return Err(IrError::ForeignValue);
            }
            f.data()
                .prologue_data
                .set(Some(prologue_data.as_value().id));
        }
        if let Some(personality_fn) = self.personality_fn {
            if personality_fn.module != f.module {
                return Err(IrError::ForeignValue);
            }
            f.data()
                .personality_fn
                .set(Some(personality_fn.as_value().id));
        }
        if let Some(comdat) = self.comdat {
            if comdat.module != f.module {
                return Err(IrError::InvalidOperation {
                    message: "comdat does not belong to this module",
                });
            }
            *f.data().comdat.borrow_mut() = Some(comdat.name().to_owned());
        }
        *f.data().attributes.borrow_mut() = self.attributes;
        for group in self.function_attr_groups {
            let mut groups = f.data().function_attr_groups.borrow_mut();
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        // Apply parameter names.
        for (slot, name) in self.param_names {
            let arg = f.param(slot)?;
            arg.as_value().set_name_internal(Some(name));
        }
        Ok(f)
    }
}

// --------------------------------------------------------------------------
// Per-marker monomorphic constructors used by typed builders
// --------------------------------------------------------------------------
//
// `FunctionValue<'ctx, W>` and friends need integer-typed
// return-type accessors. The relevant per-marker accessors live on
// the type-state-aware impl blocks where the IRBuilder constructs
// them; here we expose only what's universally needed.

impl<'ctx, W: IntWidth + ReturnMarker, B: ModuleBrand + 'ctx> FunctionValue<'ctx, W, B> {
    /// Return type as an integer-typed handle. Mirrors the
    /// `Function::getReturnType()` round-trip on a typed function.
    #[inline]
    pub fn return_int_type(self) -> crate::derived_types::IntType<'ctx, W, B> {
        let signature = self.signature();
        crate::derived_types::IntType::new(signature.return_type().id(), self.module)
    }
}

impl<'ctx, K: FloatKind + ReturnMarker, B: ModuleBrand + 'ctx> FunctionValue<'ctx, K, B> {
    /// Return type as a kind-typed float handle.
    #[inline]
    pub fn return_float_type(self) -> crate::derived_types::FloatType<'ctx, K, B> {
        let signature = self.signature();
        crate::derived_types::FloatType::new(signature.return_type().id(), self.module)
    }
}

impl<'ctx, R: ReturnMarker> core::fmt::Display for FunctionValue<'ctx, R> {
    /// Print the function definition as textual `.ll`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_function(f, self.as_dyn())
    }
}
