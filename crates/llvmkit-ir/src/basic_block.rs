//! Basic block (`label`-typed value) handle. Mirrors
//! `llvm/include/llvm/IR/BasicBlock.h` and `llvm/lib/IR/BasicBlock.cpp`.
//!
//! ## Storage shape
//!
//! A basic block lives in the value arena under the basic-block
//! category, with type [`LabelType`](crate::LabelType). It owns
//! a list of instruction value-ids, mutated through the [`IRBuilder`]
//! and other future helpers via interior mutability so the same
//! `&'ctx Module<'ctx>` borrow can be passed around freely.
//!
//! ## Return-marker propagation
//!
//! [`BasicBlock<'ctx, R>`] inherits its parent function's
//! [`ReturnMarker`]. When the IRBuilder positions itself inside a
//! block, the marker propagates to the builder so its `build_ret`
//! is statically typed.
//!
//! [`IRBuilder`]: crate::ir_builder::IRBuilder

use super::block_params::{BlockParams, BlockParamsDyn};
use super::block_state::{BlockTerminationState, Unterminated};
use super::function::FunctionValue;
use super::function_signature::{CallArgs, FunctionParamList};
use super::instruction::{InstructionKindData, InstructionView};
use super::ir_builder::constant_folder::ConstantFolder;
use super::ir_builder::{IRBuilder, Positioned};
use super::marker::{Dyn, ReturnMarker};
use super::module::{Brand, Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::r#type::TypeId;
use super::value::{HasDebugLoc, HasName, Typed, Value, ValueId, ValueKindData, sealed};
use super::{DebugLoc, IrError, IrResult, Type};
use core::cell::RefCell;
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
    pub(super) parent: RefCell<Option<ValueId>>,
    /// Linear list of instruction value ids in program order.
    pub(super) instructions: RefCell<Vec<ValueId>>,
}

impl BasicBlockData {
    /// Construct an empty block, optionally already attached to a
    /// parent function.
    pub(super) fn new(parent: Option<ValueId>) -> Self {
        Self {
            parent: RefCell::new(parent),
            instructions: RefCell::new(Vec::new()),
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
/// shape at the type level so a typed [`IRBuilder`]
/// positioned inside the block can keep its compile-time `build_ret`
/// invariant.
///
/// The `Term: BlockTerminationState` parameter (default [`Unterminated`])
/// distinguishes blocks that still accept appended instructions from
/// blocks whose terminator has been emitted. The termination marker is
/// enforced at [`crate::IRBuilder::position_at_end`], which only accepts
/// an [`Unterminated`] block; once a terminator-emitting `build_*`
/// consumes the builder, the returned handle names the same block with
/// `Term = Terminated`. `BasicBlock` is intentionally linear (`!Copy` /
/// `!Clone`) so retaining an old unterminated insertion capability cannot
/// reopen a terminated construction path. Use [`BasicBlockLabel`] for
/// copyable branch targets and PHI predecessors.
pub struct BasicBlock<
    'ctx,
    R: ReturnMarker,
    Term: BlockTerminationState = Unterminated,
    B: ModuleBrand = Brand<'ctx>,
    Params: BlockParams = BlockParamsDyn,
> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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

/// Copyable label reference to a basic block.
///
/// Unlike [`BasicBlock`], this is not an insertion capability: it can name a
/// branch target or PHI predecessor, but it cannot be passed to
/// [`IRBuilder::position_at_end`](crate::IRBuilder::position_at_end).
pub struct BasicBlockLabel<
    'ctx,
    R: ReturnMarker,
    B: ModuleBrand = Brand<'ctx>,
    Params: BlockParams = BlockParamsDyn,
> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
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
    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
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
pub trait IntoBasicBlockLabel<'ctx, R: ReturnMarker, B: ModuleBrand>:
    block_label_sealed::Sealed
{
    fn into_basic_block_label(self) -> BasicBlockLabel<'ctx, R, B>;
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand> block_label_sealed::Sealed
    for BasicBlockLabel<'ctx, R, B>
{
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> IntoBasicBlockLabel<'ctx, R, B>
    for BasicBlockLabel<'ctx, R, B>
{
    #[inline]
    fn into_basic_block_label(self) -> BasicBlockLabel<'ctx, R, B> {
        self
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
    fn into_basic_block_label(self) -> BasicBlockLabel<'ctx, R, B> {
        BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _params: PhantomData,
        }
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
    fn into_basic_block_label(self) -> BasicBlockLabel<'ctx, R, B> {
        // `IntoBasicBlockLabel` yields the parameter-erased label (its return
        // type pins `BlockParamsDyn`), so construct it directly rather than
        // through `label()`, which now threads this block's `Params`.
        BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _params: PhantomData,
        }
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand> block_label_sealed::Sealed
    for super::ssa_builder::SsaBlock<'ctx, R, B>
{
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> IntoBasicBlockLabel<'ctx, R, B>
    for super::ssa_builder::SsaBlock<'ctx, R, B>
{
    #[inline]
    fn into_basic_block_label(self) -> BasicBlockLabel<'ctx, R, B> {
        self.label()
    }
}

// --------------------------------------------------------------------------
// Typed control-flow edge bundle
// --------------------------------------------------------------------------

/// A typed control-flow edge: a branch target ([`BasicBlockLabel`]) stamped
/// with its parameter schema `Params`, paired with the block-argument values
/// that seed the target's leading head-phis on that edge.
///
/// Constructed by [`BasicBlockLabel::call`] (or, ergonomically,
/// [`BasicBlock::call`]) on a **typed** label/block — one produced by
/// [`IRBuilder::append_block_typed`](crate::IRBuilder::append_block_typed). The
/// argument tuple is checked against `Params` at **compile time** through the
/// [`CallArgs<Params>`](crate::CallArgs) bound on `.call()`: a wrong arity has
/// no `CallArgs` impl and a wrong-typed position fails its per-position
/// [`IntoCallArg`](crate::IntoCallArg) bound, so a mismatched edge does not
/// compile — the same machinery that guards typed `build_call`.
///
/// The arguments are lowered eagerly at construction (the typed label carries
/// its owning module), so `.call()` stays infallible and ergonomic. Any
/// *value-level* lowering failure — the fallibility [`CallArgs::lower`] carries,
/// e.g. a cross-module constant — is captured and re-surfaced when the bundle
/// is consumed by
/// [`IRBuilder::build_br_call`](crate::IRBuilder::build_br_call) /
/// [`IRBuilder::build_cond_br_call`](crate::IRBuilder::build_cond_br_call),
/// where a `?` is already expected.
pub struct BlockCall<
    'ctx,
    R: ReturnMarker,
    B: ModuleBrand = Brand<'ctx>,
    Params: BlockParams = BlockParamsDyn,
> {
    target: BasicBlockLabel<'ctx, R, B, Params>,
    /// The edge's block-arguments lowered to arena value-ids in declaration
    /// order, or the deferred lowering error to surface at build time. The
    /// arity and per-position types are already fixed by the compile-time
    /// [`CallArgs<Params>`](crate::CallArgs) bound, so this only carries the
    /// value-level fallibility of [`CallArgs::lower`].
    lowered: IrResult<Box<[ValueId]>>,
}

impl<'ctx, R, B, Params> BasicBlockLabel<'ctx, R, B, Params>
where
    R: ReturnMarker,
    B: ModuleBrand + 'ctx,
    Params: BlockParams + FunctionParamList,
{
    /// Bundle this typed branch target with the block-arguments that seed its
    /// leading head-phis, forming a [`BlockCall`] edge for
    /// [`IRBuilder::build_br_call`](crate::IRBuilder::build_br_call) /
    /// [`IRBuilder::build_cond_br_call`](crate::IRBuilder::build_cond_br_call).
    ///
    /// `args` must be an argument tuple matching this block's `Params` schema:
    /// the [`CallArgs<'ctx, Params, B>`](crate::CallArgs) bound makes a wrong
    /// arity or a wrong-typed position a **compile** error, reusing the exact
    /// machinery of a typed `build_call`. The values are lowered here (this
    /// label carries its module), so `.call()` is infallible; a value-level
    /// lowering failure is deferred into the returned [`BlockCall`] and surfaces
    /// when the branch builder consumes it.
    #[inline]
    pub fn call<A>(self, args: A) -> BlockCall<'ctx, R, B, Params>
    where
        A: CallArgs<'ctx, Params, B>,
    {
        let lowered = args.lower(self.module);
        BlockCall {
            target: self,
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
    pub fn call<A>(&self, args: A) -> BlockCall<'ctx, R, B, Params>
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
    /// exactly [`IRBuilder::at_end(bb)`](crate::IRBuilder::at_end) — the
    /// return marker `R` is inferred from the block, so no turbofish is
    /// needed. Reads better when `bb` is already in hand.
    #[inline]
    pub fn builder(self) -> IRBuilder<'ctx, 'ctx, B, ConstantFolder, Positioned, R> {
        IRBuilder::at_end(self)
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx, Params: BlockParams>
    BlockCall<'ctx, R, B, Params>
{
    /// Decompose into the parameter-erased target label and the edge's
    /// lowered-or-deferred block-arguments. Crate-internal: the typed branch
    /// builders consume the bundle here, then reuse the erased phi-seeding path.
    #[inline]
    pub(crate) fn into_parts(self) -> (BasicBlockLabel<'ctx, R, B>, IrResult<Box<[ValueId]>>) {
        (self.target.erase_params(), self.lowered)
    }
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    BasicBlock<'ctx, R, Term, B, Params>
{
    #[inline]
    pub(super) fn from_parts<M>(id: ValueId, module: M, ty: TypeId) -> Self
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

    /// Copyable label reference for branch targets and PHI predecessors.
    ///
    /// The returned label threads this block's `Params` marker through, so a
    /// typed block (`BasicBlock<…, Params>`) yields a typed label
    /// (`BasicBlockLabel<…, Params>`) that keeps the parameter promise; a
    /// parameter-erased block (the [`BlockParamsDyn`] default) yields the
    /// erased label form, unchanged.
    #[inline]
    pub fn label(&self) -> BasicBlockLabel<'ctx, R, B, Params> {
        BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _params: PhantomData,
        }
    }

    /// Widen to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
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
    /// [`crate::IRBuilder::append_block_typed`] stamps a freshly appended
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
        match &self.as_value().data().kind {
            ValueKindData::BasicBlock(b) => b,
            // The handle was produced by a constructor that pushed a
            // BasicBlock variant; the kind cannot have changed.
            _ => unreachable!("BasicBlock handle invariant: kind is BasicBlock"),
        }
    }

    /// Optional textual name. Mirrors `BasicBlock::getName`.
    #[inline]
    pub fn name(&self) -> Option<String> {
        self.as_value().name()
    }

    /// Set or clear the textual name.
    /// Set the textual name.
    #[inline]
    pub fn set_name<Name>(&self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        self.as_value().set_name(module_token, name);
    }

    /// Clear the textual name.
    #[inline]
    pub fn clear_name(&self, module_token: &Module<'ctx, B, Unverified>) {
        self.as_value().clear_name(module_token);
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
    pub(super) fn parent_id(&self) -> Option<ValueId> {
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
    /// `ValueId`s rather than full instruction handles so the caller
    /// can decide which view (raw operand-traversal vs typed
    /// `Instruction<'ctx>` handle) it wants.
    pub(crate) fn instruction_ids(&self) -> Vec<ValueId> {
        self.data().instructions.borrow().clone()
    }

    /// Iterate read-only instruction views in program order.
    pub fn instructions(&self) -> impl ExactSizeIterator<Item = InstructionView<'ctx, B>> {
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

    /// Successor block labels of this block's terminator, preserving duplicate CFG edges.
    /// Returns an empty list for unterminated blocks and terminators without successors.
    pub fn successors(&self) -> Vec<BasicBlockLabel<'ctx, Dyn, B>> {
        crate::cfg::block_successors(&self.as_dyn())
    }

    /// Append an instruction value-id to the block. Crate-internal:
    /// only the IR builder calls this.
    pub(super) fn append_instruction(&self, instr: ValueId) {
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
    pub(super) fn remove_instruction(&self, instr: ValueId) -> bool {
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
        instr: ValueId,
        before: ValueId,
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
    pub(super) fn insert_instruction_after(&self, instr: ValueId, after: ValueId) -> IrResult<()> {
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
    pub(crate) fn insert_instruction_at_phi_head(&self, id: ValueId) {
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
        module_token: &Module<'ctx, B, Unverified>,
        dest: BasicBlock<'ctx, R2, S2, B>,
    ) -> IrResult<()> {
        let _ = module_token;
        let module = self.module.module();
        let source_fn_id = self.parent_id();
        let dest_fn_id = dest.parent_id();
        let rehome_names = source_fn_id != dest_fn_id;
        let dest_id = dest.as_value().id;
        let drained: Vec<ValueId> = {
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
        module_token: &Module<'ctx, B, Unverified>,
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
        let split_id = before.as_value().id;
        let suffix: Vec<ValueId> = {
            let mut src = self.data().instructions.borrow_mut();
            let pos =
                src.iter()
                    .position(|id| *id == split_id)
                    .ok_or(IrError::InvalidOperation {
                        message: "split instruction is not in this block",
                    })?;
            src.split_off(pos)
        };
        let new_id = new_block.as_value().id;
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
        self.as_value().ty()
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
    fn set_name<Name>(self, module_token: &Module<'ctx, B, Unverified>, name: Name)
    where
        Name: Into<String>,
    {
        BasicBlock::set_name(&self, module_token, name);
    }
    #[inline]
    fn clear_name(self, module_token: &Module<'ctx, B, Unverified>) {
        BasicBlock::clear_name(&self, module_token);
    }
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    HasDebugLoc for BasicBlock<'ctx, R, Term, B, Params>
{
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx, Params: BlockParams>
    From<BasicBlock<'ctx, R, Term, B, Params>> for Value<'ctx, B>
{
    #[inline]
    fn from(b: BasicBlock<'ctx, R, Term, B, Params>) -> Self {
        b.as_value()
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
                expected: crate::error::ValueCategoryLabel::BasicBlock,
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
            let slots = crate::asm_writer::SlotTracker::for_function(parent);
            crate::asm_writer::fmt_basic_block(f, self.as_dyn(), &slots, true)
        } else {
            // Orphan block: no slot tracker.
            let slots = crate::asm_writer::SlotTracker::empty();
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
        Module::with_new("bp-slice1-narrow", |m| {
            let void_ty = m.void_type().as_type();
            let fn_ty = m.fn_type_no_params(void_ty, false);
            let f = m
                .add_function::<(), _>("f", fn_ty, Linkage::External)
                .unwrap();
            let bb = f.append_basic_block(&m, "entry");

            // A label recovered from an untyped `Value` carries no static
            // parameter promise, so it must land in the `BlockParamsDyn`
            // form (proved at compile time by `assert_dyn_params`).
            let v: Value<'_, _> = bb.as_value();
            let recovered: BasicBlockLabel<'_, Dyn, _, BlockParamsDyn> = v
                .try_into()
                .expect("a basic-block value narrows to a label");
            assert_eq!(recovered.as_value().id, bb.as_value().id);
            assert_dyn_params(recovered);
        });
    }

    #[test]
    fn label_as_value_round_trips_to_dyn_params() {
        Module::with_new("bp-slice1-roundtrip", |m| {
            let void_ty = m.void_type().as_type();
            let fn_ty = m.fn_type_no_params(void_ty, false);
            let f = m
                .add_function::<(), _>("f", fn_ty, Linkage::External)
                .unwrap();
            let bb = f.append_basic_block(&m, "entry");
            let label = bb.label();

            let round: BasicBlockLabel<'_, Dyn, _, BlockParamsDyn> = label
                .as_value()
                .try_into()
                .expect("a label's value round-trips to a label");
            assert_eq!(round.as_value().id, label.as_value().id);
            assert_dyn_params(round);
        });
    }

    #[test]
    fn non_block_value_is_rejected() {
        Module::with_new("bp-slice1-reject", |m| {
            let v = m.i32_type().const_zero().as_value();
            let narrowed: IrResult<BasicBlockLabel<'_, Dyn, _, BlockParamsDyn>> = v.try_into();
            assert!(
                narrowed.is_err(),
                "a non-block value must not narrow to a label"
            );
        });
    }
}
