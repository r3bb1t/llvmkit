//! Generic [`Instruction`] handle plus the analysis-mode opcode
//! enums. Mirrors `llvm/include/llvm/IR/Instruction.h` and
//! `llvm/lib/IR/Instruction.cpp`.
//!
//! ## What's shipped
//!
//! Phase E minimum: `add`, `sub`, `mul`, and `ret`. Everything else
//! (`Br`, `CondBr`, `Switch`, `Phi`, `Load`, `Store`, `Alloca`,
//! `Call`, `GEP`, casts, comparisons, ...) is scheduled per the
//! foundation plan as its own focused session.
//!
//! ## Two-tier discriminator
//!
//! - [`InstructionKind`] — every non-terminator opcode the slice
//!   needs.
//! - [`TerminatorKind`] — the terminator subset (`Ret` today).
//!
//! Both are `#[non_exhaustive]` so future sessions can add variants
//! without breaking external consumers. Inside the crate, every match
//! is exhaustive — `#[non_exhaustive]` only constrains *external*
//! pattern matching.

use crate::instr_types::{BinaryOpData, CastOpData, ReturnOpData};
use crate::module::{Module, ModuleRef};
use crate::r#type::TypeId;
use crate::r#use::Use;
use crate::user::User;
use crate::value::{
    HasDebugLoc, HasName, IsValue, Typed, Value, ValueData, ValueId, ValueKindData, sealed,
};
use crate::{DebugLoc, IrError, IrResult, Type};

// --------------------------------------------------------------------------
// Storage payload
// --------------------------------------------------------------------------

/// Lifetime-free payload stored under
/// [`ValueKindData::Instruction`](crate::value::ValueKindData::Instruction).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct InstructionData {
    pub(crate) parent: ValueId,
    pub(crate) kind: InstructionKindData,
}

/// Lifetime-free per-opcode payload variant. Mirrors the opcode set
/// in `Instruction::OtherOps` / `BinaryOps` / `TermOps`. Variants are
/// added incrementally; non-terminator and terminator opcodes share
/// the enum so the storage type stays uniform across kinds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum InstructionKindData {
    Add(BinaryOpData),
    Sub(BinaryOpData),
    Mul(BinaryOpData),
    Cast(CastOpData),
    Ret(ReturnOpData),
}

impl InstructionKindData {
    /// Operand `ValueId`s in declaration order. Mirrors
    /// `User::operands`.
    pub(crate) fn operand_ids(&self) -> impl ExactSizeIterator<Item = ValueId> + '_ {
        let v: Vec<ValueId> = match self {
            Self::Add(b) | Self::Sub(b) | Self::Mul(b) => vec![b.lhs, b.rhs],
            Self::Cast(c) => vec![c.src],
            Self::Ret(r) => r.value.into_iter().collect(),
        };
        v.into_iter()
    }

    pub(crate) fn is_terminator(&self) -> bool {
        matches!(self, Self::Ret(_))
    }
}

// --------------------------------------------------------------------------
// Public handles
// --------------------------------------------------------------------------

/// Type-erased instruction handle. Mirrors `Instruction *` in C++ —
/// every concrete instruction (Add, Mul, Ret, ...) widens to this
/// shape. Use [`Instruction::kind`] / [`Instruction::terminator_kind`]
/// for read-only inspection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Instruction<'ctx> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
}

impl<'ctx> Instruction<'ctx> {
    /// Construct from raw parts. Crate-internal: only the IR builder
    /// hands these out.
    #[inline]
    pub(crate) fn from_parts(id: ValueId, module: &'ctx Module<'ctx>) -> Self {
        let data = module.context().value_data(id);
        Self {
            id,
            module: ModuleRef::new(module),
            ty: data.ty,
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

    /// Borrow the storage payload.
    fn data(self) -> &'ctx InstructionData {
        match &self.as_value().data().kind {
            ValueKindData::Instruction(i) => i,
            _ => unreachable!("Instruction handle invariant: kind is Instruction"),
        }
    }

    /// Owning module reference.
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    /// Result type. `void` for terminators and stores.
    #[inline]
    pub fn ty(self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }

    /// Containing basic block, in its runtime-checked form.
    pub fn parent(self) -> crate::basic_block::BasicBlock<'ctx, crate::return_marker::RDyn> {
        let parent = self.data().parent;
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        crate::basic_block::BasicBlock::from_parts(parent, module, label_ty)
    }

    /// Optional textual name. Mirrors `Value::getName`.
    #[inline]
    pub fn name(self) -> Option<String> {
        self.as_value().name()
    }

    /// Set or clear the textual name.
    #[inline]
    pub fn set_name(self, name: Option<&str>) {
        self.as_value().set_name(name);
    }

    /// Read-only opcode discriminator for non-terminator opcodes.
    /// Returns `None` if the instruction is a terminator (use
    /// [`Self::terminator_kind`] for those).
    pub fn kind(self) -> Option<InstructionKind<'ctx>> {
        match &self.data().kind {
            InstructionKindData::Add(_) => Some(InstructionKind::Add(
                crate::instructions::AddInst::wrap(self),
            )),
            InstructionKindData::Sub(_) => Some(InstructionKind::Sub(
                crate::instructions::SubInst::wrap(self),
            )),
            InstructionKindData::Mul(_) => Some(InstructionKind::Mul(
                crate::instructions::MulInst::wrap(self),
            )),
            InstructionKindData::Cast(_) => Some(InstructionKind::Cast(
                crate::instructions::CastInst::wrap(self),
            )),
            InstructionKindData::Ret(_) => None,
        }
    }

    /// Read-only opcode discriminator for terminators.
    pub fn terminator_kind(self) -> Option<TerminatorKind<'ctx>> {
        match &self.data().kind {
            InstructionKindData::Ret(_) => Some(TerminatorKind::Ret(
                crate::instructions::RetInst::wrap(self),
            )),
            _ => None,
        }
    }

    /// `true` if this instruction is a terminator (`ret`, `br`, ...).
    #[inline]
    pub fn is_terminator(self) -> bool {
        self.data().kind.is_terminator()
    }
}

impl<'ctx> sealed::Sealed for Instruction<'ctx> {}
impl<'ctx> IsValue<'ctx> for Instruction<'ctx> {
    #[inline]
    fn as_value(self) -> Value<'ctx> {
        Instruction::as_value(self)
    }
}
impl<'ctx> Typed<'ctx> for Instruction<'ctx> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Instruction::ty(self)
    }
}
impl<'ctx> HasName<'ctx> for Instruction<'ctx> {
    #[inline]
    fn name(self) -> Option<String> {
        Instruction::name(self)
    }
    #[inline]
    fn set_name(self, name: Option<&str>) {
        Instruction::set_name(self, name);
    }
}
impl HasDebugLoc for Instruction<'_> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx> User<'ctx> for Instruction<'ctx> {
    fn operand_count(self) -> u32 {
        let count = self.data().kind.operand_ids().len();
        u32::try_from(count)
            .unwrap_or_else(|_| unreachable!("instruction has more than u32::MAX operands"))
    }

    fn operand(self, index: u32) -> Option<Value<'ctx>> {
        let slot = usize::try_from(index).unwrap_or_else(|_| unreachable!("u32 fits in usize"));
        let id = self.data().kind.operand_ids().nth(slot)?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, module, data.ty))
    }

    fn operand_use(self, index: u32) -> Option<Use<'ctx>> {
        let v = self.operand(index)?;
        Some(Use::new(self.as_value(), v, index))
    }
}

impl<'ctx> From<Instruction<'ctx>> for Value<'ctx> {
    #[inline]
    fn from(i: Instruction<'ctx>) -> Self {
        i.as_value()
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for Instruction<'ctx> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
        match v.data().kind {
            ValueKindData::Instruction(_) => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
            }),
            _ => Err(IrError::ValueCategoryMismatch {
                expected: crate::error::ValueCategoryLabel::Instruction,
                got: v.category().into(),
            }),
        }
    }
}

// --------------------------------------------------------------------------
// Analysis enums
// --------------------------------------------------------------------------

/// Read-only opcode discriminator for non-terminator opcodes. Variants
/// are added incrementally per the foundation plan.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum InstructionKind<'ctx> {
    Add(crate::instructions::AddInst<'ctx>),
    Sub(crate::instructions::SubInst<'ctx>),
    Mul(crate::instructions::MulInst<'ctx>),
    Cast(crate::instructions::CastInst<'ctx>),
}

/// Read-only opcode discriminator for terminators.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum TerminatorKind<'ctx> {
    Ret(crate::instructions::RetInst<'ctx>),
}

/// Crate-internal helper: create a `ValueData` for an instruction with
/// the given parent block and kind payload.
pub(crate) fn build_instruction_value(
    ty: TypeId,
    parent_bb: ValueId,
    kind: InstructionKindData,
    name: Option<String>,
) -> ValueData {
    use core::cell::RefCell;
    ValueData {
        ty,
        name: RefCell::new(name),
        debug_loc: None,
        kind: ValueKindData::Instruction(InstructionData {
            parent: parent_bb,
            kind,
        }),
    }
}

impl<'ctx> core::fmt::Display for Instruction<'ctx> {
    /// Print a single instruction line. Mirrors LLVM's `Value::print`
    /// for instruction-category values.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let parent = self.parent();
        let slots = match parent.parent_id() {
            Some(parent_fn_id) => {
                let parent_fn =
                    crate::function::FunctionValue::<'_, crate::return_marker::RDyn>::from_parts_unchecked(
                        parent_fn_id,
                        self.module.module(),
                    );
                crate::asm_writer::SlotTracker::for_function(parent_fn)
            }
            None => crate::asm_writer::SlotTracker::empty(),
        };
        crate::asm_writer::fmt_instruction(f, *self, &slots)
    }
}
