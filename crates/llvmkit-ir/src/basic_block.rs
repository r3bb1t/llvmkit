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
use super::block_state::{BlockTerminationState, Unterminated};
use super::error::ValueCategoryLabel;
use super::function::FunctionValue;
use super::function_signature::{CallArgs, FunctionParamList};
use super::instruction::{InstructionKindData, InstructionView};
use super::ir_builder::constant_folder::ConstantFolder;
use super::ir_builder::{IrBuilder, Positioned};
use super::marker::{Dyn, ReturnMarker};
use super::module::{Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::r#type::TypeSlot;
use super::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueKindData, ValueSlot, sealed};
use super::value_id::BlockId;
use super::value_id::ViewIn;
use super::{DebugLoc, IrError, IrResult, Type};
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
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
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
    pub(super) id: ValueSlot,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeSlot,
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
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Opaque arena id of the underlying value (same id as
    /// [`to_erased`](Self::to_erased)).
    #[inline]
    pub fn slot(&self) -> ValueSlot {
        self.to_erased().id
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
        _module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>> {
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
        _module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>> {
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
        _module: ModuleRef<'ctx, B>,
    ) -> IrResult<BasicBlockLabel<'ctx, R, B>> {
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
        let lowered = args.lower(self.module);
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
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Opaque arena id of the underlying value (same id as
    /// [`to_erased`](Self::to_erased)).
    #[inline]
    pub fn slot(&self) -> ValueSlot {
        self.to_erased().id
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
    /// Returns an empty list for unterminated blocks and terminators without successors.
    pub fn successors(&self) -> Vec<BlockId<Dyn, B>> {
        crate::cfg::block_successors(&self.as_dyn())
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
/// to surface only at [`Module::verify`](crate::Module::verify)
/// (`PhiEmptyInReachableBlock`, or the shared `check_phi` count guard). The
/// caller must use the argument-carrying builder for that edge instead.
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
        let module = self.module.module();
        let source_fn_id = self.parent_id();
        let dest_fn_id = dest.parent_id();
        let rehome_names = source_fn_id != dest_fn_id;
        let dest_id = dest.slot();
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

    /// Split this block at `before`: every instruction at `before` and
    /// after is moved into a fresh block (named `name`) appended to the
    /// parent function. The original block keeps the prefix; the caller
    /// is responsible for adding a terminator that flows to the new
    /// block. Mirrors `BasicBlock::splitBasicBlock` in `lib/IR/BasicBlock.cpp`.
    pub fn split_at<Name>(
        self,
        module_token: &'ctx Module<B, Unverified>,
        before: &InstructionView<'ctx, B>,
        name: Name,
    ) -> IrResult<BasicBlock<'ctx, R, Unterminated, B>>
    where
        Name: Into<String>,
    {
        let module = module_token.core_ref();
        let parent_fn_id = match self.parent_id() {
            Some(id) => id,
            None => {
                return Err(IrError::InvalidOperation {
                    message: "cannot split an orphan basic block",
                });
            }
        };
        let parent_fn =
            FunctionValue::<'ctx, R, B>::from_parts_unchecked(parent_fn_id, self.module);
        let new_block = parent_fn.append_basic_block(module_token, name);
        let split_id = before.slot();
        let suffix: Vec<ValueSlot> = {
            let mut src = self.data().instructions.borrow_mut();
            let pos =
                src.iter()
                    .position(|id| *id == split_id)
                    .ok_or(IrError::InvalidOperation {
                        message: "split instruction is not in this block",
                    })?;
            src.split_off(pos)
        };
        let new_id = new_block.slot();
        {
            let mut dst = new_block.data().instructions.borrow_mut();
            dst.extend(suffix.iter().copied());
        }
        for id in &suffix {
            module.context().set_instruction_parent(*id, new_id);
        }
        Ok(new_block)
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
                id: v.id,
                module: v.module,
                ty: v.ty,
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
            crate::asm_writer::fmt_basic_block(f, self.as_dyn(), &slots, true)
        } else {
            // Orphan block: no slot tracker.
            let slots = SlotTracker::empty();
            crate::asm_writer::fmt_basic_block(f, self.as_dyn(), &slots, true)
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
        let fn_ty = m.fn_type_no_params(void_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let bb = m.view(f).append_basic_block(&m, "entry");

        // A label recovered from an untyped `Value` carries no static
        // parameter promise, so it must land in the `BlockParamsDyn`
        // form (proved at compile time by `assert_dyn_params`).
        let v: Value<'_, _> = bb.to_erased();
        let recovered: BasicBlockLabel<'_, Dyn, _, BlockParamsDyn> = v
            .try_into()
            .expect("a basic-block value narrows to a label");
        assert_eq!(recovered.slot(), bb.slot());
        assert_dyn_params(recovered);
    }

    #[test]
    fn label_to_erased_round_trips_to_dyn_params() {
        let m = crate::module_new!("bp-slice1-roundtrip").expect("fresh module");
        let void_ty = m.void_type().as_type();
        let fn_ty = m.fn_type_no_params(void_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External).unwrap();
        let bb = m.view(f).append_basic_block(&m, "entry");
        let label = bb.label();

        let round: BasicBlockLabel<'_, Dyn, _, BlockParamsDyn> = label
            .to_erased()
            .try_into()
            .expect("a label's value round-trips to a label");
        assert_eq!(round.slot(), label.slot());
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
