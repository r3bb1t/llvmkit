//! On-the-fly SSA construction on top of the typed [`crate::IRBuilder`].
//!
//! Ports the "simple and efficient" algorithm from Braun, Buchwald,
//! Hack, Leißa, Mehofer, Kempf, "Simple and Efficient Construction of
//! Static Single Assignment Form" (CC 2013): callers `declare_*` a
//! typed variable, `write`/`read` it per block as if it were mutable
//! local storage, and the builder inserts the minimal set of phi nodes
//! -- including "incomplete" phis for not-yet-sealed blocks and
//! trivial-phi elimination -- without a separate dominance-frontier
//! pass. The nearest Rust prior art is `cranelift-frontend`'s
//! `FunctionBuilder` (`Variable` + `declare_var`/`def_var`/`use_var`
//! over `SsaBuilder`'s `ssa::SSABuilder`); the nearest LLVM analogue is
//! `llvm/lib/Transforms/Utils/SSAUpdater.cpp`, which solves the same
//! problem incrementally for a single value at a time during
//! transformation passes rather than during initial construction.
//!
//! The public def/use/terminator-building surface lands in a later
//! session; this module ships the typed variable/block vocabulary,
//! `SsaState`, construction, `create_block`/`declare_*`/`seal_block`,
//! and the private Braun engine (`write_variable`, `read_variable_in`,
//! `add_phi_operands`, `try_remove_trivial_phi`, `emit_operandless_phi`,
//! `resolve`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use super::basic_block::{BasicBlock, BasicBlockLabel};
use super::block_state::Unterminated;
use super::float_kind::{FloatKind, StaticFloatKind};
use super::function::FunctionValue;
use super::instruction::{Instruction, state::Attached};
use super::int_width::{IntWidth, StaticIntWidth};
use super::ir_builder::constant_folder::ConstantFolder;
use super::ir_builder::folder::IRBuilderFolder;
use super::ir_builder::{BuilderPositionState, Positioned, Unpositioned};
use super::marker::{Dyn, ReturnMarker};
use super::module::{Brand, Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::r#type::TypeId;
use super::value::{Value, ValueId};
use super::{FloatType, IntType, IrError, IrResult, PointerType};

// --------------------------------------------------------------------------
// Ids, typed variables, block handle
// --------------------------------------------------------------------------

/// Per-module monotonic id for an [`SsaBuilder`]; foreign-variable /
/// foreign-block use is a typed runtime error (a generative per-builder
/// brand was rejected: it would force nested closures per function body).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SsaBuilderId(u32);

/// Typed SSA variable of integer width `W`. Cranelift analogue:
/// `cranelift_frontend::Variable`, specialised per category per llvmkit
/// convention (cf. `PhiInst` / `FpPhiInst` / `PointerPhiInst`).
pub struct IntVariable<'ctx, W: IntWidth, B: ModuleBrand = Brand<'ctx>> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeId,
    module: ModuleRef<'ctx, B>,
    _w: core::marker::PhantomData<fn() -> W>,
}

impl<'ctx, W: IntWidth, B: ModuleBrand> Clone for IntVariable<'ctx, W, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> Copy for IntVariable<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand> core::fmt::Debug for IntVariable<'ctx, W, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IntVariable")
            .field("index", &self.index)
            .field("owner", &self.owner)
            .finish()
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> PartialEq for IntVariable<'ctx, W, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.owner == other.owner && self.ty == other.ty
    }
}
impl<'ctx, W: IntWidth, B: ModuleBrand> Eq for IntVariable<'ctx, W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand> core::hash::Hash for IntVariable<'ctx, W, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.index.hash(h);
        self.owner.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IntVariable<'ctx, W, B> {
    /// The [`SsaBuilder`] that declared this variable. Foreign use (a
    /// handle from a different builder) is a typed runtime error at the
    /// def/use call sites.
    #[inline]
    pub fn owner(&self) -> SsaBuilderId {
        self.owner
    }

    /// Owning module reference.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }
}

/// Typed SSA variable of float kind `K`.
pub struct FloatVariable<'ctx, K: FloatKind, B: ModuleBrand = Brand<'ctx>> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeId,
    module: ModuleRef<'ctx, B>,
    _k: core::marker::PhantomData<fn() -> K>,
}

impl<'ctx, K: FloatKind, B: ModuleBrand> Clone for FloatVariable<'ctx, K, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> Copy for FloatVariable<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand> core::fmt::Debug for FloatVariable<'ctx, K, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FloatVariable")
            .field("index", &self.index)
            .field("owner", &self.owner)
            .finish()
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> PartialEq for FloatVariable<'ctx, K, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.owner == other.owner && self.ty == other.ty
    }
}
impl<'ctx, K: FloatKind, B: ModuleBrand> Eq for FloatVariable<'ctx, K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand> core::hash::Hash for FloatVariable<'ctx, K, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.index.hash(h);
        self.owner.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> FloatVariable<'ctx, K, B> {
    /// The [`SsaBuilder`] that declared this variable.
    #[inline]
    pub fn owner(&self) -> SsaBuilderId {
        self.owner
    }

    /// Owning module reference.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }
}

/// Typed SSA variable of pointer category (any address space).
pub struct PointerVariable<'ctx, B: ModuleBrand = Brand<'ctx>> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeId,
    module: ModuleRef<'ctx, B>,
}

impl<'ctx, B: ModuleBrand> Clone for PointerVariable<'ctx, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, B: ModuleBrand> Copy for PointerVariable<'ctx, B> {}
impl<'ctx, B: ModuleBrand> core::fmt::Debug for PointerVariable<'ctx, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PointerVariable")
            .field("index", &self.index)
            .field("owner", &self.owner)
            .finish()
    }
}
impl<'ctx, B: ModuleBrand> PartialEq for PointerVariable<'ctx, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.owner == other.owner && self.ty == other.ty
    }
}
impl<'ctx, B: ModuleBrand> Eq for PointerVariable<'ctx, B> {}
impl<'ctx, B: ModuleBrand> core::hash::Hash for PointerVariable<'ctx, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.index.hash(h);
        self.owner.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> PointerVariable<'ctx, B> {
    /// The [`SsaBuilder`] that declared this variable.
    #[inline]
    pub fn owner(&self) -> SsaBuilderId {
        self.owner
    }

    /// Owning module reference.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }
}

/// Copyable reference to a block managed by an [`SsaBuilder`]. NOT an
/// insertion capability -- the linear `BasicBlock` handles stay inside the
/// `SsaBuilder`; this implements [`crate::IntoBasicBlockLabel`] as the
/// escape hatch for feeding a `br`/successor built through the plain
/// [`IRBuilder`] surface elsewhere.
///
/// [`IRBuilder`]: crate::IRBuilder
pub struct SsaBlock<'ctx, R: ReturnMarker, B: ModuleBrand = Brand<'ctx>> {
    label: BasicBlockLabel<'ctx, R, B>,
    owner: SsaBuilderId,
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand> Clone for SsaBlock<'ctx, R, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> Copy for SsaBlock<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand> core::fmt::Debug for SsaBlock<'ctx, R, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SsaBlock")
            .field("label", &self.label)
            .field("owner", &self.owner)
            .finish()
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> PartialEq for SsaBlock<'ctx, R, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Compare through the erased `Value` rather than `self.label`
        // directly: `BasicBlockLabel<R, B>`'s derived `PartialEq` needs
        // `R: PartialEq`, which `ReturnMarker` does not guarantee.
        // Mirrors how `BasicBlock`'s own manual `PartialEq` (above)
        // compares `id`/`module`/`ty` instead of the phantom markers.
        self.label.as_value() == other.label.as_value() && self.owner == other.owner
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> Eq for SsaBlock<'ctx, R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> core::hash::Hash for SsaBlock<'ctx, R, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.label.as_value().hash(h);
        self.owner.hash(h);
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> SsaBlock<'ctx, R, B> {
    /// The underlying copyable block label, usable anywhere a
    /// [`crate::IntoBasicBlockLabel`] source is accepted (e.g. a plain
    /// `IRBuilder::build_br` target).
    #[inline]
    pub fn label(&self) -> BasicBlockLabel<'ctx, R, B> {
        self.label
    }
}

// `IntoBasicBlockLabel` is sealed to `basic_block.rs` (its `Sealed`
// marker trait is a private submodule there), so `SsaBlock`'s impl lives
// alongside the other implementors in that file instead of here.

/// Resolve a block label to the [`ValueId`] the Braun engine's block-keyed
/// maps use. Blocks are values (`LabelType`), so the label's own value-id
/// *is* the block id -- this mirrors how [`crate::cfg`] keys its
/// successor/predecessor maps off `block.as_value().id`.
#[inline]
fn label_value_id<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx>(
    label: &BasicBlockLabel<'ctx, R, B>,
) -> ValueId {
    label.as_value().id
}

/// Diagnostic name for a block id: falls back to a slot-style
/// placeholder when the block was never given a textual name, mirroring
/// how the AsmWriter falls back to numbered slots.
fn block_name<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleRef<'ctx, B>,
    block_id: ValueId,
) -> String {
    let label_ty = module.module().label_type().as_type().id();
    let label = BasicBlock::<Dyn, Unterminated, B>::from_parts(block_id, module, label_ty).label();
    label
        .as_value()
        .name()
        .unwrap_or_else(|| format!("<block {block_id:?}>"))
}

// --------------------------------------------------------------------------
// SsaState + SsaBuilder + constructors
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarCategory {
    Int,
    Float,
    Pointer,
}

struct VarData {
    ty: TypeId,
    category: VarCategory,
    name: String,
    poison_on_undef: bool,
}

struct SsaState<'ctx, R: ReturnMarker, B: ModuleBrand> {
    vars: Vec<VarData>,
    /// Braun `currentDef`: `(block, var) -> definition value`.
    current_def: HashMap<(ValueId, u32), ValueId>,
    /// Trivial-phi forwarding (path-compressed on read).
    resolved: RefCell<HashMap<ValueId, ValueId>>,
    /// Recorded CFG edges, duplicates preserved (phi operand order).
    preds: HashMap<ValueId, Vec<ValueId>>,
    sealed: HashSet<ValueId>,
    // NOTE: a `filled: HashSet<ValueId>` set (mirroring Braun's `filledBlocks`)
    // is deliberately NOT shipped in this task: `emit_operandless_phi`'s
    // insertion-point logic only needs a *read-only* "does this block have
    // an instruction yet" check (`BasicBlock::instructions().next()`), which
    // does not need a separate tracked set, and nothing else in this task's
    // surface (create_block/declare_*/seal_block) fills or queries fill
    // state. The terminator-building methods that land with the def/use API
    // will populate/consult it (for `IrError::SsaBlockAlreadyFilled` /
    // `SsaUnfilledBlock`, both pre-declared in error.rs); adding the field
    // now with no reader would be dead code under `-D warnings` (Task-4
    // precedent: defer until the first real caller lands).
    /// Braun `incompletePhis`: `block -> [(var index, phi value)]`.
    incomplete_phis: HashMap<ValueId, Vec<(u32, ValueId)>>,
    /// Linear insertion capabilities for not-yet-current blocks.
    open_blocks: HashMap<ValueId, BasicBlock<'ctx, R, Unterminated, B>>,
    /// Linear lifecycle handles for layer-created phis (RAUW / erase).
    created_phis: HashMap<ValueId, Instruction<'ctx, Attached, B>>,
    /// Deterministic iteration for a future `finish()`.
    block_order: Vec<ValueId>,
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand> SsaState<'ctx, R, B> {
    fn new() -> Self {
        Self {
            vars: Vec::new(),
            current_def: HashMap::new(),
            resolved: RefCell::new(HashMap::new()),
            preds: HashMap::new(),
            sealed: HashSet::new(),
            incomplete_phis: HashMap::new(),
            open_blocks: HashMap::new(),
            created_phis: HashMap::new(),
            block_order: Vec::new(),
        }
    }
}

/// Cranelift-`FunctionBuilder`-style layer on top of the typed
/// [`IRBuilder`] implementing Braun et al.'s on-the-fly SSA construction
/// (sealed blocks, incomplete phis, trivial-phi elimination). See the
/// module docs for the algorithm citation.
///
/// [`IRBuilder`]: crate::IRBuilder
pub struct SsaBuilder<'m, 'ctx, B, F = ConstantFolder, S = Unpositioned, R = Dyn>
where
    B: ModuleBrand,
    F: IRBuilderFolder<'ctx, B> + Clone,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    module: &'m Module<'ctx, B, Unverified>,
    function: FunctionValue<'ctx, R, B>,
    id: SsaBuilderId,
    folder: F,
    /// `Some` iff `S = Positioned` (mirrors the `insert_block()` `Option`
    /// precedent at `ir_builder.rs`'s `IRBuilder::insert_block`).
    inner: Option<super::ir_builder::IRBuilder<'m, 'ctx, B, F, Positioned, R>>,
    state: SsaState<'ctx, R, B>,
    _s: core::marker::PhantomData<S>,
}

impl<'m, 'ctx, B: ModuleBrand + 'ctx> SsaBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, Dyn> {
    /// Construct an `SsaBuilder` for `function` using the default
    /// [`ConstantFolder`]. Errors with [`IrError::SsaFunctionHasBlocks`]
    /// if `function` already has a body -- the layer must observe every
    /// CFG edge from birth.
    pub fn for_function<R: ReturnMarker>(
        module: &'m Module<'ctx, B, Unverified>,
        function: FunctionValue<'ctx, R, B>,
    ) -> IrResult<SsaBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, R>> {
        Self::with_folder_for_function(module, function, ConstantFolder)
    }
}

impl<'m, 'ctx, B: ModuleBrand + 'ctx, F: IRBuilderFolder<'ctx, B> + Clone>
    SsaBuilder<'m, 'ctx, B, F, Unpositioned, Dyn>
{
    /// Construct an `SsaBuilder` for `function` using a caller-supplied
    /// folder.
    pub fn with_folder_for_function<R>(
        module: &'m Module<'ctx, B, Unverified>,
        function: FunctionValue<'ctx, R, B>,
        folder: F,
    ) -> IrResult<SsaBuilder<'m, 'ctx, B, F, Unpositioned, R>>
    where
        R: ReturnMarker,
    {
        if function.entry_block().is_some() {
            return Err(IrError::SsaFunctionHasBlocks);
        }
        Ok(SsaBuilder {
            module,
            function,
            id: SsaBuilderId(module.next_ssa_builder_id()),
            folder,
            inner: None,
            state: SsaState::new(),
            _s: core::marker::PhantomData,
        })
    }
}

// --------------------------------------------------------------------------
// Any-state surface: create_block, variable declarations, seal_block
// --------------------------------------------------------------------------

impl<'m, 'ctx, B, F, S, R> SsaBuilder<'m, 'ctx, B, F, S, R>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B> + Clone,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    /// This builder's per-module id. Exposed for diagnostics /
    /// cross-checking; ordinary callers do not need to inspect it.
    #[inline]
    pub fn id(&self) -> SsaBuilderId {
        self.id
    }

    /// Append a block. The FIRST created block is the entry block and is
    /// auto-Braun-sealed: entry has no predecessors by definition
    /// (`Verifier::visitFunction`), so a later branch TO it errors with
    /// [`IrError::SsaBranchToSealedBlock`] once edge-recording lands.
    pub fn create_block<Name: Into<String>>(&mut self, name: Name) -> SsaBlock<'ctx, R, B> {
        let block = self.function.append_basic_block(self.module, name);
        let label = block.label();
        let block_id = label_value_id(&label);
        if self.state.block_order.is_empty() {
            self.state.sealed.insert(block_id);
        }
        self.state.block_order.push(block_id);
        self.state.preds.entry(block_id).or_default();
        self.state.open_blocks.insert(block_id, block);
        SsaBlock {
            label,
            owner: self.id,
        }
    }

    /// Declare a strict int variable: reading it on a def-less path is a
    /// typed error (D10).
    pub fn declare_int_var<W: StaticIntWidth, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> IntVariable<'ctx, W, B> {
        let ty = W::ir_type(self.module_ref()).as_type().id();
        self.declare_var_raw(ty, name, VarCategory::Int, false)
            .into()
    }

    /// Poison twin of [`Self::declare_int_var`]: reading on a def-less
    /// path yields `poison` (explicit opt-in, separate method per the
    /// no-bool-params rule).
    pub fn declare_int_var_poison<W: StaticIntWidth, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> IntVariable<'ctx, W, B> {
        let ty = W::ir_type(self.module_ref()).as_type().id();
        self.declare_var_raw(ty, name, VarCategory::Int, true)
            .into()
    }

    /// Runtime-width int variable. Takes the type explicitly because
    /// [`super::int_width::IntDyn`] carries no static width.
    pub fn declare_int_var_dyn<Name: Into<String>>(
        &mut self,
        ty: IntType<'ctx, super::int_width::IntDyn, B>,
        name: Name,
    ) -> IntVariable<'ctx, super::int_width::IntDyn, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Int, false)
            .into()
    }

    /// Poison twin of [`Self::declare_int_var_dyn`].
    pub fn declare_int_var_dyn_poison<Name: Into<String>>(
        &mut self,
        ty: IntType<'ctx, super::int_width::IntDyn, B>,
        name: Name,
    ) -> IntVariable<'ctx, super::int_width::IntDyn, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Int, true)
            .into()
    }

    /// Declare a strict float variable.
    pub fn declare_float_var<K: StaticFloatKind, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> FloatVariable<'ctx, K, B> {
        let ty = K::ir_type(self.module_ref()).as_type().id();
        self.declare_var_raw(ty, name, VarCategory::Float, false)
            .into()
    }

    /// Poison twin of [`Self::declare_float_var`].
    pub fn declare_float_var_poison<K: StaticFloatKind, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> FloatVariable<'ctx, K, B> {
        let ty = K::ir_type(self.module_ref()).as_type().id();
        self.declare_var_raw(ty, name, VarCategory::Float, true)
            .into()
    }

    /// Runtime-kind float variable. Takes the type explicitly because
    /// [`super::float_kind::FloatDyn`] carries no static kind.
    pub fn declare_float_var_dyn<Name: Into<String>>(
        &mut self,
        ty: FloatType<'ctx, super::float_kind::FloatDyn, B>,
        name: Name,
    ) -> FloatVariable<'ctx, super::float_kind::FloatDyn, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Float, false)
            .into()
    }

    /// Poison twin of [`Self::declare_float_var_dyn`].
    pub fn declare_float_var_dyn_poison<Name: Into<String>>(
        &mut self,
        ty: FloatType<'ctx, super::float_kind::FloatDyn, B>,
        name: Name,
    ) -> FloatVariable<'ctx, super::float_kind::FloatDyn, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Float, true)
            .into()
    }

    /// Declare a strict pointer variable in the default address space
    /// (addrspace 0).
    pub fn declare_pointer_var<Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> PointerVariable<'ctx, B> {
        let ty = self.module.ptr_type(0).as_type().id();
        self.declare_var_raw(ty, name, VarCategory::Pointer, false)
            .into()
    }

    /// Poison twin of [`Self::declare_pointer_var`].
    pub fn declare_pointer_var_poison<Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> PointerVariable<'ctx, B> {
        let ty = self.module.ptr_type(0).as_type().id();
        self.declare_var_raw(ty, name, VarCategory::Pointer, true)
            .into()
    }

    /// Declare a strict pointer variable in a caller-specified address
    /// space.
    pub fn declare_pointer_var_in_addrspace<Name: Into<String>>(
        &mut self,
        ty: PointerType<'ctx, B>,
        name: Name,
    ) -> PointerVariable<'ctx, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Pointer, false)
            .into()
    }

    /// Poison twin of [`Self::declare_pointer_var_in_addrspace`].
    pub fn declare_pointer_var_in_addrspace_poison<Name: Into<String>>(
        &mut self,
        ty: PointerType<'ctx, B>,
        name: Name,
    ) -> PointerVariable<'ctx, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Pointer, true)
            .into()
    }

    /// Shared declare-slot helper: pushes a `VarData` and returns a
    /// same-shaped [`VarHandle`]. Each public `declare_*` method above
    /// narrows the handle into its own phantom shape via `Into`, since
    /// `IntVariable`/`FloatVariable`/`PointerVariable` have different
    /// phantom fields (the pointer variant has none).
    fn declare_var_raw<Name: Into<String>>(
        &mut self,
        ty: TypeId,
        name: Name,
        category: VarCategory,
        poison_on_undef: bool,
    ) -> VarHandle<'ctx, B> {
        let index = u32::try_from(self.state.vars.len())
            .unwrap_or_else(|_| unreachable!("more than u32::MAX SSA variables declared"));
        self.state.vars.push(VarData {
            ty,
            category,
            name: name.into(),
            poison_on_undef,
        });
        VarHandle {
            index,
            owner: self.id,
            ty,
            module: self.module_ref(),
        }
    }

    #[inline]
    fn module_ref(&self) -> ModuleRef<'ctx, B> {
        ModuleRef::new(self.module.core_ref())
    }

    fn check_owner_block(&self, block: &SsaBlock<'ctx, R, B>) -> IrResult<()> {
        if block.owner != self.id {
            return Err(IrError::SsaForeignBlock);
        }
        Ok(())
    }

    // NOTE: a `check_owner_var` sibling to `check_owner_block` (returning
    // `IrError::SsaForeignVariable` for a variable handle from a different
    // `SsaBuilder`) is deliberately NOT shipped in this task: nothing in
    // this task's surface (create_block/declare_*/seal_block) ever reads
    // or writes a declared variable, so the check has no caller yet. The
    // def/use methods landing with Task 18 (`read_var`/`write_var` or
    // equivalent) are the first real call sites; per the Task-4 precedent,
    // an unused private check is deferred to its first caller rather than
    // shipped dead under `-D warnings`.

    /// Braun `sealBlock`: the predecessor set is complete; complete this
    /// block's incomplete phis.
    pub fn seal_block(&mut self, block: SsaBlock<'ctx, R, B>) -> IrResult<()> {
        self.check_owner_block(&block)?;
        let block_id = label_value_id(&block.label);
        if self.state.sealed.contains(&block_id) {
            return Err(IrError::SsaBlockAlreadySealed {
                block: block_name(self.module_ref(), block_id),
            });
        }
        let pending = self
            .state
            .incomplete_phis
            .remove(&block_id)
            .unwrap_or_default();
        self.state.sealed.insert(block_id);
        for (var, phi_id) in pending {
            self.add_phi_operands(var, phi_id, block_id)?;
        }
        Ok(())
    }
}

/// Shared field layout produced by [`SsaBuilder::declare_var_raw`]; each
/// public `IntVariable`/`FloatVariable`/`PointerVariable` constructor
/// below narrows this into its own phantom shape.
struct VarHandle<'ctx, B: ModuleBrand> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeId,
    module: ModuleRef<'ctx, B>,
}

impl<'ctx, B: ModuleBrand> From<VarHandle<'ctx, B>> for PointerVariable<'ctx, B> {
    #[inline]
    fn from(h: VarHandle<'ctx, B>) -> Self {
        PointerVariable {
            index: h.index,
            owner: h.owner,
            ty: h.ty,
            module: h.module,
        }
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand> From<VarHandle<'ctx, B>> for IntVariable<'ctx, W, B> {
    #[inline]
    fn from(h: VarHandle<'ctx, B>) -> Self {
        IntVariable {
            index: h.index,
            owner: h.owner,
            ty: h.ty,
            module: h.module,
            _w: core::marker::PhantomData,
        }
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand> From<VarHandle<'ctx, B>> for FloatVariable<'ctx, K, B> {
    #[inline]
    fn from(h: VarHandle<'ctx, B>) -> Self {
        FloatVariable {
            index: h.index,
            owner: h.owner,
            ty: h.ty,
            module: h.module,
            _k: core::marker::PhantomData,
        }
    }
}

/// Emit a category-dispatched, name-only, operandless phi through
/// whichever positioned builder `emit_operandless_phi` has prepared for
/// the target insertion point, returning the raw [`ValueId`] of the new
/// phi instruction. `ty` is the declared variable's cached [`TypeId`];
/// `module` resolves it back to the category-appropriate typed handle
/// each dyn phi builder expects.
fn build_typed_phi<'m, 'ctx, B, F, R>(
    builder: &super::ir_builder::IRBuilder<'m, 'ctx, B, F, Positioned, R>,
    category: VarCategory,
    ty: TypeId,
    module: ModuleRef<'ctx, B>,
    name: &str,
) -> IrResult<ValueId>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    let id = match category {
        VarCategory::Int => {
            let int_ty = IntType::<super::int_width::IntDyn, B>::new(ty, module);
            builder.build_int_phi_dyn(int_ty, name)?.as_value().id
        }
        VarCategory::Float => {
            let float_ty = FloatType::<super::float_kind::FloatDyn, B>::new(ty, module);
            builder.build_fp_phi_dyn(float_ty, name)?.as_value().id
        }
        VarCategory::Pointer => {
            let ptr_ty = PointerType::<B>::new(ty, module);
            builder
                .build_pointer_phi_in_addrspace(ptr_ty, name)?
                .as_value()
                .id
        }
    };
    Ok(id)
}

// --------------------------------------------------------------------------
// The Braun engine (private)
// --------------------------------------------------------------------------
//
// Faithful port of the paper's four procedures (`writeVariable`,
// `readVariable`/`readVariableRecursive`, `addPhiOperands`,
// `tryRemoveTrivialPhi`), plus the head-insertion helper
// (`emit_operandless_phi`) and the trivial-phi forwarding lookup
// (`resolve`) that the paper describes as replacing every use of the
// removed phi with the value it forwarded to.
//
// Recursion shape: `read_variable_in` chases a chain of single-predecessor
// blocks *iteratively* (the `loop { ... block = preds[0] ... }` below), so
// that path never grows the Rust call stack. The two procedures that *do*
// recurse -- `add_phi_operands` (via `read_variable_in` on a multi-pred
// block) and `try_remove_trivial_phi` (into other layer-created phis whose
// operand list became trivial as a side effect) -- are bounded by
// construction: each phi is created at most once per (block, var) pair
// (`write_variable` immediately records the fresh phi as the block's
// current definition, breaking cycles per the paper's "mark" step), and
// `try_remove_trivial_phi` only re-examines phis already present in
// `created_phis`, a strictly shrinking set (each successful removal pops
// its entry before recursing into its users). So recursion depth is
// bounded by the number of blocks in the function, which is itself
// bounded by available memory -- there is no pathological input that
// grows this past a reasonable native stack.
impl<'m, 'ctx, B, F, S, R> SsaBuilder<'m, 'ctx, B, F, S, R>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B> + Clone,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    /// Braun `writeVariable`.
    fn write_variable(&mut self, var: u32, block: ValueId, value: ValueId) {
        self.state.current_def.insert((block, var), value);
    }

    /// Braun `readVariable` + `readVariableRecursive`, restated
    /// iteratively for the single-predecessor chase.
    fn read_variable_in(&mut self, var: u32, mut block: ValueId) -> IrResult<ValueId> {
        loop {
            if let Some(v) = self.state.current_def.get(&(block, var)) {
                return Ok(self.resolve(*v));
            }
            if !self.state.sealed.contains(&block) {
                // Incomplete CFG: operandless phi at the head, completed
                // once the block is sealed (see `Self::seal_block`).
                let phi = self.emit_operandless_phi(var, block)?;
                self.state
                    .incomplete_phis
                    .entry(block)
                    .or_default()
                    .push((var, phi));
                self.write_variable(var, block, phi);
                return Ok(phi);
            }
            let preds = self.state.preds.get(&block).cloned().unwrap_or_default();
            match preds.len() {
                0 => return self.undefined_read(var, block),
                1 => block = preds[0], // single-pred chase: no phi needed
                _ => {
                    let phi = self.emit_operandless_phi(var, block)?;
                    self.write_variable(var, block, phi); // breaks cycles
                    return self.add_phi_operands(var, phi, block);
                }
            }
        }
    }

    /// Braun `addPhiOperands` + `tryRemoveTrivialPhi`.
    fn add_phi_operands(&mut self, var: u32, phi: ValueId, block: ValueId) -> IrResult<ValueId> {
        let preds = self.state.preds.get(&block).cloned().unwrap_or_default();
        for pred in preds {
            let operand = self.read_variable_in(var, pred)?;
            self.phi_add_incoming_raw(phi, operand, pred)?;
        }
        self.try_remove_trivial_phi(phi)
    }

    /// Braun `tryRemoveTrivialPhi`: a phi merging exactly one distinct
    /// value (ignoring self-references) is redundant. Replace every use
    /// with that value and erase the phi, then re-check any layer-created
    /// phi that used to reference it (removing this phi as an operand can
    /// make one of *those* trivial too).
    fn try_remove_trivial_phi(&mut self, phi: ValueId) -> IrResult<ValueId> {
        let mut same: Option<ValueId> = None;
        for op in self.phi_incoming_values(phi) {
            let op = self.resolve(op);
            if op == phi || Some(op) == same {
                continue;
            }
            if same.is_some() {
                // Merges >= 2 distinct values: not trivial.
                return Ok(phi);
            }
            same = Some(op);
        }
        let same = match same {
            Some(v) => v,
            None => return self.undefined_phi_replacement(phi),
        };
        // Snapshot users BEFORE mutating (RAUW/erase invalidate the live
        // use-list); only recurse into phis this layer created and still
        // tracks -- a user that isn't in `created_phis` is either a
        // non-phi instruction (nothing to re-check) or a phi some earlier
        // step already resolved away.
        let users: Vec<ValueId> = self.phi_user_ids(phi);
        let handle = self.state.created_phis.remove(&phi).unwrap_or_else(|| {
            unreachable!(
                "SsaBuilder invariant: every ValueId reachable through try_remove_trivial_phi \
                 was produced by Self::emit_operandless_phi, which always records its handle in \
                 created_phis before returning"
            )
        });
        let module = self.module_ref();
        let same_ty = module.value_data(same).ty;
        let replacement = Value::from_parts(same, module, same_ty);
        // `replace_all_uses_with`'s only failure mode is a type mismatch
        // between the phi's cached result type and `replacement`'s type
        // (instruction.rs). `same` is one of this very phi's own incoming
        // operands (the loop above only ever assigns `same` from
        // `self.phi_incoming_values(phi)`). IMPORTANT: unlike the typed
        // `PhiInst::add_incoming`, the dyn path this engine uses
        // (`phi_add_incoming_raw` -> `IRBuilder::phi_add_incoming_from_value`,
        // ir_builder.rs) performs NO type check of its own -- its own doc
        // comment defers "value-type ... coherence" to `Module::verify`.
        // The real guarantee here is narrower and currently-true-by-
        // construction rather than checked at the phi-mutation call site:
        // every operand this engine has EVER pushed onto a layer-created
        // phi's incoming list is either (a) another layer-created phi's
        // own id (`emit_operandless_phi` always builds it from the same
        // declared variable's `VarData.ty`), or (b) `undefined_read` /
        // `undefined_phi_replacement`'s poison value (built from that same
        // `VarData.ty`), or (c) a value passed to `write_variable` -- and
        // this task ships no public write path, so every current
        // `write_variable` call site (inside this same file) only ever
        // writes a value already known same-typed. Task 18's def/use API,
        // the first real external `write_variable` caller, MUST validate
        // the written value's type against the variable's declared type
        // before calling in (mirroring `PhiInst::add_incoming`'s own
        // check), or this `unreachable!` becomes reachable.
        handle
            .replace_all_uses_with(self.module, replacement)
            .unwrap_or_else(|_| {
                unreachable!(
                    "SsaBuilder invariant: every value this engine ever writes into a variable's \
                     current_def (and therefore ever feeds into a phi's incoming list) is typed \
                     to that variable's declared VarData.ty -- see the long-form justification \
                     immediately above this call"
                )
            });
        // `replace_all_uses_with` does not erase (see its doc comment in
        // instruction.rs); rediscover a fresh handle over the now-unused
        // phi and remove it from the block.
        Instruction::<Attached, B>::from_parts(phi, module).erase_from_parent(self.module);
        self.state.resolved.borrow_mut().insert(phi, same);
        for user in users {
            if self.state.created_phis.contains_key(&user) {
                self.try_remove_trivial_phi(user)?;
            }
        }
        Ok(self.resolve(same))
    }

    /// Path-compressed forwarding lookup: chase the `resolved` chain built
    /// by [`Self::try_remove_trivial_phi`] and flatten it so future
    /// lookups are O(1).
    fn resolve(&self, mut v: ValueId) -> ValueId {
        let mut chain = Vec::new();
        loop {
            let next = self.state.resolved.borrow().get(&v).copied();
            match next {
                Some(next) => {
                    chain.push(v);
                    v = next;
                }
                None => break,
            }
        }
        if !chain.is_empty() {
            let mut resolved = self.state.resolved.borrow_mut();
            for id in chain {
                resolved.insert(id, v);
            }
        }
        v
    }

    /// Emit an operandless phi at the head of `block`. Every phi this
    /// engine ever creates lands at position 0 (Braun's algorithm never
    /// grows a phi list after the fact except via `add_incoming`, so
    /// "the head" is always exactly "before the block's current first
    /// instruction" -- this collapses to two cases, keyed on emptiness
    /// rather than on which of `self.inner` / `open_blocks` currently
    /// owns the block's linear handle:
    ///
    /// - `block` has >= 1 instruction already (whether it is open,
    ///   current, or filled/terminated): a fresh throwaway builder
    ///   positioned via `position_before(&first_instruction)` derives
    ///   its own insertion block from the anchor's parent, so no linear
    ///   `BasicBlock` handle is needed at all here.
    /// - `block` is empty: `position_before` has no anchor to derive
    ///   from, so head-insertion needs an actual end-of-block position,
    ///   which requires the block's linear `Unterminated` handle. If
    ///   `block` is the live positioned block, `self.inner`'s own
    ///   append (`&self`-based phi builders, `insert_before: None`) IS
    ///   that position. Otherwise the handle is borrowed out of
    ///   `open_blocks`, used, and stored back.
    fn emit_operandless_phi(&mut self, var: u32, block: ValueId) -> IrResult<ValueId> {
        let idx = usize::try_from(var).unwrap_or_else(|_| {
            unreachable!("SsaBuilder invariant: var indices are u32::try_from(vars.len())")
        });
        let var_ty = self.state.vars[idx].ty;
        let var_category = self.state.vars[idx].category;
        let var_name = self.state.vars[idx].name.clone();
        let module = self.module_ref();
        let label_ty = module.module().label_type().as_type().id();

        // Read-only peek at the block's current first instruction,
        // independent of which state (open/current/filled) it is in --
        // `BasicBlock::instructions()` only needs `&self`, and
        // reconstructing a view via `from_parts` does not disturb
        // whatever linear handle (if any) is live elsewhere.
        let dyn_block = BasicBlock::<Dyn, super::block_state::Terminated, B>::from_parts(
            block, module, label_ty,
        );
        let first = dyn_block.instructions().next();

        let inst = if let Some(anchor) = first {
            // Non-empty: derive the insertion block from the anchor, no
            // linear handle required. Pinned to `Dyn` -- this throwaway
            // builder never emits a terminator, so the return-marker
            // parameter carries no real invariant here.
            let builder: super::ir_builder::IRBuilder<'_, 'ctx, B, F, Positioned, Dyn> =
                super::ir_builder::IRBuilder::with_folder(self.module, self.folder.clone())
                    .position_before(&anchor);
            build_typed_phi(&builder, var_category, var_ty, module, &var_name)?
        } else if let Some(current) = self
            .inner
            .as_ref()
            .filter(|b| b.insert_block().as_value().id == block)
        {
            // Empty and currently positioned: the phi builders take
            // `&self`, so appending through the live builder directly
            // IS head-insertion (no repositioning round-trip needed).
            build_typed_phi(current, var_category, var_ty, module, &var_name)?
        } else {
            // Empty and not current: borrow the linear handle out of
            // `open_blocks`, position at its end (which is also its
            // head, since it is empty), emit, and store the handle back.
            let open = self.state.open_blocks.remove(&block).unwrap_or_else(|| {
                unreachable!(
                    "SsaBuilder invariant: every block id ever passed to emit_operandless_phi \
                     came from a recorded CFG edge or entry, so it is either the live positioned \
                     block, still open, or already filled (handled by the non-empty branch above)"
                )
            });
            let positioned =
                super::ir_builder::IRBuilder::with_folder(self.module, self.folder.clone())
                    .position_at_end(open);
            let inst = build_typed_phi(&positioned, var_category, var_ty, module, &var_name)?;
            self.state
                .open_blocks
                .insert(block, positioned.into_insert_block());
            inst
        };
        self.state
            .created_phis
            .insert(inst, Instruction::<Attached, B>::from_parts(inst, module));
        Ok(inst)
    }

    /// Add `(operand, pred)` to the layer-created phi named by `phi`.
    /// Thin wrapper over the same dyn phi-mutation idiom
    /// `IRBuilder::phi_add_incoming_from_value` uses, since the engine
    /// only ever holds category-erased `ValueId`s. Pinned to `Dyn`: the
    /// return-marker parameter is irrelevant to a payload-only mutation
    /// that never emits a terminator.
    fn phi_add_incoming_raw(&self, phi: ValueId, operand: ValueId, pred: ValueId) -> IrResult<()> {
        let module = self.module_ref();
        let phi_value = Value::from_parts(phi, module, module.value_data(phi).ty);
        let operand_value = Value::from_parts(operand, module, module.value_data(operand).ty);
        let label_ty = module.module().label_type().as_type().id();
        let pred_block = BasicBlock::<Dyn, Unterminated, B>::from_parts(pred, module, label_ty);
        let ib: super::ir_builder::IRBuilder<'_, 'ctx, B, F, super::ir_builder::Unpositioned, Dyn> =
            super::ir_builder::IRBuilder::with_folder(self.module, self.folder.clone());
        ib.phi_add_incoming_from_value(phi_value, operand_value, pred_block)
    }

    /// Read the current incoming-value list of a layer-created phi,
    /// resolved through the same value-arena path `PhiInst::payload`
    /// uses (category-agnostic: works for the int/float/pointer phi
    /// handles alike, since they all share `InstructionKindData::Phi`).
    fn phi_incoming_values(&self, phi: ValueId) -> Vec<ValueId> {
        let module = self.module_ref();
        match &module.value_data(phi).kind {
            super::value::ValueKindData::Instruction(i) => match &i.kind {
                super::instruction::InstructionKindData::Phi(p) => {
                    p.incoming.borrow().iter().map(|(v, _)| v.get()).collect()
                }
                _ => {
                    unreachable!("SsaBuilder invariant: created_phis only stores phi instructions")
                }
            },
            _ => unreachable!("SsaBuilder invariant: created_phis only stores instruction values"),
        }
    }

    /// Structural users of `phi` restricted to other instructions (the
    /// only category the trivial-phi recursion cares about).
    fn phi_user_ids(&self, phi: ValueId) -> Vec<ValueId> {
        let module = self.module_ref();
        let value = Value::from_parts(phi, module, module.value_data(phi).ty);
        value.users().map(|u| u.as_value().id).collect()
    }

    /// A strict variable's read reached function entry with no write on
    /// the path: `Err(SsaUseOfUndefinedVariable)`. A poison-on-undef
    /// variable instead materialises `poison <ty>` for that read.
    fn undefined_read(&mut self, var: u32, block: ValueId) -> IrResult<ValueId> {
        let idx = usize::try_from(var).unwrap_or_else(|_| {
            unreachable!("SsaBuilder invariant: var indices are u32::try_from(vars.len())")
        });
        let data = &self.state.vars[idx];
        if data.poison_on_undef {
            let module = self.module_ref();
            let ty = super::r#type::Type::new(data.ty, module);
            let poison = ty.get_poison();
            return Ok(poison.as_value().id);
        }
        Err(IrError::SsaUseOfUndefinedVariable {
            variable: data.name.clone(),
            block: block_name(self.module_ref(), block),
        })
    }

    /// A phi with no non-self incoming operand at all (only reachable for
    /// an unreachable block, i.e. one whose only predecessors are
    /// themselves unreachable): same strict-vs-poison split as
    /// [`Self::undefined_read`], keyed by the phi's originating variable.
    fn undefined_phi_replacement(&mut self, phi: ValueId) -> IrResult<ValueId> {
        let module = self.module_ref();
        let ty = module.value_data(phi).ty;
        let block_id = self
            .state
            .created_phis
            .get(&phi)
            .map(|h| h.parent().as_value().id)
            .unwrap_or_else(|| {
                unreachable!(
                    "SsaBuilder invariant: try_remove_trivial_phi only calls this helper on a \
                     phi still present in created_phis"
                )
            });
        // Recover which declared variable this phi belongs to by matching
        // its cached type against the declared variable table. Ambiguous
        // only if two variables share both type and poison policy, in
        // which case either is a faithful diagnostic source.
        let var = self
            .state
            .vars
            .iter()
            .find(|v| v.ty == ty)
            .unwrap_or_else(|| {
                unreachable!(
                    "SsaBuilder invariant: every layer-created phi's type was taken from a \
                     declared VarData.ty in Self::emit_operandless_phi"
                )
            });
        if var.poison_on_undef {
            let poison_ty = super::r#type::Type::new(ty, module);
            let poison = poison_ty.get_poison();
            self.state.created_phis.remove(&phi);
            Instruction::<Attached, B>::from_parts(phi, module).erase_from_parent(self.module);
            let resolved = poison.as_value().id;
            self.state.resolved.borrow_mut().insert(phi, resolved);
            return Ok(resolved);
        }
        Err(IrError::SsaUseOfUndefinedVariable {
            variable: var.name.clone(),
            block: block_name(module, block_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Linkage, Type};

    /// llvmkit-specific: no upstream C++ equivalent (LLVM's `IRBuilder`
    /// has no on-the-fly SSA layer -- the closest functional relative is
    /// `SSAUpdater::Initialize`, which likewise treats the first block it
    /// sees as needing no predecessor completion). Locks that
    /// `create_block`'s FIRST call auto-seals the entry block, matching
    /// `Verifier::visitFunction`'s invariant that the entry block has no
    /// predecessors.
    #[test]
    fn first_created_block_is_auto_sealed() -> Result<(), IrError> {
        Module::with_new("ssa-entry-seal", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let mut b = SsaBuilder::for_function(&m, f)?;
            let entry = b.create_block("entry");
            let entry_id = label_value_id(&entry.label);
            assert!(b.state.sealed.contains(&entry_id));

            // A second block is NOT auto-sealed.
            let second = b.create_block("second");
            let second_id = label_value_id(&second.label);
            assert!(!b.state.sealed.contains(&second_id));
            Ok(())
        })
    }

    /// llvmkit-specific: locks `seal_block`'s double-seal rejection
    /// (Braun's algorithm assumes each block is sealed exactly once,
    /// after which its predecessor set is considered final).
    #[test]
    fn seal_block_twice_errors() -> Result<(), IrError> {
        Module::with_new("ssa-double-seal", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let mut b = SsaBuilder::for_function(&m, f)?;
            let _entry = b.create_block("entry");
            let second = b.create_block("second"); // not entry -- unsealed
            b.seal_block(second)?;
            match b.seal_block(second) {
                Err(IrError::SsaBlockAlreadySealed { .. }) => {}
                other => panic!("expected SsaBlockAlreadySealed, got {other:?}"),
            }
            Ok(())
        })
    }

    /// llvmkit-specific: locks `SsaFunctionHasBlocks` -- the layer must
    /// observe every CFG edge from birth, so grafting onto a function
    /// that already has a body is rejected rather than silently missing
    /// the pre-existing blocks' edges.
    #[test]
    fn for_function_rejects_function_with_existing_blocks() -> Result<(), IrError> {
        Module::with_new("ssa-nonempty-fn", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let _entry = f.append_basic_block(&m, "entry");
            match SsaBuilder::for_function(&m, f) {
                Err(IrError::SsaFunctionHasBlocks) => {}
                Ok(_) => panic!("expected SsaFunctionHasBlocks, got Ok"),
                Err(other) => panic!("expected SsaFunctionHasBlocks, got {other:?}"),
            }
            Ok(())
        })
    }

    /// llvmkit-specific: locks `SsaForeignBlock` -- a block handle from a
    /// different `SsaBuilder` is a typed runtime error at `seal_block`.
    #[test]
    fn seal_block_rejects_foreign_block() -> Result<(), IrError> {
        Module::with_new("ssa-foreign-block", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f1 = m.add_function::<(), _>("f1", fn_ty, Linkage::External)?;
            let f2 = m.add_function::<(), _>("f2", fn_ty, Linkage::External)?;
            let mut b1 = SsaBuilder::for_function(&m, f1)?;
            let _entry1 = b1.create_block("entry");
            let other1 = b1.create_block("other");

            let mut b2 = SsaBuilder::for_function(&m, f2)?;
            let _entry2 = b2.create_block("entry");

            match b2.seal_block(other1) {
                Err(IrError::SsaForeignBlock) => {}
                other => panic!("expected SsaForeignBlock, got {other:?}"),
            }
            Ok(())
        })
    }

    /// llvmkit-specific: locks the declared-variable handle shape
    /// (`owner`/`module` accessors) across all three categories.
    #[test]
    fn declare_var_family_reports_owner_and_module() -> Result<(), IrError> {
        Module::with_new("ssa-declare", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let mut b = SsaBuilder::for_function(&m, f)?;
            let int_var = b.declare_int_var::<i32, _>("x");
            let float_var = b.declare_float_var::<f64, _>("y");
            let ptr_var = b.declare_pointer_var("z");
            assert_eq!(int_var.owner(), b.id());
            assert_eq!(float_var.owner(), b.id());
            assert_eq!(ptr_var.owner(), b.id());
            assert_eq!(int_var.module().id(), m.id());
            assert_eq!(float_var.module().id(), m.id());
            assert_eq!(ptr_var.module().id(), m.id());
            assert_eq!(b.state.vars.len(), 3);
            Ok(())
        })
    }

    /// Ports the paper's central example (Braun et al. 2013, Fig. 2/4):
    /// a single strict int variable written once in the entry block and
    /// read back from the SAME block with no intervening control flow.
    /// `read_variable_in` on a sealed, single-def block returns the
    /// write directly -- no phi at all. Closest upstream functional
    /// reference: `SSAUpdater::GetValueInMiddleOfBlock`'s single-
    /// predecessor fast path (no PHI insertion needed).
    #[test]
    fn read_after_write_same_block_needs_no_phi() -> Result<(), IrError> {
        Module::with_new("ssa-straight-line", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let mut b = SsaBuilder::for_function(&m, f)?;
            let entry = b.create_block("entry");
            let entry_id = label_value_id(&entry.label);

            let var: IntVariable<i32, _> = b.declare_int_var("x");
            let one = m.i32_type().const_int(1_i32).as_value().id;
            b.write_variable(var.index, entry_id, one);
            let read = b.read_variable_in(var.index, entry_id)?;
            assert_eq!(read, one);
            assert!(b.state.created_phis.is_empty());
            Ok(())
        })
    }

    /// Ports Braun et al. 2013's incomplete-phi + completion flow: a
    /// variable is read inside a NOT-YET-sealed loop block before its
    /// own back-edge write is recorded (`readVariableRecursive`'s
    /// "not sealed" branch, Fig. 4). `seal_block` later completes the
    /// resulting incomplete phi via `add_phi_operands`. Closest upstream
    /// functional reference: `SSAUpdater`'s deferred-PHI-completion model
    /// (LLVM completes eagerly per-value rather than per-block, but the
    /// "operandless placeholder, filled in once the CFG is known" shape
    /// is the same idea).
    #[test]
    fn incomplete_phi_completes_on_seal() -> Result<(), IrError> {
        Module::with_new("ssa-incomplete-phi", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let mut b = SsaBuilder::for_function(&m, f)?;
            let _entry = b.create_block("entry");
            let entry_id = label_value_id(&_entry.label);
            let loop_bb = b.create_block("loop");
            let loop_id = label_value_id(&loop_bb.label);

            // Record edges: entry -> loop, loop -> loop (self back-edge).
            b.state.preds.entry(loop_id).or_default().push(entry_id);
            b.state.preds.entry(loop_id).or_default().push(loop_id);

            let var: IntVariable<i32, _> = b.declare_int_var("i");
            let zero = m.i32_type().const_int(0_i32).as_value().id;
            b.write_variable(var.index, entry_id, zero);

            // Read inside the not-yet-sealed loop block: creates an
            // incomplete (operandless) phi and records it for later
            // completion.
            let read_before_seal = b.read_variable_in(var.index, loop_id)?;
            assert_eq!(b.state.incomplete_phis.get(&loop_id).map(Vec::len), Some(1));
            assert!(b.state.created_phis.contains_key(&read_before_seal));

            // Record the loop body's own write (e.g. `i + 1`, modeled
            // here as reusing a fresh constant is fine -- the engine
            // does not care what the value IS, only that a def exists).
            let one = m.i32_type().const_int(1_i32).as_value().id;
            b.write_variable(var.index, loop_id, one);

            // Sealing completes the incomplete phi: two distinct incoming
            // values (`zero` from entry, `one` from the loop back-edge),
            // so it is NOT trivial and survives as a real phi.
            b.seal_block(loop_bb)?;
            assert!(
                b.state
                    .incomplete_phis
                    .get(&loop_id)
                    .is_none_or(Vec::is_empty)
            );
            let text = format!("{m}");
            assert!(
                text.contains("phi i32"),
                "expected a real phi, got:\n{text}"
            );
            Ok(())
        })
    }

    /// Ports Braun et al. 2013's trivial-phi elimination (Fig. 3,
    /// `tryRemoveTrivialPhi`): a phi merging exactly one DISTINCT
    /// incoming value (the same constant from two predecessors) is
    /// redundant and is replaced by that value, leaving no phi
    /// instruction behind. Closest upstream functional reference:
    /// `SSAUpdater::RewriteUse`'s "AvailableVal has a single value"
    /// short-circuit (LLVM's own trivial-phi-avoidance heuristic).
    #[test]
    fn trivial_phi_is_eliminated() -> Result<(), IrError> {
        Module::with_new("ssa-trivial-join", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let mut b = SsaBuilder::for_function(&m, f)?;
            let _entry = b.create_block("entry");
            let entry_id = label_value_id(&_entry.label);
            let left = b.create_block("left");
            let left_id = label_value_id(&left.label);
            let right = b.create_block("right");
            let right_id = label_value_id(&right.label);
            let join = b.create_block("join");
            let join_id = label_value_id(&join.label);

            b.state.preds.entry(left_id).or_default().push(entry_id);
            b.state.preds.entry(right_id).or_default().push(entry_id);
            b.state.preds.entry(join_id).or_default().push(left_id);
            b.state.preds.entry(join_id).or_default().push(right_id);
            b.seal_block(left)?;
            b.seal_block(right)?;

            let var: IntVariable<i32, _> = b.declare_int_var("x");
            let same_value = m.i32_type().const_int(7_i32).as_value().id;
            // Both predecessors write the SAME value.
            b.write_variable(var.index, left_id, same_value);
            b.write_variable(var.index, right_id, same_value);

            b.seal_block(join)?;
            let read = b.read_variable_in(var.index, join_id)?;
            assert_eq!(
                read, same_value,
                "trivial phi should resolve to the shared value"
            );
            assert!(
                b.state.created_phis.is_empty(),
                "the trivial join phi should have been erased"
            );
            let text = format!("{m}");
            assert!(!text.contains("phi"), "no phi should remain, got:\n{text}");
            Ok(())
        })
    }

    /// Locks the strict-variable undefined-read error: a read that
    /// chases back to the (sealed, predecessor-less) entry block with no
    /// write anywhere on the path is `Err(SsaUseOfUndefinedVariable)`.
    /// Mirrors LLVM's "use of undefined value" outcome for an
    /// uninitialized local in a from-scratch frontend (there is no
    /// single upstream C++ unit test for this -- `mem2reg`/`SSAUpdater`
    /// assume the caller already proved definedness via dominance
    /// analysis on existing IR, whereas this layer is documenting new
    /// IR into existence and must reject the same case itself).
    #[test]
    fn strict_variable_undefined_read_errors() -> Result<(), IrError> {
        Module::with_new("ssa-undefined-strict", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let mut b = SsaBuilder::for_function(&m, f)?;
            let entry = b.create_block("entry");
            let entry_id = label_value_id(&entry.label);

            let var: IntVariable<i32, _> = b.declare_int_var("x");
            match b.read_variable_in(var.index, entry_id) {
                Err(IrError::SsaUseOfUndefinedVariable { .. }) => {}
                other => panic!("expected SsaUseOfUndefinedVariable, got {other:?}"),
            }
            Ok(())
        })
    }

    /// Poison twin of [`strict_variable_undefined_read_errors`]: a
    /// `declare_int_var_poison` variable read on the same def-less path
    /// yields `poison i32` instead of an error (D10's explicit-opt-in
    /// escape hatch, mirroring `PoisonValue::get`'s "the value never
    /// caused control flow to depend on it" invariant more directly than
    /// LLVM's own frontends usually do -- Clang, e.g., emits an
    /// uninitialized `undef`/zero-init rather than `poison`).
    #[test]
    fn poison_variable_undefined_read_yields_poison() -> Result<(), IrError> {
        Module::with_new("ssa-undefined-poison", |m| {
            let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
            let mut b = SsaBuilder::for_function(&m, f)?;
            let entry = b.create_block("entry");
            let entry_id = label_value_id(&entry.label);

            let var: IntVariable<i32, _> = b.declare_int_var_poison("x");
            let read = b.read_variable_in(var.index, entry_id)?;
            let i32_ty = m.i32_type();
            let poison_id = i32_ty.as_type().get_poison().as_value().id;
            assert_eq!(read, poison_id);
            Ok(())
        })
    }
}
