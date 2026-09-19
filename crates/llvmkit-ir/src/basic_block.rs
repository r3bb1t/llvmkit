//! Basic block (`label`-typed value) handle. Mirrors
//! `llvm/include/llvm/IR/BasicBlock.h` and `llvm/lib/IR/BasicBlock.cpp`.
//!
//! ## Storage shape
//!
//! A basic block lives in the value arena under the basic-block
//! category, with type [`LabelType`](crate::LabelType). It owns
//! a list of instruction value-ids, mutated through the [`IrBuilder`]
//! and other future helpers via interior mutability so the same
//! `&'ctx Module<B, Unverified>` borrow can be passed around freely.
//!
//! ## Return-marker propagation
//!
//! [`BasicBlock<'ctx, R>`] inherits its parent function's
//! [`ReturnMarker`]. When the IrBuilder positions itself inside a
//! block, the marker propagates to the builder so its `ret`
//! is statically typed.
//!
//! [`IrBuilder`]: crate::ir_builder::IrBuilder

use super::asm_writer::SlotTracker;
use super::block_params::{BlockParams, BlockParamsDyn};
use super::block_state::{BlockTerminationState, Terminated, Unterminated};
use super::error::ValueCategoryLabel;
use super::function::FunctionValue;
use super::function_signature::{CallArgs, FunctionParamList};
use super::instruction::{InstructionKindData, InstructionView, absorb_debug_records};
use super::ir_builder::constant_folder::ConstantFolder;
use super::ir_builder::{IrBuilder, Positioned};
use super::marker::{Dyn, ReturnMarker};
use super::metadata::{
    MetadataAttachmentKind, MetadataField, MetadataFieldValue, MetadataId, MetadataKind,
    SpecializedMetadataKind, SpecializedMetadataNode, StoredBrand,
};
use super::module::{Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::r#type::{TypeSlot, TypeSlotAccess};
use super::value::{
    HasDebugLoc, HasName, Typed, Value, ValueKindData, ValueSlot, ValueSlotAccess, sealed,
};
use super::value_id::BlockId;
use super::value_id::ViewIn;
use super::{DebugLoc, IrError, IrResult, Type};
use crate::Branded;
use core::cell::{Cell, RefCell};
use core::iter::FusedIterator;
use core::marker::PhantomData;

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload stored under
/// [`ValueKindData::BasicBlock`](crate::value::ValueKindData::BasicBlock).
#[derive(Debug)]
pub(super) struct BasicBlockData {
    /// Owning function. `None` for an orphan block (no function yet
    /// attached). Mirrors LLVM's `BasicBlock::Parent`.
    pub(super) parent: RefCell<Option<ValueSlot>>,
    /// Linear list of instruction value ids in program order.
    pub(super) instructions: RefCell<Vec<ValueSlot>>,
    /// How many **block parameters** this block was created with, in the
    /// Swift-SIL / MLIR sense: the count declared by
    /// [`IrBuilder::append_block_with_params`](crate::IrBuilder::append_block_with_params),
    /// its naming twin, or the typed
    /// [`append_block_typed`](crate::IrBuilder::append_block_typed). Zero for
    /// every other block — a plain `append_basic_block`, a parsed `.ll` block,
    /// an auto-SSA block, a pass-created block — even when such a block
    /// carries leading phis, because those phis are seeded through their own
    /// checked paths rather than by branch arguments.
    ///
    /// This is *not* the parameter list; the parameters themselves are the
    /// block's leading head-phis (see
    /// [`block_parameter_phis`]). It is the one-`Cell` fact that lets
    /// [`require_no_block_parameters`] leave the hot path — every branch to a
    /// param-less block — without touching the instruction list.
    pub(super) parameter_count: Cell<usize>,
}

impl BasicBlockData {
    /// Construct an empty block, optionally already attached to a
    /// parent function.
    pub(super) fn new(parent: Option<ValueSlot>) -> Self {
        Self {
            parent: RefCell::new(parent),
            instructions: RefCell::new(Vec::new()),
            parameter_count: Cell::new(0),
        }
    }
}

// --------------------------------------------------------------------------
// Public handle
// --------------------------------------------------------------------------

/// Typed handle to a basic block. The wrapped value's IR type is
/// always [`LabelType`](crate::derived_types::LabelType); the cached
/// `ty` field carries that label type's id without allocating.
///
/// The `R: ReturnMarker` parameter pins the parent function's return
/// shape at the type level so a typed [`IrBuilder`]
/// positioned inside the block can keep its compile-time `ret`
/// invariant.
///
/// The `Term: BlockTerminationState` parameter (default [`Unterminated`])
/// distinguishes blocks that still accept appended instructions from
/// blocks whose terminator has been emitted. The termination marker is
/// enforced at [`crate::IrBuilder::position_at_end`], which only accepts
/// an [`Unterminated`] block; once a terminator-emitting the emitters
/// consumes the builder, the returned handle names the same block with
/// `Term = Terminated`. `BasicBlock` is intentionally linear (`!Copy` /
/// `!Clone`) so retaining an old unterminated insertion capability cannot
/// reopen a terminated construction path. Use [`id`](Self::id) to mint the
/// copyable [`BlockId`] that names this block at branch-target and
/// PHI-predecessor positions.
pub struct BasicBlock<
    'ctx,
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand,
    Params: BlockParams = BlockParamsDyn,
> {
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    ty: TypeSlot,
    pub(super) _r: PhantomData<R>,
    pub(super) _term: PhantomData<Term>,
    pub(super) _params: PhantomData<Params>,
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand, Params: BlockParams>
    PartialEq for BasicBlock<'ctx, R, Term, B, Params>
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand, Params: BlockParams> Eq
    for BasicBlock<'ctx, R, Term, B, Params>
{
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand, Params: BlockParams>
    core::hash::Hash for BasicBlock<'ctx, R, Term, B, Params>
{
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand, Params: BlockParams>
    core::fmt::Debug for BasicBlock<'ctx, R, Term, B, Params>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BasicBlock")
            .field("id", &self.id)
            .field("ty", &self.ty)
            .finish()
    }
}

/// Copyable, borrowing *view* of a basic block — the handle a
/// [`BlockId`] resolves to through
/// [`Module::view`](crate::Module::view) / [`IrBuilder::view`](crate::IrBuilder::view).
///
/// Unlike [`BasicBlock`], this is not an insertion capability: it can name a
/// branch target or PHI predecessor, but it cannot be passed to
/// [`IrBuilder::position_at_end`](crate::IrBuilder::position_at_end) — use the
/// checked [`IrBuilder::position_at_end_dyn`](crate::IrBuilder::position_at_end_dyn)
/// with a [`BlockId`] for that.
///
/// Since 0.0.4 this is the ephemeral read view, not the stored currency:
/// producers hand back [`BlockId`] and consumers accept it, so a label is
/// something you *take* to read a block, not something you keep.
pub struct BasicBlockLabel<
    'ctx,
    R: ReturnMarker,
    B: ModuleBrand,
    Params: BlockParams = BlockParamsDyn,
> {
    id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    ty: TypeSlot,
    pub(super) _r: PhantomData<R>,
    pub(super) _params: PhantomData<Params>,
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand, Params: BlockParams> Clone
    for BasicBlockLabel<'ctx, R, B, Params>
{
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand, Params: BlockParams> Copy
    for BasicBlockLabel<'ctx, R, B, Params>
{
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand, Params: BlockParams> PartialEq
    for BasicBlockLabel<'ctx, R, B, Params>
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand, Params: BlockParams> Eq
    for BasicBlockLabel<'ctx, R, B, Params>
{
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand, Params: BlockParams> core::hash::Hash
    for BasicBlockLabel<'ctx, R, B, Params>
{
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand, Params: BlockParams> core::fmt::Debug
    for BasicBlockLabel<'ctx, R, B, Params>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BasicBlockLabel")
            .field("id", &self.id)
            .field("ty", &self.ty)
            .finish()
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx, Params: BlockParams>
    BasicBlockLabel<'ctx, R, B, Params>
{
    /// Widen this copyable label reference to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes, so the label stays usable afterwards.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// The unchecked door for this handle: its erased value's
    /// [`ValueSlotAccess::slot_trusting_same_module`], for a read that stays
    /// inside this block's module. No public route hands out the bare slot;
    /// [`id`](Self::id) mints the storable, module-tagged id.
    #[inline]
    pub(crate) fn slot_trusting_same_module(&self) -> ValueSlot {
        self.to_erased().slot_trusting_same_module()
    }

    /// Storable, module-tagged [`BlockId<R, B, Params>`] for this block
    /// (0.0.4), resolvable via [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view) back into a copyable
    /// [`BasicBlockLabel`]. Preserves the return-shape and parameter markers.
    #[inline]
    pub fn id(&self) -> BlockId<R, B, Params> {
        BlockId::from_raw(self.module.id(), self.id)
    }

    /// Drop the typed parameter marker, yielding the parameter-erased
    /// ([`BlockParamsDyn`]) label form. Crate-internal: the typed branch
    /// builders lower a [`BlockCall`] to this erased label before reusing the
    /// erased phi-seeding path, which is written against the `BlockParamsDyn`
    /// label.
    #[inline]
    pub(crate) fn erase_params(self) -> BasicBlockLabel<'ctx, R, B> {
        BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _params: PhantomData,
        }
    }

    /// Crate-internal: the label for the block at `id` in `module`, typed
    /// `ty`. Built by [`BlockId`]'s `resolve_in` after it compares the id's
    /// module tag, and by nothing that skips that comparison.
    #[inline]
    pub(crate) fn from_parts(id: ValueSlot, module: ModuleRef<'ctx, B>, ty: TypeSlot) -> Self {
        Self {
            id,
            module,
            ty,
            _r: PhantomData,
            _params: PhantomData,
        }
    }

    /// The linear block handle for this label's block, in the label's own
    /// module. Crate-internal: the builder reopens a block it has already
    /// admitted — through [`ViewIn::resolve_in`] or
    /// [`IntoBasicBlockLabel`] — as its insertion point.
    #[inline]
    pub(crate) fn to_block<Term: BlockTerminationState>(self) -> BasicBlock<'ctx, R, Term, B> {
        BasicBlock::from_parts(self.id, self.module, self.ty)
    }
}

mod block_label_sealed {
    pub trait Sealed {}
}

/// Values accepted where an instruction names a basic-block label.
///
/// The storable currency at these positions is [`BlockId`] — that is what a
/// producer hands back and what a struct stores. This trait is the *accepting*
/// bound: it also takes the borrowing block handles directly, so an
/// in-scope [`BasicBlock`] can name its own branch target without a round trip
/// through the module. Resolution is module-checked and fallible, exactly like
/// [`IntoErasedValue`](crate::IntoErasedValue) at operand positions: a
/// [`BlockId`] minted in another module yields
/// [`IrError::ForeignValueId`] instead of silently naming a same-numbered slot
/// here.
///
/// The produced [`BasicBlockLabel`] is the ephemeral *view*, parameter-erased:
/// the typed parameter schema is honoured by the [`BlockCall`] edge
/// ([`BasicBlockLabel::call`] / [`BasicBlock::call`]), not by the plain label
/// positions.
pub trait IntoBasicBlockLabel<'ctx, R: ReturnMarker, B: ModuleBrand>:
    block_label_sealed::Sealed
{
    fn into_basic_block_label(
        self,
        module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>>;
}

impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> block_label_sealed::Sealed
    for BlockId<R, B, Params>
{
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx, Params: BlockParams>
    IntoBasicBlockLabel<'ctx, R, B> for BlockId<R, B, Params>
{
    #[inline]
    fn into_basic_block_label(
        self,
        module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>> {
        ViewIn::resolve_in(self, module)
            .map(BasicBlockLabel::erase_params)
            .ok_or(IrError::ForeignValueId)
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand> block_label_sealed::Sealed
    for BasicBlockLabel<'ctx, R, B>
{
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> IntoBasicBlockLabel<'ctx, R, B>
    for BasicBlockLabel<'ctx, R, B>
{
    #[inline]
    fn into_basic_block_label(
        self,
        module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>> {
        // Boundary: refuse a block another module minted.
        self.to_erased().slot_in(module.id())?;
        Ok(self)
    }
}

impl<'ctx, R, Term, B, Params> block_label_sealed::Sealed for BasicBlock<'ctx, R, Term, B, Params>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
    Params: BlockParams,
{
}

impl<'ctx, R, Term, B, Params> IntoBasicBlockLabel<'ctx, R, B>
    for BasicBlock<'ctx, R, Term, B, Params>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
    Params: BlockParams,
{
    #[inline]
    fn into_basic_block_label(
        self,
        module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>> {
        // Boundary: refuse a block another module minted.
        self.to_erased().slot_in(module.id())?;
        Ok(BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _params: PhantomData,
        })
    }
}

impl<'ctx, R, Term, B, Params> block_label_sealed::Sealed for &BasicBlock<'ctx, R, Term, B, Params>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
    Params: BlockParams,
{
}

impl<'ctx, R, Term, B, Params> IntoBasicBlockLabel<'ctx, R, B>
    for &BasicBlock<'ctx, R, Term, B, Params>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
    Params: BlockParams,
{
    #[inline]
    fn into_basic_block_label(
        self,
        module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>> {
        // Boundary: refuse a block another module minted.
        self.to_erased().slot_in(module.id())?;
        // `IntoBasicBlockLabel` yields the parameter-erased label (its return
        // type pins `BlockParamsDyn`), so construct it directly rather than
        // through `label()`, which threads this block's `Params`.
        Ok(BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _params: PhantomData,
        })
    }
}

impl<R: ReturnMarker, B: ModuleBrand> block_label_sealed::Sealed
    for super::ssa_builder::SsaBlock<R, B>
{
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> IntoBasicBlockLabel<'ctx, R, B>
    for super::ssa_builder::SsaBlock<R, B>
{
    #[inline]
    fn into_basic_block_label(
        self,
        module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>> {
        self.id().into_basic_block_label(module)
    }
}

// --------------------------------------------------------------------------
// Typed control-flow edge bundle
// --------------------------------------------------------------------------

/// A typed control-flow edge: a branch target ([`BlockId`]) stamped
/// with its parameter schema `Params`, paired with the block-argument values
/// that seed the target's leading head-phis on that edge.
///
/// Constructed by [`BasicBlockLabel::call`] (or, ergonomically,
/// [`BasicBlock::call`]) on a **typed** label/block — one produced by
/// [`IrBuilder::append_block_typed`](crate::IrBuilder::append_block_typed). The
/// argument tuple is checked against `Params` at **compile time** through the
/// [`CallArgs<Params>`](crate::CallArgs) bound on `.call()`: a wrong arity has
/// no `CallArgs` impl and a wrong-typed position fails its per-position
/// [`IntoCallArg`](crate::IntoCallArg) bound, so a mismatched edge does not
/// compile — the same machinery that guards typed `call`.
///
/// The arguments are lowered eagerly at construction (the typed label carries
/// its owning module), so `.call()` stays infallible and ergonomic. Any
/// *value-level* lowering failure — the fallibility [`CallArgs::lower`] carries,
/// e.g. a cross-module constant — is captured and re-surfaced when the bundle
/// is consumed by
/// [`IrBuilder::br_call`](crate::IrBuilder::br_call) /
/// [`IrBuilder::cond_br_call`](crate::IrBuilder::cond_br_call),
/// where a `?` is already expected.
#[derive(Branded)]
#[branded(Debug)]
pub struct BlockCall<R: ReturnMarker, B: ModuleBrand, Params: BlockParams = BlockParamsDyn> {
    target: BlockId<R, B, Params>,
    /// The edge's block-arguments lowered to arena value-ids in declaration
    /// order, or the deferred lowering error to surface at build time. The
    /// arity and per-position types are already fixed by the compile-time
    /// [`CallArgs<Params>`](crate::CallArgs) bound, so this only carries the
    /// value-level fallibility of [`CallArgs::lower`].
    lowered: IrResult<Box<[ValueSlot]>>,
}

impl<'ctx, R, B, Params> BasicBlockLabel<'ctx, R, B, Params>
where
    R: ReturnMarker,
    B: ModuleBrand + 'ctx,
    Params: BlockParams + FunctionParamList,
{
    /// Bundle this typed branch target with the block-arguments that seed its
    /// leading head-phis, forming a [`BlockCall`] edge for
    /// [`IrBuilder::br_call`](crate::IrBuilder::br_call) /
    /// [`IrBuilder::cond_br_call`](crate::IrBuilder::cond_br_call).
    ///
    /// `args` must be an argument tuple matching this block's `Params` schema:
    /// the [`CallArgs<'ctx, Params, B>`](crate::CallArgs) bound makes a wrong
    /// arity or a wrong-typed position a **compile** error, reusing the exact
    /// machinery of a typed `call`. The values are lowered here (this
    /// label carries its module), so `.call()` is infallible; a value-level
    /// lowering failure is deferred into the returned [`BlockCall`] and surfaces
    /// when the branch builder consumes it.
    #[inline]
    pub fn call<A>(self, args: A) -> BlockCall<R, B, Params>
    where
        A: CallArgs<'ctx, Params, B>,
    {
        let lowered = args.lower(self.module).map(|lowered| lowered.0);
        BlockCall {
            target: self.id(),
            lowered,
        }
    }
}

impl<'ctx, R, Term, B, Params> BasicBlock<'ctx, R, Term, B, Params>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
    Params: BlockParams + FunctionParamList,
{
    /// Convenience wrapper for `self.label().call(args)`: bundle this typed
    /// block as a branch target with the block-arguments that seed its head-phis.
    /// Borrows the block, so the handle stays usable (e.g. to reposition the
    /// builder into it afterwards). See [`BasicBlockLabel::call`].
    #[inline]
    pub fn call<A>(&self, args: A) -> BlockCall<R, B, Params>
    where
        A: CallArgs<'ctx, Params, B>,
    {
        self.label().call(args)
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx, Params: BlockParams>
    BasicBlock<'ctx, R, Unterminated, B, Params>
{
    /// Positioned builder at the end of this block. `bb.builder()` is
    /// exactly [`IrBuilder::at_end(bb)`](crate::IrBuilder::at_end) — the
    /// return marker `R` is inferred from the block, so no turbofish is
    /// needed. Reads better when `bb` is already in hand.
    #[inline]
    pub fn builder(self) -> IrBuilder<'ctx, 'ctx, B, ConstantFolder, Positioned, R> {
        IrBuilder::at_end(self)
    }
}

impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> BlockCall<R, B, Params> {
    /// Decompose into the parameter-erased target id and the edge's
    /// lowered-or-deferred block-arguments. Crate-internal: the typed branch
    /// builders consume the bundle here, then reuse the erased phi-seeding path.
    #[inline]
    pub(crate) fn into_parts(self) -> (BlockId<R, B>, IrResult<Box<[ValueSlot]>>) {
        (self.target.erase_params(), self.lowered)
    }
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    BasicBlock<'ctx, R, Term, B, Params>
{
    #[inline]
    pub(super) fn from_parts<M>(id: ValueSlot, module: M, ty: TypeSlot) -> Self
    where
        M: Into<ModuleRef<'ctx, B>>,
    {
        Self {
            id,
            module: module.into(),
            ty,
            _r: PhantomData,
            _term: PhantomData,
            _params: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn copy_handle(&self) -> Self {
        Self {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _term: PhantomData,
            _params: PhantomData,
        }
    }

    /// Copyable label *view* of this block.
    ///
    /// Crate-internal since 0.0.4: [`BlockId`] is the branch-target and
    /// PHI-predecessor currency a caller stores and passes around, minted with
    /// [`id`](Self::id); [`BasicBlockLabel`] is the ephemeral view, reached
    /// publicly through [`Module::view`](crate::Module::view) /
    /// [`IrBuilder::view`](crate::IrBuilder::view) like every other handle.
    /// In-crate this stays the cheap way to get a label from a block that
    /// already carries its module.
    ///
    /// The returned label threads this block's `Params` marker through, so a
    /// typed block (`BasicBlock<…, Params>`) yields a typed label
    /// (`BasicBlockLabel<…, Params>`) that keeps the parameter promise; a
    /// parameter-erased block (the [`BlockParamsDyn`] default) yields the
    /// erased label form, unchanged.
    #[inline]
    pub(crate) fn label(&self) -> BasicBlockLabel<'ctx, R, B, Params> {
        BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _params: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle.
    ///
    /// Borrows rather than consumes, so the block stays usable afterwards.
    #[inline]
    pub fn to_erased(&self) -> Value<'ctx, B> {
        Value::from_parts(self.id, self.module, self.ty)
    }

    /// The unchecked door for this handle: its erased value's
    /// [`ValueSlotAccess::slot_trusting_same_module`], for a read that stays
    /// inside this block's module. No public route hands out the bare slot;
    /// [`id`](Self::id) mints the storable, module-tagged id.
    #[inline]
    pub(crate) fn slot_trusting_same_module(&self) -> ValueSlot {
        self.to_erased().slot_trusting_same_module()
    }

    /// Storable, module-tagged [`BlockId<R, B, Params>`] for this block
    /// (0.0.4), resolvable via [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view) back into a copyable
    /// [`BasicBlockLabel`]. The block handle is linear (`!Copy`), so this
    /// borrows `self` and leaves it usable — minting a `Copy` id from a
    /// non-`Copy` block.
    #[inline]
    pub fn id(&self) -> BlockId<R, B, Params> {
        BlockId::from_raw(self.module.id(), self.id)
    }

    /// Erase the return-shape marker (and the parameter marker), producing
    /// the runtime-checked [`Dyn`] / [`BlockParamsDyn`] form. Crate-internal
    /// only: this duplicates the handle for storage and printing helpers, so
    /// public code should use [`label`](Self::label) when it needs a copyable
    /// non-insertion reference.
    #[inline]
    pub(crate) fn as_dyn(&self) -> BasicBlock<'ctx, Dyn, Term, B> {
        BasicBlock {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _term: PhantomData,
            _params: PhantomData,
        }
    }

    /// Re-tag the termination-state marker. Crate-internal: only the
    /// terminator-emitting build path produces a terminated view from
    /// an unterminated builder block.
    #[inline]
    pub(super) fn retag_termination<S2: BlockTerminationState>(self) -> BasicBlock<'ctx, R, S2, B> {
        BasicBlock {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _term: PhantomData,
            _params: PhantomData,
        }
    }

    /// Re-tag the block-parameter marker, keeping the return-shape and
    /// termination markers. Crate-internal: only the typed constructor
    /// [`crate::IrBuilder::append_block_typed`] stamps a freshly appended
    /// block with the `Params` schema whose head-phis it just built.
    #[inline]
    pub(crate) fn retag_params<P2: BlockParams>(self) -> BasicBlock<'ctx, R, Term, B, P2> {
        BasicBlock {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _term: PhantomData,
            _params: PhantomData,
        }
    }

    /// Borrow the storage payload.
    fn data(&self) -> &'ctx BasicBlockData {
        match &self.to_erased().data().kind {
            ValueKindData::BasicBlock(b) => b,
            // The handle was produced by a constructor that pushed a
            // BasicBlock variant; the kind cannot have changed.
            _ => unreachable!("BasicBlock handle invariant: kind is BasicBlock"),
        }
    }

    /// Optional textual name. Mirrors `BasicBlock::getName`.
    #[inline]
    pub fn name(&self) -> Option<String> {
        self.to_erased().name()
    }

    /// Set or clear the textual name.
    /// Set the textual name.
    #[inline]
    pub fn set_name<Name>(&self, module_token: &'ctx Module<B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.to_erased().set_name(module_token, name);
    }

    /// Clear the textual name.
    #[inline]
    pub fn clear_name(&self, module_token: &'ctx Module<B, Unverified>) {
        self.to_erased().clear_name(module_token);
    }

    /// Owning module reference.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module.module())
    }

    /// Owning module reference with the compile-time brand.
    #[inline]
    pub(super) fn module_ref(&self) -> ModuleRef<'ctx, B> {
        self.module
    }

    /// Owning function value-id, or `None` if the block is an orphan.
    pub(super) fn parent_id(&self) -> Option<ValueSlot> {
        *self.data().parent.borrow()
    }

    /// Parent function as a runtime-checked [`FunctionValue<Dyn>`](FunctionValue).
    /// `None` if the block is an orphan (no parent attached). The
    /// caller can narrow back to its static `R` via
    /// [`crate::FunctionValue::as_dyn`] / `try_into` if needed.
    pub fn parent_function(&self) -> Option<FunctionValue<'ctx, Dyn, B>> {
        let id = self.parent_id()?;
        Some(FunctionValue::<'ctx, Dyn, B>::from_parts_unchecked(
            id,
            self.module,
        ))
    }

    /// Iterate the instruction value-ids in program order. Returns
    /// `ValueSlot`s rather than full instruction handles so the caller
    /// can decide which view (raw operand-traversal vs typed
    /// `Instruction<'ctx>` handle) it wants.
    pub(crate) fn instruction_ids(&self) -> Vec<ValueSlot> {
        self.data().instructions.borrow().clone()
    }

    /// Iterate read-only instruction views in program order.
    ///
    /// The `use<..>` bound keeps `&self` *out* of the returned opaque type.
    /// The iterator owns its ids and a copied [`ModuleRef`], so it borrows
    /// nothing from the receiver — without the bound, edition 2024 would
    /// capture the `&self` lifetime anyway and reject
    /// `blocks.flat_map(|block| block.instructions())`.
    pub fn instructions(
        &self,
    ) -> impl ExactSizeIterator<Item = InstructionView<'ctx, B>>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, R, Term, B, Params> {
        let module = self.module;
        let ids = self.instruction_ids();
        ids.into_iter()
            .map(move |id| InstructionView::from_parts(id, module))
    }

    /// `true` if the block currently has no instructions.
    pub fn is_empty(&self) -> bool {
        self.data().instructions.borrow().is_empty()
    }

    /// Last instruction view (the terminator if the block is well-formed),
    /// or `None` for an empty block.
    pub fn terminator(&self) -> Option<InstructionView<'ctx, B>> {
        let last = *self.data().instructions.borrow().last()?;
        Some(InstructionView::from_parts(last, self.module))
    }

    /// Successor block ids of this block's terminator, preserving duplicate CFG edges.
    /// Yields nothing for unterminated blocks and terminators without successors.
    ///
    /// A snapshot, not a borrow: the terminator's destination list lives behind
    /// a [`RefCell`], so the ids are read out once and the iterator owns them —
    /// holding the iterator can never conflict with an edit. The `use<..>`
    /// bound keeps `&self` out of the returned opaque type for the same reason
    /// it does on [`instructions`](Self::instructions).
    pub fn successors(
        &self,
    ) -> impl ExactSizeIterator<Item = BlockId<Dyn, B>>
    + DoubleEndedIterator
    + FusedIterator
    + use<'ctx, R, Term, B, Params> {
        let tag = self.module.id();
        crate::cfg::successor_ids(&self.as_dyn())
            .into_iter()
            .map(move |slot| BlockId::<Dyn, B>::from_raw(tag, slot))
    }

    /// Append an instruction value-id to the block. Crate-internal:
    /// only the IR builder calls this.
    pub(super) fn append_instruction(&self, instr: ValueSlot) {
        self.data().instructions.borrow_mut().push(instr);
    }

    /// Remove `instr` from this block's instruction list. Returns
    /// `true` if the id was present and removed, `false` if the
    /// block did not contain it. Crate-internal: only the mutation
    /// API ([`Instruction::erase_from_parent`](crate::Instruction))
    /// reaches for this.
    ///
    /// Mirrors LLVM's `BasicBlock::getInstList().remove(I)`
    /// (`lib/IR/BasicBlock.cpp`).
    pub(super) fn remove_instruction(&self, instr: ValueSlot) -> bool {
        let mut list = self.data().instructions.borrow_mut();
        if let Some(pos) = list.iter().position(|id| *id == instr) {
            list.remove(pos);
            true
        } else {
            false
        }
    }

    /// Insert `instr` immediately before `before` in this block's
    /// instruction list. Errors with [`IrError::InvalidOperation`] if
    /// `before` is not present in this block. Crate-internal: lifecycle
    /// primitives in [`crate::instruction`] reach for this.
    ///
    /// Mirrors `BasicBlock::getInstList().insert(before, I)`
    /// (`lib/IR/BasicBlock.cpp`).
    pub(super) fn insert_instruction_before(
        &self,
        instr: ValueSlot,
        before: ValueSlot,
    ) -> IrResult<()> {
        let mut list = self.data().instructions.borrow_mut();
        match list.iter().position(|id| *id == before) {
            Some(pos) => {
                list.insert(pos, instr);
                Ok(())
            }
            None => Err(IrError::InvalidOperation {
                message: "instruction anchor is not in this block",
            }),
        }
    }

    /// Insert `instr` immediately after `after` in this block's
    /// instruction list. Errors with [`IrError::InvalidOperation`] if
    /// `after` is not present in this block.
    pub(super) fn insert_instruction_after(
        &self,
        instr: ValueSlot,
        after: ValueSlot,
    ) -> IrResult<()> {
        let mut list = self.data().instructions.borrow_mut();
        match list.iter().position(|id| *id == after) {
            Some(pos) => {
                list.insert(pos + 1, instr);
                Ok(())
            }
            None => Err(IrError::InvalidOperation {
                message: "instruction anchor is not in this block",
            }),
        }
    }

    /// Insert `id` after the block's existing leading phis and before its
    /// first non-phi instruction. Keeps the "phis grouped at the top"
    /// invariant a construction-time fact instead of a verifier-time one:
    /// the IR builder routes every phi through here, so a phi built while
    /// the cursor sits past a non-phi still lands at the phi head. Mirrors
    /// the placement `IRBuilder::SetInsertPoint(&BB.getFirstNonPHI())`
    /// gives phis in `llvm/lib/IR/IRBuilder.cpp`.
    pub(crate) fn insert_instruction_at_phi_head(&self, id: ValueSlot) {
        let mut list = self.data().instructions.borrow_mut();
        let at = list
            .iter()
            .position(|iid| {
                // First instruction that is NOT a phi.
                !matches!(
                    &self.module.module().context().value_data(*iid).kind,
                    ValueKindData::Instruction(i)
                        if matches!(i.kind, InstructionKindData::Phi(_))
                )
            })
            .unwrap_or(list.len());
        list.insert(at, id);
    }

    /// Record that this block was created with `count` **block parameters**.
    /// Crate-internal: only the three block-parameter constructors
    /// ([`IrBuilder::append_block_with_params`](crate::IrBuilder::append_block_with_params),
    /// [`append_block_with_named_params`](crate::IrBuilder::append_block_with_named_params),
    /// [`append_block_typed`](crate::IrBuilder::append_block_typed)) call it,
    /// right after materialising that many head-phis.
    ///
    /// The count is what makes "is this a parameterised block?" a single
    /// [`Cell`] read for [`require_no_block_parameters`], so an argument-less
    /// branch to an ordinary block never walks an instruction list.
    #[inline]
    pub(crate) fn set_parameter_count(&self, count: usize) {
        self.data().parameter_count.set(count);
    }
}

// --------------------------------------------------------------------------
// Block parameters (the block-argument authoring model)
// --------------------------------------------------------------------------

/// Borrow a block's storage payload straight from the arena, given its slot.
///
/// The slot always comes from a resolved [`BasicBlockLabel`], which is only
/// ever minted over a real basic block — the same invariant
/// [`BasicBlock::data`] relies on.
fn block_data<'ctx, B: ModuleBrand>(
    module: ModuleRef<'ctx, B>,
    block: ValueSlot,
) -> &'ctx BasicBlockData {
    match &module.module().context().value_data(block).kind {
        ValueKindData::BasicBlock(data) => data,
        _ => unreachable!("branch-target invariant: a resolved label names a basic block"),
    }
}

/// The value-ids of `block`'s **parameters**: its leading head-phis, in
/// declaration order.
///
/// Scans from the block top and stops at the first non-phi — phis are grouped
/// at the head (an invariant `insert_instruction_at_phi_head` keeps at
/// construction time and the verifier re-checks), so the leading run of phis
/// *is* the parameter list.
///
/// Single source of truth for "how many parameters does this block have":
/// shared by the block-argument seeding path
/// (`IrBuilder::add_block_args`) and by [`require_no_block_parameters`],
/// so the arity a `_with_args` builder checks against and the arity a plain
/// branch is rejected for cannot drift apart.
pub(crate) fn block_parameter_phis<'ctx, B: ModuleBrand>(
    module: ModuleRef<'ctx, B>,
    block: ValueSlot,
) -> Vec<ValueSlot> {
    let context = module.module().context();
    let instructions = block_data(module, block).instructions.borrow();
    let mut params = Vec::new();
    for id in instructions.iter().copied() {
        let ValueKindData::Instruction(inst) = &context.value_data(id).kind else {
            continue;
        };
        let InstructionKindData::Phi(_) = &inst.kind else {
            break;
        };
        params.push(id);
    }
    params
}

/// Reject an edge that carries **no** block arguments into a block created
/// *with* block parameters.
///
/// This is the guard on the plain terminator builders — `br`,
/// `cond_br`, `switch`/`switch_dyn`'s default target and
/// [`SwitchInst::add_case`](crate::SwitchInst::add_case), both edges of every
/// `invoke*`, `callbr*`'s default and indirect destinations, and
/// [`IndirectBrInst::add_destination`](crate::IndirectBrInst::add_destination).
/// Branching
/// into a parameterised block without arguments adds no incomings, so the
/// target's parameter-phis stay one entry short — an incomplete phi that used
/// to surface only at [`Module::verify`](crate::Module::verify), through the
/// shared `check_phi` count guard. The caller must use the argument-carrying
/// builder for that edge instead.
///
/// Reports the same [`IrError::PhiArgArityMismatch`] the `_with_args` builders
/// already produce for a wrong argument count, so one wrong count reads the
/// same wherever it is caught.
///
/// **Hot path.** Every unconditional branch in every program reaches here, and
/// the overwhelming majority target param-less blocks. The declared-parameter
/// [`Cell`] read is the early-out: only a block that was *created* with
/// parameters walks its instruction list, and only to name the arity in the
/// error. A parsed `.ll` block, an auto-SSA block mid-Braun-construction, and a
/// pass-created block all leave on the first line even when they carry leading
/// phis — those phis are not block parameters and their incomings arrive
/// through their own checked paths.
pub(crate) fn require_no_block_parameters<'ctx, B: ModuleBrand>(
    module: ModuleRef<'ctx, B>,
    target: ValueSlot,
) -> IrResult<()> {
    if block_data(module, target).parameter_count.get() == 0 {
        return Ok(());
    }
    // The parameters are the leading head-phis, not the recorded count: if a
    // pass has since erased them there is nothing left to seed, and rejecting
    // with `expected: 0` would be a nonsense diagnostic.
    let expected = block_parameter_phis(module, target).len();
    if expected == 0 {
        return Ok(());
    }
    Err(IrError::PhiArgArityMismatch { expected, got: 0 })
}

// --------------------------------------------------------------------------
// Splice helpers (T1)
// --------------------------------------------------------------------------

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    BasicBlock<'ctx, R, Term, B, Params>
{
    /// Move every instruction from `self` into `dest`, appending at the
    /// end. After the call, `self` is empty and every moved instruction's
    /// `parent` field has been re-pointed at `dest`. Mirrors
    /// `BasicBlock::splice` in `lib/IR/BasicBlock.cpp`.
    pub fn splice_into<R2: ReturnMarker, S2: BlockTerminationState>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        dest: BasicBlock<'ctx, R2, S2, B>,
    ) -> IrResult<()> {
        let _ = module_token;
        // Boundary: the caller's destination block, admitted against this
        // block's module before either block is read or drained.
        let dest_id = dest.to_erased().slot_in(self.module.id())?;
        let module = self.module.module();
        let source_fn_id = self.parent_id();
        let dest_fn_id = dest.parent_id();
        let rehome_names = source_fn_id != dest_fn_id;
        let drained: Vec<ValueSlot> = {
            let mut src = self.data().instructions.borrow_mut();
            core::mem::take(&mut *src)
        };
        if rehome_names && let Some(source_fn_id) = source_fn_id {
            let source_fn =
                FunctionValue::<Dyn, B>::from_parts_unchecked(source_fn_id, self.module);
            for id in &drained {
                source_fn.remove_local_value_name(*id);
            }
        }
        {
            let mut dst = dest.data().instructions.borrow_mut();
            dst.extend(drained.iter().copied());
        }
        for id in &drained {
            module.context().set_instruction_parent(*id, dest_id);
        }
        if rehome_names && let Some(dest_fn_id) = dest_fn_id {
            let dest_fn = FunctionValue::<Dyn, B>::from_parts_unchecked(dest_fn_id, self.module);
            for id in &drained {
                let ty = module.context().value_data(*id).ty;
                let value = Value::from_parts(*id, self.module, ty);
                let current_name = value.name();
                if let Some(name) = current_name.as_deref() {
                    value.set_name_internal(None);
                    dest_fn.set_local_value_name(*id, Some(name));
                }
            }
        }
        Ok(())
    }

    /// The terminator's slot, or `None` when the block is empty or its last
    /// instruction is not a terminator. Ports `BasicBlock::getTerminator`.
    fn terminator_slot(&self) -> Option<ValueSlot> {
        let last = *self.data().instructions.borrow().last()?;
        match &self.module.value_data(last).kind {
            ValueKindData::Instruction(data) if data.kind.is_terminator() => Some(last),
            _ => None,
        }
    }

    /// Where `instruction` sits in this block's list. The split routines take
    /// an [`InstructionView`], which can name an instruction of any block;
    /// upstream takes an iterator into this block's own list, so it has no
    /// counterpart to this refusal.
    fn split_point_position(&self, instruction: ValueSlot) -> IrResult<usize> {
        self.data()
            .instructions
            .borrow()
            .iter()
            .position(|id| *id == instruction)
            .ok_or(IrError::InvalidOperation {
                message: "split instruction is not in this block",
            })
    }

    /// The parent function, and this block's position in its block list.
    /// Upstream's `BasicBlock::Create` accepts a null parent and makes a
    /// parentless block; llvmkit refuses to split an orphan instead.
    fn parent_and_position(&self) -> IrResult<(FunctionValue<'ctx, R, B>, usize)> {
        let parent_fn_id = self.parent_id().ok_or(IrError::InvalidOperation {
            message: "cannot split an orphan basic block",
        })?;
        let parent_fn =
            FunctionValue::<'ctx, R, B>::from_parts_unchecked(parent_fn_id, self.module);
        let position = parent_fn
            .data()
            .basic_blocks
            .borrow()
            .iter()
            .position(|id| *id == self.id)
            .ok_or(IrError::InvalidOperation {
                message: "block does not belong to function",
            })?;
        Ok((parent_fn, position))
    }

    /// Ports `BasicBlock::replacePhiUsesWith`: every incoming entry of this
    /// block's leading phis that names `old` names `new` instead
    /// (`PHINode::replaceIncomingBlockWith`). A phi's incoming blocks are not
    /// `Use`s upstream or here, so no use list changes.
    pub(crate) fn replace_phi_uses_with(&self, old: ValueSlot, new: ValueSlot) {
        // N.B. This might not be a complete BasicBlock, so don't assume
        // that it ends with a non-phi instruction.
        for instruction in self.instruction_ids() {
            let ValueKindData::Instruction(data) = &self.module.value_data(instruction).kind else {
                break;
            };
            let InstructionKindData::Phi(phi) = &data.kind else {
                break;
            };
            for incoming in phi.incoming.borrow_mut().iter_mut() {
                if incoming.1 == old {
                    incoming.1 = new;
                }
            }
        }
    }

    /// Ports `BasicBlock::replaceSuccessorsPhiUsesWith(Old, New)`: the phi
    /// rewrite, applied to each successor of this block's terminator,
    /// duplicate edges included.
    pub(crate) fn replace_successors_phi_uses_with(&self, old: ValueSlot, new: ValueSlot) {
        let Some(terminator) = self.terminator_slot() else {
            // Cope with being called on a BasicBlock that doesn't have a
            // terminator yet.
            return;
        };
        let ValueKindData::Instruction(data) = &self.module.value_data(terminator).kind else {
            return;
        };
        for successor in crate::cfg::kind_successor_ids(&data.kind) {
            BasicBlock::<'ctx, Dyn, Terminated, B>::from_parts(successor, self.module, self.ty)
                .replace_phi_uses_with(old, new);
        }
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx, Params: BlockParams>
    BasicBlock<'ctx, R, Terminated, B, Params>
{
    /// Split this block in two at `before` and return the new block.
    ///
    /// Ports `BasicBlock::splitBasicBlock` (`lib/IR/BasicBlock.cpp`) with
    /// `Before = false`; [`split_before`](Self::split_before) is the
    /// `Before = true` form. A block named `name` is created right after this
    /// one, `before` and every instruction after it move into it, this block
    /// gains `br label %name` carrying `before`'s location without its atom,
    /// and the phis of the moved terminator's successors are rewritten to name
    /// the new block. Both blocks end in a terminator afterwards.
    ///
    /// The debug records attached ahead of `before` stay in this block, ahead
    /// of the new branch. `splitBasicBlock` splices from an iterator whose head
    /// bit is clear, which leaves them behind for the branch to adopt.
    ///
    /// # Errors
    ///
    /// Every refusal happens before anything is created, moved or rewritten:
    ///
    /// - [`IrError::ForeignValueId`] if `before` belongs to another module.
    /// - [`IrError::InvalidOperation`] if this block has no terminator
    ///   (upstream's `assert(getTerminator())`; a `Terminated` handle is not
    ///   proof on its own, because [`FunctionValue::basic_blocks`] hands one
    ///   out for every block), if `before` is not one of its instructions, or
    ///   if this block has no parent function or is missing from its block
    ///   list.
    pub fn split_at<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        before: &InstructionView<'ctx, B>,
        name: Name,
    ) -> IrResult<BasicBlock<'ctx, R, Terminated, B>>
    where
        Name: Into<String>,
    {
        // Boundary: the caller's split point, admitted before the block is
        // read or a new block created.
        let split_id = before.slot_in(self.module.id())?;
        // assert(getTerminator() && "Can't use splitBasicBlock on degenerate BB!");
        if self.terminator_slot().is_none() {
            return Err(IrError::InvalidOperation {
                message: "Can't use splitBasicBlock on degenerate BB!",
            });
        }
        // assert(I != InstList.end() && "Trying to get me to create degenerate
        // basic block!"): an `InstructionView` always names an instruction.
        let split_position = self.split_point_position(split_id)?;
        let (parent_fn, this_position) = self.parent_and_position()?;
        // DebugLoc Loc = I->getStableDebugLoc(); if (Loc) Loc = Loc->getWithoutAtom();
        // Taken ahead of `BasicBlock::Create` rather than after it: building
        // the atom-free location is this routine's one fallible step, and the
        // two steps touch disjoint state.
        let location = split_point_location(module_token, before)?;
        let module = module_token.core_ref();

        // BasicBlock *New = BasicBlock::Create(getContext(), BBName, getParent(),
        //                                      this->getNextNode());
        let new_block = parent_fn.insert_basic_block_at_unchecked(this_position + 1, name);
        // Internal: the block was minted in this module just above.
        let new_id = new_block.slot_trusting_same_module();

        // New->splice(New->end(), this, I, end());
        let suffix: Vec<ValueSlot> = self
            .data()
            .instructions
            .borrow_mut()
            .split_off(split_position);
        new_block
            .data()
            .instructions
            .borrow_mut()
            .extend(suffix.iter().copied());
        for id in &suffix {
            module.context().set_instruction_parent(*id, new_id);
        }

        // BranchInst *BI = BranchInst::Create(New, this);
        let (_, branch) = IrBuilder::new_for::<R>(module_token)
            .position_at_end(self.copy_handle().retag_termination::<Unterminated>())
            .br_to_slot_unchecked(new_id);
        // The splice left `I`'s records at this block's end, and inserting the
        // branch there adopts them (`Instruction::insertBefore`).
        // Internal: the branch was minted in this module just above.
        absorb_debug_records(module, split_id, branch.slot_trusting_same_module());
        // BI->setDebugLoc(Loc);
        set_debug_location(self.module, branch.slot_trusting_same_module(), location);

        // New->replaceSuccessorsPhiUsesWith(this, New);
        new_block.replace_successors_phi_uses_with(self.id, new_id);
        Ok(new_block.retag_termination::<Terminated>())
    }

    /// Split this block in two before `before` and return the new block, which
    /// comes first.
    ///
    /// Ports `BasicBlock::splitBasicBlockBefore` (`lib/IR/BasicBlock.cpp`), the
    /// `Before = true` form of `splitBasicBlock`. A block named `name` is
    /// created right before this one and every instruction ahead of `before`
    /// moves into it. Each predecessor's terminator is retargeted to it, this
    /// block's phis name it in place of each predecessor, and it ends in a
    /// branch to this block carrying `before`'s location without its atom. Both
    /// blocks end in a terminator afterwards.
    ///
    /// The debug records attached ahead of `before` move into the new block,
    /// ahead of its branch: the splice ends at `before` with its tail bit
    /// clear, which carries them along for the branch to adopt.
    ///
    /// # Errors
    ///
    /// Every refusal happens before anything is created, moved or retargeted:
    ///
    /// - [`IrError::ForeignValueId`] if `before` belongs to another module.
    /// - [`IrError::InvalidOperation`] if this block has no terminator
    ///   (upstream's `assert(getTerminator())`, which a `Terminated` handle does
    ///   not prove on its own), if `before` is not one of its instructions, if
    ///   `before` is a phi and this block does not have exactly one predecessor
    ///   edge (upstream's `assert(!isa<PHINode>(*I) || getSinglePredecessor())`),
    ///   or if this block has no parent function or is missing from its block
    ///   list.
    pub fn split_before<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        before: &InstructionView<'ctx, B>,
        name: Name,
    ) -> IrResult<BasicBlock<'ctx, R, Terminated, B>>
    where
        Name: Into<String>,
    {
        // Boundary: the caller's split point, admitted before the block is
        // read or a new block created.
        let split_id = before.slot_in(self.module.id())?;
        // assert(getTerminator() && "Can't use splitBasicBlockBefore on degenerate BB!");
        if self.terminator_slot().is_none() {
            return Err(IrError::InvalidOperation {
                message: "Can't use splitBasicBlockBefore on degenerate BB!",
            });
        }
        // assert(I != InstList.end() && "Trying to get me to create degenerate
        // basic block!"): an `InstructionView` always names an instruction.
        let split_position = self.split_point_position(split_id)?;
        // assert((!isa<PHINode>(*I) || getSinglePredecessor()) &&
        //        "cannot split on multi incoming phis");
        let split_point_is_phi = matches!(
            &self.module.value_data(split_id).kind,
            ValueKindData::Instruction(data) if matches!(data.kind, InstructionKindData::Phi(_))
        );
        if split_point_is_phi && crate::cfg::single_predecessor(self.to_erased()).is_none() {
            return Err(IrError::InvalidOperation {
                message: "cannot split on multi incoming phis",
            });
        }
        let (parent_fn, this_position) = self.parent_and_position()?;
        // DebugLoc Loc = I->getDebugLoc(); if (Loc) Loc = Loc->getWithoutAtom();
        // Taken ahead of `BasicBlock::Create`, for the reason `split_at` gives.
        let location = split_point_location(module_token, before)?;
        let module = module_token.core_ref();

        // BasicBlock *New = BasicBlock::Create(getContext(), BBName, getParent(), this);
        let new_block = parent_fn.insert_basic_block_at_unchecked(this_position, name);
        // Internal: the block was minted in this module just above.
        let new_id = new_block.slot_trusting_same_module();

        // New->splice(New->end(), this, begin(), I);
        let prefix: Vec<ValueSlot> = self
            .data()
            .instructions
            .borrow_mut()
            .drain(..split_position)
            .collect();
        new_block
            .data()
            .instructions
            .borrow_mut()
            .extend(prefix.iter().copied());
        for id in &prefix {
            module.context().set_instruction_parent(*id, new_id);
        }

        // SmallVector<BasicBlock *, 4> Predecessors(predecessors(this));
        let predecessors = crate::cfg::block_predecessors(self.to_erased());
        for predecessor in predecessors {
            // Instruction *TI = Pred->getTerminator();
            let predecessor_block = BasicBlock::<'ctx, Dyn, Terminated, B>::from_parts(
                predecessor,
                self.module,
                self.ty,
            );
            // A predecessor is found through a terminator that uses this block,
            // so it has one unless instructions follow that terminator. Upstream
            // would dereference a null `TI` there; llvmkit leaves such a
            // malformed predecessor alone rather than crash.
            let Some(terminator) = predecessor_block.terminator_slot() else {
                continue;
            };
            // TI->replaceSuccessorWith(this, New);
            if let ValueKindData::Instruction(data) = &self.module.value_data(terminator).kind {
                crate::cfg::replace_successor_with(&data.kind, self.id, new_id);
            }
            crate::cfg::sync_block_uses(self.module, terminator, self.id);
            crate::cfg::sync_block_uses(self.module, terminator, new_id);
            // this->replacePhiUsesWith(Pred, New);
            self.replace_phi_uses_with(predecessor, new_id);
        }

        // BranchInst *BI = BranchInst::Create(this, New);
        let (_, branch) = IrBuilder::new_for::<R>(module_token)
            .position_at_end(new_block)
            .br_to_slot_unchecked(self.id);
        // The splice moved `I`'s records to the new block's end, and inserting
        // the branch there adopts them (`Instruction::insertBefore`).
        // Internal: the branch was minted in this module just above.
        absorb_debug_records(module, split_id, branch.slot_trusting_same_module());
        // BI->setDebugLoc(Loc);
        set_debug_location(self.module, branch.slot_trusting_same_module(), location);
        Ok(BasicBlock::from_parts(new_id, self.module, self.ty))
    }
}

/// `I->getStableDebugLoc()` followed by `Loc->getWithoutAtom()`, in stored form.
///
/// `Instruction::getStableDebugLoc` returns `getDebugLoc()`
/// (`lib/IR/Instruction.cpp`), which llvmkit keeps as the `!dbg` attachment.
/// `DILocation::getWithoutAtom` returns the location itself when its
/// `atomGroup` and `atomRank` are both zero, and otherwise the uniqued location
/// with the same `line`, `column`, `scope`, `inlinedAt` and `isImplicitCode`
/// and no atom. A `!dbg` attachment that is not a `DILocation` node has no atom
/// to drop and is kept as it is.
fn split_point_location<'ctx, B: ModuleBrand + 'ctx>(
    module_token: &'ctx Module<B, Unverified>,
    split_point: &InstructionView<'ctx, B>,
) -> IrResult<Option<MetadataId<StoredBrand>>> {
    let Some(location) = split_point.metadata().get(&MetadataAttachmentKind::Dbg) else {
        return Ok(None);
    };
    let location = match module_token.metadata_get(location) {
        Some(MetadataKind::Specialized(node))
            if node.kind() == SpecializedMetadataKind::DiLocation
                && node.fields().iter().any(is_nonzero_atom_field) =>
        {
            let without_atom = node
                .fields()
                .iter()
                .filter(|field| !is_atom_field(field))
                .cloned();
            module_token.metadata_specialized(
                SpecializedMetadataNode::new(SpecializedMetadataKind::DiLocation)
                    .with_fields(without_atom),
            )?
        }
        _ => location,
    };
    location.into_stored(module_token.id()).map(Some)
}

/// Whether `field` is one of `DILocation`'s two atom fields.
fn is_atom_field<B: ModuleBrand>(field: &MetadataField<B>) -> bool {
    matches!(field.name(), "atomGroup" | "atomRank")
}

/// Whether `field` is an atom field holding a non-zero value, the condition
/// under which `DILocation::getWithoutAtom` builds a new location.
fn is_nonzero_atom_field<B: ModuleBrand>(field: &MetadataField<B>) -> bool {
    is_atom_field(field) && !matches!(field.value(), MetadataFieldValue::Integer(0))
}

/// `BI->setDebugLoc(Loc)` on a branch created a moment earlier: the location
/// becomes its `!dbg` attachment, and an empty `Loc` leaves none.
fn set_debug_location<B: ModuleBrand>(
    module: ModuleRef<'_, B>,
    instruction: ValueSlot,
    location: Option<MetadataId<StoredBrand>>,
) {
    let Some(location) = location else {
        return;
    };
    if let ValueKindData::Instruction(data) = &module.value_data(instruction).kind {
        data.metadata
            .borrow_mut()
            .insert(MetadataAttachmentKind::Dbg, location);
    }
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    sealed::Sealed for BasicBlock<'ctx, R, Term, B, Params>
{
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    Typed<'ctx, B> for BasicBlock<'ctx, R, Term, B, Params>
{
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        self.to_erased().ty()
    }
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    HasName<'ctx, B> for BasicBlock<'ctx, R, Term, B, Params>
{
    #[inline]
    fn name(self) -> Option<String> {
        BasicBlock::name(&self)
    }
    #[inline]
    fn set_name<Name>(self, module_token: &'ctx Module<B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        BasicBlock::set_name(&self, module_token, name);
    }
    #[inline]
    fn clear_name(self, module_token: &'ctx Module<B, Unverified>) {
        BasicBlock::clear_name(&self, module_token);
    }
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    HasDebugLoc for BasicBlock<'ctx, R, Term, B, Params>
{
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.to_erased().debug_loc()
    }
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    From<BasicBlock<'ctx, R, Term, B, Params>> for Value<'ctx, B>
{
    #[inline]
    fn from(b: BasicBlock<'ctx, R, Term, B, Params>) -> Self {
        b.to_erased()
    }
}

// Erased narrowing: a `Value` that is a basic block lands in the
// parameter-erased [`BlockParamsDyn`] label. This is the non-leak point —
// a label recovered from an untyped `Value` legitimately carries no static
// parameter promise, so `BlockParamsDyn` is the correct marker.
impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>>
    for BasicBlockLabel<'ctx, Dyn, B, BlockParamsDyn>
{
    type Error = IrError;

    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        match v.data().kind {
            ValueKindData::BasicBlock(_) => Ok(Self {
                // Internal: a re-wrap that keeps `v`'s own module.
                id: v.slot_trusting_same_module(),
                module: v.module,
                ty: v.ty().slot_trusting_same_module(),
                _r: PhantomData,
                _params: PhantomData,
            }),
            _ => Err(IrError::ValueCategoryMismatch {
                expected: ValueCategoryLabel::BasicBlock,
                got: v.category().into(),
            }),
        }
    }
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    core::fmt::Display for BasicBlock<'ctx, R, Term, B, Params>
{
    /// Print the basic block including its label and instructions.
    /// Mirrors LLVM's `BasicBlock::print`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Without an enclosing function, build a one-block slot tracker
        // ad hoc.
        if let Some(parent_id) = self.parent_id() {
            let parent = FunctionValue::<'_, Dyn, B>::from_parts_unchecked(parent_id, self.module);
            let slots = SlotTracker::for_function(parent);
            // `bool IsEntryBlock = BB->getParent() && BB->isEntryBlock();`
            let is_entry_block = parent
                .entry_block()
                // Internal: `entry` is this block's own function's entry.
                .is_some_and(|entry| {
                    entry.slot_trusting_same_module() == self.slot_trusting_same_module()
                });
            crate::asm_writer::fmt_basic_block(f, self.as_dyn(), &slots, is_entry_block)
        } else {
            // Orphan block: no slot tracker, and `BB->getParent()` is null so
            // `IsEntryBlock` is false — upstream prints the label *and* a
            // predecessors comment, which for a detached block reads
            // `; No predecessors!`.
            let slots = SlotTracker::empty();
            crate::asm_writer::fmt_basic_block(f, self.as_dyn(), &slots, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Linkage;

    /// Accepts only the parameter-erased label form. Passing a recovered
    /// label here is a compile-time assertion that the erased `TryFrom`
    /// lands in [`BlockParamsDyn`] — the non-leak point of this slice.
    fn assert_dyn_params<'ctx, B: ModuleBrand + 'ctx>(
        _label: BasicBlockLabel<'ctx, Dyn, B, BlockParamsDyn>,
    ) {
    }

    #[test]
    fn erased_block_value_narrows_to_dyn_params_label() {
        let m = crate::module_new!("bp-slice1-narrow").expect("fresh module");
        let void_ty = m.void_type().as_type();
        let fn_ty = m.function_type_no_parameters(void_ty);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let bb = m.view(f).append_basic_block(&m, "entry");

        // A label recovered from an untyped `Value` carries no static
        // parameter promise, so it must land in the `BlockParamsDyn`
        // form (proved at compile time by `assert_dyn_params`).
        let v: Value<'_, _> = bb.to_erased();
        let recovered: BasicBlockLabel<'_, Dyn, _, BlockParamsDyn> = v
            .try_into()
            .expect("a basic-block value narrows to a label");
        assert_eq!(
            recovered.slot_trusting_same_module(),
            bb.slot_trusting_same_module()
        );
        assert_dyn_params(recovered);
    }

    #[test]
    fn label_to_erased_round_trips_to_dyn_params() {
        let m = crate::module_new!("bp-slice1-roundtrip").expect("fresh module");
        let void_ty = m.void_type().as_type();
        let fn_ty = m.function_type_no_parameters(void_ty);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let bb = m.view(f).append_basic_block(&m, "entry");
        let label = bb.label();

        let round: BasicBlockLabel<'_, Dyn, _, BlockParamsDyn> = label
            .to_erased()
            .try_into()
            .expect("a label's value round-trips to a label");
        assert_eq!(
            round.slot_trusting_same_module(),
            label.slot_trusting_same_module()
        );
        assert_dyn_params(round);
    }

    #[test]
    fn non_block_value_is_rejected() {
        let m = crate::module_new!("bp-slice1-reject").expect("fresh module");
        let v = m.i32_type().const_zero().as_erased();
        let narrowed: IrResult<BasicBlockLabel<'_, Dyn, _, BlockParamsDyn>> = v.try_into();
        assert!(
            narrowed.is_err(),
            "a non-block value must not narrow to a label"
        );
    }
}
