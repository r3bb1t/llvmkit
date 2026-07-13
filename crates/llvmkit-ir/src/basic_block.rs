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

use super::block_state::{BlockTerminationState, Unterminated};
use super::function::FunctionValue;
use super::instruction::{InstructionKindData, InstructionView};
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
/// shape at the type level so a typed [`IRBuilder`](crate::IRBuilder)
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
> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    pub(super) _r: PhantomData<R>,
    pub(super) _term: PhantomData<Term>,
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand> PartialEq
    for BasicBlock<'ctx, R, Term, B>
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand> Eq
    for BasicBlock<'ctx, R, Term, B>
{
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand> core::hash::Hash
    for BasicBlock<'ctx, R, Term, B>
{
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand> core::fmt::Debug
    for BasicBlock<'ctx, R, Term, B>
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BasicBlockLabel<'ctx, R: ReturnMarker, B: ModuleBrand = Brand<'ctx>> {
    pub(super) id: ValueId,
    pub(super) module: ModuleRef<'ctx, B>,
    pub(super) ty: TypeId,
    pub(super) _r: PhantomData<R>,
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> BasicBlockLabel<'ctx, R, B> {
    /// Widen this copyable label reference to the erased [`Value`] handle.
    #[inline]
    pub fn as_value(&self) -> Value<'ctx, B> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
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

impl<'ctx, R, Term, B> block_label_sealed::Sealed for BasicBlock<'ctx, R, Term, B>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
}

impl<'ctx, R, Term, B> IntoBasicBlockLabel<'ctx, R, B> for BasicBlock<'ctx, R, Term, B>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
    #[inline]
    fn into_basic_block_label(self) -> BasicBlockLabel<'ctx, R, B> {
        BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
        }
    }
}

impl<'ctx, R, Term, B> block_label_sealed::Sealed for &BasicBlock<'ctx, R, Term, B>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
}

impl<'ctx, R, Term, B> IntoBasicBlockLabel<'ctx, R, B> for &BasicBlock<'ctx, R, Term, B>
where
    R: ReturnMarker,
    Term: BlockTerminationState,
    B: ModuleBrand + 'ctx,
{
    #[inline]
    fn into_basic_block_label(self) -> BasicBlockLabel<'ctx, R, B> {
        self.label()
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

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx>
    BasicBlock<'ctx, R, Term, B>
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
        }
    }

    /// Copyable label reference for branch targets and PHI predecessors.
    #[inline]
    pub fn label(&self) -> BasicBlockLabel<'ctx, R, B> {
        BasicBlockLabel {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
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

    /// Erase the return-shape marker, producing the runtime-checked
    /// [`Dyn`] form. Crate-internal only: this duplicates the handle for
    /// storage and printing helpers, so public code should use [`label`](Self::label)
    /// when it needs a copyable non-insertion reference.
    #[inline]
    pub(crate) fn as_dyn(&self) -> BasicBlock<'ctx, Dyn, Term, B> {
        BasicBlock {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
            _term: PhantomData,
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
        crate::cfg::block_successors(self)
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

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx>
    BasicBlock<'ctx, R, Term, B>
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

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx> sealed::Sealed
    for BasicBlock<'ctx, R, Term, B>
{
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx> Typed<'ctx, B>
    for BasicBlock<'ctx, R, Term, B>
{
    #[inline]
    fn ty(self) -> Type<'ctx, B> {
        self.as_value().ty()
    }
}
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx> HasName<'ctx, B>
    for BasicBlock<'ctx, R, Term, B>
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
impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx> HasDebugLoc
    for BasicBlock<'ctx, R, Term, B>
{
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx>
    From<BasicBlock<'ctx, R, Term, B>> for Value<'ctx, B>
{
    #[inline]
    fn from(b: BasicBlock<'ctx, R, Term, B>) -> Self {
        b.as_value()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> TryFrom<Value<'ctx, B>> for BasicBlockLabel<'ctx, Dyn, B> {
    type Error = IrError;

    fn try_from(v: Value<'ctx, B>) -> IrResult<Self> {
        match v.data().kind {
            ValueKindData::BasicBlock(_) => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
                _r: PhantomData,
            }),
            _ => Err(IrError::ValueCategoryMismatch {
                expected: crate::error::ValueCategoryLabel::BasicBlock,
                got: v.category().into(),
            }),
        }
    }
}

impl<'ctx, R: ReturnMarker, Term: BlockTerminationState, B: ModuleBrand + 'ctx> core::fmt::Display
    for BasicBlock<'ctx, R, Term, B>
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
