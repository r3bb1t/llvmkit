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
//! ## Return-marker propagation (Phase A3)
//!
//! [`BasicBlock<'ctx, R>`] inherits its parent function's
//! [`ReturnMarker`]. When the IRBuilder positions itself inside a
//! block, the marker propagates to the builder so its `build_ret`
//! is statically typed.
//!
//! [`IRBuilder`]: crate::ir_builder::IRBuilder

use core::cell::RefCell;
use core::marker::PhantomData;

use crate::module::{Module, ModuleRef};
use crate::return_marker::{RDyn, ReturnMarker};
use crate::r#type::TypeId;
use crate::value::{HasDebugLoc, HasName, IsValue, Typed, Value, ValueId, ValueKindData, sealed};
use crate::{DebugLoc, IrError, IrResult, Type};

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload stored under
/// [`ValueKindData::BasicBlock`](crate::value::ValueKindData::BasicBlock).
#[derive(Debug)]
pub(crate) struct BasicBlockData {
    /// Owning function. `None` for an orphan block (no function yet
    /// attached). Mirrors LLVM's `BasicBlock::Parent`.
    pub(crate) parent: RefCell<Option<ValueId>>,
    /// Linear list of instruction value ids in program order.
    pub(crate) instructions: RefCell<Vec<ValueId>>,
}

impl BasicBlockData {
    /// Construct an empty block, optionally already attached to a
    /// parent function.
    pub(crate) fn new(parent: Option<ValueId>) -> Self {
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
pub struct BasicBlock<'ctx, R: ReturnMarker> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    pub(crate) _r: PhantomData<R>,
}

impl<'ctx, R: ReturnMarker> Clone for BasicBlock<'ctx, R> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<'ctx, R: ReturnMarker> Copy for BasicBlock<'ctx, R> {}
impl<'ctx, R: ReturnMarker> PartialEq for BasicBlock<'ctx, R> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, R: ReturnMarker> Eq for BasicBlock<'ctx, R> {}
impl<'ctx, R: ReturnMarker> core::hash::Hash for BasicBlock<'ctx, R> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}
impl<'ctx, R: ReturnMarker> core::fmt::Debug for BasicBlock<'ctx, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BasicBlock")
            .field("id", &self.id)
            .field("ty", &self.ty)
            .finish()
    }
}

impl<'ctx, R: ReturnMarker> BasicBlock<'ctx, R> {
    #[inline]
    pub(crate) fn from_parts(id: ValueId, module: &'ctx Module<'ctx>, ty: TypeId) -> Self {
        Self {
            id,
            module: ModuleRef::new(module),
            ty,
            _r: PhantomData,
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

    /// Erase the return-shape marker, producing the runtime-checked
    /// [`RDyn`] form. Useful for storage and printing helpers that
    /// shouldn't have to be generic in `R`.
    #[inline]
    pub fn as_dyn(self) -> BasicBlock<'ctx, RDyn> {
        BasicBlock {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _r: PhantomData,
        }
    }

    /// Borrow the storage payload.
    fn data(self) -> &'ctx BasicBlockData {
        match &self.as_value().data().kind {
            ValueKindData::BasicBlock(b) => b,
            // The handle was produced by a constructor that pushed a
            // BasicBlock variant; the kind cannot have changed.
            _ => unreachable!("BasicBlock handle invariant: kind is BasicBlock"),
        }
    }

    /// Optional textual name. Mirrors `BasicBlock::getName`.
    #[inline]
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }

    /// Set or clear the textual name.
    #[inline]
    pub fn set_name(self, name: Option<&str>) {
        self.as_value().set_name(name);
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    /// Owning function value-id, or `None` if the block is an orphan.
    pub(crate) fn parent_id(self) -> Option<ValueId> {
        *self.data().parent.borrow()
    }

    /// Parent function as a runtime-checked [`FunctionValue<RDyn>`](crate::function::FunctionValue).
    /// `None` if the block is an orphan (no parent attached). The
    /// caller can narrow back to its static `R` via
    /// [`crate::FunctionValue::as_dyn`] / `try_into` if needed.
    pub fn parent_function(self) -> Option<crate::function::FunctionValue<'ctx, RDyn>> {
        let id = self.parent_id()?;
        Some(
            crate::function::FunctionValue::<'ctx, RDyn>::from_parts_unchecked(
                id,
                self.module.module(),
            ),
        )
    }

    /// Iterate the instruction value-ids in program order. Returns
    /// `ValueId`s rather than full instruction handles so the caller
    /// can decide which view (raw operand-traversal vs typed
    /// `Instruction<'ctx>` handle) it wants.
    pub(crate) fn instruction_ids(self) -> Vec<ValueId> {
        self.data().instructions.borrow().clone()
    }

    /// Iterate instruction handles in program order.
    pub fn instructions(
        self,
    ) -> impl ExactSizeIterator<Item = crate::instruction::Instruction<'ctx>> {
        let module = self.module.module();
        let ids = self.instruction_ids();
        ids.into_iter()
            .map(move |id| crate::instruction::Instruction::from_parts(id, module))
    }

    /// `true` if the block currently has no instructions.
    pub fn is_empty(self) -> bool {
        self.data().instructions.borrow().is_empty()
    }

    /// Last instruction (the terminator if the block is well-formed),
    /// or `None` for an empty block.
    pub fn terminator(self) -> Option<crate::instruction::Instruction<'ctx>> {
        let last = *self.data().instructions.borrow().last()?;
        Some(crate::instruction::Instruction::from_parts(
            last,
            self.module.module(),
        ))
    }

    /// Append an instruction value-id to the block. Crate-internal:
    /// only the IR builder calls this.
    pub(crate) fn append_instruction(self, instr: ValueId) {
        self.data().instructions.borrow_mut().push(instr);
    }
}

impl<'ctx, R: ReturnMarker> sealed::Sealed for BasicBlock<'ctx, R> {}
impl<'ctx, R: ReturnMarker> IsValue<'ctx> for BasicBlock<'ctx, R> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        BasicBlock::as_value(self)
    }
}
impl<'ctx, R: ReturnMarker> Typed<'ctx> for BasicBlock<'ctx, R> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        self.as_value().ty()
    }
}
impl<'ctx, R: ReturnMarker> HasName<'ctx> for BasicBlock<'ctx, R> {
    #[inline]
    fn name(self) -> Option<String> {
        BasicBlock::name(self)
    }
    #[inline]
    fn set_name(self, name: Option<&str>) {
        BasicBlock::set_name(self, name);
    }
}
impl<R: ReturnMarker> HasDebugLoc for BasicBlock<'_, R> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx, R: ReturnMarker> From<BasicBlock<'ctx, R>> for Value<'ctx> {
    #[inline]
    fn from(b: BasicBlock<'ctx, R>) -> Self {
        b.as_value()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for BasicBlock<'ctx, RDyn> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
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

impl<'ctx, R: ReturnMarker> core::fmt::Display for BasicBlock<'ctx, R> {
    /// Print the basic block including its label and instructions.
    /// Mirrors LLVM's `BasicBlock::print`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Without an enclosing function, build a one-block slot tracker
        // ad hoc.
        if let Some(parent_id) = self.parent_id() {
            let parent = crate::function::FunctionValue::<'_, RDyn>::from_parts_unchecked(
                parent_id,
                self.module.module(),
            );
            let slots = crate::asm_writer::SlotTracker::for_function(parent);
            crate::asm_writer::fmt_basic_block(f, self.as_dyn(), &slots, true)
        } else {
            // Orphan block: no slot tracker.
            let slots = crate::asm_writer::SlotTracker::empty();
            crate::asm_writer::fmt_basic_block(f, self.as_dyn(), &slots, true)
        }
    }
}
