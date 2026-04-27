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

use crate::instr_types::{
    BinaryOpData, BranchInstData, BranchKind, CastOpData, CmpInstData, FCmpInstData, PhiData,
    ReturnOpData, UnreachableInstData,
};
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
#[derive(Debug)]
pub(crate) struct InstructionData {
    pub(crate) parent: core::cell::Cell<ValueId>,
    pub(crate) kind: InstructionKindData,
}

impl InstructionData {
    pub(crate) fn new(parent: ValueId, kind: InstructionKindData) -> Self {
        Self {
            parent: core::cell::Cell::new(parent),
            kind,
        }
    }
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
    UDiv(BinaryOpData),
    SDiv(BinaryOpData),
    URem(BinaryOpData),
    SRem(BinaryOpData),
    Shl(BinaryOpData),
    LShr(BinaryOpData),
    AShr(BinaryOpData),
    And(BinaryOpData),
    Or(BinaryOpData),
    Xor(BinaryOpData),
    FAdd(BinaryOpData),
    FSub(BinaryOpData),
    FMul(BinaryOpData),
    FDiv(BinaryOpData),
    FRem(BinaryOpData),
    FCmp(FCmpInstData),
    Alloca(crate::instr_types::AllocaInstData),
    Load(crate::instr_types::LoadInstData),
    Store(crate::instr_types::StoreInstData),
    Gep(crate::instr_types::GepInstData),
    Call(crate::instr_types::CallInstData),
    Select(crate::instr_types::SelectInstData),
    Cast(CastOpData),
    ICmp(CmpInstData),
    Phi(PhiData),
    Ret(ReturnOpData),
    Br(BranchInstData),
    Unreachable(UnreachableInstData),
}

impl InstructionKindData {
    /// Operand `ValueId`s in declaration order. Mirrors
    /// `User::operands`. Block references in branch terminators and
    /// phi incoming pairs are NOT SSA operands at this layer; they
    /// live in the per-variant payload and are surfaced via
    /// per-opcode handles.
    pub(crate) fn operand_ids(&self) -> Vec<ValueId> {
        match self {
            Self::Add(b)
            | Self::Sub(b)
            | Self::Mul(b)
            | Self::UDiv(b)
            | Self::SDiv(b)
            | Self::URem(b)
            | Self::SRem(b)
            | Self::Shl(b)
            | Self::LShr(b)
            | Self::AShr(b)
            | Self::And(b)
            | Self::Or(b)
            | Self::Xor(b)
            | Self::FAdd(b)
            | Self::FSub(b)
            | Self::FMul(b)
            | Self::FDiv(b)
            | Self::FRem(b) => vec![b.lhs.get(), b.rhs.get()],
            Self::Cast(c) => vec![c.src.get()],
            Self::Alloca(a) => a.num_elements.get().into_iter().collect(),
            Self::Load(l) => vec![l.ptr.get()],
            Self::Store(s) => vec![s.value.get(), s.ptr.get()],
            Self::Gep(g) => {
                let mut v = vec![g.ptr.get()];
                v.extend(g.indices.iter().map(|c| c.get()));
                v
            }
            Self::Call(c) => {
                let mut v = vec![c.callee.get()];
                v.extend(c.args.iter().map(|c| c.get()));
                v
            }
            Self::Select(s) => vec![s.cond.get(), s.true_val.get(), s.false_val.get()],
            Self::ICmp(c) => vec![c.lhs.get(), c.rhs.get()],
            Self::FCmp(c) => vec![c.lhs.get(), c.rhs.get()],
            Self::Phi(p) => p.incoming.borrow().iter().map(|(v, _)| v.get()).collect(),
            Self::Ret(r) => r.value.get().into_iter().collect(),
            Self::Br(b) => match &b.kind {
                BranchKind::Unconditional(_) => Vec::new(),
                BranchKind::Conditional { cond, .. } => vec![cond.get()],
            },
            Self::Unreachable(_) => Vec::new(),
        }
    }

    pub(crate) fn is_terminator(&self) -> bool {
        matches!(self, Self::Ret(_) | Self::Br(_) | Self::Unreachable(_))
    }
}

// --------------------------------------------------------------------------
// Lifecycle typestate (T1 / Doctrine D1, D2)
// --------------------------------------------------------------------------

/// Sealed marker traits for [`Instruction`] lifecycle states.
///
/// Two states exist:
/// - [`state::Attached`]: the instruction lives in some basic block and
///   participates in the IR. Operand reads, type queries, RAUW, and erase
///   are all valid.
/// - [`state::Detached`]: the instruction was removed from its parent but
///   not destroyed. Operand reads still work; the instruction can be
///   reattached via [`Instruction::insert_before`] / [`Instruction::insert_after`] /
///   [`Instruction::append_to`].
///
/// Mirrors `Instruction::eraseFromParent` / `removeFromParent` /
/// `insertBefore` / `insertAfter` in `lib/IR/Instruction.cpp`. The state
/// is enforced at compile time through a `PhantomData<S>` marker so
/// use-after-erase and double-erase are *compile* errors rather than
/// runtime no-ops.
pub mod state {
    pub(crate) mod sealed {
        pub trait Sealed {}
    }
    /// Sealed marker trait for instruction lifecycle states.
    pub trait InstructionState: sealed::Sealed {}

    /// The instruction is currently attached to a basic block.
    #[derive(Debug)]
    pub struct Attached(());
    /// The instruction has been removed from its parent block but not
    /// destroyed; its operand wiring is still intact.
    #[derive(Debug)]
    pub struct Detached(());

    impl sealed::Sealed for Attached {}
    impl sealed::Sealed for Detached {}
    impl InstructionState for Attached {}
    impl InstructionState for Detached {}
}

// --------------------------------------------------------------------------
// Public handles
// --------------------------------------------------------------------------

/// Type-erased instruction handle. Mirrors `Instruction *` in C++ ---
/// every concrete instruction (`Add`, `Mul`, `Ret`, ...) widens to this
/// shape. Use [`Instruction::kind`] /
/// [`Instruction::terminator_kind`] for read-only inspection.
///
/// The `S: InstructionState` parameter pins the lifecycle state at compile
/// time (Doctrine D1). Defaults to [`state::Attached`] so existing call
/// sites that build via the IRBuilder do not need to spell the state.
/// `Instruction` is intentionally **`!Copy` and `!Clone`** (Doctrine D2):
/// methods that consume the lifecycle (`erase_from_parent`,
/// `detach_from_parent`, `replace_all_uses_with`) take `self` by value,
/// and the compiler then prevents use-after-erase. Per-opcode handles
/// (`AddInst`, ...) remain `Copy`; reach for `as_instruction()` on a per-opcode handle
/// when you need a single-use lifecycle handle from a per-opcode view.
pub struct Instruction<'ctx, S: state::InstructionState = state::Attached> {
    pub(crate) id: ValueId,
    pub(crate) module: ModuleRef<'ctx>,
    pub(crate) ty: TypeId,
    pub(crate) _state: core::marker::PhantomData<S>,
}

// Hand-rolled trait impls so that consumers do not have to spell `S`
// bounds at every match position, and so that `Instruction` is
// definitively neither `Clone` nor `Copy`.
impl<'ctx, S: state::InstructionState> core::fmt::Debug for Instruction<'ctx, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Instruction")
            .field("id", &self.id)
            .field("ty", &self.ty)
            .finish()
    }
}
impl<'ctx, S: state::InstructionState> PartialEq for Instruction<'ctx, S> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.module == other.module && self.ty == other.ty
    }
}
impl<'ctx, S: state::InstructionState> Eq for Instruction<'ctx, S> {}
impl<'ctx, S: state::InstructionState> core::hash::Hash for Instruction<'ctx, S> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.module.hash(h);
        self.ty.hash(h);
    }
}

impl<'ctx, S: state::InstructionState> Instruction<'ctx, S> {
    /// Widen to the erased [`Value`] handle. Read-only access; safe in
    /// either lifecycle state.
    #[inline]
    pub fn as_value(&self) -> Value<'ctx> {
        Value {
            id: self.id,
            module: self.module,
            ty: self.ty,
        }
    }

    /// Borrow the storage payload.
    fn data(&self) -> &'ctx InstructionData {
        match &self.as_value().data().kind {
            ValueKindData::Instruction(i) => i,
            _ => unreachable!("Instruction handle invariant: kind is Instruction"),
        }
    }

    /// Owning module reference.
    #[inline]
    pub fn module(&self) -> &'ctx Module<'ctx> {
        self.module.module()
    }

    /// Result type. `void` for terminators and stores.
    #[inline]
    pub fn ty(&self) -> Type<'ctx> {
        Type::new(self.ty, self.module.module())
    }

    /// Optional textual name. Mirrors `Value::getName`.
    #[inline]
    pub fn name(&self) -> Option<String> {
        self.as_value().name()
    }

    /// Set or clear the textual name.
    #[inline]
    pub fn set_name(&self, name: Option<&str>) {
        self.as_value().set_name(name);
    }

    /// Read-only opcode discriminator for non-terminator opcodes.
    /// Returns `None` if the instruction is a terminator (use
    /// [`Self::terminator_kind`] for those).
    pub fn kind(&self) -> Option<InstructionKind<'ctx>> {
        let module = self.module.module();
        let id = self.id;
        let ty = self.ty;
        match &self.data().kind {
            InstructionKindData::Add(_) => Some(InstructionKind::Add(
                crate::instructions::AddInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Sub(_) => Some(InstructionKind::Sub(
                crate::instructions::SubInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Mul(_) => Some(InstructionKind::Mul(
                crate::instructions::MulInst::from_raw(id, module, ty),
            )),
            InstructionKindData::UDiv(_) => Some(InstructionKind::UDiv(
                crate::instructions::UDivInst::from_raw(id, module, ty),
            )),
            InstructionKindData::SDiv(_) => Some(InstructionKind::SDiv(
                crate::instructions::SDivInst::from_raw(id, module, ty),
            )),
            InstructionKindData::URem(_) => Some(InstructionKind::URem(
                crate::instructions::URemInst::from_raw(id, module, ty),
            )),
            InstructionKindData::SRem(_) => Some(InstructionKind::SRem(
                crate::instructions::SRemInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Shl(_) => Some(InstructionKind::Shl(
                crate::instructions::ShlInst::from_raw(id, module, ty),
            )),
            InstructionKindData::LShr(_) => Some(InstructionKind::LShr(
                crate::instructions::LShrInst::from_raw(id, module, ty),
            )),
            InstructionKindData::AShr(_) => Some(InstructionKind::AShr(
                crate::instructions::AShrInst::from_raw(id, module, ty),
            )),
            InstructionKindData::And(_) => Some(InstructionKind::And(
                crate::instructions::AndInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Or(_) => Some(InstructionKind::Or(
                crate::instructions::OrInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Xor(_) => Some(InstructionKind::Xor(
                crate::instructions::XorInst::from_raw(id, module, ty),
            )),
            InstructionKindData::FAdd(_) => Some(InstructionKind::FAdd(
                crate::instructions::FAddInst::from_raw(id, module, ty),
            )),
            InstructionKindData::FSub(_) => Some(InstructionKind::FSub(
                crate::instructions::FSubInst::from_raw(id, module, ty),
            )),
            InstructionKindData::FMul(_) => Some(InstructionKind::FMul(
                crate::instructions::FMulInst::from_raw(id, module, ty),
            )),
            InstructionKindData::FDiv(_) => Some(InstructionKind::FDiv(
                crate::instructions::FDivInst::from_raw(id, module, ty),
            )),
            InstructionKindData::FRem(_) => Some(InstructionKind::FRem(
                crate::instructions::FRemInst::from_raw(id, module, ty),
            )),
            InstructionKindData::FCmp(_) => Some(InstructionKind::FCmp(
                crate::instructions::FCmpInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Alloca(_) => Some(InstructionKind::Alloca(
                crate::instructions::AllocaInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Load(_) => Some(InstructionKind::Load(
                crate::instructions::LoadInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Call(_) => Some(InstructionKind::Call(
                crate::instructions::CallInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Select(_) => Some(InstructionKind::Select(
                crate::instructions::SelectInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Store(_) => Some(InstructionKind::Store(
                crate::instructions::StoreInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Gep(_) => Some(InstructionKind::Gep(
                crate::instructions::GepInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Cast(_) => Some(InstructionKind::Cast(
                crate::instructions::CastInst::from_raw(id, module, ty),
            )),
            InstructionKindData::ICmp(_) => Some(InstructionKind::ICmp(
                crate::instructions::ICmpInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Phi(_) => Some(InstructionKind::Phi(
                crate::instructions::PhiInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Ret(_)
            | InstructionKindData::Br(_)
            | InstructionKindData::Unreachable(_) => None,
        }
    }

    /// Read-only opcode discriminator for terminators.
    pub fn terminator_kind(&self) -> Option<TerminatorKind<'ctx>> {
        let module = self.module.module();
        let id = self.id;
        let ty = self.ty;
        match &self.data().kind {
            InstructionKindData::Ret(_) => Some(TerminatorKind::Ret(
                crate::instructions::RetInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Br(_) => Some(TerminatorKind::Br(
                crate::instructions::BranchInst::from_raw(id, module, ty),
            )),
            InstructionKindData::Unreachable(_) => Some(TerminatorKind::Unreachable(
                crate::instructions::UnreachableInst::from_raw(id, module, ty),
            )),
            _ => None,
        }
    }

    /// `true` if this instruction is a terminator (`ret`, `br`, ...).
    #[inline]
    pub fn is_terminator(&self) -> bool {
        self.data().kind.is_terminator()
    }

    /// Operand value-ids in declaration order. Crate-internal helper
    /// used by the use-list machinery.
    pub(crate) fn operand_ids(&self) -> Vec<ValueId> {
        self.data().kind.operand_ids()
    }
}

impl<'ctx> Instruction<'ctx, state::Attached> {
    /// Construct an attached handle from raw parts. Crate-internal:
    /// only the IR builder hands these out, and only after the value-id
    /// has been pushed onto the parent block's instruction list.
    #[inline]
    pub(crate) fn from_parts(id: ValueId, module: &'ctx Module<'ctx>) -> Self {
        let data = module.context().value_data(id);
        Self {
            id,
            module: ModuleRef::new(module),
            ty: data.ty,
            _state: core::marker::PhantomData,
        }
    }

    /// Containing basic block, in its runtime-checked form.
    pub fn parent(&self) -> crate::basic_block::BasicBlock<'ctx, crate::marker::Dyn> {
        let parent = self.data().parent.get();
        let module = self.module.module();
        let label_ty = module.label_type().as_type().id();
        crate::basic_block::BasicBlock::from_parts(parent, module, label_ty)
    }

    // ---- Mutation API (Phase G / T1) ----

    /// Replace every existing use of this instruction's result with
    /// `replacement`. Walks the reverse use-list, rewriting each
    /// operand `Cell<ValueId>` slot in place, and migrates the entries
    /// onto `replacement`'s use-list. Mirrors `Value::replaceAllUsesWith`
    /// in `lib/IR/Value.cpp`.
    ///
    /// Consumes `self`: the binding is single-use after RAUW. The
    /// underlying instruction's slot is otherwise untouched (it is not
    /// erased); call [`Self::erase_from_parent`] separately if needed.
    /// Mirrors LLVM's two-step pattern
    /// `I->replaceAllUsesWith(V); I->eraseFromParent();`.
    pub fn replace_all_uses_with<V: IsValue<'ctx>>(self, replacement: V) -> IrResult<()> {
        let new_value = replacement.as_value();
        if new_value.module().id() != self.module.module().id() {
            return Err(IrError::ForeignValue);
        }
        if new_value.id == self.id {
            // `self.replaceAllUsesWith(self)` is a no-op upstream; mirror.
            return Ok(());
        }
        if new_value.ty != self.ty {
            return Err(IrError::TypeMismatch {
                expected: self.ty().kind_label(),
                got: new_value.ty().kind_label(),
            });
        }
        let module = self.module.module();
        let self_id = self.id;
        let new_id = new_value.id;
        // Snapshot the user list under a borrow so we can release it
        // before mutating each user's operand slots.
        let user_ids: Vec<ValueId> = module
            .context()
            .value_data(self_id)
            .use_list
            .borrow()
            .clone();
        // For each user, rewrite every operand slot whose Cell currently
        // points at `self_id` so it points at `new_id`. We touch the cells
        // through `rewrite_operand_cells` to keep the operand-walker
        // exhaustive.
        for user_id in &user_ids {
            let user_data = module.context().value_data(*user_id);
            if let ValueKindData::Instruction(idata) = &user_data.kind {
                rewrite_operand_cells(&idata.kind, self_id, new_id);
            }
        }
        // Migrate use-list entries: drain ours, push onto replacement.
        {
            let mut self_uses = module.context().value_data(self_id).use_list.borrow_mut();
            self_uses.clear();
        }
        {
            let mut new_uses = module.context().value_data(new_id).use_list.borrow_mut();
            new_uses.extend(user_ids);
        }
        Ok(())
    }

    /// Remove this instruction from its parent block and deregister
    /// it from each operand's reverse use-list. Mirrors
    /// `Instruction::eraseFromParent` in `lib/IR/Instruction.cpp`.
    ///
    /// Consumes `self`: use-after-erase is a *compile* error.
    pub fn erase_from_parent(self) {
        let self_id = self.id;
        let module = self.module.module();
        deregister_operand_uses(self_id, &self.data().kind, module);
        let parent_block_id = self.data().parent.get();
        let bb = crate::basic_block::BasicBlock::<crate::marker::Dyn>::from_parts(
            parent_block_id,
            module,
            module.label_type().as_type().id(),
        );
        bb.remove_instruction(self_id);
    }

    /// Remove this instruction from its parent block but leave its
    /// operand wiring (and reverse use-list registrations) intact so
    /// it can be reattached elsewhere via [`Instruction::insert_before`] /
    /// [`Instruction::insert_after`] / [`Instruction::append_to`].
    /// Mirrors `Instruction::removeFromParent` in `lib/IR/Instruction.cpp`.
    pub fn detach_from_parent(self) -> Instruction<'ctx, state::Detached> {
        let module = self.module.module();
        let self_id = self.id;
        let parent_block_id = self.data().parent.get();
        let bb = crate::basic_block::BasicBlock::<crate::marker::Dyn>::from_parts(
            parent_block_id,
            module,
            module.label_type().as_type().id(),
        );
        bb.remove_instruction(self_id);
        // Clear the parent pointer so iteration over orphan instructions
        // can detect detachment without consulting the block list.
        Instruction {
            id: self.id,
            module: self.module,
            ty: self.ty,
            _state: core::marker::PhantomData,
        }
    }

    /// Move this instruction so it appears immediately before `other`
    /// in `other`'s parent block. Mirrors `Instruction::moveBefore` in
    /// `lib/IR/Instruction.cpp`.
    pub fn move_before(self, other: &Instruction<'ctx, state::Attached>) -> IrResult<()> {
        if self.module.module().id() != other.module.module().id() {
            return Err(IrError::ForeignValue);
        }
        let module = self.module.module();
        let self_id = self.id;
        let other_id = other.id;
        // Remove from current parent.
        let cur_parent = self.data().parent.get();
        let cur_bb = crate::basic_block::BasicBlock::<crate::marker::Dyn>::from_parts(
            cur_parent,
            module,
            module.label_type().as_type().id(),
        );
        cur_bb.remove_instruction(self_id);
        // Insert before other in other's parent.
        let new_parent = other.data().parent.get();
        let new_bb = crate::basic_block::BasicBlock::<crate::marker::Dyn>::from_parts(
            new_parent,
            module,
            module.label_type().as_type().id(),
        );
        new_bb.insert_instruction_before(self_id, other_id)?;
        update_instruction_parent(module, self_id, new_parent);
        Ok(())
    }

    /// Move this instruction so it appears immediately after `other` in
    /// `other`'s parent block. Mirrors `Instruction::moveAfter`.
    pub fn move_after(self, other: &Instruction<'ctx, state::Attached>) -> IrResult<()> {
        if self.module.module().id() != other.module.module().id() {
            return Err(IrError::ForeignValue);
        }
        let module = self.module.module();
        let self_id = self.id;
        let other_id = other.id;
        let cur_parent = self.data().parent.get();
        let cur_bb = crate::basic_block::BasicBlock::<crate::marker::Dyn>::from_parts(
            cur_parent,
            module,
            module.label_type().as_type().id(),
        );
        cur_bb.remove_instruction(self_id);
        let new_parent = other.data().parent.get();
        let new_bb = crate::basic_block::BasicBlock::<crate::marker::Dyn>::from_parts(
            new_parent,
            module,
            module.label_type().as_type().id(),
        );
        new_bb.insert_instruction_after(self_id, other_id)?;
        update_instruction_parent(module, self_id, new_parent);
        Ok(())
    }
}

impl<'ctx> Instruction<'ctx, state::Detached> {
    /// Insert this detached instruction immediately before `other` in
    /// `other`'s parent block. Mirrors `Instruction::insertBefore` in
    /// `lib/IR/Instruction.cpp`.
    pub fn insert_before(
        self,
        other: &Instruction<'ctx, state::Attached>,
    ) -> IrResult<Instruction<'ctx, state::Attached>> {
        if self.module.module().id() != other.module.module().id() {
            return Err(IrError::ForeignValue);
        }
        let module = self.module.module();
        let parent_id = other.data().parent.get();
        let bb = crate::basic_block::BasicBlock::<crate::marker::Dyn>::from_parts(
            parent_id,
            module,
            module.label_type().as_type().id(),
        );
        bb.insert_instruction_before(self.id, other.id)?;
        update_instruction_parent(module, self.id, parent_id);
        Ok(Instruction::from_parts(self.id, module))
    }

    /// Insert this detached instruction immediately after `other` in
    /// `other`'s parent block. Mirrors `Instruction::insertAfter`.
    pub fn insert_after(
        self,
        other: &Instruction<'ctx, state::Attached>,
    ) -> IrResult<Instruction<'ctx, state::Attached>> {
        if self.module.module().id() != other.module.module().id() {
            return Err(IrError::ForeignValue);
        }
        let module = self.module.module();
        let parent_id = other.data().parent.get();
        let bb = crate::basic_block::BasicBlock::<crate::marker::Dyn>::from_parts(
            parent_id,
            module,
            module.label_type().as_type().id(),
        );
        bb.insert_instruction_after(self.id, other.id)?;
        update_instruction_parent(module, self.id, parent_id);
        Ok(Instruction::from_parts(self.id, module))
    }

    /// Append this detached instruction to the end of `block`'s
    /// instruction list. Mirrors `Instruction::insertInto(BB, BB->end())`.
    pub fn append_to<R: crate::marker::ReturnMarker>(
        self,
        block: &crate::basic_block::BasicBlock<'ctx, R>,
    ) -> IrResult<Instruction<'ctx, state::Attached>> {
        if self.module.module().id() != block.as_value().module().id() {
            return Err(IrError::ForeignValue);
        }
        let module = self.module.module();
        let parent_id = block.as_value().id;
        block.as_dyn().append_instruction(self.id);
        update_instruction_parent(module, self.id, parent_id);
        Ok(Instruction::from_parts(self.id, module))
    }

    /// Discard a detached instruction without inserting it. Removes the
    /// instruction's id from the use-lists of its operands and leaves
    /// the value-arena slot tombstoned (still occupied for id-stability,
    /// but unreferenced by any block). Mirrors `Instruction::deleteValue`
    /// in `lib/IR/Instruction.cpp`.
    pub fn drop_detached(self) {
        let self_id = self.id;
        let module = self.module.module();
        deregister_operand_uses(self_id, &self.data().kind, module);
    }
}

// --------------------------------------------------------------------------
// Crate-private operand-walker helpers
// --------------------------------------------------------------------------

/// For each operand `Cell<ValueId>` in `kind` whose current value is
/// `from`, replace it with `to`. The match arms are exhaustive so
/// future opcodes will fail to compile until they are added here.
fn rewrite_operand_cells(kind: &InstructionKindData, from: ValueId, to: ValueId) {
    use crate::instr_types::BranchKind;
    let swap = |c: &core::cell::Cell<ValueId>| {
        if c.get() == from {
            c.set(to);
        }
    };
    let swap_opt = |c: &core::cell::Cell<Option<ValueId>>| {
        if c.get() == Some(from) {
            c.set(Some(to));
        }
    };
    match kind {
        InstructionKindData::Add(b)
        | InstructionKindData::Sub(b)
        | InstructionKindData::Mul(b)
        | InstructionKindData::UDiv(b)
        | InstructionKindData::SDiv(b)
        | InstructionKindData::URem(b)
        | InstructionKindData::SRem(b)
        | InstructionKindData::Shl(b)
        | InstructionKindData::LShr(b)
        | InstructionKindData::AShr(b)
        | InstructionKindData::And(b)
        | InstructionKindData::Or(b)
        | InstructionKindData::Xor(b)
        | InstructionKindData::FAdd(b)
        | InstructionKindData::FSub(b)
        | InstructionKindData::FMul(b)
        | InstructionKindData::FDiv(b)
        | InstructionKindData::FRem(b) => {
            swap(&b.lhs);
            swap(&b.rhs);
        }
        InstructionKindData::Cast(c) => swap(&c.src),
        InstructionKindData::Alloca(a) => swap_opt(&a.num_elements),
        InstructionKindData::Load(l) => swap(&l.ptr),
        InstructionKindData::Store(s) => {
            swap(&s.value);
            swap(&s.ptr);
        }
        InstructionKindData::Gep(g) => {
            swap(&g.ptr);
            for idx in g.indices.iter() {
                swap(idx);
            }
        }
        InstructionKindData::Call(c) => {
            swap(&c.callee);
            for arg in c.args.iter() {
                swap(arg);
            }
        }
        InstructionKindData::Select(s) => {
            swap(&s.cond);
            swap(&s.true_val);
            swap(&s.false_val);
        }
        InstructionKindData::ICmp(c) => {
            swap(&c.lhs);
            swap(&c.rhs);
        }
        InstructionKindData::FCmp(c) => {
            swap(&c.lhs);
            swap(&c.rhs);
        }
        InstructionKindData::Phi(p) => {
            for entry in p.incoming.borrow().iter() {
                swap(&entry.0);
            }
        }
        InstructionKindData::Ret(r) => swap_opt(&r.value),
        InstructionKindData::Br(b) => match &b.kind {
            BranchKind::Unconditional(_) => {}
            BranchKind::Conditional { cond, .. } => swap(cond),
        },
        InstructionKindData::Unreachable(_) => {}
    }
}

/// Remove `inst_id` from the reverse use-list of every operand it
/// references. Used by both `erase_from_parent` and `drop_detached`.
fn deregister_operand_uses(inst_id: ValueId, kind: &InstructionKindData, module: &Module<'_>) {
    use std::collections::HashMap;
    let mut occurrences: HashMap<ValueId, usize> = HashMap::new();
    for op_id in kind.operand_ids() {
        *occurrences.entry(op_id).or_insert(0) += 1;
    }
    for (op_id, n) in occurrences {
        let mut ul = module.context().value_data(op_id).use_list.borrow_mut();
        for _ in 0..n {
            if let Some(pos) = ul.iter().position(|id| *id == inst_id) {
                ul.remove(pos);
            }
        }
    }
}

fn update_instruction_parent(module: &Module<'_>, inst_id: ValueId, new_parent: ValueId) {
    let data = module.context().value_data(inst_id);
    if let ValueKindData::Instruction(_) = &data.kind {
        // The parent field is plain; mutate via a dedicated helper on
        // the value arena. Until we add cell-based parent tracking, we
        // model parent updates by deferring to a helper on the context.
        module.context().set_instruction_parent(inst_id, new_parent);
    }
}

impl<'ctx, S: state::InstructionState> sealed::Sealed for Instruction<'ctx, S> {}
// `IsValue` requires `Copy` (every other implementer is a thin Copy
// handle). `Instruction<state::Attached>` is intentionally `!Copy`
// (Doctrine D2: linear-typed handle for irreversible operations like
// `erase_from_parent`). Use [`Instruction::as_value`] (inherent,
// `&self`) when an erased view is needed.
impl<'ctx> Typed<'ctx> for Instruction<'ctx, state::Attached> {
    #[inline]
    fn ty(self) -> Type<'ctx> {
        Instruction::ty(&self)
    }
}
impl<'ctx> HasName<'ctx> for Instruction<'ctx, state::Attached> {
    #[inline]
    fn name(self) -> Option<String> {
        Instruction::name(&self)
    }
    #[inline]
    fn set_name(self, name: Option<&str>) {
        Instruction::set_name(&self, name);
    }
}
impl HasDebugLoc for Instruction<'_, state::Attached> {
    #[inline]
    fn debug_loc(self) -> Option<DebugLoc> {
        self.as_value().debug_loc()
    }
}

impl<'ctx> User<'ctx> for Instruction<'ctx, state::Attached> {
    fn operand_count(self) -> u32 {
        let count = Instruction::operand_ids(&self).len();
        u32::try_from(count)
            .unwrap_or_else(|_| unreachable!("instruction has more than u32::MAX operands"))
    }

    fn operand(self, index: u32) -> Option<Value<'ctx>> {
        let slot = usize::try_from(index).unwrap_or_else(|_| unreachable!("u32 fits in usize"));
        let id = *Instruction::operand_ids(&self).get(slot)?;
        let module = self.module.module();
        let data = module.context().value_data(id);
        Some(Value::from_parts(id, module, data.ty))
    }

    fn operand_use(self, index: u32) -> Option<Use<'ctx>> {
        let user = Instruction::as_value(&self);
        let v = self.operand(index)?;
        Some(Use::new(user, v, index))
    }
}

impl<'ctx> From<Instruction<'ctx, state::Attached>> for Value<'ctx> {
    #[inline]
    fn from(i: Instruction<'ctx, state::Attached>) -> Self {
        Instruction::as_value(&i)
    }
}

impl<'ctx> TryFrom<Value<'ctx>> for Instruction<'ctx, state::Attached> {
    type Error = IrError;
    fn try_from(v: Value<'ctx>) -> IrResult<Self> {
        match v.data().kind {
            ValueKindData::Instruction(_) => Ok(Self {
                id: v.id,
                module: v.module,
                ty: v.ty,
                _state: core::marker::PhantomData,
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
    UDiv(crate::instructions::UDivInst<'ctx>),
    SDiv(crate::instructions::SDivInst<'ctx>),
    URem(crate::instructions::URemInst<'ctx>),
    SRem(crate::instructions::SRemInst<'ctx>),
    Shl(crate::instructions::ShlInst<'ctx>),
    LShr(crate::instructions::LShrInst<'ctx>),
    AShr(crate::instructions::AShrInst<'ctx>),
    And(crate::instructions::AndInst<'ctx>),
    Or(crate::instructions::OrInst<'ctx>),
    Xor(crate::instructions::XorInst<'ctx>),
    FAdd(crate::instructions::FAddInst<'ctx>),
    FSub(crate::instructions::FSubInst<'ctx>),
    FMul(crate::instructions::FMulInst<'ctx>),
    FDiv(crate::instructions::FDivInst<'ctx>),
    FRem(crate::instructions::FRemInst<'ctx>),
    FCmp(crate::instructions::FCmpInst<'ctx>),
    Alloca(crate::instructions::AllocaInst<'ctx>),
    Load(crate::instructions::LoadInst<'ctx>),
    Store(crate::instructions::StoreInst<'ctx>),
    Gep(crate::instructions::GepInst<'ctx>),
    Call(crate::instructions::CallInst<'ctx>),
    Select(crate::instructions::SelectInst<'ctx>),
    Cast(crate::instructions::CastInst<'ctx>),
    ICmp(crate::instructions::ICmpInst<'ctx>),
    Phi(crate::instructions::PhiInst<'ctx, crate::int_width::IntDyn>),
}

/// Read-only opcode discriminator for terminators.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum TerminatorKind<'ctx> {
    Ret(crate::instructions::RetInst<'ctx>),
    Br(crate::instructions::BranchInst<'ctx>),
    Unreachable(crate::instructions::UnreachableInst<'ctx>),
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
        kind: ValueKindData::Instruction(InstructionData::new(parent_bb, kind)),
        use_list: RefCell::new(Vec::new()),
    }
}

impl<'ctx, S: state::InstructionState> core::fmt::Display for Instruction<'ctx, S> {
    /// Print a single instruction line. Mirrors LLVM's `Value::print`
    /// for instruction-category values.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let module = self.module.module();
        let parent_id = self.data().parent.get();
        let label_ty = module.label_type().as_type().id();
        let parent = crate::basic_block::BasicBlock::<'ctx, crate::marker::Dyn>::from_parts(
            parent_id, module, label_ty,
        );
        let slots = match parent.parent_id() {
            Some(parent_fn_id) => {
                let parent_fn =
                    crate::function::FunctionValue::<'_, crate::marker::Dyn>::from_parts_unchecked(
                        parent_fn_id,
                        module,
                    );
                crate::asm_writer::SlotTracker::for_function(parent_fn)
            }
            None => crate::asm_writer::SlotTracker::empty(),
        };
        let attached_view = Instruction::<'ctx, state::Attached>::from_parts(self.id, module);
        crate::asm_writer::fmt_instruction(f, &attached_view, &slots)
    }
}
