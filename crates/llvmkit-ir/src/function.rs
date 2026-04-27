//! Function value. Mirrors `llvm/include/llvm/IR/Function.h` and
//! `llvm/lib/IR/Function.cpp`.
//!
//! ## What's shipped
//!
//! Phase D minimum: enough to model `define <ret> @name(<params>) { ... }`
//! with `Linkage`, calling convention, [`UnnamedAddr`], and per-index
//! attribute slots. Visibility, DLL storage class, GC, comdat, prefix
//! data, prologue data, personality, and section/partition controls
//! are deferred to the full Phase D session.
//!
//! ## Storage shape
//!
//! - The function value lives at one value-arena entry.
//! - Each parameter is its own value-arena entry (one argument-category
//!   record per slot), so an `Argument<'ctx>` can be `Copy` and
//!   round-trip through the user/use machinery exactly like any other
//!   value.
//! - Basic blocks live in a `RefCell<Vec<ValueId>>` so the IRBuilder
//!   can append while holding a `&'ctx Module<'ctx>` borrow.
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

use core::cell::RefCell;
use core::marker::PhantomData;

use crate::AttrIndex;
use crate::DebugLoc;
use crate::argument::Argument;
use crate::attributes::AttributeStorage;
use crate::basic_block::{BasicBlock, BasicBlockData};
use crate::calling_conv::CallingConv;
use crate::derived_types::FunctionType;
use crate::error::{IrError, IrResult};
use crate::float_kind::FloatKind;
use crate::global_value::Linkage;
use crate::int_width::IntWidth;
use crate::marker::{Dyn, ReturnMarker};
use crate::module::{Module, ModuleRef};
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
    pub(crate) calling_conv: RefCell<CallingConv>,
    pub(crate) unnamed_addr: RefCell<UnnamedAddr>,
    /// One value-id per parameter, in declaration order. Set once at
    /// function-creation time after every argument value-id is known;
    /// LLVM does not allow adding parameters in place afterwards, so
    /// this stays effectively immutable past the constructor.
    pub(crate) args: RefCell<Box<[ValueId]>>,
    pub(crate) basic_blocks: RefCell<Vec<ValueId>>,
    pub(crate) attributes: RefCell<AttributeStorage>,
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
            calling_conv: RefCell::new(calling_conv),
            unnamed_addr: RefCell::new(UnnamedAddr::None),
            args: RefCell::new(Box::new([])),
            basic_blocks: RefCell::new(Vec::new()),
            attributes: RefCell::new(AttributeStorage::new()),
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
pub struct FunctionValue<'ctx, R: ReturnMarker> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    /// Cached signature type id. The value's value-arena type is the
    /// pointer-to-function on real LLVM; here we cache the function-
    /// type id directly so `signature()` is a thin lookup.
    pub(crate) signature: TypeId,
    pub(crate) _r: PhantomData<R>,
}

// Manual derives — `derive` would propagate `R: Trait` bounds that
// callers should not have to spell. The fields themselves are all
// trivially `Copy`/`Hash`/`Eq`.
impl<'ctx, R: ReturnMarker> Clone for FunctionValue<'ctx, R> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: ReturnMarker> Copy for FunctionValue<'ctx, R> {}
impl<'ctx, R: ReturnMarker> PartialEq for FunctionValue<'ctx, R> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.signature == other.signature
    }
}
impl<'ctx, R: ReturnMarker> Eq for FunctionValue<'ctx, R> {}
impl<'ctx, R: ReturnMarker> core::hash::Hash for FunctionValue<'ctx, R> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.signature.hash(h);
    }
}
impl<'ctx, R: ReturnMarker> core::fmt::Debug for FunctionValue<'ctx, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FunctionValue")
            .field("id", &self.id)
            .field("signature", &self.signature)
            .finish()
    }
}

impl<'ctx, R: ReturnMarker> FunctionValue<'ctx, R> {
    /// Construct from raw parts. Crate-internal: only the
    /// function-creation paths hand these out, after they've
    /// validated that the signature's return type matches `R`.
    #[inline]
    pub(crate) fn from_parts_unchecked(id: ValueId, module: &'ctx Module<'ctx>) -> Self {
        let data = module.context().value_data(id);
        let signature = match &data.kind {
            ValueKindData::Function(f) => f.signature,
            _ => unreachable!("FunctionValue handle invariant: kind is Function"),
        };
        Self {
            id,
            module: ModuleRef::new(module),
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
    pub fn as_value(self) -> Value<'ctx> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.signature,
        }
    }

    /// Erase the return-shape marker, producing a runtime-checked
    /// [`Dyn`] handle.
    #[inline]
    pub fn as_dyn(self) -> FunctionValue<'ctx, Dyn> {
        FunctionValue {
            id: self.id,
            module: self.module,
            signature: self.signature,
            _r: PhantomData,
        }
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
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    /// Function signature.
    #[inline]
    pub fn signature(self) -> FunctionType<'ctx> {
        FunctionType::new(self.signature, self.module.module())
    }

    /// Function return type.
    #[inline]
    pub fn return_type(self) -> Type<'ctx> {
        self.signature().return_type()
    }

    /// Linkage of this function.
    #[inline]
    pub fn linkage(self) -> Linkage {
        *self.data().linkage.borrow()
    }

    /// Update linkage.
    #[inline]
    pub fn set_linkage(self, linkage: Linkage) {
        *self.data().linkage.borrow_mut() = linkage;
    }

    /// Calling convention.
    #[inline]
    pub fn calling_conv(self) -> CallingConv {
        *self.data().calling_conv.borrow()
    }

    /// Update calling convention.
    #[inline]
    pub fn set_calling_conv(self, cc: CallingConv) {
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
    pub fn set_unnamed_addr(self, value: UnnamedAddr) {
        *self.data().unnamed_addr.borrow_mut() = value;
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
    pub fn append_basic_block(self, name: impl Into<String>) -> BasicBlock<'ctx, R> {
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

    /// Entry block, or `None` for a function with no body.
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

impl<'ctx, R: ReturnMarker> sealed::Sealed for FunctionValue<'ctx, R> {}
impl<'ctx, R: ReturnMarker> IsValue<'ctx> for FunctionValue<'ctx, R> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        FunctionValue::as_value(self)
    }
}
impl<'ctx, R: ReturnMarker> Typed<'ctx> for FunctionValue<'ctx, R> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Type::new(self.signature, self.module.module())
    }
}
impl<'ctx, R: ReturnMarker> HasName<'ctx> for FunctionValue<'ctx, R> {
    #[inline]
    fn name(self) -> Option<String> {
        Some(FunctionValue::name(self).to_owned())
    }
    #[inline]
    fn set_name(self, _name: Option<&str>) {
        // Renaming a function in place is its own diff: the symbol
        // table needs updating, and external linkers care. Phase D
        // adds the proper path; today this is a no-op.
    }
}
impl<R: ReturnMarker> HasDebugLoc for FunctionValue<'_, R> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx, R: ReturnMarker> From<FunctionValue<'ctx, R>> for Value<'ctx> {
    #[inline]
    fn from(f: FunctionValue<'ctx, R>) -> Self {
        f.as_value()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for FunctionValue<'ctx, Dyn> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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
    module: &'ctx Module<'ctx>,
    name: String,
    signature: FunctionType<'ctx>,
    linkage: Linkage,
    calling_conv: CallingConv,
    unnamed_addr: UnnamedAddr,
    attributes: AttributeStorage,
    /// Pending `(slot, name)` pairs to apply after the function value
    /// exists. Slots out of range error at `build()` time.
    param_names: Vec<(u32, String)>,
    _r: PhantomData<R>,
}

impl<'ctx, R: ReturnMarker> FunctionBuilder<'ctx, R> {
    /// Crate-internal constructor; users start through
    /// [`Module::function_builder`].
    pub(crate) fn new(
        module: &'ctx Module<'ctx>,
        name: impl Into<String>,
        signature: FunctionType<'ctx>,
    ) -> Self {
        Self {
            module,
            name: name.into(),
            signature,
            linkage: Linkage::default(),
            calling_conv: CallingConv::default(),
            unnamed_addr: UnnamedAddr::None,
            attributes: AttributeStorage::new(),
            param_names: Vec::new(),
            _r: PhantomData,
        }
    }

    /// Override the linkage.
    pub fn linkage(mut self, linkage: Linkage) -> Self {
        self.linkage = linkage;
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

    /// Add an attribute at `index`.
    pub fn attribute(mut self, index: AttrIndex, attr: crate::Attribute<'ctx>) -> Self {
        self.attributes.add(index, attr);
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
        f.set_calling_conv(self.calling_conv);
        f.set_unnamed_addr(self.unnamed_addr);
        // Move the accumulated attribute set into the function.
        *f.data().attributes.borrow_mut() = self.attributes;
        // Apply parameter names.
        for (slot, name) in self.param_names {
            let arg = f.param(slot)?;
            arg.set_name(Some(&name));
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

impl<'ctx, W: IntWidth + ReturnMarker> FunctionValue<'ctx, W> {
    /// Return type as an integer-typed handle. Mirrors the
    /// `Function::getReturnType()` round-trip on a typed function.
    #[inline]
    pub fn return_int_type(self) -> crate::derived_types::IntType<'ctx, W> {
        let signature = self.signature();
        crate::derived_types::IntType::new(signature.return_type().id(), self.module.module())
    }
}

impl<'ctx, K: FloatKind + ReturnMarker> FunctionValue<'ctx, K> {
    /// Return type as a kind-typed float handle.
    #[inline]
    pub fn return_float_type(self) -> crate::derived_types::FloatType<'ctx, K> {
        let signature = self.signature();
        crate::derived_types::FloatType::new(signature.return_type().id(), self.module.module())
    }
}

impl<'ctx, R: ReturnMarker> core::fmt::Display for FunctionValue<'ctx, R> {
    /// Print the function definition as textual `.ll`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_function(f, self.as_dyn())
    }
}
