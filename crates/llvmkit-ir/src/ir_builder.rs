//! IR builder. Mirrors `llvm/include/llvm/IR/IRBuilder.h` and
//! `llvm/lib/IR/IRBuilder.cpp`.
//!
//! ## Type-state
//!
//! The builder carries two state-marker generics:
//!
//! - `S` ([`Unpositioned`] / [`Positioned`]) — distinguishes "I have an
//!   insertion point" from "I do not". The `build_*` methods are only
//!   available on the [`Positioned`] state.
//! - `R: ReturnMarker` — the parent function's return shape. The
//!   typed `build_ret` methods are dispatched on `R` so calling
//!   `build_ret(int_value)` against a `void`-returning builder is a
//!   compile-time error rather than a runtime
//!   [`IrError::ReturnTypeMismatch`].
//!
//! Mirrors the inkwell `Builder<'ctx>` shape but with the additional
//! invariants that an unpositioned builder has no `build_*` API at
//! all and a `void`-returning builder cannot accidentally emit a
//! value-bearing return.
//!
//! ## What's shipped
//!
//! The builder routes side-effect-free arithmetic, cast, compare, GEP,
//! select, vector, and aggregate construction through
//! [`folder::IRBuilderFolder`] before materialising an instruction.
//! [`constant_folder::ConstantFolder`] is the default strategy, with
//! [`no_folder::NoFolder`] available for callers that want instructions
//! unconditionally.
//!
//! Other `build_*` methods land as their consumers do; the trait /
//! method names are stable.

pub mod constant_folder;
pub mod folder;
pub mod no_folder;

use core::marker::PhantomData;

use super::align::{Align, MaybeAlign};
use super::atomic_ordering::AtomicOrdering;
use super::basic_block::{BasicBlock, IntoBasicBlockLabel};
use super::block_state::{Terminated, Unterminated};
use super::calling_conv::CallingConv;
use super::cmp_predicate::CmpPredicate;
use super::constant::{Constant, ConstantExprFlags, ConstantExprOpcode};
use super::constant_fold;
use super::constants::ConstantExprOptions;
use super::derived_types::{FloatType, FunctionType, IntType, PointerType, StructType};
use super::error::{IrError, IrResult, TypeKindLabel};
use super::float_kind::{FloatDyn, FloatKind, FloatWiderThan, IntoFloatValue};
use super::fmf::FastMathFlags;
use super::function::FunctionValue;
use super::function_signature::{
    CallArgs, FunctionParamList, FunctionReturn, FunctionSignature, TypedFunctionValue,
    TypedVarArgsFunctionValue,
};
use super::gep_no_wrap_flags::GepNoWrapFlags;
use super::inline_asm::InlineAsm;
use super::instr_types::FNegInstData;
use super::instr_types::{
    BinaryOpData, BinaryOpcode, CallAttributeData, CastOpData, CastOpcode, LoadInstData,
    OverflowFlags, POISON_MASK_ELEM, ReturnOpData, StoreInstData, UnaryOpcode,
};
use super::instruction::{
    Instruction, InstructionKind, InstructionKindData, InstructionView, build_instruction_value,
    state::Attached,
};
use super::instructions::{
    AtomicCmpXchgInst, AtomicRMWInst, CallBrInst, CallInst, CatchPadInst, CatchSwitchInst,
    CleanupPadInst, FpPhiInst, FreezeInst, IndirectBrInst, InvokeInst, LandingPadInst, PhiInst,
    PointerPhiInst, StoreInst, SwitchInst, TypedCallInst, VAArgInst,
};
use super::int_width::{IntDyn, IntWidth, IntoIntValue};
use super::intrinsic_inst::IntrinsicInst;
use super::intrinsics::{BinaryIntrinsic, IntrinsicDescriptor, IntrinsicId};
use super::ir_builder::constant_folder::ConstantFolder;
use super::ir_builder::folder::IRBuilderFolder;
use super::marker::{Dyn, Ptr, ReturnMarker};
use super::module::{Brand, Module, ModuleBrand, ModuleCore, ModuleRef, ModuleView, Unverified};
use super::phi_state::Open as PhiOpen;
use super::struct_body_state::StructBodyDyn;
use super::struct_schema::{FieldOf, IntoIrField, IrField, StructFieldAt, StructSchema};
use super::sync_scope::SyncScope;
use super::term_open_state::Open;
use super::r#type::{IrType, MAX_INT_BITS, MIN_INT_BITS, Type, TypeData, TypeId};
use super::typed_pointer_value::TypedPointerValue;
use super::value::{
    FloatValue, IntValue, IntoPointerValue, IsValue, PointerValue, Value, ValueId, ValueKindData,
    ValueUse, VectorValue,
};

/// Pair returned by terminator builders: the terminated insertion block and
/// the emitted terminator instruction.
pub type TerminatedBlockInst<'ctx, R, B = Brand<'ctx>> = (
    BasicBlock<'ctx, R, Terminated, B>,
    Instruction<'ctx, Attached, B>,
);

/// Pair returned by `switch` builders before the case list is closed.
pub type TerminatedBlockSwitch<'ctx, R, B = Brand<'ctx>> = (
    BasicBlock<'ctx, R, Terminated, B>,
    SwitchInst<'ctx, Open, B>,
);

/// Pair returned by `indirectbr` builders before destination insertion closes.
pub type TerminatedBlockIndirectBr<'ctx, R, B = Brand<'ctx>> = (
    BasicBlock<'ctx, R, Terminated, B>,
    IndirectBrInst<'ctx, Open, B>,
);

/// Pair returned by `invoke` builders.
pub type TerminatedBlockInvoke<'ctx, R, Ret, B = Brand<'ctx>> =
    (BasicBlock<'ctx, R, Terminated, B>, InvokeInst<'ctx, Ret, B>);

/// Pair returned by the TYPED `invoke` builders
/// ([`IRBuilder::build_invoke`] / [`IRBuilder::build_invoke_with_config`]).
/// `R` is the parent function's return marker (drives the terminated
/// block's typestate); `Ret` is the invoke instruction's own schema —
/// the inner [`InvokeInst`] is tagged with `Ret::Marker`, derived from
/// the callee, matching [`TerminatedBlockInvoke`]'s shape one level up.
pub type TerminatedBlockTypedInvoke<'ctx, R, Ret, B = Brand<'ctx>> = (
    BasicBlock<'ctx, R, Terminated, B>,
    InvokeInst<'ctx, <Ret as FunctionReturn>::Marker, B>,
);

/// Pair returned by `catchswitch` builders before handler insertion closes.
pub type TerminatedBlockCatchSwitch<'ctx, R, B = Brand<'ctx>> = (
    BasicBlock<'ctx, R, Terminated, B>,
    CatchSwitchInst<'ctx, Open, B>,
);

/// Pair returned by `ret void` when the builder's return marker is statically
/// void.
pub type VoidReturnInst<'ctx, B = Brand<'ctx>> = TerminatedBlockInst<'ctx, (), B>;

/// Type-state marker: the builder has no insertion point. None of the
/// `build_*` methods are reachable in this state.
#[derive(Debug, Clone, Copy)]
pub struct Unpositioned;

/// Type-state marker: the builder has an insertion point and
/// can produce instructions.
#[derive(Debug, Clone, Copy)]
pub struct Positioned;

/// Sealed marker for the type-state generic so external crates cannot
/// invent new states.
mod state_sealed {
    pub trait Sealed {}
    impl Sealed for super::Unpositioned {}
    impl Sealed for super::Positioned {}
}

/// Sealed marker trait for the [`IRBuilder`] positioning typestate.
/// The two implementors are [`Unpositioned`] and [`Positioned`];
/// external crates cannot invent new states. Public so higher layers
/// (e.g. the Braun-SSA `SsaBuilder`) can be generic over the same states.
pub trait BuilderPositionState: state_sealed::Sealed + 'static {}

impl BuilderPositionState for Unpositioned {}
impl BuilderPositionState for Positioned {}

/// Snapshot of an [`IRBuilder`] insertion location. Mirrors
/// `IRBuilderBase::InsertPoint` in `IRBuilder.h`. The `block` is `None`
/// when the builder was unpositioned at save time; `before` is `None`
/// when the saved location was end-of-block.
#[derive(Debug)]
pub struct InsertPoint<'ctx, R: ReturnMarker, B: ModuleBrand = Brand<'ctx>> {
    pub(super) block_id: Option<ValueId>,
    pub(super) before: Option<ValueId>,
    pub(super) _marker: PhantomData<fn(&'ctx (), R, B)>,
}

#[derive(Debug, Clone)]
pub struct CallSiteConfig {
    name: String,
    calling_conv: CallingConv,
    attrs: CallAttributeData,
    call_site_fn_ty: Option<TypeId>,
}

impl CallSiteConfig {
    pub fn new<Name>(name: Name) -> Self
    where
        Name: Into<String>,
    {
        Self {
            name: name.into(),
            calling_conv: CallingConv::C,
            attrs: CallAttributeData::default(),
            call_site_fn_ty: None,
        }
    }

    pub fn calling_conv(mut self, calling_conv: CallingConv) -> Self {
        self.calling_conv = calling_conv;
        self
    }

    pub fn attrs(mut self, attrs: CallAttributeData) -> Self {
        self.attrs = attrs;
        self
    }

    /// Override the call site's function type so it no longer derives from
    /// the callee's declaration. Mirrors LLVM's `CallBase`, which carries
    /// its own `FunctionType` independent of the callee operand: an
    /// `invoke`/`callbr` may be spelled through a function type that differs
    /// from the declared callee (opaque-pointer IR, checked by the verifier
    /// against the call's own type, not the declaration). Left unset, the
    /// call site keeps deriving its type from the callee.
    pub fn call_site_type<'ctx, Brand: ModuleBrand + 'ctx>(
        mut self,
        fn_ty: FunctionType<'ctx, Brand>,
    ) -> Self {
        self.call_site_fn_ty = Some(fn_ty.as_type().id());
        self
    }

    pub(super) fn call_site_fn_ty(&self) -> Option<TypeId> {
        self.call_site_fn_ty
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn calling_conv_value(&self) -> CallingConv {
        self.calling_conv
    }

    pub fn attrs_value(&self) -> &CallAttributeData {
        &self.attrs
    }

    pub(super) fn into_parts(self) -> (String, CallingConv, CallAttributeData) {
        (self.name, self.calling_conv, self.attrs)
    }
}

/// Builder for a chain of [`Instruction`]s appended to a
/// [`BasicBlock`].
///
/// Type parameters:
/// - `F` — folder strategy (defaults to [`ConstantFolder`]).
/// - `S` — insertion-point type-state ([`Unpositioned`] / [`Positioned`]).
/// - `R` — parent function's [`ReturnMarker`].
pub struct IRBuilder<'m, 'ctx, B, F, S, R>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    module: &'ctx ModuleCore,
    _module: PhantomData<&'m Module<'ctx, B, Unverified>>,
    insert_block: Option<BasicBlock<'ctx, R, Unterminated, B>>,
    /// Optional insertion anchor: when `Some(id)`, new instructions are
    /// inserted *before* the instruction with this id (mirrors upstream
    /// `IRBuilder::SetInsertPoint(Instruction*)`). When `None`, new
    /// instructions append to the end of `insert_block`.
    insert_before: Option<ValueId>,
    folder: F,
    fmf: super::fmf::FastMathFlags,
    _state: PhantomData<S>,
}

// --------------------------------------------------------------------------
// Constructors
// --------------------------------------------------------------------------

impl<'m, 'ctx, B> IRBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, Dyn>
where
    B: ModuleBrand + 'ctx,
{
    /// Construct an unpositioned builder using the default
    /// [`ConstantFolder`]. The runtime-checked [`Dyn`] return marker
    /// matches the runtime-equality `build_ret` path; use
    /// [`IRBuilder::new_for`] when the caller already knows the return
    /// shape statically.
    pub fn new(module: &'m Module<'ctx, B, Unverified>) -> Self {
        Self {
            module: module.core_ref(),
            _module: PhantomData,
            insert_block: None,
            insert_before: None,
            folder: ConstantFolder,
            fmf: super::fmf::FastMathFlags::empty(),
            _state: PhantomData,
        }
    }

    /// Construct an unpositioned, typed-return builder. Use this
    /// when the caller already knows the parent function's return
    /// shape; the resulting builder's `build_ret` is statically
    /// typed.
    ///
    /// ```ignore
    /// let b = IRBuilder::new_for::<i32>(&module);
    /// ```
    pub fn new_for<R>(
        module: &'m Module<'ctx, B, Unverified>,
    ) -> IRBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, R>
    where
        R: ReturnMarker,
    {
        IRBuilder {
            module: module.core_ref(),
            _module: PhantomData,
            insert_block: None,
            insert_before: None,
            folder: ConstantFolder,
            fmf: super::fmf::FastMathFlags::empty(),
            _state: PhantomData,
        }
    }

    /// Construct an unpositioned builder from a Rust function-pointer
    /// signature's return schema.
    pub fn new_for_return<Sig>(
        module: &'m Module<'ctx, B, Unverified>,
    ) -> IRBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, <Sig::Ret as FunctionReturn>::Marker>
    where
        Sig: FunctionSignature,
    {
        IRBuilder {
            module: module.core_ref(),
            _module: PhantomData,
            insert_block: None,
            insert_before: None,
            folder: ConstantFolder,
            fmf: super::fmf::FastMathFlags::empty(),
            _state: PhantomData,
        }
    }
}

impl<'m, 'ctx, B, F, R> IRBuilder<'m, 'ctx, B, F, Unpositioned, R>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Construct an unpositioned builder using a caller-supplied
    /// folder.
    pub fn with_folder(module: &'m Module<'ctx, B, Unverified>, folder: F) -> Self {
        Self {
            module: module.core_ref(),
            _module: PhantomData,
            insert_block: None,
            insert_before: None,
            folder,
            fmf: super::fmf::FastMathFlags::empty(),
            _state: PhantomData,
        }
    }

    /// Position the builder at the end of `bb`. Mirrors
    /// `IRBuilder::SetInsertPoint(BasicBlock*)`. The block's
    /// [`ReturnMarker`] must match the builder's.
    pub fn position_at_end(
        self,
        bb: BasicBlock<'ctx, R, Unterminated, B>,
    ) -> IRBuilder<'m, 'ctx, B, F, Positioned, R> {
        IRBuilder {
            module: self.module,
            _module: PhantomData,
            insert_block: Some(bb),
            insert_before: None,
            folder: self.folder,
            fmf: self.fmf,
            _state: PhantomData,
        }
    }
}

// --------------------------------------------------------------------------
// Positioning methods that move from any state to Positioned.
// --------------------------------------------------------------------------

impl<'m, 'ctx, B, F, S, R> IRBuilder<'m, 'ctx, B, F, S, R>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    /// Re-anchor the builder *before* the given attached instruction.
    /// New instructions land between the prior instruction and `anchor`.
    /// Mirrors `IRBuilder::SetInsertPoint(Instruction *I)` in `IRBuilder.h`,
    /// which sets `BB = I->getParent(); InsertPt = I->getIterator();`.
    pub fn position_before(
        self,
        anchor: &InstructionView<'ctx, B>,
    ) -> IRBuilder<'m, 'ctx, B, F, Positioned, R> {
        let anchor_id = anchor.as_value().id;
        let parent_block_id = anchor.parent().as_value().id;
        let label_ty = self.module.label_type().as_type().id();
        let bb = BasicBlock::<R, Unterminated, B>::from_parts(
            parent_block_id,
            ModuleRef::<B>::new(self.module),
            label_ty,
        );
        IRBuilder {
            module: self.module,
            _module: PhantomData,
            insert_block: Some(bb),
            insert_before: Some(anchor_id),
            folder: self.folder,
            fmf: self.fmf,
            _state: PhantomData,
        }
    }

    /// Position at the entry block, past any leading `alloca`s. Mirrors
    /// `IRBuilder::SetInsertPointPastAllocas(Function*)` in `IRBuilder.h`,
    /// which sets `BB = &F->getEntryBlock(); InsertPt = BB->getFirstNonPHIOrDbgOrAlloca();`.
    pub fn position_past_allocas(
        self,
        f: FunctionValue<'ctx, R, B>,
    ) -> IRBuilder<'m, 'ctx, B, F, Positioned, R> {
        let entry = f.entry_block().unwrap_or_else(|| {
            unreachable!("position_past_allocas requires a function with at least one block")
        });
        // Find the first non-alloca instruction id, mirroring
        // `BasicBlock::getFirstNonPHIOrDbgOrAlloca`. We don't ship phi/dbg
        // filters yet, so the practical filter here is alloca-only.
        let mut anchor: Option<ValueId> = None;
        for inst in entry.instructions() {
            match inst.kind() {
                Some(InstructionKind::Alloca(_)) => continue,
                _ => {
                    anchor = Some(inst.as_value().id);
                    break;
                }
            }
        }
        IRBuilder {
            module: self.module,
            _module: PhantomData,
            insert_block: Some(entry.retag_termination::<Unterminated>()),
            insert_before: anchor,
            folder: self.folder,
            fmf: self.fmf,
            _state: PhantomData,
        }
    }

    /// Snapshot the current insertion location. Mirrors
    /// `IRBuilder::saveIP` (returns `InsertPoint(BB, InsertPt)`).
    pub fn save_insert_point(&self) -> InsertPoint<'ctx, R, B> {
        InsertPoint {
            block_id: self.insert_block.as_ref().map(|bb| bb.as_value().id),
            before: self.insert_before,
            _marker: PhantomData,
        }
    }

    /// Restore a previously-saved insertion point. Mirrors
    /// `IRBuilder::restoreIP(InsertPoint)`, but returns an error instead of
    /// reopening a block that has since grown a terminator.
    pub fn restore_insert_point(
        self,
        ip: InsertPoint<'ctx, R, B>,
    ) -> IrResult<IRBuilder<'m, 'ctx, B, F, Positioned, R>> {
        let Some(block_id) = ip.block_id else {
            return Err(IrError::InvalidOperation {
                message: "cannot restore an empty insert point",
            });
        };
        let label_ty = self.module.label_type().as_type().id();
        let insert_block = BasicBlock::<R, Unterminated, B>::from_parts(
            block_id,
            ModuleRef::<B>::new(self.module),
            label_ty,
        );
        if ip.before.is_none()
            && insert_block
                .terminator()
                .is_some_and(|inst| inst.is_terminator())
        {
            return Err(IrError::InvalidOperation {
                message: "cannot restore insert point at end of terminated block",
            });
        }
        Ok(IRBuilder {
            module: self.module,
            _module: PhantomData,
            insert_block: Some(insert_block),
            insert_before: ip.before,
            folder: self.folder,
            fmf: self.fmf,
            _state: PhantomData,
        })
    }

    /// Add an incoming `(value, block)` pair to a phi instruction identified
    /// by its erased [`Value`] handle. This is the dynamic
    /// counterpart to [`PhiInst::add_incoming`] for
    /// use by parsers and passes where compile-time type markers are
    /// unavailable.
    ///
    /// Errors if `phi_val` does not refer to a phi instruction. `val` and
    /// `block` already carry the builder brand `B`; remaining value-type and
    /// predecessor-set coherence is verified by [`Module::verify`](crate::Module::verify).
    pub fn phi_add_incoming_from_value<RBb, SBb>(
        &self,
        phi_val: Value<'ctx, B>,
        val: Value<'ctx, B>,
        block: BasicBlock<'ctx, RBb, SBb, B>,
    ) -> IrResult<()>
    where
        RBb: crate::marker::ReturnMarker,
        SBb: crate::block_state::BlockTerminationState,
    {
        // Access the phi payload via the module's instruction data.
        let inst_data = self.module.context().value_data(phi_val.id);
        let inst_kind_data = match &inst_data.kind {
            ValueKindData::Instruction(i) => &i.kind,
            _ => {
                return Err(IrError::InvalidOperation {
                    message: "phi_add_incoming_from_value: target is not an instruction",
                });
            }
        };
        let phi_payload = match inst_kind_data {
            InstructionKindData::Phi(p) => p,
            _ => {
                return Err(IrError::InvalidOperation {
                    message: "phi_add_incoming_from_value: instruction is not a phi",
                });
            }
        };
        phi_payload
            .incoming
            .borrow_mut()
            .push((core::cell::Cell::new(val.id), block.as_value().id));
        // Register phi as a user of the incoming value.
        self.module
            .context()
            .value_data(val.id)
            .use_list
            .borrow_mut()
            .push(ValueUse::Instruction(phi_val.id));
        Ok(())
    }
}

impl<'m, 'ctx, B, F, R> IRBuilder<'m, 'ctx, B, F, Positioned, R>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Re-position the builder at the end of `bb`.
    pub fn position_at_end(self, bb: BasicBlock<'ctx, R, Unterminated, B>) -> Self {
        Self {
            module: self.module,
            _module: PhantomData,
            insert_block: Some(bb),
            insert_before: None,
            folder: self.folder,
            fmf: self.fmf,
            _state: PhantomData,
        }
    }

    /// Drop the insertion point. Mirrors
    /// `IRBuilder::ClearInsertionPoint`.
    pub fn unposition(self) -> IRBuilder<'m, 'ctx, B, F, Unpositioned, R> {
        IRBuilder {
            module: self.module,
            _module: PhantomData,
            insert_block: None,
            insert_before: None,
            folder: self.folder,
            fmf: self.fmf,
            _state: PhantomData,
        }
    }

    /// Current insertion block. Always populated in the positioned
    /// state.
    #[inline]
    pub fn insert_block(&self) -> &BasicBlock<'ctx, R, Unterminated, B> {
        match self.insert_block.as_ref() {
            Some(bb) => bb,
            None => unreachable!("Positioned builder always has an insertion point"),
        }
    }

    /// Consume this positioned builder without emitting a terminator,
    /// returning its unterminated insertion block for cursor-driven mutation
    /// or later repositioning.
    #[inline]
    pub fn into_insert_block(self) -> BasicBlock<'ctx, R, Unterminated, B> {
        match self.insert_block {
            Some(bb) => bb,
            None => unreachable!("Positioned builder always has an insertion point"),
        }
    }

    // ---- Fast-math flags (builder-context) ----

    /// Get the builder's current default FMF set. Mirrors
    /// `IRBuilderBase::getFastMathFlags() const` in `IRBuilder.h`.
    #[inline]
    pub fn fast_math_flags(&self) -> FastMathFlags {
        self.fmf
    }

    /// Set the builder's default FMF. Subsequent FP-math instructions
    /// (fadd / fsub / fmul / fdiv / frem / fneg / fcmp) carry these flags.
    /// Mirrors `IRBuilderBase::setFastMathFlags(FastMathFlags)`.
    pub fn with_fast_math_flags(self, fmf: FastMathFlags) -> Self {
        Self { fmf, ..self }
    }

    /// Reset the builder's default FMF to empty. Mirrors
    /// `IRBuilderBase::clearFastMathFlags()`.
    pub fn clear_fast_math_flags(self) -> Self {
        Self {
            fmf: super::fmf::FastMathFlags::empty(),
            ..self
        }
    }

    // ---- Integer arithmetic ----

    /// Produce `add lhs, rhs`. Mirrors `IRBuilder::CreateAdd`.
    ///
    /// Operands share width `W` -- enforced at compile time by the
    /// type system. Either side accepts any [`crate::IntoIntValue<'ctx, W, B>`]:
    /// already-typed [`IntValue`]s, [`crate::ConstantIntValue`]s, and
    /// Rust scalar literals (`5_i32`, `true`, ...) all work.
    pub fn build_int_add<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_int_bin_op(BinaryOpcode::Add, lhs, rhs)? {
            return self.accept_folded_int(folded, lhs);
        }
        let payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        let inst = self.append_instruction(
            lhs.ty().as_type().id(),
            InstructionKindData::Add(payload),
            name,
        );
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `sub lhs, rhs`. Mirrors `IRBuilder::CreateSub`.
    pub fn build_int_sub<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_int_bin_op(BinaryOpcode::Sub, lhs, rhs)? {
            return self.accept_folded_int(folded, lhs);
        }
        let payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        let inst = self.append_instruction(
            lhs.ty().as_type().id(),
            InstructionKindData::Sub(payload),
            name,
        );
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `mul lhs, rhs`. Mirrors `IRBuilder::CreateMul`.
    pub fn build_int_mul<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::Mul,
            lhs,
            rhs,
            name,
            crate::instr_types::MulFlags::new(),
            InstructionKindData::Mul,
        )
    }

    /// Produce `udiv lhs, rhs`. Mirrors `IRBuilder::CreateUDiv`.
    pub fn build_int_udiv<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(
            BinaryOpcode::UDiv,
            lhs,
            rhs,
            name,
            InstructionKindData::UDiv,
        )
    }

    /// Produce `sdiv lhs, rhs`. Mirrors `IRBuilder::CreateSDiv`.
    pub fn build_int_sdiv<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(
            BinaryOpcode::SDiv,
            lhs,
            rhs,
            name,
            InstructionKindData::SDiv,
        )
    }

    /// Produce `urem lhs, rhs`. Mirrors `IRBuilder::CreateURem`.
    pub fn build_int_urem<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(
            BinaryOpcode::URem,
            lhs,
            rhs,
            name,
            InstructionKindData::URem,
        )
    }

    /// Produce `srem lhs, rhs`. Mirrors `IRBuilder::CreateSRem`.
    pub fn build_int_srem<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(
            BinaryOpcode::SRem,
            lhs,
            rhs,
            name,
            InstructionKindData::SRem,
        )
    }

    /// Produce `shl lhs, rhs`. Mirrors `IRBuilder::CreateShl`.
    pub fn build_int_shl<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::Shl,
            lhs,
            rhs,
            name,
            crate::instr_types::ShlFlags::new(),
            InstructionKindData::Shl,
        )
    }

    /// Produce `lshr lhs, rhs`. Mirrors `IRBuilder::CreateLShr`.
    pub fn build_int_lshr<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(
            BinaryOpcode::LShr,
            lhs,
            rhs,
            name,
            InstructionKindData::LShr,
        )
    }

    /// Produce `ashr lhs, rhs`. Mirrors `IRBuilder::CreateAShr`.
    pub fn build_int_ashr<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(
            BinaryOpcode::AShr,
            lhs,
            rhs,
            name,
            InstructionKindData::AShr,
        )
    }

    /// Produce `and lhs, rhs`. Mirrors `IRBuilder::CreateAnd`.
    pub fn build_int_and<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(BinaryOpcode::And, lhs, rhs, name, InstructionKindData::And)
    }

    /// Produce `or lhs, rhs`. Mirrors `IRBuilder::CreateOr`.
    pub fn build_int_or<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(BinaryOpcode::Or, lhs, rhs, name, InstructionKindData::Or)
    }

    /// Produce `or disjoint lhs, rhs` with explicit [`crate::OrFlags`].
    /// The `disjoint` flag asserts the operands have no bits in common.
    /// Mirrors `IRBuilder::CreateOr` with `IsDisjoint` set.
    pub fn build_int_or_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::OrFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::Or,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Or,
        )
    }

    /// Produce `xor lhs, rhs`. Mirrors `IRBuilder::CreateXor`.
    pub fn build_int_xor<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop(BinaryOpcode::Xor, lhs, rhs, name, InstructionKindData::Xor)
    }

    /// Produce `add lhs, rhs` with explicit [`crate::AddFlags`]. Mirrors
    /// `IRBuilder::CreateAdd` plus the `nuw`/`nsw` knobs. The flag
    /// set type only exposes flags LLVM accepts on `add`, so
    /// invalid combinations are a compile error.
    pub fn build_int_add_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::AddFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::Add,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Add,
        )
    }

    /// Produce `sub lhs, rhs` with explicit [`crate::SubFlags`].
    pub fn build_int_sub_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::SubFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::Sub,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Sub,
        )
    }

    /// Produce `mul lhs, rhs` with explicit [`crate::MulFlags`].
    pub fn build_int_mul_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::MulFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::Mul,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Mul,
        )
    }

    /// Produce `shl lhs, rhs` with explicit [`crate::ShlFlags`].
    pub fn build_int_shl_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::ShlFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::Shl,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Shl,
        )
    }

    /// Produce `udiv lhs, rhs` with explicit [`crate::UDivFlags`].
    pub fn build_int_udiv_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::UDivFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::UDiv,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::UDiv,
        )
    }

    /// Produce `sdiv lhs, rhs` with explicit [`crate::SDivFlags`].
    pub fn build_int_sdiv_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::SDivFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::SDiv,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::SDiv,
        )
    }

    /// Produce `lshr lhs, rhs` with explicit [`crate::LShrFlags`].
    pub fn build_int_lshr_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::LShrFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::LShr,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::LShr,
        )
    }

    /// Produce `ashr lhs, rhs` with explicit [`crate::AShrFlags`].
    pub fn build_int_ashr_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::AShrFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_binop_flagged(
            BinaryOpcode::AShr,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::AShr,
        )
    }

    /// Integer negation: `sub 0, V`. Mirrors `IRBuilder::CreateNeg(V, Name)`,
    /// which expands to `CreateSub(Constant::getNullValue(V->getType()), V, Name)`.
    pub fn build_int_neg<W, V, Name>(&self, value: V, name: Name) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        V: IntoIntValue<'ctx, W, B>,
    {
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        let zero = W::ir_type(ModuleRef::<B>::new(self.module)).const_zero();
        self.build_int_sub(zero, v, name)
    }

    /// Integer NSW negation. Mirrors `IRBuilder::CreateNSWNeg(V, Name)` ->
    /// `CreateNeg(V, Name, /*HasNSW=*/true)` -> `CreateSub` with `nsw`.
    pub fn build_int_neg_nsw<W, V, Name>(
        &self,
        value: V,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        V: IntoIntValue<'ctx, W, B>,
    {
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        let zero = W::ir_type(ModuleRef::<B>::new(self.module)).const_zero();
        self.build_int_sub_with_flags(zero, v, super::instr_types::SubFlags::new().nsw(), name)
    }

    /// Bitwise complement: `xor V, -1`. Mirrors `IRBuilder::CreateNot(V, Name)`,
    /// which expands to `CreateXor(V, Constant::getAllOnesValue(V->getType()))`.
    pub fn build_int_not<W, V, Name>(&self, value: V, name: Name) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        V: IntoIntValue<'ctx, W, B>,
    {
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        let all_ones = W::ir_type(ModuleRef::<B>::new(self.module)).const_all_ones();
        self.build_int_xor(v, all_ones, name)
    }

    /// Crate-internal helper: emit a flagged binary op. The flag
    /// type's `WriteBinopFlags` impl writes its bits onto the
    /// payload; the kind constructor lifts the payload into the
    /// matching `InstructionKindData` variant.
    fn build_int_binop_flagged<W, Lhs, Rhs, Flags, Kind>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
        flags: Flags,
        kind_ctor: Kind,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
        Flags: crate::instr_types::WriteBinopFlags,
        Kind: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        let mut payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        flags.apply(&mut payload);
        let folded = if payload.is_exact {
            self.folder.fold_int_bin_op_exact(opcode, lhs, rhs)?
        } else if matches!(
            opcode,
            BinaryOpcode::Add | BinaryOpcode::Sub | BinaryOpcode::Mul | BinaryOpcode::Shl
        ) {
            let flags = OverflowFlags::from_parts(payload.no_unsigned_wrap, payload.no_signed_wrap);
            self.folder
                .fold_int_bin_op_no_wrap(opcode, lhs, rhs, flags)?
        } else {
            self.folder.fold_int_bin_op(opcode, lhs, rhs)?
        };
        if let Some(folded) = folded {
            return self.accept_folded_int(folded, lhs);
        }
        let inst = self.append_instruction(lhs.ty().as_type().id(), kind_ctor(payload), name);
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Crate-internal helper: emit a binary op given a callback that
    /// wraps the payload into an [`InstructionKindData`] variant.
    /// All integer binary opcodes route through the folder before materialising
    /// an instruction.
    fn build_int_binop<W, Lhs, Rhs, F2>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
        kind_ctor: F2,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_int_bin_op(opcode, lhs, rhs)? {
            return self.accept_folded_int(folded, lhs);
        }
        let payload = BinaryOpData::new(lhs.as_value().id, rhs.as_value().id);
        let inst = self.append_instruction(lhs.ty().as_type().id(), kind_ctor(payload), name);
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    // ---- Type-erased integer binops (scalar OR integer-vector operands) ----
    //
    // The typed `build_int_*` family routes both operands through
    // `IntoIntValue<W>`, whose `TryFrom<Value>` impls accept only scalar
    // `iN` types and reject integer *vectors* (`<N x iM>`). Element-wise
    // vector arithmetic (`xor <2 x i64> ...`) is legal IR the verifier
    // already accepts (`is_int_or_int_vector`), but there was no builder
    // path to emit it. These `_dyn` wrappers take erased [`Value`] operands
    // and skip the scalar-only `IntoIntValue` conversion, mirroring the
    // untyped cast builder [`build_bitcast_dyn`]. The result type is the
    // LHS operand's type; the caller is responsible for operand-type
    // agreement (the LLVM verifier rejects ill-formed binops).

    /// Crate-internal: emit an integer binop on erased [`Value`] operands
    /// (scalar `iN` or integer vector `<N x iM>`), the result taking the LHS
    /// operand's type. Skips the scalar-only `IntoIntValue` conversion the
    /// typed `build_int_*` family performs, so it accepts vector operands.
    fn build_int_binop_dyn<F2>(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: impl AsRef<str>,
        kind_ctor: F2,
    ) -> IrResult<Value<'ctx, B>>
    where
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        if let Some(folded) = self.folder.fold_bin_op_dyn(opcode, lhs, rhs)? {
            return self.checked_folded_value(folded, lhs.ty);
        }
        let payload = BinaryOpData::new(lhs.id, rhs.id);
        let inst = self.append_instruction(lhs.ty().id(), kind_ctor(payload), name);
        Ok(inst.as_value())
    }

    /// `add lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_add_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(BinaryOpcode::Add, lhs, rhs, name, InstructionKindData::Add)
    }

    /// `sub lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_sub_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(BinaryOpcode::Sub, lhs, rhs, name, InstructionKindData::Sub)
    }

    /// `mul lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_mul_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(BinaryOpcode::Mul, lhs, rhs, name, InstructionKindData::Mul)
    }

    /// `xor lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_xor_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(BinaryOpcode::Xor, lhs, rhs, name, InstructionKindData::Xor)
    }

    /// `and lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_and_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(BinaryOpcode::And, lhs, rhs, name, InstructionKindData::And)
    }

    /// `or lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_or_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(BinaryOpcode::Or, lhs, rhs, name, InstructionKindData::Or)
    }

    /// `shl lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_shl_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(BinaryOpcode::Shl, lhs, rhs, name, InstructionKindData::Shl)
    }

    /// `lshr lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_lshr_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(
            BinaryOpcode::LShr,
            lhs,
            rhs,
            name,
            InstructionKindData::LShr,
        )
    }

    /// `ashr lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn build_int_ashr_dyn<Name>(
        &self,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_binop_dyn(
            BinaryOpcode::AShr,
            lhs,
            rhs,
            name,
            InstructionKindData::AShr,
        )
    }

    // ---- Floating-point arithmetic ----

    /// Produce `fadd lhs, rhs`. Mirrors `IRBuilder::CreateFAdd`.
    pub fn build_fp_add<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop(
            BinaryOpcode::FAdd,
            lhs,
            rhs,
            name,
            InstructionKindData::FAdd,
        )
    }

    /// Produce `fsub lhs, rhs`. Mirrors `IRBuilder::CreateFSub`.
    pub fn build_fp_sub<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop(
            BinaryOpcode::FSub,
            lhs,
            rhs,
            name,
            InstructionKindData::FSub,
        )
    }

    /// Produce `fmul lhs, rhs`. Mirrors `IRBuilder::CreateFMul`.
    pub fn build_fp_mul<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop(
            BinaryOpcode::FMul,
            lhs,
            rhs,
            name,
            InstructionKindData::FMul,
        )
    }

    /// Produce `fdiv lhs, rhs`. Mirrors `IRBuilder::CreateFDiv`.
    pub fn build_fp_div<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop(
            BinaryOpcode::FDiv,
            lhs,
            rhs,
            name,
            InstructionKindData::FDiv,
        )
    }

    /// Produce `frem lhs, rhs`. Mirrors `IRBuilder::CreateFRem`.
    pub fn build_fp_rem<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop(
            BinaryOpcode::FRem,
            lhs,
            rhs,
            name,
            InstructionKindData::FRem,
        )
    }

    /// Crate-internal helper for float binops. Same shape as
    /// `build_int_binop` but parameterised by `K: FloatKind`.
    fn build_fp_binop<K, Lhs, Rhs, F2>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        name: impl AsRef<str>,
        kind_ctor: F2,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        let lhs = lhs.into_float_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_float_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_fp_bin_op(opcode, lhs, rhs, self.fmf)? {
            return self.accept_folded_fp(folded, lhs);
        }
        let mut payload = BinaryOpData::new(IsValue::as_value(lhs).id, IsValue::as_value(rhs).id);
        // Apply the builder-context FMF (parallel to upstream
        // `IRBuilderBase::setFPAttrs` in `IRBuilder.h`, which calls
        // `I->setFastMathFlags(FMF)` on every FP-math instruction).
        payload.fmf = self.fmf;
        let inst =
            self.append_instruction(crate::value::Typed::ty(lhs).id(), kind_ctor(payload), name);
        Ok(FloatValue::<K, B>::from_value_unchecked(inst.as_value()))
    }

    /// Crate-internal helper for float binops with an explicit
    /// [`crate::fmf::FastMathFlags`] parameter rather than the builder-context
    /// FMF. Used by the `build_fp_*_fmf` family.
    fn build_fp_binop_with_fmf<K, Lhs, Rhs, F2>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        fmf: crate::fmf::FastMathFlags,
        name: impl AsRef<str>,
        kind_ctor: F2,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        let lhs = lhs.into_float_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_float_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_fp_bin_op(opcode, lhs, rhs, fmf)? {
            return self.accept_folded_fp(folded, lhs);
        }
        let mut payload = BinaryOpData::new(IsValue::as_value(lhs).id, IsValue::as_value(rhs).id);
        payload.fmf = fmf;
        let inst =
            self.append_instruction(crate::value::Typed::ty(lhs).id(), kind_ctor(payload), name);
        Ok(FloatValue::<K, B>::from_value_unchecked(inst.as_value()))
    }

    /// `fadd` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    /// Bypasses the builder-context FMF; caller supplies the exact flags.
    pub fn build_fp_add_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: crate::fmf::FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop_with_fmf(
            BinaryOpcode::FAdd,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::FAdd,
        )
    }

    /// `fsub` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    pub fn build_fp_sub_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: crate::fmf::FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop_with_fmf(
            BinaryOpcode::FSub,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::FSub,
        )
    }

    /// `fmul` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    pub fn build_fp_mul_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: crate::fmf::FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop_with_fmf(
            BinaryOpcode::FMul,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::FMul,
        )
    }

    /// `fdiv` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    pub fn build_fp_div_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: crate::fmf::FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop_with_fmf(
            BinaryOpcode::FDiv,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::FDiv,
        )
    }

    /// `frem` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    pub fn build_fp_rem_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: crate::fmf::FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_binop_with_fmf(
            BinaryOpcode::FRem,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::FRem,
        )
    }

    /// `fcmp` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    /// Bypasses the builder-context FMF. Result is `i1`.
    pub fn build_fp_cmp_fmf<K, Lhs, Rhs, Name>(
        &self,
        pred: crate::cmp_predicate::FloatPredicate,
        lhs: Lhs,
        rhs: Rhs,
        fmf: crate::fmf::FastMathFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        let lhs = lhs.into_float_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_float_value(ModuleRef::new(self.module))?;
        let i1_ty = ModuleView::<B>::new(self.module).bool_type().as_type().id();
        if let Some(folded) = self.folder.fold_fp_cmp(pred, lhs, rhs)? {
            return Ok(folded);
        }
        let mut payload = crate::instr_types::FCmpInstData::new(
            pred,
            IsValue::as_value(lhs).id,
            IsValue::as_value(rhs).id,
        );
        payload.fmf = fmf;
        let inst = self.append_instruction(i1_ty, InstructionKindData::FCmp(payload), name);
        Ok(IntValue::<bool, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `fcmp <pred> lhs, rhs`. Mirrors
    /// `IRBuilder::CreateFCmp`. Result is `i1`.
    pub fn build_fp_cmp<K, Lhs, Rhs, Name>(
        &self,
        pred: crate::cmp_predicate::FloatPredicate,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        let lhs = lhs.into_float_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_float_value(ModuleRef::new(self.module))?;
        let i1_ty = ModuleView::<B>::new(self.module).bool_type().as_type().id();
        if let Some(folded) = self.folder.fold_fp_cmp(pred, lhs, rhs)? {
            return Ok(folded);
        }
        let mut payload = crate::instr_types::FCmpInstData::new(
            pred,
            IsValue::as_value(lhs).id,
            IsValue::as_value(rhs).id,
        );
        // Apply builder-context FMF (`fcmp` is an `FPMathOperator` upstream).
        payload.fmf = self.fmf;
        let inst = self.append_instruction(i1_ty, InstructionKindData::FCmp(payload), name);
        Ok(IntValue::<bool, B>::from_value_unchecked(inst.as_value()))
    }

    // ---- Per-predicate fcmp wrappers ----
    //
    // Each method mirrors the matching `IRBuilder::CreateFCmpO<Pred>` /
    // `CreateFCmpU<Pred>` in `IRBuilder.h` (lines 2371-2475). All
    // delegate to `build_fp_cmp` with the appropriate `FloatPredicate`.

    /// Mirrors `IRBuilder::CreateFCmpOEQ`.
    pub fn build_fcmp_oeq<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Oeq,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpOGT`.
    pub fn build_fcmp_ogt<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Ogt,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpOGE`.
    pub fn build_fcmp_oge<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Oge,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpOLT`.
    pub fn build_fcmp_olt<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Olt,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpOLE`.
    pub fn build_fcmp_ole<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Ole,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpONE`.
    pub fn build_fcmp_one<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::One,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpORD`.
    pub fn build_fcmp_ord<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Ord,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpUNO`.
    pub fn build_fcmp_uno<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Uno,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpUEQ`.
    pub fn build_fcmp_ueq<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Ueq,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpUGT`.
    pub fn build_fcmp_ugt<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Ugt,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpUGE`.
    pub fn build_fcmp_uge<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Uge,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpULT`.
    pub fn build_fcmp_ult<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Ult,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpULE`.
    pub fn build_fcmp_ule<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Ule,
            lhs,
            rhs,
            name,
        )
    }

    /// Mirrors `IRBuilder::CreateFCmpUNE`.
    pub fn build_fcmp_une<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.build_fp_cmp::<K, Lhs, Rhs, _>(
            super::cmp_predicate::FloatPredicate::Une,
            lhs,
            rhs,
            name,
        )
    }

    // ---- Unary ops: fneg / freeze / va_arg ----

    /// Produce `fneg <value>`. Mirrors `IRBuilder::CreateFNeg` in
    /// `IRBuilder.h`. The result handle has the same float kind as the
    /// operand (Doctrine D4).
    pub fn build_float_neg<K, V, Name>(
        &self,
        value: V,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        V: IntoFloatValue<'ctx, K, B>,
    {
        self.build_float_neg_with_flags::<K, V, _>(value, self.fmf, name)
    }

    /// Produce `fneg <fmf> <value>`. Mirrors `IRBuilder::CreateFNegFMF`.
    /// The flags are written verbatim onto the instruction (see
    /// `FPMathOperator::setFastMathFlags`).
    pub fn build_float_neg_with_flags<K, V, Name>(
        &self,
        value: V,
        fmf: crate::fmf::FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        V: IntoFloatValue<'ctx, K, B>,
    {
        let v = value.into_float_value(ModuleRef::new(self.module))?;
        let ty = crate::value::Typed::ty(v).id();
        if let Some(folded) = self.folder.fold_fp_un_op(UnaryOpcode::FNeg, v, fmf)? {
            return self.accept_folded_fp(folded, v);
        }
        let payload = FNegInstData::new(IsValue::as_value(v).id, fmf);
        let inst = self.append_instruction(ty, InstructionKindData::FNeg(payload), name);
        Ok(FloatValue::<K, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `freeze <value>`. Mirrors `IRBuilder::CreateFreeze`.
    /// Accepts any [`IsValue`] operand; the result type
    /// matches the operand type.
    pub fn build_freeze<V, Name>(&self, value: V, name: Name) -> IrResult<FreezeInst<'ctx, B>>
    where
        Name: AsRef<str>,
        V: IsValue<'ctx, B>,
    {
        let v = value.as_value();
        let payload = crate::instr_types::FreezeInstData::new(v.id);
        let inst = self.append_instruction(v.ty, InstructionKindData::Freeze(payload), name);
        Ok(FreezeInst::<B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            v.ty,
        ))
    }

    /// Produce `va_arg <list>, <ty>`. Mirrors `IRBuilder::CreateVAArg`.
    /// The destination type can be any first-class type; the source
    /// must be a `va_list` pointer.
    pub fn build_va_arg<Name>(
        &self,
        list_ptr: PointerValue<'ctx, B>,
        result_ty: Type<'ctx, B>,
        name: Name,
    ) -> IrResult<VAArgInst<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        let v = IsValue::as_value(list_ptr);
        let payload = crate::instr_types::VAArgInstData::new(v.id);
        let inst = self.append_instruction(result_ty.id, InstructionKindData::VAArg(payload), name);
        Ok(VAArgInst::<B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            result_ty.id,
        ))
    }

    // ---- Aggregate ops: extractvalue / insertvalue ----

    /// Produce `extractvalue <agg-ty> <agg>, idx0, idx1, ...`.
    /// Mirrors `IRBuilder::CreateExtractValue`.
    ///
    /// The index list is a fixed-size array whose length is checked at
    /// compile time (Doctrine D3): `ExtractValueInst::init`
    /// (`lib/IR/Instructions.cpp`) asserts a non-empty index list, and
    /// `N > 0` pulls that assertion forward to monomorphisation instead of
    /// a runtime error. Use [`Self::build_extract_value_dyn`] for a
    /// slice/`Vec`-driven index list that keeps the runtime check.
    pub fn build_extract_value<V, const N: usize, Name>(
        &self,
        aggregate: V,
        indices: [u32; N],
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        V: IsValue<'ctx, B>,
    {
        const {
            assert!(N > 0, "extractvalue requires at least one index");
        }
        self.build_extract_value_dyn(aggregate, &indices, name)
    }

    /// Produce `extractvalue <agg-ty> <agg>, idx0, idx1, ...` from a
    /// dynamically-sized index slice. Mirrors `IRBuilder::CreateExtractValue`.
    ///
    /// Ports the empty-index-list rejection in
    /// `ExtractValueInst::init` (`lib/IR/Instructions.cpp`); see
    /// `test/Assembler/extractvalue-no-idx.ll` for the upstream assembler
    /// diagnostic this pulls forward. Prefer
    /// [`Self::build_extract_value`] when the index count is known at
    /// compile time, which upgrades this runtime check to a compile error.
    pub fn build_extract_value_dyn<V, Name>(
        &self,
        aggregate: V,
        indices: &[u32],
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        V: IsValue<'ctx, B>,
    {
        let agg = aggregate.as_value();
        if indices.is_empty() {
            return Err(IrError::InvalidOperation {
                message: "extractvalue indices must not be empty",
            });
        }
        let leaf_ty = walk_aggregate_for_builder(self.module, agg.ty, indices)?;
        if let Some(folded) = self.folder.fold_extract_value_dyn(agg, indices)? {
            return self.checked_folded_value(folded, leaf_ty);
        }
        let payload = crate::instr_types::ExtractValueInstData::new(agg.id, indices.to_vec());
        let inst =
            self.append_instruction(leaf_ty, InstructionKindData::ExtractValue(payload), name);
        Ok(inst.as_value())
    }

    /// Produce `insertvalue <agg-ty> <agg>, <elt-ty> <elt>, idx0, ...`.
    /// Mirrors `IRBuilder::CreateInsertValue`.
    ///
    /// The index list is a fixed-size array whose length is checked at
    /// compile time (Doctrine D3): `InsertValueInst::init`
    /// (`lib/IR/Instructions.cpp`) asserts a non-empty index list, and
    /// `N > 0` pulls that assertion forward to monomorphisation instead of
    /// a runtime error. Use [`Self::build_insert_value_dyn`] for a
    /// slice/`Vec`-driven index list that keeps the runtime check.
    pub fn build_insert_value<A, V, const N: usize, Name>(
        &self,
        aggregate: A,
        value: V,
        indices: [u32; N],
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        A: IsValue<'ctx, B>,
        V: IsValue<'ctx, B>,
    {
        const {
            assert!(N > 0, "insertvalue requires at least one index");
        }
        self.build_insert_value_dyn(aggregate, value, &indices, name)
    }

    /// Produce `insertvalue <agg-ty> <agg>, <elt-ty> <elt>, idx0, ...` from
    /// a dynamically-sized index slice. Mirrors
    /// `IRBuilder::CreateInsertValue`.
    ///
    /// Ports the empty-index-list rejection in `InsertValueInst::init`
    /// (`lib/IR/Instructions.cpp`); see
    /// `test/Assembler/extractvalue-no-idx.ll` for the upstream assembler
    /// diagnostic this pulls forward (the parser shares one "expected
    /// index" path for both opcodes). Prefer [`Self::build_insert_value`]
    /// when the index count is known at compile time, which upgrades this
    /// runtime check to a compile error.
    pub fn build_insert_value_dyn<A, V, Name>(
        &self,
        aggregate: A,
        value: V,
        indices: &[u32],
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        A: IsValue<'ctx, B>,
        V: IsValue<'ctx, B>,
    {
        let agg = aggregate.as_value();
        let val = value.as_value();
        if indices.is_empty() {
            return Err(IrError::InvalidOperation {
                message: "insertvalue indices must not be empty",
            });
        }
        let leaf_ty = walk_aggregate_for_builder(self.module, agg.ty, indices)?;
        if val.ty != leaf_ty {
            return Err(IrError::TypeMismatch {
                expected: Type::new(leaf_ty, self.module).kind_label(),
                got: val.ty().kind_label(),
            });
        }
        if let Some(folded) = self.folder.fold_insert_value_dyn(agg, val, indices)? {
            return self.checked_folded_value(folded, agg.ty);
        }
        let payload =
            crate::instr_types::InsertValueInstData::new(agg.id, val.id, indices.to_vec());
        let inst = self.append_instruction(agg.ty, InstructionKindData::InsertValue(payload), name);
        Ok(inst.as_value())
    }

    /// Extract a named-struct schema field and return the field's typed wrapper.
    pub fn build_extract_field<S, Field, Aggregate, Name>(
        &self,
        aggregate: Aggregate,
        index: u32,
        name: Name,
    ) -> IrResult<Field::Value<'ctx, B>>
    where
        S: StructSchema,
        Field: IrField,
        Aggregate: IntoIrField<'ctx, S, B>,
        Name: AsRef<str>,
    {
        let module = ModuleRef::new(self.module);
        let aggregate = aggregate.into_ir_field(module)?;
        let leaf_ty = walk_aggregate_for_builder(self.module, aggregate.ty, &[index])?;
        let leaf = Type::new(leaf_ty, self.module);
        if !Field::matches_ir_type(leaf) {
            return Err(IrError::TypeMismatch {
                expected: Field::expected_kind_label(),
                got: leaf.kind_label(),
            });
        }
        let raw = self.build_extract_value(aggregate, [index], name)?;
        Field::value_from_ir_value(raw)
    }

    /// Insert a typed field value into a named-struct schema aggregate.
    pub fn build_insert_field<S, Field, Aggregate, FieldValue, Name>(
        &self,
        aggregate: Aggregate,
        value: FieldValue,
        index: u32,
        name: Name,
    ) -> IrResult<S::Value<'ctx, B>>
    where
        S: StructSchema,
        Field: IrField,
        Aggregate: IntoIrField<'ctx, S, B>,
        FieldValue: IntoIrField<'ctx, Field, B>,
        Name: AsRef<str>,
    {
        let module = ModuleRef::new(self.module);
        let aggregate = aggregate.into_ir_field(module)?;
        let value = value.into_ir_field(module)?;
        let raw = self.build_insert_value(aggregate, value, [index], name)?;
        <S as IrField>::value_from_ir_value(raw)
    }

    // ---- Vector ops: extractelement / insertelement / shufflevector ----

    /// Produce `extractelement <vec-ty> <vec>, <idx-ty> <idx>`.
    /// Mirrors `IRBuilder::CreateExtractElement`.
    pub fn build_extract_element<V, W, I, Name>(
        &self,
        vector: V,
        index: I,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        V: IsValue<'ctx, B>,
        W: crate::int_width::IntWidth,
        I: IntoIntValue<'ctx, W, B>,
    {
        let vec = vector.as_value();
        let idx_v = index.into_int_value(ModuleRef::new(self.module))?;
        let idx = IsValue::as_value(idx_v);
        let elem_ty = match self.module.context().type_data(vec.ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(IrError::TypeMismatch {
                    expected: crate::error::TypeKindLabel::FixedVector,
                    got: vec.ty().kind_label(),
                });
            }
        };
        if let Some(folded) = self.folder.fold_extract_element_dyn(vec, idx)? {
            return self.checked_folded_value(folded, elem_ty);
        }
        let payload = crate::instr_types::ExtractElementInstData::new(vec.id, idx.id);
        let inst =
            self.append_instruction(elem_ty, InstructionKindData::ExtractElement(payload), name);
        Ok(inst.as_value())
    }

    /// Produce `insertelement <vec-ty> <vec>, <elt-ty> <elt>, <idx-ty> <idx>`.
    /// Mirrors `IRBuilder::CreateInsertElement`.
    pub fn build_insert_element<V, E, W, I, Name>(
        &self,
        vector: V,
        elt: E,
        index: I,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        V: IsValue<'ctx, B>,
        E: IsValue<'ctx, B>,
        W: crate::int_width::IntWidth,
        I: IntoIntValue<'ctx, W, B>,
    {
        let vec = vector.as_value();
        let val = elt.as_value();
        let idx_v = index.into_int_value(ModuleRef::new(self.module))?;
        let idx = IsValue::as_value(idx_v);
        if let Some(folded) = self.folder.fold_insert_element_dyn(vec, val, idx)? {
            return self.checked_folded_value(folded, vec.ty);
        }
        let payload = crate::instr_types::InsertElementInstData::new(vec.id, val.id, idx.id);
        let inst =
            self.append_instruction(vec.ty, InstructionKindData::InsertElement(payload), name);
        Ok(inst.as_value())
    }

    /// Produce `shufflevector <ty> <v1>, <ty> <v2>, <mask>`. Mirrors
    /// `IRBuilder::CreateShuffleVector`. The mask is a slice of `i32`s;
    /// pass `[`[`crate::instr_types::POISON_MASK_ELEM`]`; ...]` for
    /// poison entries.
    pub fn build_shuffle_vector<L, Rhs2, Name>(
        &self,
        lhs: L,
        rhs: Rhs2,
        mask: &[i32],
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        L: IsValue<'ctx, B>,
        Rhs2: IsValue<'ctx, B>,
    {
        let l = lhs.as_value();
        let r = rhs.as_value();
        if l.ty != r.ty {
            return Err(IrError::TypeMismatch {
                expected: l.ty().kind_label(),
                got: r.ty().kind_label(),
            });
        }
        let elem = match self.module.context().type_data(l.ty).as_vector() {
            Some((e, _, scalable)) => {
                if scalable {
                    return Err(IrError::InvalidOperation {
                        message: "shufflevector with scalable input is not yet supported",
                    });
                }
                e
            }
            None => {
                return Err(IrError::TypeMismatch {
                    expected: crate::error::TypeKindLabel::FixedVector,
                    got: l.ty().kind_label(),
                });
            }
        };
        let mask_len = u32::try_from(mask.len()).map_err(|_| IrError::InvalidOperation {
            message: "shufflevector mask too large",
        })?;
        let result_ty_id = self.module.context().fixed_vector_type(elem, mask_len);
        if let Some(folded) = self.folder.fold_shuffle_vector_dyn(l, r, mask)? {
            return self.checked_folded_value(folded, result_ty_id);
        }
        let payload =
            crate::instr_types::ShuffleVectorInstData::new(l.id, r.id, mask.iter().copied());
        let inst = self.append_instruction(
            result_ty_id,
            InstructionKindData::ShuffleVector(payload),
            name,
        );
        Ok(inst.as_value())
    }

    // ---- Atomic ops: fence / cmpxchg / atomicrmw ----

    /// Produce `fence <ordering>` (or
    /// `fence syncscope("...") <ordering>`). Mirrors
    /// `IRBuilder::CreateFence`.
    pub fn build_fence<Name>(
        &self,
        ordering: AtomicOrdering,
        sync_scope: SyncScope,
        name: Name,
    ) -> IrResult<crate::instructions::FenceInst<'ctx>>
    where
        Name: AsRef<str>,
    {
        let payload = crate::instr_types::FenceInstData::new(ordering, sync_scope);
        let void_ty = self.module.void_type().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Fence(payload), name);
        Ok(crate::instructions::FenceInst::from_raw(
            inst.as_value().id,
            self.module,
            void_ty,
        ))
    }

    /// Produce `cmpxchg [weak] [volatile] <ptr-ty> <ptr>, <cmp-ty> <cmp>,
    /// <new-ty> <new> [syncscope("...")] <success> <failure>, align N`.
    /// Mirrors `IRBuilder::CreateAtomicCmpXchg`.
    ///
    /// Result type is the literal struct `{ <pointee>, i1 }`.
    pub fn build_atomic_cmpxchg<P, C, N, Name>(
        &self,
        ptr: P,
        cmp: C,
        new_val: N,
        config: crate::instr_types::AtomicCmpXchgConfig,
        name: Name,
    ) -> IrResult<AtomicCmpXchgInst<'ctx, B>>
    where
        Name: AsRef<str>,
        P: IsValue<'ctx, B>,
        C: IsValue<'ctx, B>,
        N: IsValue<'ctx, B>,
    {
        let p = ptr.as_value();
        let c = cmp.as_value();
        let n = new_val.as_value();
        if c.ty != n.ty {
            return Err(IrError::TypeMismatch {
                expected: c.ty().kind_label(),
                got: n.ty().kind_label(),
            });
        }
        let module_view = ModuleView::<B>::new(self.module);
        let result_ty = module_view.struct_type([c.ty(), module_view.bool_type().as_type()], false);
        let payload = crate::instr_types::AtomicCmpXchgInstData::new(p.id, c.id, n.id, config);
        let result_id = result_ty.as_type().id();
        let inst =
            self.append_instruction(result_id, InstructionKindData::AtomicCmpXchg(payload), name);
        Ok(AtomicCmpXchgInst::<B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            result_id,
        ))
    }

    /// Produce `atomicrmw [volatile] <op> <ptr-ty> <ptr>, <val-ty> <val>
    /// [syncscope("...")] <ordering>, align N`. Mirrors
    /// `IRBuilder::CreateAtomicRMW`.
    ///
    /// Result type matches the value-operand type (the "old" value).
    pub fn build_atomicrmw<P, V, Name>(
        &self,
        op: crate::atomicrmw_binop::AtomicRMWBinOp,
        ptr: P,
        value: V,
        config: crate::instr_types::AtomicRMWConfig,
        name: Name,
    ) -> IrResult<AtomicRMWInst<'ctx, B>>
    where
        Name: AsRef<str>,
        P: IsValue<'ctx, B>,
        V: IsValue<'ctx, B>,
    {
        let p = ptr.as_value();
        let v = value.as_value();
        let payload = crate::instr_types::AtomicRMWInstData::new(op, p.id, v.id, config);
        let inst = self.append_instruction(v.ty, InstructionKindData::AtomicRMW(payload), name);
        Ok(AtomicRMWInst::<B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            v.ty,
        ))
    }

    // ---- Casts: trunc / zext / sext ----

    /// Produce `trunc <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateTrunc`.
    ///
    /// The `Src: WiderThan<Dst>` bound enforces at compile time that
    /// the destination is strictly narrower than the source. Cross-
    /// width attempts (e.g. `build_trunc::<i32, i64>`) fail to
    /// compile rather than returning a runtime
    /// [`IrError::OperandWidthMismatch`]. Use
    /// [`Self::build_trunc_dyn`] when both widths are erased.
    ///
    pub fn build_trunc<Src, Dst, Name>(
        &self,
        value: IntValue<'ctx, Src, B>,
        dst_ty: IntType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: crate::int_width::WiderThan<Dst>,
        Dst: IntWidth,
    {
        if let Some(folded) = self.folder.fold_cast_to_int(
            crate::instr_types::CastOpcode::Trunc,
            value.as_value(),
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::Trunc, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst, B>::from_value_unchecked(inst.as_value()))
    }

    /// `trunc nuw/nsw` with explicit [`crate::TruncFlags`]. Mirrors
    /// `IRBuilder::CreateTrunc` plus `Instruction::setHasNoUnsignedWrap` /
    /// `setHasNoSignedWrap`.
    ///
    /// The `Src: WiderThan<Dst>` bound is the same one [`Self::build_trunc`]
    /// uses, enforced at compile time. Upstream `IRBuilder::CreateTrunc`
    /// (`IRBuilder.cpp`) returns `V` unchanged -- silently dropping any
    /// requested `nuw`/`nsw` -- when `SrcTy == DestTy`. Because `WiderThan`
    /// requires `Src` strictly wider than `Dst`, that same-type case is
    /// unspellable through this method: the flag-dropping branch cannot
    /// arise here (D10 -- no silent bad-codegen). Use
    /// [`Self::build_trunc_with_flags_dyn`] when both widths are erased.
    pub fn build_trunc_with_flags<Src, Dst, Name>(
        &self,
        value: IntValue<'ctx, Src, B>,
        dst_ty: IntType<'ctx, Dst, B>,
        flags: crate::instr_types::TruncFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: crate::int_width::WiderThan<Dst>,
        Dst: IntWidth,
    {
        if let Some(folded) = self.folder.fold_cast_to_int(
            crate::instr_types::CastOpcode::Trunc,
            value.as_value(),
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::Trunc, value.as_value().id);
        payload.nuw.set(flags.nuw);
        payload.nsw.set(flags.nsw);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `zext <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateZExt`.
    ///
    /// The `Dst: WiderThan<Src>` bound enforces at compile time that
    /// the destination is strictly wider than the source. Use
    /// [`Self::build_zext_dyn`] when both widths are erased.
    pub fn build_zext<Src, Dst, Name>(
        &self,
        value: IntValue<'ctx, Src, B>,
        dst_ty: IntType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: IntWidth,
        Dst: crate::int_width::WiderThan<Src>,
    {
        if let Some(folded) = self.folder.fold_cast_to_int(
            crate::instr_types::CastOpcode::ZExt,
            value.as_value(),
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::ZExt, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst, B>::from_value_unchecked(inst.as_value()))
    }

    /// `zext nneg` with explicit [`crate::ZExtFlags`]. Mirrors
    /// `IRBuilder::CreateZExt` plus `Instruction::setNonNeg`.
    ///
    /// The `Dst: WiderThan<Src>` bound is the same one [`Self::build_zext`]
    /// uses, enforced at compile time. Use [`Self::build_zext_with_flags_dyn`]
    /// when both widths are erased.
    pub fn build_zext_with_flags<Src, Dst, Name>(
        &self,
        value: IntValue<'ctx, Src, B>,
        dst_ty: IntType<'ctx, Dst, B>,
        flags: crate::instr_types::ZExtFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: IntWidth,
        Dst: crate::int_width::WiderThan<Src>,
    {
        if let Some(folded) = self.folder.fold_cast_to_int(
            crate::instr_types::CastOpcode::ZExt,
            value.as_value(),
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::ZExt, value.as_value().id);
        payload.nneg.set(flags.nneg);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `sext <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateSExt`.
    ///
    /// The `Dst: WiderThan<Src>` bound enforces at compile time that
    /// the destination is strictly wider than the source. Use
    /// [`Self::build_sext_dyn`] when both widths are erased.
    pub fn build_sext<Src, Dst, Name>(
        &self,
        value: IntValue<'ctx, Src, B>,
        dst_ty: IntType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: IntWidth,
        Dst: crate::int_width::WiderThan<Src>,
    {
        if let Some(folded) = self.folder.fold_cast_to_int(
            crate::instr_types::CastOpcode::SExt,
            value.as_value(),
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::SExt, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst, B>::from_value_unchecked(inst.as_value()))
    }

    // ---- Dyn fallbacks (runtime-checked) ----

    /// Runtime-checked `trunc` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly narrower than `value`'s runtime width.
    pub fn build_trunc_dyn<Name>(
        &self,
        value: IntValue<'ctx, IntDyn, B>,
        dst_ty: IntType<'ctx, IntDyn, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>>
    where
        Name: AsRef<str>,
    {
        let src_w = value.ty().bit_width();
        let dst_w = dst_ty.bit_width();
        if dst_w >= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        if let Some(folded) = self.folder.fold_cast_dyn(
            crate::instr_types::CastOpcode::Trunc,
            value.as_value(),
            dst_ty.as_type(),
        )? {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(IntValue::<IntDyn, B>::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::Trunc, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<IntDyn, B>::from_value_unchecked(inst.as_value()))
    }

    /// `trunc nuw/nsw` with explicit [`crate::TruncFlags`]. Runtime-checked
    /// like [`Self::build_trunc_dyn`]; additionally sets `nuw`/`nsw` on the
    /// cast payload.
    pub fn build_trunc_with_flags_dyn<Name>(
        &self,
        value: IntValue<'ctx, IntDyn, B>,
        dst_ty: IntType<'ctx, IntDyn, B>,
        flags: crate::instr_types::TruncFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>>
    where
        Name: AsRef<str>,
    {
        let src_w = value.ty().bit_width();
        let dst_w = dst_ty.bit_width();
        if dst_w >= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        if let Some(folded) = self.folder.fold_cast_dyn(
            crate::instr_types::CastOpcode::Trunc,
            value.as_value(),
            dst_ty.as_type(),
        )? {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(IntValue::<IntDyn, B>::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::Trunc, value.as_value().id);
        payload.nuw.set(flags.nuw);
        payload.nsw.set(flags.nsw);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<IntDyn, B>::from_value_unchecked(inst.as_value()))
    }

    /// Runtime-checked `zext` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly wider than `value`'s runtime width.
    pub fn build_zext_dyn<Name>(
        &self,
        value: IntValue<'ctx, IntDyn, B>,
        dst_ty: IntType<'ctx, IntDyn, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_extend_dyn(value, dst_ty, name, crate::instr_types::CastOpcode::ZExt)
    }

    /// `zext nneg` with explicit [`crate::ZExtFlags`]. Runtime-checked
    /// like [`Self::build_zext_dyn`]; additionally sets `nneg` on the cast
    /// payload.
    pub fn build_zext_with_flags_dyn<Name>(
        &self,
        src: IntValue<'ctx, IntDyn, B>,
        dst: IntType<'ctx, IntDyn, B>,
        flags: crate::instr_types::ZExtFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>>
    where
        Name: AsRef<str>,
    {
        let src_w = src.ty().bit_width();
        let dst_w = dst.bit_width();
        if dst_w <= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        if let Some(folded) = self.folder.fold_cast_dyn(
            crate::instr_types::CastOpcode::ZExt,
            src.as_value(),
            dst.as_type(),
        )? {
            let folded = self.checked_folded_value(folded, dst.as_type().id())?;
            return Ok(IntValue::<IntDyn, B>::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::ZExt, src.as_value().id);
        payload.nneg.set(flags.nneg);
        let inst =
            self.append_instruction(dst.as_type().id(), InstructionKindData::Cast(payload), name);
        Ok(IntValue::<IntDyn, B>::from_value_unchecked(inst.as_value()))
    }

    /// Runtime-checked `sext` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly wider than `value`'s runtime width.
    pub fn build_sext_dyn<Name>(
        &self,
        value: IntValue<'ctx, IntDyn, B>,
        dst_ty: IntType<'ctx, IntDyn, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>>
    where
        Name: AsRef<str>,
    {
        self.build_int_extend_dyn(value, dst_ty, name, crate::instr_types::CastOpcode::SExt)
    }

    /// Crate-internal helper for `build_zext_dyn` / `build_sext_dyn`.
    fn build_int_extend_dyn(
        &self,
        value: IntValue<'ctx, IntDyn, B>,
        dst_ty: IntType<'ctx, IntDyn, B>,
        name: impl AsRef<str>,
        opcode: crate::instr_types::CastOpcode,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>> {
        let src_w = value.ty().bit_width();
        let dst_w = dst_ty.bit_width();
        if dst_w <= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(opcode, value.as_value(), dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(IntValue::<IntDyn, B>::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(opcode, value.as_value().id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<IntDyn, B>::from_value_unchecked(inst.as_value()))
    }

    // ---- Memory: alloca / load / store ----

    /// The DataLayout ABI alignment of a type, materialised so load/store
    /// carry an explicit `align` like upstream (`computeLoadStoreDefaultAlign`
    /// = `getABITypeAlign`).
    fn default_abi_align(&self, ty_id: TypeId) -> MaybeAlign {
        let dl = self.module.data_layout();
        MaybeAlign::new(dl.abi_align_of_id(self.module, ty_id))
    }

    /// The DataLayout preferred alignment of a type, materialised so alloca
    /// carries an explicit `align` like upstream (`computeAllocaDefaultAlign`
    /// = `getPrefTypeAlign`).
    fn default_pref_align(&self, ty_id: TypeId) -> MaybeAlign {
        let dl = self.module.data_layout();
        MaybeAlign::new(dl.pref_align_of_id(self.module, ty_id))
    }

    /// The DataLayout alloca address space (`IRBuilder::CreateAlloca` uses
    /// `getAllocaAddrSpace`).
    fn alloca_addr_space(&self) -> u32 {
        self.module.data_layout().alloca_addr_space()
    }

    /// Produce `alloca <ty>`. Mirrors `IRBuilder::CreateAlloca`.
    /// The result is a `ptr` in the DataLayout's alloca address space, with
    /// the type's preferred alignment materialised.
    pub fn build_alloca<T, Name>(&self, ty: T, name: Name) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
    {
        self.build_alloca_inner(
            ty.as_type().id(),
            None,
            MaybeAlign::NONE,
            self.alloca_addr_space(),
            name,
        )
    }

    /// Produce `alloca <ty>, <size-ty> <num_elements>`. Mirrors
    /// `IRBuilder::CreateAlloca` with an array-size operand.
    pub fn build_array_alloca<T, N, Name>(
        &self,
        ty: T,
        num_elements: N,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        N: IntoIntValue<'ctx, IntDyn, B>,
    {
        let n = num_elements.into_int_value(ModuleRef::new(self.module))?;
        self.build_alloca_inner(
            ty.as_type().id(),
            Some(n.as_value().id),
            MaybeAlign::NONE,
            self.alloca_addr_space(),
            name,
        )
    }

    /// Produce `alloca <ty>, align <N>`. Mirrors
    /// `IRBuilder::CreateAlignedAlloca`.
    pub fn build_alloca_with_align<T, Name>(
        &self,
        ty: T,
        align: Align,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
    {
        self.build_alloca_inner(
            ty.as_type().id(),
            None,
            MaybeAlign::new(align),
            self.alloca_addr_space(),
            name,
        )
    }

    fn build_alloca_inner(
        &self,
        allocated_ty: TypeId,
        num_elements: Option<ValueId>,
        align: MaybeAlign,
        addr_space: u32,
        name: impl AsRef<str>,
    ) -> IrResult<PointerValue<'ctx, B>> {
        // Materialise the DataLayout preferred alignment when omitted, like
        // upstream — every alloca funnels through here
        // (`computeAllocaDefaultAlign`).
        let align = if align.align().is_none() {
            self.default_pref_align(allocated_ty)
        } else {
            align
        };
        let payload =
            crate::instr_types::AllocaInstData::new(allocated_ty, num_elements, align, addr_space);
        let ptr_ty = self.module.ptr_type(addr_space).as_type().id();
        let inst = self.append_instruction(ptr_ty, InstructionKindData::Alloca(payload), name);
        Ok(PointerValue::from_value_unchecked(inst.as_value()))
    }

    /// `alloca` for schema `T`, returning a pointee-typed pointer. The
    /// pointee schema `T` is Rust-side bookkeeping only -- the emitted
    /// IR is identical to [`Self::build_alloca`] with `T::ir_type`.
    /// Mirrors `IRBuilder::CreateAlloca` + the Rust-side
    /// [`TypedPointerValue`] overlay.
    pub fn build_typed_alloca<T, Name>(&self, name: Name) -> IrResult<TypedPointerValue<'ctx, T, B>>
    where
        T: IrField,
        Name: AsRef<str>,
    {
        let ty = T::ir_type(&Module::from_core(self.module))?;
        let ptr = self.build_alloca(ty, name)?;
        Ok(ptr.with_pointee::<T>())
    }

    /// Erased load: `load <ty>, ptr <ptr>`. Result type is whatever
    /// `ty` decodes to at runtime; returned as a [`Value`] handle the
    /// caller narrows via `try_into()`. Mirrors
    /// `IRBuilder::CreateLoad`.
    pub fn build_load<T, P, Name>(&self, ty: T, ptr: P, name: Name) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty_id = ty.as_type().id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty_id,
            IsValue::as_value(p).id,
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(inst.as_value())
    }

    /// `load <ty>, ptr <ptr>, align N`. Non-volatile non-atomic load with explicit
    /// alignment. Mirrors `IRBuilder::CreateLoad` with an explicit `Align` slot.
    pub fn build_load_with_align<T, P, Name>(
        &self,
        ty: T,
        ptr: P,
        align: Align,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty_id = ty.as_type().id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty_id,
            IsValue::as_value(p).id,
            MaybeAlign::new(align),
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(inst.as_value())
    }

    /// Typed integer load: `load iN, ptr <ptr>`. Marker-only form:
    /// the result type comes from `W` via [`crate::StaticIntWidth`].
    /// Mirrors `IRBuilder::CreateLoad` with a fixed integer width.
    pub fn build_int_load<W, P, Name>(&self, ptr: P, name: Name) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: crate::int_width::StaticIntWidth,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = W::ir_type(ModuleRef::<B>::new(self.module));
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            IsValue::as_value(p).id,
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Runtime-width integer load. Takes the type explicitly because
    /// the [`crate::IntDyn`] marker carries no static width.
    pub fn build_int_load_dyn<P, Name>(
        &self,
        ty: IntType<'ctx, IntDyn, B>,
        ptr: P,
        name: Name,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            IsValue::as_value(p).id,
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(IntValue::<IntDyn, B>::from_value_unchecked(inst.as_value()))
    }

    /// Typed float load: `load <fpty>, ptr <ptr>`. Marker-only.
    pub fn build_fp_load<K, P, Name>(&self, ptr: P, name: Name) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        K: crate::float_kind::StaticFloatKind,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = K::ir_type(ModuleRef::<B>::new(self.module));
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            IsValue::as_value(p).id,
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(FloatValue::<K, B>::from_value_unchecked(inst.as_value()))
    }

    /// Runtime-kind float load. Takes the type explicitly because
    /// [`crate::FloatDyn`] carries no static kind.
    pub fn build_fp_load_dyn<P, Name>(
        &self,
        ty: FloatType<'ctx, FloatDyn, B>,
        ptr: P,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, FloatDyn, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            IsValue::as_value(p).id,
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Pointer-typed load: `load ptr, ptr <ptr>`. Pointer types are
    /// uniform (only address space varies); the loaded ptr is in the
    /// default address space. Use [`Self::build_load`] erased form for
    /// other address spaces.
    pub fn build_pointer_load<P, Name>(&self, ptr: P, name: Name) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = self.module.ptr_type(0);
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            IsValue::as_value(p).id,
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(PointerValue::from_value_unchecked(inst.as_value()))
    }

    /// Same as [`Self::build_int_load`] plus an explicit alignment.
    pub fn build_int_load_with_align<W, P, Name>(
        &self,
        ptr: P,
        align: Align,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: crate::int_width::StaticIntWidth,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = W::ir_type(ModuleRef::<B>::new(self.module));
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            IsValue::as_value(p).id,
            MaybeAlign::new(align),
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Typed `load`: the result type is derived from the pointer's
    /// schema `T`. Mirrors `IRBuilder::CreateLoad` + the Rust-side
    /// [`TypedPointerValue`] overlay.
    pub fn build_typed_load<T, Name>(
        &self,
        ptr: TypedPointerValue<'ctx, T, B>,
        name: Name,
    ) -> IrResult<T::Value<'ctx, B>>
    where
        T: IrField,
        Name: AsRef<str>,
    {
        let ty = T::ir_type(&Module::from_core(self.module))?;
        let raw = self.build_load(ty, ptr.as_pointer_value(), name)?;
        T::value_from_ir_value(raw)
    }

    /// Same as [`Self::build_typed_load`] plus an explicit alignment.
    pub fn build_typed_load_with_align<T, Name>(
        &self,
        ptr: TypedPointerValue<'ctx, T, B>,
        align: Align,
        name: Name,
    ) -> IrResult<T::Value<'ctx, B>>
    where
        T: IrField,
        Name: AsRef<str>,
    {
        let ty = T::ir_type(&Module::from_core(self.module))?;
        let raw = self.build_load_with_align(ty, ptr.as_pointer_value(), align, name)?;
        T::value_from_ir_value(raw)
    }

    fn build_load_inner(
        &self,
        mut payload: LoadInstData,
        name: impl AsRef<str>,
    ) -> IrResult<Instruction<'ctx, Attached, B>> {
        let pointee_ty = payload.pointee_ty;
        // Materialise the DataLayout default like upstream — every load
        // (plain / volatile / atomic) funnels through here, so an omitted
        // alignment is filled once (`computeLoadStoreDefaultAlign`).
        if payload.align.align().is_none() {
            payload.align = self.default_abi_align(pointee_ty);
        }
        Ok(self.append_instruction(pointee_ty, InstructionKindData::Load(payload), name))
    }

    /// `load volatile <ty>, ptr <ptr>`. Non-atomic volatile load.
    /// Mirrors `IRBuilder::CreateLoad` with `isVolatile = true`.
    pub fn build_load_volatile<T, P, Name>(
        &self,
        ty: T,
        ptr: P,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty_id = ty.as_type().id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty_id,
            IsValue::as_value(p).id,
            MaybeAlign::NONE,
            true,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(inst.as_value())
    }

    /// `load volatile <ty>, ptr <ptr>, align N`. Volatile load with explicit
    /// alignment.
    pub fn build_load_volatile_with_align<T, P, Name>(
        &self,
        ty: T,
        ptr: P,
        align: Align,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty_id = ty.as_type().id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty_id,
            IsValue::as_value(p).id,
            MaybeAlign::new(align),
            true,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(inst.as_value())
    }

    /// Produce `store <value>, ptr <ptr>`. Mirrors
    /// `IRBuilder::CreateStore`.
    pub fn build_store<V, P>(&self, value: V, ptr: P) -> IrResult<StoreInst<'ctx, B>>
    where
        V: IsValue<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let payload = self.store_payload(
            value,
            ptr,
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        )?;
        self.build_store_inner(payload)
    }

    /// Same as `build_store` plus an explicit alignment slot.
    pub fn build_store_with_align<V, P>(
        &self,
        value: V,
        ptr: P,
        align: Align,
    ) -> IrResult<StoreInst<'ctx, B>>
    where
        V: IsValue<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let payload = self.store_payload(
            value,
            ptr,
            MaybeAlign::new(align),
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        )?;
        self.build_store_inner(payload)
    }

    /// Typed `store`: the value lifts through the schema's
    /// [`IntoIrField`]. Mirrors `IRBuilder::CreateStore` + the
    /// Rust-side [`TypedPointerValue`] overlay.
    pub fn build_typed_store<T, V>(
        &self,
        value: V,
        ptr: TypedPointerValue<'ctx, T, B>,
    ) -> IrResult<StoreInst<'ctx, B>>
    where
        T: IrField,
        V: IntoIrField<'ctx, T, B>,
    {
        let v = value.into_ir_field(ModuleRef::new(self.module))?;
        self.build_store(v, ptr.as_pointer_value())
    }

    /// Same as [`Self::build_typed_store`] plus an explicit alignment slot.
    pub fn build_typed_store_with_align<T, V>(
        &self,
        value: V,
        ptr: TypedPointerValue<'ctx, T, B>,
        align: Align,
    ) -> IrResult<StoreInst<'ctx, B>>
    where
        T: IrField,
        V: IntoIrField<'ctx, T, B>,
    {
        let v = value.into_ir_field(ModuleRef::new(self.module))?;
        self.build_store_with_align(v, ptr.as_pointer_value(), align)
    }

    /// `store volatile <value>, ptr <ptr>`. Non-atomic volatile store.
    /// Mirrors `IRBuilder::CreateStore(V, P, /*isVolatile=*/true)`.
    pub fn build_store_volatile<V, P>(&self, value: V, ptr: P) -> IrResult<StoreInst<'ctx, B>>
    where
        V: IsValue<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let payload = self.store_payload(
            value,
            ptr,
            MaybeAlign::NONE,
            true,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        )?;
        self.build_store_inner(payload)
    }

    /// `store volatile <value>, ptr <ptr>, align N`. Volatile store with
    /// explicit alignment.
    pub fn build_store_volatile_with_align<V, P>(
        &self,
        value: V,
        ptr: P,
        align: Align,
    ) -> IrResult<StoreInst<'ctx, B>>
    where
        V: IsValue<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let payload = self.store_payload(
            value,
            ptr,
            MaybeAlign::new(align),
            true,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        )?;
        self.build_store_inner(payload)
    }

    /// Inner store: caller has already computed the payload and validated
    /// the pointer/value modules. Single-arg helper used by the four
    /// public store builders.
    fn build_store_inner(&self, payload: StoreInstData) -> IrResult<StoreInst<'ctx, B>> {
        let void_ty = self.module.void_type().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Store(payload), "");
        Ok(StoreInst::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        ))
    }

    fn store_payload<V, P>(
        &self,
        value: V,
        ptr: P,
        align: MaybeAlign,
        volatile: bool,
        ordering: AtomicOrdering,
        sync_scope: SyncScope,
    ) -> IrResult<StoreInstData>
    where
        V: IsValue<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let v = value.as_value();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        // Materialise the DataLayout default off the stored value's type,
        // like upstream (`computeLoadStoreDefaultAlign` /
        // `getABITypeAlign(Val->getType())`). Every store funnels through
        // here, so an omitted alignment is filled once.
        let align = if align.align().is_none() {
            self.default_abi_align(v.ty().id())
        } else {
            align
        };
        Ok(StoreInstData::new(
            v.id,
            IsValue::as_value(p).id,
            align,
            volatile,
            ordering,
            sync_scope,
        ))
    }

    /// Atomic load: `load atomic [volatile] iN, ptr <ptr> [syncscope(\"...\")]
    /// <ordering>, align N`. Mirrors the 5-arg upstream constructor
    /// `LoadInst::LoadInst(Type*, Value*, Twine&, bool isVolatile, Align,
    /// AtomicOrdering, SyncScope::ID)` (see `lib/IR/Instructions.cpp`)
    /// inserted via the IRBuilder's standard insert-point. Atomic loads
    /// require an explicit alignment per LangRef. The atomic-specific
    /// state (ordering, sync scope, align, volatile) is bundled into
    /// [`super::instr_types::AtomicLoadConfig`] (parallel to the existing
    /// [`super::instr_types::AtomicCmpXchgConfig`] /
    /// [`super::instr_types::AtomicRMWConfig`] shapes).
    pub fn build_int_load_atomic<W, P, Name>(
        &self,
        ptr: P,
        config: super::instr_types::AtomicLoadConfig,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = W::ir_type(ModuleRef::<B>::new(self.module));
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            IsValue::as_value(p).id,
            MaybeAlign::new(config.align_value()),
            config.is_volatile(),
            config.ordering_value(),
            config.sync_scope_value().clone(),
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Erased atomic load. Same upstream constructor as
    /// [`Self::build_int_load_atomic`] but with an explicit pointee type
    /// (caller narrows the returned [`Value`]).
    pub fn build_load_atomic<T, P, Name>(
        &self,
        ty: T,
        ptr: P,
        config: super::instr_types::AtomicLoadConfig,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty_id = ty.as_type().id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty_id,
            IsValue::as_value(p).id,
            MaybeAlign::new(config.align_value()),
            config.is_volatile(),
            config.ordering_value(),
            config.sync_scope_value().clone(),
        );
        let inst = self.build_load_inner(payload, name)?;
        Ok(inst.as_value())
    }

    /// Atomic store: `store atomic [volatile] <ty> <val>, ptr <ptr>
    /// [syncscope("...")] <ordering>, align N`. Mirrors the 6-arg upstream
    /// `StoreInst::StoreInst(Value*, Value*, bool isVolatile, Align,
    /// AtomicOrdering, SyncScope::ID)` constructor (see
    /// `lib/IR/Instructions.cpp`). Atomic stores require an explicit
    /// alignment carried in [`super::instr_types::AtomicStoreConfig`].
    pub fn build_store_atomic<V, P>(
        &self,
        value: V,
        ptr: P,
        config: super::instr_types::AtomicStoreConfig,
    ) -> IrResult<StoreInst<'ctx, B>>
    where
        V: IsValue<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let payload = self.store_payload(
            value,
            ptr,
            MaybeAlign::new(config.align_value()),
            config.is_volatile(),
            config.ordering_value(),
            config.sync_scope_value().clone(),
        )?;
        self.build_store_inner(payload)
    }

    /// Ports the `CallInst::init` / `CallBrInst::init` assertions
    /// ("Calling a function with a bad signature!",
    /// `lib/IR/Instructions.cpp`) and `Verifier::visitCallBase`'s
    /// authoritative arity/type check to build time: argument count
    /// must equal the parameter count exactly (or be at least the
    /// parameter count for a vararg callee), and each fixed argument's
    /// type must equal the parameter type at that position exactly.
    /// Shared by every dyn call/invoke/callbr/inline-asm builder path.
    fn validate_call_site_args(
        &self,
        fn_ty: FunctionType<'ctx, B>,
        args: &[ValueId],
    ) -> IrResult<()> {
        let params: Vec<Type<'ctx, B>> = fn_ty.params().collect();
        let expected = u32::try_from(params.len())
            .unwrap_or_else(|_| unreachable!("parameter count bounded by u32"));
        let got = u32::try_from(args.len())
            .unwrap_or_else(|_| unreachable!("argument count bounded by u32"));
        let count_ok = if fn_ty.is_var_arg() {
            got >= expected
        } else {
            got == expected
        };
        if !count_ok {
            return Err(IrError::CallArgumentCountMismatch { expected, got });
        }
        for (i, (&arg, param_ty)) in args.iter().zip(params.iter()).enumerate() {
            let arg_ty_id = self.module.context().value_data(arg).ty;
            if arg_ty_id != param_ty.id() {
                let arg_ty = Type::<'ctx, B>::new(arg_ty_id, ModuleRef::<B>::new(self.module));
                return Err(IrError::CallArgumentTypeMismatch {
                    index: u32::try_from(i)
                        .unwrap_or_else(|_| unreachable!("argument index bounded by u32")),
                    expected: param_ty.to_string(),
                    got: arg_ty.to_string(),
                });
            }
        }
        Ok(())
    }

    // ---- Call ----

    /// TYPED flat call — the primary call-construction form. Wrong
    /// arity, wrong argument types, and wrong result use are all
    /// compile errors; the return marker is derived from the callee,
    /// never caller-asserted. Mirrors `IRBuilder::CreateCall(FunctionCallee,
    /// ArrayRef<Value*>, ...)` with the callee schema statically pinned.
    ///
    /// No runtime argument-count/type check is needed here (unlike the
    /// dyn paths): [`TypedFunctionValue::try_from_function`] already
    /// proved the callee's real declared parameter types match
    /// `Params` exactly, and the `A: CallArgs<'ctx, Params, B>` bound
    /// already proves `args` lowers to the same schema — the two facts
    /// compose transitively, so the argument list is correct by
    /// construction.
    pub fn build_call<Ret, Params, A, Name>(
        &self,
        callee: TypedFunctionValue<'ctx, Ret, Params, B>,
        args: A,
        name: Name,
    ) -> IrResult<TypedCallInst<'ctx, Ret, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
        Name: AsRef<str>,
    {
        let f = callee.as_function();
        let arg_ids = args.lower(ModuleRef::new(self.module))?;
        let payload = crate::instr_types::CallInstData::new(
            f.as_value().id,
            f.signature().as_type().id(),
            arg_ids,
            f.calling_conv(),
            crate::instr_types::TailCallKind::None,
        );
        let inst = self.append_instruction(
            f.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(TypedCallInst::from_call(CallInst::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        )))
    }

    /// Typed flat call with explicit call-site configuration
    /// (calling convention / attributes), otherwise identical to
    /// [`Self::build_call`].
    pub fn build_call_with_config<Ret, Params, A>(
        &self,
        callee: TypedFunctionValue<'ctx, Ret, Params, B>,
        args: A,
        config: CallSiteConfig,
    ) -> IrResult<TypedCallInst<'ctx, Ret, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
    {
        let f = callee.as_function();
        let arg_ids = args.lower(ModuleRef::new(self.module))?;
        let (name, calling_conv, attrs) = config.into_parts();
        let payload = crate::instr_types::CallInstData::new_with_attrs(
            f.as_value().id,
            f.signature().as_type().id(),
            arg_ids,
            calling_conv,
            crate::instr_types::TailCallKind::None,
            attrs,
        );
        let inst = self.append_instruction(
            f.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(TypedCallInst::from_call(CallInst::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        )))
    }

    /// Typed chainable call builder: same schema guarantees as
    /// [`Self::build_call`], with `tail()` / `must_tail()` / `no_tail()`
    /// / `calling_conv(cc)` / `call_attributes(attrs)` / `name(n)`
    /// accumulated before `.build()` emits the call.
    pub fn typed_call_builder<Ret, Params, A>(
        &self,
        callee: TypedFunctionValue<'ctx, Ret, Params, B>,
        args: A,
    ) -> TypedCallBuilder<'_, 'm, 'ctx, B, F, R, Ret, Params, A>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
    {
        TypedCallBuilder {
            parent: self,
            callee,
            args,
            tail_kind: crate::instr_types::TailCallKind::None,
            calling_conv: None,
            attrs: CallAttributeData::default(),
            name: String::new(),
        }
    }

    /// TYPED varargs call: the fixed-prefix arguments are schema-typed
    /// through `Params` exactly like [`Self::build_call`]; the trailing
    /// `varargs` are erased [`IsValue`] handles, matching LLVM's own
    /// variadic-argument contract (the `...` tail carries no static
    /// type checking — only the fixed prefix does). Mirrors
    /// `IRBuilder::CreateCall` against a variadic `FunctionCallee`.
    pub fn build_varargs_call<Ret, Params, A, I, V, Name>(
        &self,
        callee: TypedVarArgsFunctionValue<'ctx, Ret, Params, B>,
        fixed_args: A,
        varargs: I,
        name: Name,
    ) -> IrResult<TypedCallInst<'ctx, Ret, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Name: AsRef<str>,
    {
        let f = callee.as_function();
        let mut arg_ids: Vec<ValueId> = fixed_args.lower(ModuleRef::new(self.module))?.into_vec();
        arg_ids.extend(varargs.into_iter().map(|v| v.as_value().id));
        let payload = crate::instr_types::CallInstData::new(
            f.as_value().id,
            f.signature().as_type().id(),
            arg_ids,
            f.calling_conv(),
            crate::instr_types::TailCallKind::None,
        );
        let inst = self.append_instruction(
            f.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(TypedCallInst::from_call(CallInst::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        )))
    }

    /// Flat call form: pass a [`FunctionValue`] callee, an iterable of
    /// pre-widened arguments (each one already a [`Value<'ctx, B>`]), and
    /// a name. Mirrors the simple shape of `IRBuilder::CreateCall`.
    /// Use [`Self::call_builder`] for mixed-arg-type construction.
    pub fn build_call_dyn<R2, I, V, Name>(
        &self,
        callee: FunctionValue<'ctx, R2, B>,
        args: I,
        name: Name,
    ) -> IrResult<CallInst<'ctx, R2, B>>
    where
        Name: AsRef<str>,
        R2: crate::marker::ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
    {
        let mut builder = self.call_builder(callee).name(name);
        for arg in args {
            builder = builder.arg(arg);
        }
        builder.build()
    }

    /// Flat descriptor-backed intrinsic-call form.
    pub fn build_intrinsic_call<Name>(
        &self,
        descriptor: &IntrinsicDescriptor<'ctx, B>,
        args: &[Value<'ctx, B>],
        name: Name,
    ) -> IrResult<IntrinsicInst<'ctx, Dyn, B>>
    where
        Name: AsRef<str>,
    {
        let mut builder = self.intrinsic_call_builder(descriptor)?.name(name);
        for arg in args.iter().copied() {
            builder = builder.arg(arg);
        }
        builder.build()
    }

    /// Builder-pattern descriptor-backed intrinsic-call construction.
    pub fn intrinsic_call_builder(
        &self,
        descriptor: &IntrinsicDescriptor<'ctx, B>,
    ) -> IrResult<IntrinsicCallBuilder<'_, 'm, 'ctx, B, F, R>> {
        let callee = self
            .module
            .get_or_insert_intrinsic_declaration(descriptor)?;
        let mut inner = self.call_builder(callee);
        inner.intrinsic_descriptor = Some(descriptor.clone());
        Ok(IntrinsicCallBuilder { inner })
    }

    /// Flat ID/name intrinsic-call form for typed convenience wrappers.
    pub fn build_intrinsic_call_by_id<I, V, IntrinsicName, ResultName>(
        &self,
        id: IntrinsicId,
        intrinsic_name: IntrinsicName,
        args: I,
        result_name: ResultName,
    ) -> IrResult<IntrinsicInst<'ctx, Dyn, B>>
    where
        IntrinsicName: AsRef<str>,
        ResultName: AsRef<str>,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
    {
        let mut builder = self
            .intrinsic_call_builder_by_id(id, intrinsic_name)?
            .name(result_name);
        for arg in args {
            builder = builder.arg(arg);
        }
        builder.build()
    }

    /// Builder-pattern ID/name intrinsic-call construction for typed
    /// convenience wrappers.
    pub fn intrinsic_call_builder_by_id<Name>(
        &self,
        id: IntrinsicId,
        intrinsic_name: Name,
    ) -> IrResult<IntrinsicCallBuilder<'_, 'm, 'ctx, B, F, R>>
    where
        Name: AsRef<str>,
    {
        let callee = self.intrinsic_callee_by_id(id, intrinsic_name)?;
        let descriptor =
            callee
                .intrinsic_descriptor()
                .ok_or_else(|| IrError::IntrinsicSignatureMismatch {
                    name: callee.name().to_owned(),
                })?;
        self.intrinsic_call_builder(&descriptor)
    }

    fn intrinsic_callee_by_id<Name>(
        &self,
        id: IntrinsicId,
        intrinsic_name: Name,
    ) -> IrResult<FunctionValue<'ctx, Dyn, B>>
    where
        Name: AsRef<str>,
    {
        let name = intrinsic_name.as_ref();
        if IntrinsicId::lookup(name) != Some(id) {
            return Err(IrError::IntrinsicSignatureMismatch {
                name: name.to_owned(),
            });
        }
        self.module
            .get_or_insert_intrinsic_declaration_by_name::<B>(name)
    }

    /// Builder-pattern call construction. Returns a
    /// [`CallBuilder`] that accumulates per-arg / flag state via
    /// chainable methods, then emits the call on `.build()`. Each
    /// `.arg()` call is statically dispatched (no `dyn`); arg types
    /// can vary across calls.
    pub fn call_builder<R2: ReturnMarker>(
        &self,
        callee: FunctionValue<'ctx, R2, B>,
    ) -> CallBuilder<'_, 'm, 'ctx, B, F, R, R2> {
        CallBuilder {
            parent: self,
            callee_id: callee.as_value().id,
            fn_ty: callee.signature().as_type().id(),
            return_ty: callee.return_type().id(),
            args: Vec::new(),
            calling_conv: callee.calling_conv(),
            tail_kind: crate::instr_types::TailCallKind::None,
            attrs: crate::instr_types::CallAttributeData::default(),
            name: String::new(),
            intrinsic_descriptor: None,
            _rp: PhantomData,
            _rc: PhantomData,
        }
    }

    /// TYPED indirect call through a function-pointer value: the
    /// callee's function type is constructed from the `Sig` schema, so
    /// it is never spelled by hand and can never drift from
    /// `Sig::Params` / `Sig::Ret`. Mirrors `IRBuilder::CreateCall(FunctionType*,
    /// Value* callee, args)` — the opaque-pointer form where the pointee
    /// type is supplied separately — with the pointee type derived
    /// instead of caller-asserted.
    ///
    /// Spell as: `b.build_indirect_call::<fn(i32) -> i32, _, _>(fp, (x,), "r")?`.
    ///
    /// No runtime argument-count/type check is needed: `fn_ty` is
    /// constructed from `Sig::Params` in this same call, and
    /// `A: CallArgs<'ctx, Sig::Params, B>` already proves `args` lowers
    /// to that identical schema — the underlying function pointer's
    /// *actual* pointee type is an indirect-call trust boundary LLVM
    /// itself does not statically check either (mirrors
    /// `IRBuilder::CreateCall`'s own opaque-pointer contract).
    pub fn build_indirect_call<Sig, A, Name>(
        &self,
        callee: PointerValue<'ctx, B>,
        args: A,
        name: Name,
    ) -> IrResult<TypedCallInst<'ctx, Sig::Ret, B>>
    where
        Sig: FunctionSignature,
        A: CallArgs<'ctx, Sig::Params, B>,
        Name: AsRef<str>,
    {
        let module: Module<'ctx, B, Unverified> = Module::from_core(self.module);
        let ret = <Sig::Ret as FunctionReturn>::ir_type(&module)?;
        let params = <Sig::Params as FunctionParamList>::ir_types(&module)?;
        let fn_ty = module.fn_type(ret, params, false);
        let callee_v = IsValue::as_value(callee);
        let arg_ids = args.lower(ModuleRef::new(self.module))?;
        let payload = crate::instr_types::CallInstData::new(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            crate::CallingConv::C,
            crate::instr_types::TailCallKind::None,
        );
        let inst = self.append_instruction(
            fn_ty.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(TypedCallInst::from_call(CallInst::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        )))
    }

    /// Produce an indirect `call` through a function-pointer **value** (not a
    /// named `@function`), with the callee's function type given explicitly.
    /// Mirrors `IRBuilder::CreateCall(FunctionType*, Value* callee, args)` — the
    /// opaque-pointer form where the pointee type is supplied separately. Used
    /// to lower a computed code pointer (`call rax`, a vtable slot) to a real
    /// indirect call rather than routing through a named dispatcher.
    ///
    /// `fn_ty` is the callee's signature; `callee` is the function pointer; the
    /// caller picks the return marker `R2` to match `fn_ty`'s return type.
    pub fn build_indirect_call_dyn<R2, I, V, Name>(
        &self,
        fn_ty: FunctionType<'ctx, B>,
        callee: PointerValue<'ctx, B>,
        args: I,
        name: Name,
    ) -> IrResult<CallInst<'ctx, R2, B>>
    where
        Name: AsRef<str>,
        R2: crate::marker::ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
    {
        let callee_v = IsValue::as_value(callee);
        let ret_data = self.module.context().type_data(fn_ty.return_type().id());
        if !crate::function::signature_matches_marker::<R2>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: crate::marker::marker_kind_label::<R2>()
                    .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
                got: fn_ty.return_type().kind_label(),
            });
        }
        let mut arg_ids: Vec<ValueId> = Vec::new();
        for arg in args {
            let v = arg.as_value();
            arg_ids.push(v.id);
        }
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let payload = crate::instr_types::CallInstData::new(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids.into_boxed_slice(),
            crate::CallingConv::C,
            crate::instr_types::TailCallKind::None,
        );
        let inst = self.append_instruction(
            fn_ty.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(CallInst::<R2, B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        ))
    }

    /// Produce a `call` whose callee is an inline-assembly value. Mirrors
    /// `IRBuilder::CreateCall(InlineAsm*, args)` — the asm carries its own
    /// function type, so the call's return / argument shape comes from
    /// [`InlineAsm::function_type`](InlineAsm). The
    /// result prints as the `asm` form, e.g.
    /// `call i64 asm sideeffect "...", "=r,r,r"(i64 %a, i64 %b)`, instead
    /// of an `@name` operand.
    ///
    /// The caller picks the return marker `R2` to match the asm's wrapped
    /// return type; a mismatch fails with
    /// [`IrError::ReturnTypeMismatch`]. The calling convention is `C`,
    /// matching what LLVM emits for an inline-asm call.
    pub fn build_inline_asm_call<R2, I, V, Name>(
        &self,
        asm: InlineAsm<'ctx, B>,
        args: I,
        name: Name,
    ) -> IrResult<CallInst<'ctx, R2, B>>
    where
        Name: AsRef<str>,
        R2: crate::marker::ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
    {
        let asm_v = asm.as_value();
        let fn_ty = asm.function_type();
        // Reject a return-marker / signature mismatch up front, mirroring
        // `Module::add_function`'s `signature_matches_marker` gate.
        let ret_data = self.module.context().type_data(fn_ty.return_type().id());
        if !crate::function::signature_matches_marker::<R2>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: crate::marker::marker_kind_label::<R2>()
                    .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
                got: fn_ty.return_type().kind_label(),
            });
        }
        let mut arg_ids: Vec<ValueId> = Vec::new();
        for arg in args {
            let v = arg.as_value();
            arg_ids.push(v.id);
        }
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let payload = crate::instr_types::CallInstData::new_with_attrs(
            asm_v.id,
            fn_ty.as_type().id(),
            arg_ids.into_boxed_slice(),
            crate::CallingConv::C,
            crate::instr_types::TailCallKind::None,
            crate::instr_types::CallAttributeData::default(),
        );
        let inst = self.append_instruction(
            fn_ty.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(CallInst::<R2, B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        ))
    }

    // ---- GEP ----

    /// Produce `getelementptr <source-ty>, ptr <ptr>, <indices>`.
    /// Mirrors `IRBuilder::CreateGEP`.
    pub fn build_gep<T, P, I, V, Name>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
        I: IntoIterator<Item = V>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        self.build_gep_inner(
            source_ty,
            ptr,
            indices,
            crate::gep_no_wrap_flags::GepNoWrapFlags::empty(),
            name,
        )
    }

    /// Produce `getelementptr inbounds <source-ty>, ptr <ptr>,
    /// <indices>`. Mirrors `IRBuilder::CreateInBoundsGEP`.
    pub fn build_inbounds_gep<T, P, I, V, Name>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
        I: IntoIterator<Item = V>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        self.build_gep_inner(
            source_ty,
            ptr,
            indices,
            crate::gep_no_wrap_flags::GepNoWrapFlags::inbounds(),
            name,
        )
    }

    /// Produce `getelementptr inbounds nuw <struct-ty>, ptr <ptr>,
    /// i32 0, i32 <field-idx>`. Mirrors `IRBuilder::CreateStructGEP`
    /// (`IRBuilder.h`), which passes `GEPNoWrapFlags::inBounds() |
    /// GEPNoWrapFlags::noUnsignedWrap()` -- a struct-field offset can
    /// never wrap the pointer's index-width arithmetic, so upstream
    /// asserts `nuw` in addition to `inbounds`.
    pub fn build_struct_gep<P, Name>(
        &self,
        struct_ty: StructType<'ctx, StructBodyDyn, B>,
        ptr: P,
        idx: u32,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let i32_ty = ModuleView::<B>::new(self.module).i32_type();
        let zero = i32_ty.const_zero().as_dyn();
        let idx_val = i32_ty
            .const_int(i32::try_from(idx).map_err(|_| IrError::InvalidOperation {
                message: "struct field index exceeds i32::MAX",
            })?)
            .as_dyn();
        self.build_gep_inner(
            struct_ty,
            ptr,
            [zero, idx_val],
            crate::gep_no_wrap_flags::GepNoWrapFlags::inbounds()
                | crate::gep_no_wrap_flags::GepNoWrapFlags::NUW,
            name,
        )
    }

    /// `getelementptr inbounds %S, ptr %p, i32 0, i32 I` with the field
    /// type projected at compile time from the [`StructSchema`]. An
    /// out-of-range `I` fails to compile (no [`StructFieldAt<I>`] impl).
    /// Mirrors `IRBuilder::CreateStructGEP` + the Rust-side
    /// [`TypedPointerValue`] overlay.
    pub fn build_field_gep<S, const I: u32, Name>(
        &self,
        ptr: TypedPointerValue<'ctx, S, B>,
        name: Name,
    ) -> IrResult<TypedPointerValue<'ctx, FieldOf<S, I>, B>>
    where
        S: StructSchema,
        S::FieldParams: StructFieldAt<I>,
        Name: AsRef<str>,
    {
        let struct_ty = S::ir_type(&Module::from_core(self.module))?.as_dyn();
        let raw = self.build_struct_gep(struct_ty, ptr.as_pointer_value(), I, name)?;
        Ok(raw.with_pointee::<FieldOf<S, I>>())
    }

    /// `getelementptr T, ptr %p, <idx>` -- element-stride arithmetic;
    /// the pointee schema is preserved. Mirrors the 1-index
    /// `IRBuilder::CreateGEP` + the Rust-side [`TypedPointerValue`]
    /// overlay.
    pub fn build_element_gep<T, W, Idx, Name>(
        &self,
        ptr: TypedPointerValue<'ctx, T, B>,
        index: Idx,
        name: Name,
    ) -> IrResult<TypedPointerValue<'ctx, T, B>>
    where
        T: IrField,
        W: IntWidth,
        Idx: IntoIntValue<'ctx, W, B>,
        Name: AsRef<str>,
    {
        let elem_ty = T::ir_type(&Module::from_core(self.module))?;
        let idx_value = index.into_int_value(ModuleRef::new(self.module))?;
        let raw = self.build_gep(
            elem_ty,
            ptr.as_pointer_value(),
            core::iter::once(IsValue::as_value(idx_value)),
            name,
        )?;
        Ok(raw.with_pointee::<T>())
    }

    /// `getelementptr inbounds T, ptr %p, <idx>`. Mirrors the 1-index
    /// `IRBuilder::CreateInBoundsGEP` + the Rust-side
    /// [`TypedPointerValue`] overlay.
    pub fn build_inbounds_element_gep<T, W, Idx, Name>(
        &self,
        ptr: TypedPointerValue<'ctx, T, B>,
        index: Idx,
        name: Name,
    ) -> IrResult<TypedPointerValue<'ctx, T, B>>
    where
        T: IrField,
        W: IntWidth,
        Idx: IntoIntValue<'ctx, W, B>,
        Name: AsRef<str>,
    {
        let elem_ty = T::ir_type(&Module::from_core(self.module))?;
        let idx_value = index.into_int_value(ModuleRef::new(self.module))?;
        let raw = self.build_inbounds_gep(
            elem_ty,
            ptr.as_pointer_value(),
            core::iter::once(IsValue::as_value(idx_value)),
            name,
        )?;
        Ok(raw.with_pointee::<T>())
    }

    /// `getelementptr` with explicit [`crate::GepNoWrapFlags`]. Use this
    /// when the parser has decoded `inbounds`, `nuw`, or `nusw` flags directly.
    /// Mirrors `IRBuilder::CreateGEP` with the full flags bitfield.
    pub fn build_gep_with_flags<T, P, I, V, Name>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        flags: crate::gep_no_wrap_flags::GepNoWrapFlags,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
        I: IntoIterator<Item = V>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        self.build_gep_inner(source_ty, ptr, indices, flags, name)
    }

    fn build_gep_inner<T, P, I, V>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        flags: crate::gep_no_wrap_flags::GepNoWrapFlags,
        name: impl AsRef<str>,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
        I: IntoIterator<Item = V>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        let source_ty = source_ty.as_type();
        let source_ty_id = source_ty.id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let ptr_value = IsValue::as_value(p);
        let mut idx_ids = Vec::new();
        let mut idx_values = Vec::new();
        for index in indices {
            let iv = index.into_int_value(ModuleRef::new(self.module))?;
            idx_values.push(iv.as_value());
            idx_ids.push(iv.as_value().id);
        }
        // Mirrors `GetElementPtrInst::getGEPReturnType` (`IR/Instructions.h`):
        // for the scalar (non-vector-of-pointers) case the result type is
        // exactly the base pointer's type, i.e. it lives in the SAME address
        // space as `ptr`, not always address space 0.
        let result_ty = self.module.ptr_type(p.ty().address_space()).as_type().id();
        if let Some(folded) = self
            .folder
            .fold_gep_dyn(source_ty, ptr_value, &idx_values, flags)?
        {
            let folded = self.checked_folded_value(folded, result_ty)?;
            return Ok(PointerValue::from_value_unchecked(folded));
        }
        let payload = crate::instr_types::GepInstData::new(
            source_ty_id,
            ptr_value.id,
            idx_ids.into_boxed_slice(),
            flags,
        );
        let inst = self.append_instruction(result_ty, InstructionKindData::Gep(payload), name);
        Ok(PointerValue::from_value_unchecked(inst.as_value()))
    }

    // ---- Floating-point casts ----

    /// Produce `fpext <value> to <dst>`. Compile-time check:
    /// `Dst: FloatWiderThan<Src>`. Mirrors `IRBuilder::CreateFPExt`.
    pub fn build_fp_ext<Src, Dst, Name>(
        &self,
        value: FloatValue<'ctx, Src, B>,
        dst_ty: FloatType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: FloatKind,
        Dst: FloatKind + FloatWiderThan<Src>,
    {
        self.build_fp_cast(value, dst_ty, name, crate::instr_types::CastOpcode::FpExt)
    }

    /// Produce `fptrunc <value> to <dst>`. Compile-time check:
    /// `Src: FloatWiderThan<Dst>`. Mirrors `IRBuilder::CreateFPTrunc`.
    pub fn build_fp_trunc<Src, Dst, Name>(
        &self,
        value: FloatValue<'ctx, Src, B>,
        dst_ty: FloatType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: FloatKind + FloatWiderThan<Dst>,
        Dst: FloatKind,
    {
        self.build_fp_cast(value, dst_ty, name, crate::instr_types::CastOpcode::FpTrunc)
    }

    /// Runtime-kind `fptrunc`. Mirrors [`Self::build_fp_trunc`] but
    /// accepts dynamically-typed operands so the parser can call it
    /// without static `FloatWiderThan` bounds.
    ///
    /// No compile-time width ordering check is performed; the LLVM
    /// verifier will reject `fptrunc` where `src` is not strictly wider
    /// than `dst`.
    pub fn build_fp_trunc_dyn<Name>(
        &self,
        value: FloatValue<'ctx, FloatDyn, B>,
        dst_ty: FloatType<'ctx, FloatDyn, B>,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, FloatDyn, B>>
    where
        Name: AsRef<str>,
    {
        let v = IsValue::as_value(value);
        if let Some(folded) = self.folder.fold_cast_dyn(
            crate::instr_types::CastOpcode::FpTrunc,
            v,
            dst_ty.as_type(),
        )? {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::FpTrunc, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Runtime-kind `fpext`. Mirrors [`Self::build_fp_ext`] but accepts
    /// dynamically-typed operands so the parser can call it without
    /// static `FloatWiderThan` bounds.
    ///
    /// No compile-time width ordering check is performed; the LLVM
    /// verifier will reject `fpext` where `dst` is not strictly wider
    /// than `src`.
    pub fn build_fp_ext_dyn<Name>(
        &self,
        value: FloatValue<'ctx, FloatDyn, B>,
        dst_ty: FloatType<'ctx, FloatDyn, B>,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, FloatDyn, B>>
    where
        Name: AsRef<str>,
    {
        let v = IsValue::as_value(value);
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(crate::instr_types::CastOpcode::FpExt, v, dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::FpExt, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    /// Crate-internal helper for `build_fp_ext` / `build_fp_trunc`.
    fn build_fp_cast<Src, Dst>(
        &self,
        value: FloatValue<'ctx, Src, B>,
        dst_ty: FloatType<'ctx, Dst, B>,
        name: impl AsRef<str>,
        opcode: crate::instr_types::CastOpcode,
    ) -> IrResult<FloatValue<'ctx, Dst, B>>
    where
        Src: FloatKind,
        Dst: FloatKind,
    {
        let v = IsValue::as_value(value);
        if let Some(folded) = self.folder.fold_cast_to_fp(opcode, v, dst_ty)? {
            return self.accept_folded_cast_fp(folded, dst_ty);
        }
        let payload = CastOpData::new(opcode, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(FloatValue::<Dst, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `fptoui <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateFPToUI`.
    pub fn build_fp_to_ui<K, W, Name>(
        &self,
        value: FloatValue<'ctx, K, B>,
        dst_ty: IntType<'ctx, W, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        W: IntWidth,
    {
        self.build_fp_to_int(value, dst_ty, name, crate::instr_types::CastOpcode::FpToUI)
    }

    /// Produce `fptosi <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateFPToSI`.
    pub fn build_fp_to_si<K, W, Name>(
        &self,
        value: FloatValue<'ctx, K, B>,
        dst_ty: IntType<'ctx, W, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        W: IntWidth,
    {
        self.build_fp_to_int(value, dst_ty, name, crate::instr_types::CastOpcode::FpToSI)
    }

    fn build_fp_to_int<K, W>(
        &self,
        value: FloatValue<'ctx, K, B>,
        dst_ty: IntType<'ctx, W, B>,
        name: impl AsRef<str>,
        opcode: crate::instr_types::CastOpcode,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        K: FloatKind,
        W: IntWidth,
    {
        let v = IsValue::as_value(value);
        if let Some(folded) = self.folder.fold_cast_to_int(opcode, v, dst_ty)? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(opcode, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `uitofp <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateUIToFP`.
    pub fn build_ui_to_fp<W, K, Name>(
        &self,
        value: IntValue<'ctx, W, B>,
        dst_ty: FloatType<'ctx, K, B>,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        K: FloatKind,
    {
        self.build_int_to_fp(value, dst_ty, name, crate::instr_types::CastOpcode::UIToFp)
    }

    /// `uitofp nneg` with explicit [`crate::UIToFpFlags`]. Mirrors
    /// `IRBuilder::CreateUIToFP` plus `Instruction::setNonNeg`. The `nneg`
    /// flag asserts the source value is non-negative.
    pub fn build_ui_to_fp_with_flags<W, K, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, K, B>,
        flags: crate::instr_types::UIToFpFlags,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        K: FloatKind,
        V: IntoIntValue<'ctx, W, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        let v = value.as_value();
        if let Some(folded) =
            self.folder
                .fold_cast_to_fp(crate::instr_types::CastOpcode::UIToFp, v, dst_ty)?
        {
            return self.accept_folded_cast_fp(folded, dst_ty);
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::UIToFp, v.id);
        payload.nneg.set(flags.nneg);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(FloatValue::<K, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `sitofp <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateSIToFP`.
    pub fn build_si_to_fp<W, K, Name>(
        &self,
        value: IntValue<'ctx, W, B>,
        dst_ty: FloatType<'ctx, K, B>,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        K: FloatKind,
    {
        self.build_int_to_fp(value, dst_ty, name, crate::instr_types::CastOpcode::SIToFp)
    }

    /// `uitofp nneg` with explicit [`crate::UIToFpFlags`]. The `nneg` flag
    /// asserts the source value is non-negative. Both source and destination
    /// types are erased (dyn variants).
    pub fn build_ui_to_fp_with_flags_dyn<Name>(
        &self,
        src: IntValue<'ctx, IntDyn, B>,
        dst: FloatType<'ctx, FloatDyn, B>,
        flags: crate::instr_types::UIToFpFlags,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, FloatDyn, B>>
    where
        Name: AsRef<str>,
    {
        if let Some(folded) = self.folder.fold_cast_dyn(
            crate::instr_types::CastOpcode::UIToFp,
            src.as_value(),
            dst.as_type(),
        )? {
            let folded = self.checked_folded_value(folded, dst.as_type().id())?;
            return Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::UIToFp, src.as_value().id);
        payload.nneg.set(flags.nneg);
        let inst =
            self.append_instruction(dst.as_type().id(), InstructionKindData::Cast(payload), name);
        Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(
            inst.as_value(),
        ))
    }

    fn build_int_to_fp<W, K>(
        &self,
        value: IntValue<'ctx, W, B>,
        dst_ty: FloatType<'ctx, K, B>,
        name: impl AsRef<str>,
        opcode: crate::instr_types::CastOpcode,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        W: IntWidth,
        K: FloatKind,
    {
        let v = value.as_value();
        if let Some(folded) = self.folder.fold_cast_to_fp(opcode, v, dst_ty)? {
            return self.accept_folded_cast_fp(folded, dst_ty);
        }
        let payload = CastOpData::new(opcode, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(FloatValue::<K, B>::from_value_unchecked(inst.as_value()))
    }

    // ---- Pointer casts ----

    /// Produce `ptrtoaddr <value> to <address type>`. Mirrors
    /// `IRBuilder::CreatePtrToAddr`, using the module
    /// [`DataLayout`](crate::DataLayout) address type for the pointer
    /// operand's address space.
    pub fn build_ptr_to_addr<Name>(
        &self,
        value: PointerValue<'ctx, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>>
    where
        Name: AsRef<str>,
    {
        let value = value.as_value();
        let dst_ty = self.ptr_to_addr_result_type(value.ty())?;
        let result = self.build_ptr_to_addr_dyn(value, dst_ty, name)?;
        Ok(IntValue::<IntDyn, B>::from_value_unchecked(result))
    }

    /// Runtime-typed `ptrtoaddr`. Accepts either a scalar pointer or a
    /// pointer vector and requires `dst_ty` to be the DataLayout address type
    /// for the source address space (index width, preserving vector shape).
    /// Mirrors `DataLayout::getAddressType(V->getType())`.
    pub fn build_ptr_to_addr_dyn<Name>(
        &self,
        value: Value<'ctx, B>,
        dst_ty: Type<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        let expected_ty = self.ptr_to_addr_result_type(value.ty())?;
        if expected_ty.id() != dst_ty.id() {
            return Err(IrError::InvalidOperation {
                message: "PtrToAddr result must be address width",
            });
        }
        if let Some(folded) = self
            .folder
            .fold_cast_dyn(CastOpcode::PtrToAddr, value, dst_ty)?
        {
            return self.checked_folded_value(folded, dst_ty.id());
        }
        let payload = CastOpData::new(CastOpcode::PtrToAddr, value.id);
        let inst = self.append_instruction(dst_ty.id(), InstructionKindData::Cast(payload), name);
        Ok(inst.as_value())
    }

    /// Produce `ptrtoint <value> to <dst>`. Mirrors
    /// `IRBuilder::CreatePtrToInt`.
    pub fn build_ptr_to_int<W, Name>(
        &self,
        value: PointerValue<'ctx, B>,
        dst_ty: IntType<'ctx, W, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
    {
        let v = IsValue::as_value(value);
        if let Some(folded) =
            self.folder
                .fold_cast_to_int(crate::instr_types::CastOpcode::PtrToInt, v, dst_ty)?
        {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::PtrToInt, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Produce `inttoptr <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateIntToPtr`.
    pub fn build_int_to_ptr<W, Name>(
        &self,
        value: IntValue<'ctx, W, B>,
        dst_ty: PointerType<'ctx, B>,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
    {
        let v = value.as_value();
        if let Some(folded) = self.folder.fold_cast_dyn(
            crate::instr_types::CastOpcode::IntToPtr,
            v,
            dst_ty.as_type(),
        )? {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(PointerValue::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::IntToPtr, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(PointerValue::from_value_unchecked(inst.as_value()))
    }

    /// Generic bitcast on values of equal bit width. Mirrors
    /// `IRBuilder::CreateBitCast` (`IRBuilder.h`), which is itself
    /// `CreateCast(Instruction::BitCast, V, DestTy)`. The width
    /// equality is enforced statically through
    /// [`super::int_width::StaticIntWidth::STATIC_BITS`] /
    /// [`super::float_kind::StaticFloatKind::STATIC_BITS`]
    /// `const { assert!(...) }` blocks at monomorphisation; under-spec'd
    /// instantiations are *compile* errors.
    pub fn build_bitcast_int_to_int<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: super::int_width::StaticIntWidth,
        Dst: super::int_width::StaticIntWidth,
        V: IntoIntValue<'ctx, Src, B>,
    {
        const {
            assert!(
                <Src as super::int_width::StaticIntWidth>::STATIC_BITS
                    == <Dst as super::int_width::StaticIntWidth>::STATIC_BITS,
                "bitcast int->int requires Src::STATIC_BITS == Dst::STATIC_BITS",
            );
        }
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        let v_value = IsValue::as_value(v);
        if let Some(folded) = self.folder.fold_cast_to_int(
            super::instr_types::CastOpcode::BitCast,
            v_value,
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, v_value.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<Dst, B>::from_value_unchecked(inst.as_value()))
    }

    /// Bitcast an integer value to a same-bit-width float. Mirrors the
    /// `Instruction::BitCast` arm of `CastInst::Create` in
    /// `lib/IR/Instructions.cpp` for the `int -> fp` shape. Width
    /// equality is enforced statically.
    pub fn build_bitcast_int_to_fp<W, K, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, K, B>,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        K: super::float_kind::StaticFloatKind,
        V: IntoIntValue<'ctx, W, B>,
    {
        const {
            assert!(
                <W as super::int_width::StaticIntWidth>::STATIC_BITS
                    == <K as super::float_kind::StaticFloatKind>::STATIC_BITS,
                "bitcast int->fp requires W::STATIC_BITS == K::STATIC_BITS",
            );
        }
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        let v_value = IsValue::as_value(v);
        if let Some(folded) =
            self.folder
                .fold_cast_to_fp(super::instr_types::CastOpcode::BitCast, v_value, dst_ty)?
        {
            return self.accept_folded_cast_fp(folded, dst_ty);
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, v_value.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(FloatValue::<K, B>::from_value_unchecked(inst.as_value()))
    }

    /// Bitcast a float to a same-bit-width integer. Mirrors the
    /// `Instruction::BitCast` arm of `CastInst::Create` in
    /// `lib/IR/Instructions.cpp` for the `fp -> int` shape. Width
    /// equality is enforced statically.
    pub fn build_bitcast_fp_to_int<K, W, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, W, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::StaticFloatKind,
        W: super::int_width::StaticIntWidth,
        V: IntoFloatValue<'ctx, K, B>,
    {
        const {
            assert!(
                <K as super::float_kind::StaticFloatKind>::STATIC_BITS
                    == <W as super::int_width::StaticIntWidth>::STATIC_BITS,
                "bitcast fp->int requires K::STATIC_BITS == W::STATIC_BITS",
            );
        }
        let v = value.into_float_value(ModuleRef::new(self.module))?;
        let v_value = IsValue::as_value(v);
        if let Some(folded) = self.folder.fold_cast_to_int(
            super::instr_types::CastOpcode::BitCast,
            v_value,
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, v_value.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(IntValue::<W, B>::from_value_unchecked(inst.as_value()))
    }

    /// Bitcast a float to a same-bit-width float. Used for
    /// `bfloat <-> half` (both 16 bits) and `fp128 <-> ppc_fp128` (both
    /// 128 bits). Mirrors `Instruction::BitCast` in
    /// `lib/IR/Instructions.cpp`.
    pub fn build_bitcast_fp_to_fp<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<FloatValue<'ctx, Dst, B>>
    where
        Name: AsRef<str>,
        Src: super::float_kind::StaticFloatKind,
        Dst: super::float_kind::StaticFloatKind,
        V: IntoFloatValue<'ctx, Src, B>,
    {
        const {
            assert!(
                <Src as super::float_kind::StaticFloatKind>::STATIC_BITS
                    == <Dst as super::float_kind::StaticFloatKind>::STATIC_BITS,
                "bitcast fp->fp requires Src::STATIC_BITS == Dst::STATIC_BITS",
            );
        }
        let v = value.into_float_value(ModuleRef::new(self.module))?;
        let v_value = IsValue::as_value(v);
        if let Some(folded) =
            self.folder
                .fold_cast_to_fp(super::instr_types::CastOpcode::BitCast, v_value, dst_ty)?
        {
            return self.accept_folded_cast_fp(folded, dst_ty);
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, v_value.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(FloatValue::<Dst, B>::from_value_unchecked(inst.as_value()))
    }

    /// Runtime-typed bitcast: produce `bitcast <src> to <dst>` with both
    /// types erased to [`Type`]. The caller is responsible for
    /// ensuring `src` and `dst` have the same bit width; the LLVM verifier
    /// will reject ill-formed bitcasts.
    ///
    /// Used by the parser where compile-time static markers are unavailable.
    pub fn build_bitcast_dyn<Name>(
        &self,
        value: Value<'ctx, B>,
        dst_ty: Type<'ctx, B>,
        name: Name,
    ) -> IrResult<Value<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(super::instr_types::CastOpcode::BitCast, value, dst_ty)?
        {
            return self.checked_folded_value(folded, dst_ty.id());
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, value.id);
        let inst = self.append_instruction(dst_ty.id(), InstructionKindData::Cast(payload), name);
        Ok(inst.as_value())
    }

    /// Produce `addrspacecast <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateAddrSpaceCast`.
    pub fn build_addrspace_cast<Name>(
        &self,
        value: PointerValue<'ctx, B>,
        dst_ty: PointerType<'ctx, B>,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        let v = IsValue::as_value(value);
        if let Some(folded) = self.folder.fold_cast_dyn(
            crate::instr_types::CastOpcode::AddrSpaceCast,
            v,
            dst_ty.as_type(),
        )? {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(PointerValue::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(crate::instr_types::CastOpcode::AddrSpaceCast, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(PointerValue::from_value_unchecked(inst.as_value()))
    }

    /// Pointer cast: pick `bitcast` for same-addrspace pointer-to-pointer
    /// (a no-op in opaque-pointer LLVM, but a structurally-distinct `Cast`
    /// instruction) and `addrspacecast` when address spaces differ.
    /// Mirrors `IRBuilder::CreatePointerBitCastOrAddrSpaceCast`
    /// (`IRBuilder.h`), which dispatches the same way.
    pub fn build_pointer_cast<Name>(
        &self,
        value: PointerValue<'ctx, B>,
        dst_ty: PointerType<'ctx, B>,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        let v = IsValue::as_value(value);
        let opcode = if value.ty().address_space() == dst_ty.address_space() {
            super::instr_types::CastOpcode::BitCast
        } else {
            super::instr_types::CastOpcode::AddrSpaceCast
        };
        if let Ok(constant) = Constant::try_from(v)
            && let Some(folded) = self
                .folder
                .create_pointer_bitcast_or_addrspace_cast(constant, dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(PointerValue::from_value_unchecked(folded));
        }
        if let Some(folded) = self.folder.fold_cast_dyn(opcode, v, dst_ty.as_type())? {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(PointerValue::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(opcode, v.id);
        let inst = self.append_instruction(
            dst_ty.as_type().id(),
            InstructionKindData::Cast(payload),
            name,
        );
        Ok(PointerValue::from_value_unchecked(inst.as_value()))
    }

    /// `icmp eq <ptr>, null` -- pointer-null test. Mirrors
    /// `IRBuilder::CreateIsNull(Arg)` ->
    /// `CreateICmpEQ(Arg, Constant::getNullValue(Arg->getType()))`.
    pub fn build_is_null<Name>(
        &self,
        ptr: PointerValue<'ctx, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
    {
        self.build_pointer_cmp(
            super::cmp_predicate::IntPredicate::Eq,
            ptr,
            ptr.ty().const_null(),
            name,
        )
    }

    /// `icmp ne <ptr>, null` -- pointer-non-null test. Mirrors
    /// `IRBuilder::CreateIsNotNull(Arg)` ->
    /// `CreateICmpNE(Arg, Constant::getNullValue(Arg->getType()))`.
    pub fn build_is_not_null<Name>(
        &self,
        ptr: PointerValue<'ctx, B>,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
    {
        self.build_pointer_cmp(
            super::cmp_predicate::IntPredicate::Ne,
            ptr,
            ptr.ty().const_null(),
            name,
        )
    }

    /// Pointer-pointer comparison. Mirrors `IRBuilder::CreateICmp` with
    /// pointer operands; LLVM's `icmp` works on integers OR pointers, but
    /// our typed [`Self::build_int_cmp`] is integer-only. This helper
    /// covers the pointer arm directly (used by `build_is_null` /
    /// `build_is_not_null`).
    pub fn build_pointer_cmp<L, R2, Name>(
        &self,
        pred: super::cmp_predicate::IntPredicate,
        lhs: L,
        rhs: R2,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        L: IntoPointerValue<'ctx, B>,
        R2: IntoPointerValue<'ctx, B>,
    {
        let lhs = lhs.into_pointer_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_pointer_value(ModuleRef::new(self.module))?;
        let folded = self.folder.fold_cmp_dyn(
            pred.into(),
            IsValue::as_value(lhs),
            IsValue::as_value(rhs),
        )?;
        if let Some(folded) = folder::narrow_folded_bool(folded)? {
            return Ok(folded);
        }
        let payload = super::instr_types::CmpInstData::new(
            pred,
            IsValue::as_value(lhs).id,
            IsValue::as_value(rhs).id,
        );
        let i1_ty = ModuleView::<B>::new(self.module).bool_type().as_type().id();
        let inst = self.append_instruction(i1_ty, InstructionKindData::ICmp(payload), name);
        Ok(IntValue::<bool, B>::from_value_unchecked(inst.as_value()))
    }

    // ---- Vector splat / ptr arithmetic / aggregate ret convenience ----

    /// Broadcast `scalar` across a fixed-width vector of `count` lanes.
    /// Mirrors `IRBuilderBase::CreateVectorSplat(unsigned NumElts, Value*,
    /// const Twine&)` (`lib/IR/IRBuilder.cpp` line 1141), which expands to
    /// `insertelement <count x T> poison, <T> %v, i64 0` followed by
    /// `shufflevector ..., <count x T> poison, <count x i32> zeroinitializer`.
    /// The result is named `<name>.splat`; the intermediate insertelement
    /// is `<name>.splatinsert`.
    pub fn build_vector_splat<V, Name>(
        &self,
        count: u32,
        scalar: V,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, B>>
    where
        Name: AsRef<str>,
        V: IsValue<'ctx, B>,
    {
        if count == 0 {
            return Err(IrError::InvalidOperation {
                message: "build_vector_splat requires at least one lane",
            });
        }
        let scalar_value = scalar.as_value();
        let elem_ty = scalar_value.ty();
        let vec_ty = ModuleView::<B>::new(self.module).vector_type(elem_ty, count, false);
        let poison = vec_ty.as_type().get_poison();
        let i64_ty = ModuleView::<B>::new(self.module).i64_type();
        let zero_idx = i64_ty.const_int(0_u32);
        let name_ref = name.as_ref();
        let insert_name = if name_ref.is_empty() {
            String::from("splatinsert")
        } else {
            format!("{name_ref}.splatinsert")
        };
        let inserted =
            self.build_insert_element::<_, _, i64, _, _>(poison, scalar, zero_idx, insert_name)?;
        let n = usize::try_from(count).map_err(|_| IrError::InvalidOperation {
            message: "vector splat lane count exceeds the platform address range",
        })?;
        let mask = vec![0_i32; n];
        let splat_name = if name_ref.is_empty() {
            String::from("splat")
        } else {
            format!("{name_ref}.splat")
        };
        let shuf = self.build_shuffle_vector(inserted, poison, &mask, splat_name)?;
        Ok(VectorValue::from_value_unchecked(shuf))
    }

    // ---- ptr_add / inbounds_ptr_add ----

    /// `getelementptr i8, ptr <ptr>, <offset>` -- byte-offset pointer
    /// arithmetic. Mirrors `IRBuilder::CreatePtrAdd` in `IRBuilder.h`
    /// (line 2039), which expands to `CreateGEP(getInt8Ty(), Ptr, Offset, ...)`.
    pub fn build_ptr_add<P, O, W, Name>(
        &self,
        ptr: P,
        offset: O,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
        W: super::int_width::IntWidth,
        O: IntoIntValue<'ctx, W, B>,
    {
        let i8_ty = ModuleView::<B>::new(self.module).i8_type();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let offset_v = offset.into_int_value(ModuleRef::new(self.module))?;
        let offset_value = IsValue::as_value(offset_v);
        self.build_gep(i8_ty, p, core::iter::once(offset_value), name)
    }

    /// `getelementptr inbounds i8, ptr <ptr>, <offset>`. Mirrors
    /// `IRBuilder::CreateInBoundsPtrAdd` (`IRBuilder.h` line 2044), which
    /// expands to `CreateGEP(getInt8Ty(), Ptr, Offset, Name, GEPNoWrapFlags::inBounds())`.
    pub fn build_inbounds_ptr_add<P, O, W, Name>(
        &self,
        ptr: P,
        offset: O,
        name: Name,
    ) -> IrResult<PointerValue<'ctx, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
        W: super::int_width::IntWidth,
        O: IntoIntValue<'ctx, W, B>,
    {
        let i8_ty = ModuleView::<B>::new(self.module).i8_type();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let offset_v = offset.into_int_value(ModuleRef::new(self.module))?;
        let offset_value = IsValue::as_value(offset_v);
        self.build_inbounds_gep(i8_ty, p, core::iter::once(offset_value), name)
    }

    // ---- Integer comparison ----

    /// Produce `icmp <pred> <ty> <lhs>, <rhs>`. Mirrors
    /// `IRBuilder::CreateICmp`.
    ///
    /// Both operands share width `W` at the type level. The result
    /// type is always `i1` (`IntValue<'ctx, bool, B>`).
    pub fn build_int_cmp<W, Lhs, Rhs, Name>(
        &self,
        pred: crate::cmp_predicate::IntPredicate,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        let i1_ty = ModuleView::<B>::new(self.module).bool_type().as_type().id();
        if let Some(folded) = self.folder.fold_int_cmp(pred, lhs, rhs)? {
            return Ok(folded);
        }
        let payload =
            crate::instr_types::CmpInstData::new(pred, lhs.as_value().id, rhs.as_value().id);
        let inst = self.append_instruction(i1_ty, InstructionKindData::ICmp(payload), name);
        Ok(IntValue::<bool, B>::from_value_unchecked(inst.as_value()))
    }

    /// `icmp samesign` with explicit [`crate::ICmpFlags`]. Mirrors
    /// `IRBuilder::CreateICmp` plus `ICmpInst::setSameSign`. The `samesign`
    /// flag asserts both operands carry the same sign (LLVM 20+).
    ///
    /// Upstream sets `samesign` post-hoc via `ICmpInst::setSameSign`
    /// (`Instructions.h`) after construction; llvmkit's construction-time
    /// flag parameter is a deliberate Rust-side improvement -- the flag is
    /// part of the payload from the moment the instruction exists, so there
    /// is no window where an `ICmpInst` is live with a stale `samesign` bit.
    pub fn build_int_cmp_with_flags<W, Lhs, Rhs, Name>(
        &self,
        predicate: crate::cmp_predicate::IntPredicate,
        lhs: Lhs,
        rhs: Rhs,
        flags: crate::instr_types::ICmpFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        let i1_ty = ModuleView::<B>::new(self.module).bool_type().as_type().id();
        if let Some(folded) = self.folder.fold_int_cmp(predicate, lhs, rhs)? {
            return Ok(folded);
        }
        let mut payload =
            crate::instr_types::CmpInstData::new(predicate, lhs.as_value().id, rhs.as_value().id);
        payload.samesign = flags.samesign;
        let inst = self.append_instruction(i1_ty, InstructionKindData::ICmp(payload), name);
        Ok(IntValue::<bool, B>::from_value_unchecked(inst.as_value()))
    }

    /// `icmp samesign` with explicit [`crate::ICmpFlags`]. Both operands
    /// must be dynamically-typed (`IntDyn`). The `samesign` flag asserts
    /// both operands carry the same sign (LLVM 20+).
    pub fn build_int_cmp_with_flags_dyn<Name>(
        &self,
        pred: crate::cmp_predicate::IntPredicate,
        lhs: IntValue<'ctx, IntDyn, B>,
        rhs: IntValue<'ctx, IntDyn, B>,
        flags: crate::instr_types::ICmpFlags,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
    {
        let i1_ty = ModuleView::<B>::new(self.module).bool_type().as_type().id();
        if let Some(folded) = self.folder.fold_int_cmp(pred, lhs, rhs)? {
            return Ok(folded);
        }
        let mut payload =
            crate::instr_types::CmpInstData::new(pred, lhs.as_value().id, rhs.as_value().id);
        payload.samesign = flags.samesign;
        let inst = self.append_instruction(i1_ty, InstructionKindData::ICmp(payload), name);
        Ok(IntValue::<bool, B>::from_value_unchecked(inst.as_value()))
    }

    // Per-predicate convenience wrappers. Mirror the LLVM C++
    // `IRBuilder::CreateICmp{EQ,NE,SLT,...}` family (`IRBuilder.h`):
    // each one bakes the predicate into the method name so the call
    // site spells signedness intent explicitly. The predicate is
    // signedness-agnostic at the LLVM IR value level (the `i32` bit
    // pattern is the same either way) -- the *operation* is what
    // carries the sign, and these methods make that visible without a
    // free-floating `IntPredicate::Slt` token.

    /// `icmp eq` -- equal. Signedness-irrelevant. Mirrors
    /// `IRBuilder::CreateICmpEQ`.
    #[inline]
    pub fn build_icmp_eq<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(crate::cmp_predicate::IntPredicate::Eq, lhs, rhs, name)
    }

    /// `icmp ne` -- not equal. Signedness-irrelevant. Mirrors
    /// `IRBuilder::CreateICmpNE`.
    #[inline]
    pub fn build_icmp_ne<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(crate::cmp_predicate::IntPredicate::Ne, lhs, rhs, name)
    }

    /// `icmp ult` -- unsigned less than. Mirrors
    /// `IRBuilder::CreateICmpULT`.
    #[inline]
    pub fn build_icmp_ult<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(
            crate::cmp_predicate::IntPredicate::Ult,
            lhs,
            rhs,
            name,
        )
    }

    /// `icmp ule` -- unsigned less than or equal. Mirrors
    /// `IRBuilder::CreateICmpULE`.
    #[inline]
    pub fn build_icmp_ule<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(
            crate::cmp_predicate::IntPredicate::Ule,
            lhs,
            rhs,
            name,
        )
    }

    /// `icmp ugt` -- unsigned greater than. Mirrors
    /// `IRBuilder::CreateICmpUGT`.
    #[inline]
    pub fn build_icmp_ugt<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(
            crate::cmp_predicate::IntPredicate::Ugt,
            lhs,
            rhs,
            name,
        )
    }

    /// `icmp uge` -- unsigned greater than or equal. Mirrors
    /// `IRBuilder::CreateICmpUGE`.
    #[inline]
    pub fn build_icmp_uge<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(
            crate::cmp_predicate::IntPredicate::Uge,
            lhs,
            rhs,
            name,
        )
    }

    /// `icmp slt` -- signed less than. Mirrors
    /// `IRBuilder::CreateICmpSLT`.
    #[inline]
    pub fn build_icmp_slt<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(
            crate::cmp_predicate::IntPredicate::Slt,
            lhs,
            rhs,
            name,
        )
    }

    /// `icmp sle` -- signed less than or equal. Mirrors
    /// `IRBuilder::CreateICmpSLE`.
    #[inline]
    pub fn build_icmp_sle<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(
            crate::cmp_predicate::IntPredicate::Sle,
            lhs,
            rhs,
            name,
        )
    }

    /// `icmp sgt` -- signed greater than. Mirrors
    /// `IRBuilder::CreateICmpSGT`.
    #[inline]
    pub fn build_icmp_sgt<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(
            crate::cmp_predicate::IntPredicate::Sgt,
            lhs,
            rhs,
            name,
        )
    }

    /// `icmp sge` -- signed greater than or equal. Mirrors
    /// `IRBuilder::CreateICmpSGE`.
    #[inline]
    pub fn build_icmp_sge<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValue<'ctx, bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.build_int_cmp::<W, Lhs, Rhs, _>(
            crate::cmp_predicate::IntPredicate::Sge,
            lhs,
            rhs,
            name,
        )
    }

    // ---- Phi ----

    /// Produce `phi <ty>` with no initial incoming edges. Marker-only
    /// form: the result type comes from the `W` type parameter via
    /// [`crate::StaticIntWidth`], so callers spell it as
    /// `b.build_int_phi::<i32, _>("acc")?` without first binding
    /// `let i32_ty = m.i32_type();`. Mirrors `IRBuilder::CreatePHI`
    /// followed by zero `PHINode::addIncoming` calls. Subsequent
    /// edges are added through [`crate::PhiInst::add_incoming`],
    /// which returns `Self` so calls chain.
    pub fn build_int_phi<W, Name>(&self, name: Name) -> IrResult<PhiInst<'ctx, W, PhiOpen, B>>
    where
        Name: AsRef<str>,
        W: crate::int_width::StaticIntWidth,
    {
        let ty = W::ir_type(ModuleRef::<B>::new(self.module));
        let payload = crate::instr_types::PhiData::new();
        let inst =
            self.append_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok({
            let _i = inst;
            PhiInst::<W, PhiOpen, B>::from_raw(
                _i.as_value().id,
                ModuleRef::<B>::new(self.module),
                _i.ty().id(),
            )
        })
    }

    /// Runtime-width phi for the [`crate::IntDyn`] case. Takes the
    /// type explicitly because the marker carries no static width.
    pub fn build_int_phi_dyn<Name>(
        &self,
        ty: IntType<'ctx, IntDyn, B>,
        name: Name,
    ) -> IrResult<PhiInst<'ctx, IntDyn, PhiOpen, B>>
    where
        Name: AsRef<str>,
    {
        let payload = crate::instr_types::PhiData::new();
        let inst =
            self.append_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok({
            let _i = inst;
            PhiInst::<IntDyn, PhiOpen, B>::from_raw(
                _i.as_value().id,
                ModuleRef::<B>::new(self.module),
                _i.ty().id(),
            )
        })
    }

    /// Float-typed phi: `phi <fpty>`. Marker-only form keyed on
    /// `K: StaticFloatKind`. Mirrors `IRBuilder::CreatePHI(Type*, ...)`
    /// applied to a floating-point type.
    pub fn build_fp_phi<K, Name>(&self, name: Name) -> IrResult<FpPhiInst<'ctx, K, PhiOpen, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::StaticFloatKind,
    {
        let ty = K::ir_type(ModuleRef::<B>::new(self.module));
        let payload = super::instr_types::PhiData::new();
        let inst =
            self.append_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(FpPhiInst::<K, PhiOpen, B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        ))
    }

    /// Runtime-kind float phi: takes the type explicitly because
    /// [`crate::FloatDyn`] carries no static kind.
    pub fn build_fp_phi_dyn<Name>(
        &self,
        ty: FloatType<'ctx, FloatDyn, B>,
        name: Name,
    ) -> IrResult<FpPhiInst<'ctx, FloatDyn, PhiOpen, B>>
    where
        Name: AsRef<str>,
    {
        let payload = super::instr_types::PhiData::new();
        let inst =
            self.append_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(FpPhiInst::<FloatDyn, PhiOpen, B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        ))
    }

    /// Pointer-typed phi in the default address space (addrspace 0).
    /// Mirrors `IRBuilder::CreatePHI(PointerType::getUnqual(...), ...)`.
    pub fn build_pointer_phi<Name>(&self, name: Name) -> IrResult<PointerPhiInst<'ctx, PhiOpen, B>>
    where
        Name: AsRef<str>,
    {
        let ty = self.module.ptr_type(0);
        let payload = super::instr_types::PhiData::new();
        let inst =
            self.append_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(PointerPhiInst::<PhiOpen, B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        ))
    }

    /// Pointer-typed phi in a caller-specified address space. Mirrors
    /// `IRBuilder::CreatePHI(PointerType::get(Ctx, AS), ...)`.
    pub fn build_pointer_phi_in_addrspace<Name>(
        &self,
        ty: PointerType<'ctx, B>,
        name: Name,
    ) -> IrResult<PointerPhiInst<'ctx, PhiOpen, B>>
    where
        Name: AsRef<str>,
    {
        let payload = super::instr_types::PhiData::new();
        let inst =
            self.append_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(PointerPhiInst::<PhiOpen, B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            inst.ty().id(),
        ))
    }

    // ---- Branch / Unreachable ----

    /// Produce `br label %target`. Mirrors `IRBuilder::CreateBr`.
    ///
    /// Consumes `self`: the builder's insertion block is terminated and
    /// returned alongside the new terminator instruction. The branch
    /// target may be in any termination state -- backward edges (loop
    /// back-edges) target already-terminated blocks.
    pub fn build_br<T>(self, target: T) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        T: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let target = target.into_basic_block_label();
        let payload = crate::instr_types::BranchInstData {
            kind: crate::instr_types::BranchKind::Unconditional(target.as_value().id),
        };
        let void_ty = self.module.void_type().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Br(payload), "");
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `br i1 <cond>, label %then, label %else`. Mirrors
    /// `IRBuilder::CreateCondBr`.
    ///
    /// Consumes `self`; both target blocks may be in any termination state.
    pub fn build_cond_br<C, Then, Else>(
        self,
        cond: C,
        then_bb: Then,
        else_bb: Else,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        C: IntoIntValue<'ctx, bool, B>,
        Then: IntoBasicBlockLabel<'ctx, R, B>,
        Else: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let then_bb = then_bb.into_basic_block_label();
        let else_bb = else_bb.into_basic_block_label();
        let cond = cond.into_int_value(ModuleRef::new(self.module))?;
        let payload = crate::instr_types::BranchInstData {
            kind: crate::instr_types::BranchKind::Conditional {
                cond: core::cell::Cell::new(cond.as_value().id),
                then_bb: then_bb.as_value().id,
                else_bb: else_bb.as_value().id,
            },
        };
        let void_ty = self.module.void_type().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Br(payload), "");
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `switch <cond>, label <default> [...]`. Mirrors
    /// `IRBuilder::CreateSwitch`.
    ///
    /// Returns the terminated parent block plus an [`Open`]-typestate
    /// [`SwitchInst`]. The caller adds
    /// cases via [`SwitchInst::add_case`](SwitchInst::add_case)
    /// (chainable) and seals the case list with
    /// [`SwitchInst::finish`](SwitchInst::finish).
    pub fn build_switch<C, DefaultTarget, Name>(
        self,
        cond: C,
        default_target: DefaultTarget,
        name: Name,
    ) -> IrResult<TerminatedBlockSwitch<'ctx, R, B>>
    where
        Name: AsRef<str>,
        C: IsValue<'ctx, B>,
        DefaultTarget: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let default_target = default_target.into_basic_block_label();
        let cond_v = cond.as_value();
        let void_ty = self.module.void_type().as_type().id();
        let payload =
            crate::instr_types::SwitchInstData::new(cond_v.id, default_target.as_value().id);
        let inst = self.append_instruction(void_ty, InstructionKindData::Switch(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            SwitchInst::<Open, B>::from_raw(inst.as_value().id, module_ref, void_ty),
        ))
    }

    /// Produce `indirectbr <addr>, [...]`. Mirrors
    /// `IRBuilder::CreateIndirectBr`.
    ///
    /// Returns the terminated parent block plus an [`Open`]-typestate
    /// [`IndirectBrInst`]. The
    /// caller adds destinations via
    /// [`IndirectBrInst::add_destination`](IndirectBrInst::add_destination)
    pub fn build_indirectbr<A, Name>(
        self,
        address: A,
        name: Name,
    ) -> IrResult<TerminatedBlockIndirectBr<'ctx, R, B>>
    where
        Name: AsRef<str>,
        A: IsValue<'ctx, B>,
    {
        let addr_v = address.as_value();
        let void_ty = self.module.void_type().as_type().id();
        let payload = crate::instr_types::IndirectBrInstData::new(addr_v.id);
        let inst = self.append_instruction(void_ty, InstructionKindData::IndirectBr(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            IndirectBrInst::<Open, B>::from_raw(inst.as_value().id, module_ref, void_ty),
        ))
    }

    /// TYPED `invoke <ret-ty> <callee>(<args>) to label %normal unwind
    /// label %unwind`. Wrong arity / wrong argument types / wrong
    /// result use are compile errors; the invoke's return marker is
    /// derived from the callee. Mirrors `IRBuilder::CreateInvoke` with
    /// the callee schema statically pinned.
    pub fn build_invoke<Ret, Params, A, Normal, Unwind, Name>(
        self,
        callee: TypedFunctionValue<'ctx, Ret, Params, B>,
        args: A,
        normal_dest: Normal,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockTypedInvoke<'ctx, R, Ret, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
        Name: AsRef<str>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.build_invoke_with_config(
            callee,
            args,
            normal_dest,
            unwind_dest,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Produce a TYPED `invoke` with explicit call-site configuration.
    pub fn build_invoke_with_config<Ret, Params, A, Normal, Unwind>(
        self,
        callee: TypedFunctionValue<'ctx, Ret, Params, B>,
        args: A,
        normal_dest: Normal,
        unwind_dest: Unwind,
        config: CallSiteConfig,
    ) -> IrResult<TerminatedBlockTypedInvoke<'ctx, R, Ret, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let normal_dest = normal_dest.into_basic_block_label();
        let unwind_dest = unwind_dest.into_basic_block_label();
        let f = callee.as_function();
        let arg_ids = args.lower(ModuleRef::new(self.module))?;
        let (name, calling_conv, attrs) = config.into_parts();
        let payload = crate::instr_types::InvokeInstData::new_with_attrs(
            f.as_value().id,
            f.signature().as_type().id(),
            arg_ids,
            calling_conv,
            normal_dest.as_value().id,
            unwind_dest.as_value().id,
            attrs,
        );
        let ret_ty = f.return_type().id();
        let inst = self.append_instruction(ret_ty, InstructionKindData::Invoke(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            InvokeInst::<Dyn, B>::from_raw(inst.as_value().id, module_ref, ret_ty)
                .retag::<Ret::Marker>(),
        ))
    }

    /// Produce `invoke <ret-ty> <callee>(<args>) to label %normal
    /// unwind label %unwind`. Mirrors `IRBuilder::CreateInvoke`.
    pub fn build_invoke_dyn<R2, I, V, Normal, Unwind, Name>(
        self,
        callee: FunctionValue<'ctx, R2, B>,
        args: I,
        normal_dest: Normal,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockInvoke<'ctx, R, R2, B>>
    where
        Name: AsRef<str>,
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.build_invoke_dyn_with_config(
            callee,
            args,
            normal_dest,
            unwind_dest,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// The `(function_type, return_type)` a call site should carry: the
    /// caller-spelled override from [`CallSiteConfig::call_site_type`] when
    /// present (mirroring `CallBase`'s own `FunctionType`), else the callee's
    /// declared signature.
    fn resolve_call_site_type<R2: ReturnMarker>(
        &self,
        callee: &FunctionValue<'ctx, R2, B>,
        config: &CallSiteConfig,
    ) -> (FunctionType<'ctx, B>, TypeId) {
        match config.call_site_fn_ty() {
            Some(id) => {
                let ft = FunctionType::<'ctx, B>::new(id, ModuleRef::<B>::new(self.module));
                let ret = ft.return_type().id();
                (ft, ret)
            }
            None => (callee.signature(), callee.return_type().id()),
        }
    }

    /// Produce `invoke` with explicit call-site configuration.
    pub fn build_invoke_dyn_with_config<R2, I, V, Normal, Unwind>(
        self,
        callee: FunctionValue<'ctx, R2, B>,
        args: I,
        normal_dest: Normal,
        unwind_dest: Unwind,
        config: CallSiteConfig,
    ) -> IrResult<TerminatedBlockInvoke<'ctx, R, R2, B>>
    where
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let normal_dest = normal_dest.into_basic_block_label();
        let unwind_dest = unwind_dest.into_basic_block_label();
        let callee_v = callee.as_value();
        let (fn_ty, ret_ty) = self.resolve_call_site_type(&callee, &config);
        let (name, calling_conv, attrs) = config.into_parts();
        let arg_ids: Vec<ValueId> = args.into_iter().map(|a| a.as_value().id).collect();
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let payload = crate::instr_types::InvokeInstData::new_with_attrs(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            normal_dest.as_value().id,
            unwind_dest.as_value().id,
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::Invoke(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            InvokeInst::<Dyn, B>::from_raw(inst.as_value().id, module_ref, ret_ty).retag::<R2>(),
        ))
    }

    /// Produce an `invoke` whose callee is an inline-assembly value.
    pub fn build_inline_asm_invoke<R2, I, V, Normal, Unwind, Name>(
        self,
        asm: InlineAsm<'ctx, B>,
        args: I,
        normal_dest: Normal,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockInvoke<'ctx, R, R2, B>>
    where
        Name: AsRef<str>,
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.build_inline_asm_invoke_with_config(
            asm,
            args,
            normal_dest,
            unwind_dest,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Produce an inline-assembly `invoke` with explicit call-site configuration.
    pub fn build_inline_asm_invoke_with_config<R2, I, V, Normal, Unwind>(
        self,
        asm: InlineAsm<'ctx, B>,
        args: I,
        normal_dest: Normal,
        unwind_dest: Unwind,
        config: CallSiteConfig,
    ) -> IrResult<TerminatedBlockInvoke<'ctx, R, R2, B>>
    where
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let normal_dest = normal_dest.into_basic_block_label();
        let unwind_dest = unwind_dest.into_basic_block_label();
        let asm_v = asm.as_value();
        let fn_ty = asm.function_type();
        let ret_ty = fn_ty.return_type().id();
        let ret_data = self.module.context().type_data(ret_ty);
        if !crate::function::signature_matches_marker::<R2>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: crate::marker::marker_kind_label::<R2>()
                    .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
                got: fn_ty.return_type().kind_label(),
            });
        }
        let mut arg_ids: Vec<ValueId> = Vec::new();
        for arg in args {
            let v = arg.as_value();
            arg_ids.push(v.id);
        }
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let (name, calling_conv, attrs) = config.into_parts();
        let payload = crate::instr_types::InvokeInstData::new_with_attrs(
            asm_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            normal_dest.as_value().id,
            unwind_dest.as_value().id,
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::Invoke(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            InvokeInst::<Dyn, B>::from_raw(inst.as_value().id, module_ref, ret_ty).retag::<R2>(),
        ))
    }

    /// Produce `callbr <ret-ty> <callee>(<args>) to label %default
    /// [label %indirect1, ...]`. Mirrors `IRBuilder::CreateCallBr`.
    pub fn build_callbr<R2, I, V, Default, Indirects, Indirect, Name>(
        self,
        callee: FunctionValue<'ctx, R2, B>,
        args: I,
        default_dest: Default,
        indirect_dests: Indirects,
        name: Name,
    ) -> IrResult<(BasicBlock<'ctx, R, Terminated, B>, CallBrInst<'ctx, B>)>
    where
        Name: AsRef<str>,
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Default: IntoBasicBlockLabel<'ctx, R, B>,
        Indirects: IntoIterator<Item = Indirect>,
        Indirect: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.build_callbr_with_config(
            callee,
            args,
            default_dest,
            indirect_dests,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Produce `callbr` with explicit call-site configuration.
    pub fn build_callbr_with_config<R2, I, V, Default, Indirects, Indirect>(
        self,
        callee: FunctionValue<'ctx, R2, B>,
        args: I,
        default_dest: Default,
        indirect_dests: Indirects,
        config: CallSiteConfig,
    ) -> IrResult<(BasicBlock<'ctx, R, Terminated, B>, CallBrInst<'ctx, B>)>
    where
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Default: IntoBasicBlockLabel<'ctx, R, B>,
        Indirects: IntoIterator<Item = Indirect>,
        Indirect: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let default_dest = default_dest.into_basic_block_label();
        let callee_v = callee.as_value();
        let (fn_ty, ret_ty) = self.resolve_call_site_type(&callee, &config);
        let (name, calling_conv, attrs) = config.into_parts();
        let arg_ids: Vec<ValueId> = args.into_iter().map(|a| a.as_value().id).collect();
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let indirect_ids: Vec<ValueId> = indirect_dests
            .into_iter()
            .map(|d| d.into_basic_block_label().as_value().id)
            .collect();
        let payload = crate::instr_types::CallBrInstData::new_with_attrs(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            default_dest.as_value().id,
            indirect_ids,
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::CallBr(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            CallBrInst::<B>::from_raw(inst.as_value().id, module_ref, ret_ty),
        ))
    }

    /// Produce a `callbr` whose callee is an inline-assembly value.
    pub fn build_inline_asm_callbr<R2, I, V, Default, Indirects, Indirect, Name>(
        self,
        asm: InlineAsm<'ctx, B>,
        args: I,
        default_dest: Default,
        indirect_dests: Indirects,
        name: Name,
    ) -> IrResult<(BasicBlock<'ctx, R, Terminated, B>, CallBrInst<'ctx, B>)>
    where
        Name: AsRef<str>,
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Default: IntoBasicBlockLabel<'ctx, R, B>,
        Indirects: IntoIterator<Item = Indirect>,
        Indirect: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.build_inline_asm_callbr_with_config::<R2, _, _, _, _, _>(
            asm,
            args,
            default_dest,
            indirect_dests,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Produce an inline-assembly `callbr` with explicit call-site configuration.
    pub fn build_inline_asm_callbr_with_config<R2, I, V, Default, Indirects, Indirect>(
        self,
        asm: InlineAsm<'ctx, B>,
        args: I,
        default_dest: Default,
        indirect_dests: Indirects,
        config: CallSiteConfig,
    ) -> IrResult<(BasicBlock<'ctx, R, Terminated, B>, CallBrInst<'ctx, B>)>
    where
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Default: IntoBasicBlockLabel<'ctx, R, B>,
        Indirects: IntoIterator<Item = Indirect>,
        Indirect: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let default_dest = default_dest.into_basic_block_label();
        let asm_v = asm.as_value();
        let fn_ty = asm.function_type();
        let ret_ty = fn_ty.return_type().id();
        let ret_data = self.module.context().type_data(ret_ty);
        if !crate::function::signature_matches_marker::<R2>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: crate::marker::marker_kind_label::<R2>()
                    .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
                got: fn_ty.return_type().kind_label(),
            });
        }
        let mut arg_ids: Vec<ValueId> = Vec::new();
        for arg in args {
            let v = arg.as_value();
            arg_ids.push(v.id);
        }
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let indirect_ids: Vec<ValueId> = indirect_dests
            .into_iter()
            .map(|d| d.into_basic_block_label().as_value().id)
            .collect();
        let (name, calling_conv, attrs) = config.into_parts();
        let payload = crate::instr_types::CallBrInstData::new_with_attrs(
            asm_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            default_dest.as_value().id,
            indirect_ids,
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::CallBr(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            CallBrInst::<B>::from_raw(inst.as_value().id, module_ref, ret_ty),
        ))
    }

    /// Produce `unreachable`. Mirrors `IRBuilder::CreateUnreachable`.
    ///
    /// Consumes `self`; infallible (no operands, no brand check).
    /// Produce `landingpad <ty>`. Mirrors `IRBuilder::CreateLandingPad`.
    /// Returns an [`Open`]-typestate
    /// handle; the caller adds clauses with `add_catch_clause` /
    /// `add_filter_clause` and seals the list with `finish`.
    pub fn build_landingpad<Name>(
        &self,
        result_ty: Type<'ctx, B>,
        cleanup: bool,
        name: Name,
    ) -> IrResult<LandingPadInst<'ctx, Open, B>>
    where
        Name: AsRef<str>,
    {
        let payload = crate::instr_types::LandingPadInstData::new(cleanup);
        let inst =
            self.append_instruction(result_ty.id, InstructionKindData::LandingPad(payload), name);
        Ok(LandingPadInst::<Open, B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            result_ty.id,
        ))
    }

    /// Produce `resume <ty> <value>`. Mirrors `IRBuilder::CreateResume`.
    /// The `value` is typically a previously-built `landingpad` result.
    pub fn build_resume<V, Name>(
        self,
        value: V,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Name: AsRef<str>,
        V: IsValue<'ctx, B>,
    {
        let v = value.as_value();
        let void_ty = self.module.void_type().as_type().id();
        let payload = crate::instr_types::ResumeInstData::new(v.id);
        let inst = self.append_instruction(void_ty, InstructionKindData::Resume(payload), name);
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `cleanuppad within <parent> [<args>]`. Mirrors
    /// `IRBuilder::CreateCleanupPad`.
    pub fn build_cleanup_pad<I, V, Name>(
        &self,
        parent_pad: Value<'ctx, B>,
        args: I,
        name: Name,
    ) -> IrResult<CleanupPadInst<'ctx, B>>
    where
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Name: AsRef<str>,
    {
        self.build_cleanup_pad_raw(Some(parent_pad.id), args, name)
    }

    /// Produce `cleanuppad within none [<args>]`. Mirrors
    /// `IRBuilder::CreateCleanupPad`.
    pub fn build_cleanup_pad_within_none<I, V, Name>(
        &self,
        args: I,
        name: Name,
    ) -> IrResult<CleanupPadInst<'ctx, B>>
    where
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Name: AsRef<str>,
    {
        self.build_cleanup_pad_raw(None, args, name)
    }

    fn build_cleanup_pad_raw<I, V, Name>(
        &self,
        parent_id: Option<ValueId>,
        args: I,
        name: Name,
    ) -> IrResult<CleanupPadInst<'ctx, B>>
    where
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
        Name: AsRef<str>,
    {
        let arg_ids: Vec<ValueId> = args.into_iter().map(|a| a.as_value().id).collect();
        let payload = crate::instr_types::CleanupPadInstData::new(parent_id, arg_ids);
        let token_ty = self.module.token_type().as_type().id();
        let inst =
            self.append_instruction(token_ty, InstructionKindData::CleanupPad(payload), name);
        Ok(CleanupPadInst::<B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            token_ty,
        ))
    }

    /// Produce `catchpad within <catchswitch> [<args>]`. Mirrors
    /// `IRBuilder::CreateCatchPad`.
    pub fn build_catch_pad<I, V, Name>(
        &self,
        catch_switch: Value<'ctx, B>,
        args: I,
        name: Name,
    ) -> IrResult<CatchPadInst<'ctx, B>>
    where
        Name: AsRef<str>,
        I: IntoIterator<Item = V>,
        V: IsValue<'ctx, B>,
    {
        let arg_ids: Vec<ValueId> = args.into_iter().map(|a| a.as_value().id).collect();
        let payload = crate::instr_types::CatchPadInstData::new(Some(catch_switch.id), arg_ids);
        let token_ty = self.module.token_type().as_type().id();
        let inst = self.append_instruction(token_ty, InstructionKindData::CatchPad(payload), name);
        Ok(CatchPadInst::<B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.module),
            token_ty,
        ))
    }

    /// Produce `catchret from <catchpad> to label <bb>`. Mirrors
    /// `IRBuilder::CreateCatchRet`.
    pub fn build_catch_ret<Target, Name>(
        self,
        catch_pad: Value<'ctx, B>,
        target: Target,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Name: AsRef<str>,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let target = target.into_basic_block_label();
        let void_ty = self.module.void_type().as_type().id();
        let payload =
            crate::instr_types::CatchReturnInstData::new(catch_pad.id, target.as_value().id);
        let inst =
            self.append_instruction(void_ty, InstructionKindData::CatchReturn(payload), name);
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `cleanupret from <cleanuppad> unwind label <bb>`.
    /// Mirrors `IRBuilder::CreateCleanupRet`.
    pub fn build_cleanup_ret<Unwind, Name>(
        self,
        cleanup_pad: Value<'ctx, B>,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
        Name: AsRef<str>,
    {
        let unwind_dest = unwind_dest.into_basic_block_label();
        self.build_cleanup_ret_raw(cleanup_pad.id, Some(unwind_dest.as_value().id), name)
    }

    /// Produce `cleanupret from <cleanuppad> unwind to caller`.
    /// Mirrors `IRBuilder::CreateCleanupRet`.
    pub fn build_cleanup_ret_to_caller<Name>(
        self,
        cleanup_pad: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Name: AsRef<str>,
    {
        self.build_cleanup_ret_raw(cleanup_pad.id, None, name)
    }

    fn build_cleanup_ret_raw<Name>(
        self,
        cleanup_pad_id: ValueId,
        unwind_id: Option<ValueId>,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Name: AsRef<str>,
    {
        let void_ty = self.module.void_type().as_type().id();
        let payload = crate::instr_types::CleanupReturnInstData::new(cleanup_pad_id, unwind_id);
        let inst =
            self.append_instruction(void_ty, InstructionKindData::CleanupReturn(payload), name);
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `catchswitch within <parent> [...] unwind label <bb>`.
    /// Mirrors `IRBuilder::CreateCatchSwitch`.
    pub fn build_catch_switch<Unwind, Name>(
        self,
        parent_pad: Value<'ctx, B>,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
        Name: AsRef<str>,
    {
        let unwind_dest = unwind_dest.into_basic_block_label();
        self.build_catch_switch_raw(Some(parent_pad.id), Some(unwind_dest.as_value().id), name)
    }

    /// Produce `catchswitch within <parent> [...] unwind to caller`.
    /// Mirrors `IRBuilder::CreateCatchSwitch`.
    pub fn build_catch_switch_to_caller<Name>(
        self,
        parent_pad: Value<'ctx, B>,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Name: AsRef<str>,
    {
        self.build_catch_switch_raw(Some(parent_pad.id), None, name)
    }

    /// Produce `catchswitch within none [...] unwind label <bb>`.
    /// Mirrors `IRBuilder::CreateCatchSwitch`.
    pub fn build_catch_switch_within_none<Unwind, Name>(
        self,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
        Name: AsRef<str>,
    {
        let unwind_dest = unwind_dest.into_basic_block_label();
        self.build_catch_switch_raw(None, Some(unwind_dest.as_value().id), name)
    }

    /// Produce `catchswitch within none [...] unwind to caller`.
    /// Mirrors `IRBuilder::CreateCatchSwitch`.
    pub fn build_catch_switch_within_none_to_caller<Name>(
        self,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Name: AsRef<str>,
    {
        self.build_catch_switch_raw(None, None, name)
    }

    fn build_catch_switch_raw<Name>(
        self,
        parent_id: Option<ValueId>,
        unwind_id: Option<ValueId>,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Name: AsRef<str>,
    {
        let token_ty = self.module.token_type().as_type().id();
        let payload = crate::instr_types::CatchSwitchInstData::new(parent_id, unwind_id);
        let inst =
            self.append_instruction(token_ty, InstructionKindData::CatchSwitch(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            CatchSwitchInst::<Open, B>::from_raw(inst.as_value().id, module_ref, token_ty),
        ))
    }

    pub fn build_unreachable(
        self,
    ) -> (
        BasicBlock<'ctx, R, Terminated, B>,
        Instruction<'ctx, Attached, B>,
    ) {
        let payload = crate::instr_types::UnreachableInstData;
        let void_ty = self.module.void_type().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Unreachable(payload), "");
        let bb = self.into_insert_block();
        (bb.retag_termination::<Terminated>(), inst)
    }

    // ---- Internal helpers ----

    /// Crate-internal: append a freshly-built instruction to the
    /// insertion block. `name` populates the value-symbol-table when
    /// non-empty.
    fn append_instruction<N: AsRef<str>>(
        &self,
        ty: TypeId,
        kind: InstructionKindData,
        name: N,
    ) -> Instruction<'ctx, Attached, B> {
        let name = name.as_ref();
        let bb = self.insert_block();
        let bb_id = bb.as_value().id;
        let value = build_instruction_value(ty, bb_id, kind, None);
        // Snapshot operand ids before the value is moved into the arena;
        // we need them to register the new instruction in each operand's
        // reverse use-list. Mirrors `User::setOperand` in
        // `llvm/lib/IR/User.cpp`, which threads each `Use` into its
        // operand's use-list at construction time.
        let operand_ids = match &value.kind {
            ValueKindData::Instruction(i) => i.kind.operand_ids(),
            // append_instruction always builds an Instruction-kind value.
            _ => unreachable!("append_instruction built non-instruction value"),
        };
        let id = self.module.context().push_value(value);
        for op in operand_ids {
            self.module
                .context()
                .value_data(op)
                .use_list
                .borrow_mut()
                .push(ValueUse::Instruction(id));
        }
        match self.insert_before {
            Some(anchor) => {
                // Mirrors `IRBuilder::SetInsertPoint(Instruction*)`: new
                // instruction is inserted before the anchor.
                if bb.insert_instruction_before(id, anchor).is_err() {
                    unreachable!(
                        "insert_before anchor not in the builder's insertion block: \
                         positioning methods must keep block and anchor coherent"
                    );
                }
            }
            None => bb.append_instruction(id),
        }
        if !name.is_empty()
            && !Type::new(ty, self.module).is_void()
            && let Some(parent_fn_id) = bb.parent_id()
        {
            let parent_fn = FunctionValue::<Dyn, B>::from_parts_unchecked(
                parent_fn_id,
                ModuleRef::<B>::new(self.module),
            );
            parent_fn.set_local_value_name(id, Some(name));
        }
        Instruction::from_parts(id, ModuleRef::<B>::new(self.module))
    }

    fn int_type_for_bits(&self, bits: u32) -> IrResult<IntType<'ctx, IntDyn, B>> {
        if !(MIN_INT_BITS..=MAX_INT_BITS).contains(&bits) {
            return Err(IrError::InvalidIntegerWidth { bits });
        }
        Ok(IntType::new(
            self.module.context().int_type(bits),
            ModuleRef::<B>::new(self.module),
        ))
    }

    fn ptr_to_addr_result_type(&self, src_ty: Type<'ctx, B>) -> IrResult<Type<'ctx, B>> {
        let (addr_space, vector_shape) = self.ptr_to_addr_source_shape(src_ty)?;
        let address_bits = self.module.data_layout().index_size_in_bits(addr_space);
        let int_ty = self.int_type_for_bits(address_bits)?.as_type();
        let Some((lanes, scalable)) = vector_shape else {
            return Ok(int_ty);
        };
        let vector_id = if scalable {
            self.module
                .context()
                .scalable_vector_type(int_ty.id(), lanes)
        } else {
            self.module.context().fixed_vector_type(int_ty.id(), lanes)
        };
        Ok(Type::new(vector_id, ModuleRef::<B>::new(self.module)))
    }

    fn ptr_to_addr_source_shape(
        &self,
        src_ty: Type<'ctx, B>,
    ) -> IrResult<(u32, Option<(u32, bool)>)> {
        match src_ty.data() {
            TypeData::Pointer { addr_space } => Ok((*addr_space, None)),
            TypeData::FixedVector { elem, n } => match self.module.context().type_data(*elem) {
                TypeData::Pointer { addr_space } => Ok((*addr_space, Some((*n, false)))),
                _ => Err(IrError::InvalidOperation {
                    message: "PtrToAddr source must be pointer",
                }),
            },
            TypeData::ScalableVector { elem, min } => {
                match self.module.context().type_data(*elem) {
                    TypeData::Pointer { addr_space } => Ok((*addr_space, Some((*min, true)))),
                    _ => Err(IrError::InvalidOperation {
                        message: "PtrToAddr source must be pointer",
                    }),
                }
            }
            _ => Err(IrError::InvalidOperation {
                message: "PtrToAddr source must be pointer",
            }),
        }
    }

    /// Validate a custom folder's returned value before the builder narrows it
    /// to a typed handle or returns it as the instruction result.
    fn checked_folded_value(
        &self,
        folded: Value<'ctx, B>,
        expected_ty: TypeId,
    ) -> IrResult<Value<'ctx, B>> {
        if folded.ty != expected_ty {
            return Err(IrError::TypeMismatch {
                expected: Type::new(expected_ty, self.module).kind_label(),
                got: folded.ty().kind_label(),
            });
        }
        Ok(folded)
    }

    /// Accept a typed fold result. For static markers this is the identity —
    /// the type system already guarantees the width/kind. For dyn markers
    /// (IntDyn) the marker doesn't pin the width, so keep a TypeId check.
    /// The branch monomorphizes away for static W.
    fn accept_folded_int<W: IntWidth>(
        &self,
        folded: IntValue<'ctx, W, B>,
        like: IntValue<'ctx, W, B>,
    ) -> IrResult<IntValue<'ctx, W, B>> {
        if W::static_bits().is_none() && folded.as_value().ty().id() != like.as_value().ty().id() {
            return Err(IrError::TypeMismatch {
                expected: like.as_value().ty().kind_label(),
                got: folded.as_value().ty().kind_label(),
            });
        }
        Ok(folded)
    }

    /// Mirrors [`Self::accept_folded_int`] for float kinds, keyed on
    /// `K::ieee_label().is_none()` (the erased `FloatDyn` marker) instead of
    /// `W::static_bits()`.
    fn accept_folded_fp<K: FloatKind>(
        &self,
        folded: FloatValue<'ctx, K, B>,
        like: FloatValue<'ctx, K, B>,
    ) -> IrResult<FloatValue<'ctx, K, B>> {
        if K::ieee_label().is_none()
            && crate::value::Typed::ty(folded).id() != crate::value::Typed::ty(like).id()
        {
            return Err(IrError::TypeMismatch {
                expected: crate::value::Typed::ty(like).kind_label(),
                got: crate::value::Typed::ty(folded).kind_label(),
            });
        }
        Ok(folded)
    }

    /// Accept a typed cast fold result against the destination int type.
    /// Casts have no same-type operand to compare against (unlike binops),
    /// so this checks against `dst_ty` instead of a `like` operand; otherwise
    /// mirrors [`Self::accept_folded_int`].
    fn accept_folded_cast_int<W: IntWidth>(
        &self,
        folded: IntValue<'ctx, W, B>,
        dst_ty: IntType<'ctx, W, B>,
    ) -> IrResult<IntValue<'ctx, W, B>> {
        if W::static_bits().is_none() && folded.as_value().ty().id() != dst_ty.as_type().id() {
            return Err(IrError::TypeMismatch {
                expected: dst_ty.as_type().kind_label(),
                got: folded.as_value().ty().kind_label(),
            });
        }
        Ok(folded)
    }

    /// Mirrors [`Self::accept_folded_cast_int`] for float destination kinds.
    fn accept_folded_cast_fp<K: FloatKind>(
        &self,
        folded: FloatValue<'ctx, K, B>,
        dst_ty: FloatType<'ctx, K, B>,
    ) -> IrResult<FloatValue<'ctx, K, B>> {
        if K::ieee_label().is_none()
            && crate::value::Typed::ty(folded).id() != dst_ty.as_type().id()
        {
            return Err(IrError::TypeMismatch {
                expected: dst_ty.as_type().kind_label(),
                got: crate::value::Typed::ty(folded).kind_label(),
            });
        }
        Ok(folded)
    }

    /// Build the `ret` payload and append. Crate-internal: the typed
    /// `build_ret` methods funnel here after their per-marker
    /// validation. Cannot fail by construction.
    fn append_ret(&self, value: Option<Value<'ctx, B>>) -> Instruction<'ctx, Attached, B> {
        let payload = ReturnOpData::new(value.map(|v| v.id));
        let void_ty = self.module.void_type().as_type().id();
        self.append_instruction(void_ty, InstructionKindData::Ret(payload), "")
    }
}

// --------------------------------------------------------------------------
// `build_ret` dispatch via the [`IntoReturnValue`] trait
// --------------------------------------------------------------------------
//
// Rust's coherence checker rejects two blanket impls (`<W: IntWidth>` +
// `<K: FloatKind>`) on `IRBuilder<R>` even when no type implements both
// traits. We dispatch through a single sealed trait that pins the
// return-value lift per concrete marker. Each impl is concrete-typed so
// no overlap arises. Mirrors `IRBuilder::CreateRet` in `IRBuilder.h`.

/// Sealed: types that can be passed to [`IRBuilder::build_ret`] for a
/// function carrying [`ReturnMarker`] `R`. Concrete impls are provided
/// per `(value-shape, R)` pair so a typed builder accepts every Rust
/// scalar / typed handle that lifts to the correct IR type, while a
/// runtime-checked [`Dyn`] builder accepts anything that implements
/// [`IsValue`].
pub trait IntoReturnValue<'ctx, R: ReturnMarker, B: ModuleBrand = Brand<'ctx>>: Sized {
    #[doc(hidden)]
    fn into_return_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}

// Int-marker impls: every `IntoIntValue<'ctx, W, B>` is also a
// `IntoReturnValue<'ctx, W>`. Expanded per concrete `W` so coherence
// stays sane (a single blanket would conflict with the float side).
macro_rules! impl_into_return_value_int {
    ($($w:ty),+ $(,)?) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx, V> IntoReturnValue<'ctx, $w, B> for V
        where
            V: IntoIntValue<'ctx, $w, B>,
        {
            #[inline]
            fn into_return_value(
                self,
                module: ModuleRef<'ctx, B>,
            ) -> IrResult<Value<'ctx, B>> {
                Ok(IsValue::as_value(self.into_int_value(module)?))
            }
        }
    )+ };
}
impl_into_return_value_int!(bool, i8, i16, i32, i64, i128, IntDyn);

// Float-marker impls. Phase 2 introduces `IntoFloatValue<'ctx, K, B>`; for
// now the typed `FloatValue<'ctx, K, B>` itself is the only direct
// `IntoReturnValue<'ctx, K>` source. Phase 2 will replace these with
// macro-expanded blanket-on-IntoFloatValue impls (matching the int
// side).
macro_rules! impl_into_return_value_float {
    ($($k:ty),+ $(,)?) => { $(
        impl<'ctx, B: ModuleBrand + 'ctx, V> IntoReturnValue<'ctx, $k, B> for V
        where
            V: IntoFloatValue<'ctx, $k, B>,
        {
            #[inline]
            fn into_return_value(
                self,
                module: ModuleRef<'ctx, B>,
            ) -> IrResult<Value<'ctx, B>> {
                Ok(IsValue::as_value(self.into_float_value(module)?))
            }
        }
    )+ };
}
impl_into_return_value_float!(
    f32,
    f64,
    crate::float_kind::Half,
    crate::float_kind::BFloat,
    crate::float_kind::Fp128,
    crate::float_kind::X86Fp80,
    crate::float_kind::PpcFp128,
    FloatDyn,
);

// Pointer-marker impl: `Ptr` accepts any pointer-valued operand source.
impl<'ctx, B: ModuleBrand + 'ctx, V> IntoReturnValue<'ctx, Ptr, B> for V
where
    V: IntoPointerValue<'ctx, B>,
{
    #[inline]
    fn into_return_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(IsValue::as_value(self.into_pointer_value(module)?))
    }
}

// Top-level erased `Dyn` accepts anything implementing `IsValue`.
impl<'ctx, B: ModuleBrand + 'ctx, V> IntoReturnValue<'ctx, Dyn, B> for V
where
    V: IsValue<'ctx, B>,
{
    #[inline]
    fn into_return_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(self.as_value())
    }
}

impl<'m, 'ctx, B, F, R> IRBuilder<'m, 'ctx, B, F, Positioned, R>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Produce `ret <value>` against the function's declared return
    /// type. The accepted operand types are pinned by `R` through the
    /// [`IntoReturnValue`] trait - a builder for `i32`-returning
    /// function takes any `IntoIntValue<'ctx, i32, B>`, the float / ptr
    /// builders take their corresponding handles, and a [`Dyn`]
    /// builder accepts anything implementing
    /// [`IsValue`] but runs an extra runtime
    /// type-equality check.
    pub fn build_ret<V>(self, value: V) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        V: IntoReturnValue<'ctx, R, B>,
    {
        let v = value.into_return_value(ModuleRef::new(self.module))?;
        // Runtime-check for the fully-erased `Dyn` marker.
        if R::expected_kind() == crate::marker::ExpectedRetKind::Dyn {
            let parent_fn = self.parent_function_dyn();
            let expected = parent_fn.return_type();
            if v.ty().id() != expected.id() {
                return Err(IrError::ReturnTypeMismatch {
                    expected: expected.kind_label(),
                    got: v.ty().kind_label(),
                });
            }
        }
        let inst = self.append_ret(Some(v));
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Owning function of the current insertion block, in its
    /// runtime-checked form. Used by the `Dyn`-marker fall-back inside
    /// [`Self::build_ret`].
    fn parent_function_dyn(&self) -> FunctionValue<'ctx, Dyn, B> {
        let bb = self.insert_block();
        let parent_id = bb.parent_id().unwrap_or_else(|| {
            unreachable!("Positioned builder block always has a parent function")
        });
        FunctionValue::<Dyn, B>::from_parts_unchecked(parent_id, ModuleRef::<B>::new(self.module))
    }
}

impl<'m, 'ctx, B, F> IRBuilder<'m, 'ctx, B, F, Positioned, ()>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
{
    /// Produce `ret void`. Mirrors `IRBuilder::CreateRetVoid`. The
    /// `()` builder does not expose `build_ret(value)` at all (no
    /// `IntoReturnValue<'ctx, ()>` impls exist), so `build_ret_void`
    /// is the only return option.
    pub fn build_ret_void(self) -> VoidReturnInst<'ctx, B> {
        let inst = self.append_ret(None);
        let bb = self.into_insert_block();
        (bb.retag_termination::<Terminated>(), inst)
    }
}

impl<'m, 'ctx, B, F> IRBuilder<'m, 'ctx, B, F, Positioned, Dyn>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
{
    /// Produce `ret void`. Errors with
    /// [`IrError::ReturnTypeMismatch`] if the parent function does
    /// not actually return `void`.
    pub fn build_ret_void(self) -> IrResult<TerminatedBlockInst<'ctx, Dyn, B>> {
        let parent_id = self.insert_block().parent_id().unwrap_or_else(|| {
            unreachable!("Positioned builder block always has a parent function")
        });
        let parent_fn = FunctionValue::<Dyn, B>::from_parts_unchecked(
            parent_id,
            ModuleRef::<B>::new(self.module),
        );
        let expected = parent_fn.return_type();
        if !expected.is_void() {
            return Err(IrError::ReturnTypeMismatch {
                expected: expected.kind_label(),
                got: TypeKindLabel::Void,
            });
        }
        let inst = self.append_ret(None);
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }
}

// --------------------------------------------------------------------------
// CallBuilder
// --------------------------------------------------------------------------

/// Builder for [`crate::IRBuilder::call_builder`]. Accumulates
/// per-arg / flag state via chainable methods, then emits the call
/// instruction on `.build()`. Each `.arg(...)` call is statically
/// dispatched against `V: IsValue<'ctx, B>`; arg types can vary
/// across calls without trait objects.
pub struct CallBuilder<'a, 'm, 'ctx, B, F, RP, RC>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    RC: ReturnMarker,
{
    parent: &'a IRBuilder<'m, 'ctx, B, F, Positioned, RP>,
    callee_id: ValueId,
    fn_ty: TypeId,
    return_ty: TypeId,
    args: Vec<ValueId>,
    calling_conv: crate::CallingConv,
    tail_kind: crate::instr_types::TailCallKind,
    attrs: crate::instr_types::CallAttributeData,
    name: String,
    intrinsic_descriptor: Option<IntrinsicDescriptor<'ctx, B>>,
    _rp: PhantomData<RP>,
    _rc: PhantomData<RC>,
}

impl<'a, 'm, 'ctx, B, F, RP, RC> CallBuilder<'a, 'm, 'ctx, B, F, RP, RC>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    RC: ReturnMarker,
{
    /// Add an argument. Statically dispatched per `V: IsValue` so
    /// mixed-type argument lists work without homogeneity.
    pub fn arg<V: IsValue<'ctx, B>>(mut self, value: V) -> Self {
        let v = value.as_value();
        self.args.push(v.id);
        self
    }

    pub fn tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::Tail;
        self
    }

    pub fn must_tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::MustTail;
        self
    }

    pub fn no_tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::NoTail;
        self
    }

    pub fn calling_conv(mut self, cc: CallingConv) -> Self {
        self.calling_conv = cc;
        self
    }
    pub fn call_attributes(mut self, attrs: CallAttributeData) -> Self {
        self.attrs = attrs;
        self
    }

    pub fn name<Name>(mut self, name: Name) -> Self
    where
        Name: AsRef<str>,
    {
        self.name = name.as_ref().to_owned();
        self
    }

    fn validate_intrinsic_descriptor_args(&self) -> IrResult<()> {
        let Some(descriptor) = &self.intrinsic_descriptor else {
            return Ok(());
        };
        let fn_ty = descriptor.function_type_ref(ModuleRef::<B>::new(self.parent.module))?;
        let params: Vec<_> = fn_ty.params().collect();
        let wrong_count = if fn_ty.is_var_arg() {
            self.args.len() < params.len()
        } else {
            self.args.len() != params.len()
        };
        if wrong_count {
            return Err(IrError::IntrinsicSignatureMismatch {
                name: intrinsic_descriptor_error_name(descriptor),
            });
        }
        for (arg, expected) in self.args.iter().zip(params) {
            let actual_ty = self.parent.module.context().value_data(*arg).ty;
            if actual_ty != expected.id() {
                return Err(IrError::IntrinsicSignatureMismatch {
                    name: intrinsic_descriptor_error_name(descriptor),
                });
            }
        }
        Ok(())
    }

    /// Emit the call instruction.
    pub fn build(self) -> IrResult<CallInst<'ctx, RC, B>> {
        self.validate_intrinsic_descriptor_args()?;
        let fn_ty =
            FunctionType::<'ctx, B>::new(self.fn_ty, ModuleRef::<B>::new(self.parent.module));
        self.parent.validate_call_site_args(fn_ty, &self.args)?;
        let payload = crate::instr_types::CallInstData::new_with_attrs(
            self.callee_id,
            self.fn_ty,
            self.args.into_boxed_slice(),
            self.calling_conv,
            self.tail_kind,
            self.attrs,
        );
        let inst = self.parent.append_instruction(
            self.return_ty,
            InstructionKindData::Call(payload),
            self.name,
        );
        Ok(CallInst::<RC, B>::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.parent.module),
            inst.ty().id(),
        ))
    }
}

impl<'a, 'm, 'ctx, B, F, RP> CallBuilder<'a, 'm, 'ctx, B, F, RP, Dyn>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
{
    /// Override the call site's function type so it no longer derives from
    /// the callee's declaration. Mirrors LLVM's `CallBase`, which carries
    /// its own `FunctionType` independent of the callee operand: a direct
    /// `call` may be spelled through a function type that differs from the
    /// declared callee (opaque-pointer IR — the verifier checks the call
    /// against its own type, not the declaration; `LLParser::parseCall`
    /// resolves the callee as a bare pointer). Offered only on the erased
    /// (`Dyn`) builder, where overriding the result type cannot desync a
    /// static return marker.
    pub fn call_site_type(mut self, fn_ty: FunctionType<'ctx, B>) -> Self {
        self.return_ty = fn_ty.return_type().id();
        self.fn_ty = fn_ty.as_type().id();
        self
    }
}

// --------------------------------------------------------------------------
// TypedCallBuilder
// --------------------------------------------------------------------------

/// Chainable builder for [`crate::IRBuilder::typed_call_builder`]. Same
/// schema guarantees as [`crate::IRBuilder::build_call`] — the callee's
/// return marker, parameter schema, and lowered arguments are all
/// pinned by `Ret` / `Params` / `A` — with tail-call kind / calling
/// convention / attributes / result name accumulated via chainable
/// methods before `.build()` emits the call.
pub struct TypedCallBuilder<'a, 'm, 'ctx, B, F, RP, Ret, Params, A>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    Ret: FunctionReturn,
    Params: FunctionParamList,
    A: CallArgs<'ctx, Params, B>,
{
    parent: &'a IRBuilder<'m, 'ctx, B, F, Positioned, RP>,
    callee: TypedFunctionValue<'ctx, Ret, Params, B>,
    args: A,
    tail_kind: crate::instr_types::TailCallKind,
    calling_conv: Option<CallingConv>,
    attrs: CallAttributeData,
    name: String,
}

impl<'a, 'm, 'ctx, B, F, RP, Ret, Params, A>
    TypedCallBuilder<'a, 'm, 'ctx, B, F, RP, Ret, Params, A>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    Ret: FunctionReturn,
    Params: FunctionParamList,
    A: CallArgs<'ctx, Params, B>,
{
    pub fn tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::Tail;
        self
    }

    pub fn must_tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::MustTail;
        self
    }

    pub fn no_tail(mut self) -> Self {
        self.tail_kind = crate::instr_types::TailCallKind::NoTail;
        self
    }

    pub fn calling_conv(mut self, cc: CallingConv) -> Self {
        self.calling_conv = Some(cc);
        self
    }

    pub fn call_attributes(mut self, attrs: CallAttributeData) -> Self {
        self.attrs = attrs;
        self
    }

    pub fn name<Name>(mut self, name: Name) -> Self
    where
        Name: AsRef<str>,
    {
        self.name = name.as_ref().to_owned();
        self
    }

    /// Emit the call instruction.
    pub fn build(self) -> IrResult<TypedCallInst<'ctx, Ret, B>> {
        let f = self.callee.as_function();
        let arg_ids = self.args.lower(ModuleRef::new(self.parent.module))?;
        let calling_conv = self.calling_conv.unwrap_or_else(|| f.calling_conv());
        let payload = crate::instr_types::CallInstData::new_with_attrs(
            f.as_value().id,
            f.signature().as_type().id(),
            arg_ids,
            calling_conv,
            self.tail_kind,
            self.attrs,
        );
        let inst = self.parent.append_instruction(
            f.return_type().id(),
            InstructionKindData::Call(payload),
            self.name,
        );
        Ok(TypedCallInst::from_call(CallInst::from_raw(
            inst.as_value().id,
            ModuleRef::<B>::new(self.parent.module),
            inst.ty().id(),
        )))
    }
}

/// Builder for descriptor-backed intrinsic calls.
pub struct IntrinsicCallBuilder<'a, 'm, 'ctx, B, F, RP>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
{
    inner: CallBuilder<'a, 'm, 'ctx, B, F, RP, Dyn>,
}

impl<'a, 'm, 'ctx, B, F, RP> IntrinsicCallBuilder<'a, 'm, 'ctx, B, F, RP>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
{
    /// Add an argument. Statically dispatched per `V: IsValue` so
    /// mixed-type argument lists work without homogeneity.
    pub fn arg<V: IsValue<'ctx, B>>(mut self, value: V) -> Self {
        self.inner = self.inner.arg(value);
        self
    }

    pub fn tail(mut self) -> Self {
        self.inner = self.inner.tail();
        self
    }

    pub fn must_tail(mut self) -> Self {
        self.inner = self.inner.must_tail();
        self
    }

    pub fn no_tail(mut self) -> Self {
        self.inner = self.inner.no_tail();
        self
    }

    pub fn calling_conv(mut self, cc: CallingConv) -> Self {
        self.inner = self.inner.calling_conv(cc);
        self
    }

    pub fn call_attributes(mut self, attrs: CallAttributeData) -> Self {
        self.inner = self.inner.call_attributes(attrs);
        self
    }

    pub fn name<Name>(mut self, name: Name) -> Self
    where
        Name: AsRef<str>,
    {
        self.inner = self.inner.name(name);
        self
    }

    /// Emit the intrinsic call instruction.
    pub fn build(self) -> IrResult<IntrinsicInst<'ctx, Dyn, B>> {
        let descriptor = self.inner.intrinsic_descriptor.clone();
        let call = self.inner.build()?;
        IntrinsicInst::from_call(call).ok_or_else(|| IrError::IntrinsicSignatureMismatch {
            name: descriptor
                .as_ref()
                .map(intrinsic_descriptor_error_name)
                .unwrap_or_else(|| "intrinsic call".to_owned()),
        })
    }
}

fn intrinsic_descriptor_error_name<B: ModuleBrand>(
    descriptor: &IntrinsicDescriptor<'_, B>,
) -> String {
    match descriptor.mangled_name() {
        Ok(name) => name,
        Err(_) => descriptor.base_name().to_owned(),
    }
}

// `require_same_int_width` is no longer needed: the IRBuilder's binary-

// --------------------------------------------------------------------------
// SelectArm + build_select
// --------------------------------------------------------------------------

#[doc(hidden)]
pub mod select_narrow_token {
    use core::marker::PhantomData;

    /// Evidence that a select fold/result value has already been checked
    /// against the arm type. Only this crate can mint it (private field,
    /// `pub(crate)` constructor), so downstream code can *name* the type in
    /// trait impls but cannot call `from_select_value` with a forged value.
    /// Follows the `ValidatedStructValue` capability-token precedent
    /// (`struct_schema.rs`).
    #[derive(Debug)]
    pub struct SelectNarrow<'a> {
        _private: PhantomData<&'a ()>,
    }

    impl<'a> SelectNarrow<'a> {
        #[inline]
        pub(crate) fn new() -> Self {
            Self {
                _private: PhantomData,
            }
        }
    }
}

pub use select_narrow_token::SelectNarrow;

/// Sealed: types that can appear as the true/false arms of a
/// `select`. The associated `Output` pins the result handle's
/// shape so `b.build_select(cond, a, b)` returns the same handle
/// type the user passed in. Mirrors LangRef's invariant that the
/// two arms must have identical IR types.
pub trait SelectArm<'ctx, B: ModuleBrand = Brand<'ctx>>: Sized + select_arm_sealed::Sealed {
    type Output;
    #[doc(hidden)]
    fn from_select_value(v: Value<'ctx, B>, narrow: &SelectNarrow<'_>) -> Self::Output;
    #[doc(hidden)]
    fn arm_value(self) -> Value<'ctx, B>;
}

mod select_arm_sealed {
    use super::{FloatKind, FloatValue, IntValue, IntWidth, ModuleBrand, PointerValue};

    pub trait Sealed {}

    impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Sealed for IntValue<'ctx, W, B> {}
    impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Sealed for FloatValue<'ctx, K, B> {}
    impl<'ctx, B: ModuleBrand + 'ctx> Sealed for PointerValue<'ctx, B> {}
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for IntValue<'ctx, W, B> {
    type Output = IntValue<'ctx, W, B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        IntValue::<W, B>::from_value_unchecked(v)
    }
    #[inline]
    fn arm_value(self) -> Value<'ctx, B> {
        IsValue::as_value(self)
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for FloatValue<'ctx, K, B> {
    type Output = FloatValue<'ctx, K, B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        FloatValue::<K, B>::from_value_unchecked(v)
    }
    #[inline]
    fn arm_value(self) -> Value<'ctx, B> {
        IsValue::as_value(self)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for PointerValue<'ctx, B> {
    type Output = PointerValue<'ctx, B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        PointerValue::from_value_unchecked(v)
    }
    #[inline]
    fn arm_value(self) -> Value<'ctx, B> {
        IsValue::as_value(self)
    }
}

impl<'m, 'ctx, B, F, R> IRBuilder<'m, 'ctx, B, F, Positioned, R>
where
    B: ModuleBrand + 'ctx,
    F: IRBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Produce `select i1 <cond>, <ty> <true>, <ty> <false>`.
    /// Mirrors `IRBuilder::CreateSelect`.
    ///
    /// Both arms must share the same Rust type `A`, which pins the
    /// IR-type invariant that LangRef requires. The returned handle
    /// is `A::Output`, statically tied to the arm category.
    pub fn build_select<C, A, Name>(
        &self,
        cond: C,
        true_arm: A,
        false_arm: A,
        name: Name,
    ) -> IrResult<A::Output>
    where
        Name: AsRef<str>,
        C: IntoIntValue<'ctx, bool, B>,
        A: SelectArm<'ctx, B> + Copy,
    {
        let c = cond.into_int_value(ModuleRef::new(self.module))?;
        let true_v = true_arm.arm_value();
        let true_ty = true_arm.arm_value().ty().id();
        let false_v = false_arm.arm_value();
        let false_ty = false_arm.arm_value().ty().id();
        if true_ty != false_ty {
            return Err(IrError::TypeMismatch {
                expected: true_v.ty().kind_label(),
                got: false_v.ty().kind_label(),
            });
        }
        if let Some(folded) = self.folder.fold_select_dyn(c.as_value(), true_v, false_v)? {
            let folded = self.checked_folded_value(folded, true_ty)?;
            return Ok(A::from_select_value(folded, &SelectNarrow::new()));
        }
        let payload =
            crate::instr_types::SelectInstData::new(c.as_value().id, true_v.id, false_v.id);
        let inst = self.append_instruction(true_ty, InstructionKindData::Select(payload), name);
        Ok(A::from_select_value(inst.as_value(), &SelectNarrow::new()))
    }
}

// --------------------------------------------------------------------------
// Aggregate path resolution helper
// --------------------------------------------------------------------------

/// Walk the aggregate `root` by `indices` and return the leaf type.
/// Mirrors `ExtractValueInst::getIndexedType` in `Instructions.cpp`, which
/// rejects (rather than clamps) an index at or past the element count.
fn walk_aggregate_for_builder(m: &ModuleCore, root: TypeId, indices: &[u32]) -> IrResult<TypeId> {
    let mut cur = root;
    for &idx in indices {
        let d = m.context().type_data(cur);
        match d {
            TypeData::Array { elem, n } => {
                let count_u64 = *n;
                if u64::from(idx) >= count_u64 {
                    return Err(IrError::AggregateIndexOutOfRange {
                        index: idx,
                        count: count_u64,
                    });
                }
                cur = *elem;
            }
            TypeData::Struct(s) => {
                let body = s.body.borrow();
                match body.as_ref() {
                    Some(b) => {
                        // `elements.len()` is a `usize` count of an in-memory
                        // Vec, so it always fits `u64` on every platform this
                        // targets; treat overflow as out-of-range rather
                        // than masking it, matching the array arm above.
                        let count_u64 = u64::try_from(b.elements.len()).map_err(|_| {
                            IrError::AggregateIndexOutOfRange {
                                index: idx,
                                count: u64::MAX,
                            }
                        })?;
                        if u64::from(idx) >= count_u64 {
                            return Err(IrError::AggregateIndexOutOfRange {
                                index: idx,
                                count: count_u64,
                            });
                        }
                        // 16-bit-usize targets are unsupported; erroring (not unreachable!) here keeps
                        // the aggregate walk total without a new invariant.
                        let i = usize::try_from(idx).map_err(|_| {
                            IrError::AggregateIndexOutOfRange {
                                index: idx,
                                count: count_u64,
                            }
                        })?;
                        cur = b.elements[i];
                    }
                    None => {
                        return Err(IrError::TypeMismatch {
                            expected: crate::error::TypeKindLabel::Struct,
                            got: Type::new(cur, m).kind_label(),
                        });
                    }
                }
            }
            _ => {
                return Err(IrError::TypeMismatch {
                    expected: crate::error::TypeKindLabel::Struct,
                    got: Type::new(cur, m).kind_label(),
                });
            }
        }
    }
    Ok(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Linkage;

    /// Hostile in-crate folder simulating a *buggy* native typed-hook
    /// override (the class of bug `ConstantFolder`'s own native
    /// `fold_int_bin_op<W>` override -- see `ir_builder/constant_folder.rs`
    /// -- is trusted, by kernel-invariant audit, not to commit). Unlike
    /// `tests/constant_folder_builder.rs`'s external `WideningDynFolder`
    /// (which can only override the erased `fold_bin_op_dyn` hook and so
    /// gets caught by `folder::narrow_folded_int`'s TypeId re-check before
    /// the builder ever sees the result), this folder overrides the
    /// *typed* `fold_int_bin_op<W>` hook directly and answers with an
    /// `IntValue<'ctx, W, B>` built via the crate-internal
    /// `IntValue::from_value_unchecked` escape hatch (`pub(super)` in
    /// `value.rs`, reachable here because this module lives at the crate
    /// root, same as `value`). That constructor performs no width check at
    /// all, so this override can -- and deliberately does -- lie about the
    /// width of its `stored` payload: it always answers with a 64-bit
    /// constant, regardless of `W`. Because this is a *native* override
    /// (not the trait's delegating default body at `folder.rs`'s
    /// `fold_int_bin_op<W>`), `narrow_folded_int` never runs on this path;
    /// the only remaining guard is the builder's own
    /// `accept_folded_int` dyn-marker re-check.
    #[derive(Debug, Clone, Copy)]
    struct HostileTypedFolder<'ctx, B: ModuleBrand + 'ctx> {
        /// Always a 64-bit constant, deliberately the wrong width for any
        /// 32-bit `W` the builder calls this with.
        stored: IntValue<'ctx, i64, B>,
    }

    impl<'ctx, B: ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B> for HostileTypedFolder<'ctx, B> {
        fn fold_int_bin_op<W: IntWidth>(
            &self,
            _opcode: BinaryOpcode,
            _lhs: IntValue<'ctx, W, B>,
            _rhs: IntValue<'ctx, W, B>,
        ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
            // Bypasses `narrow_folded_int` entirely: this reuses the
            // already-erased `Value` payload behind `self.stored` (a
            // 64-bit constant) and rewraps it as `IntValue<'ctx, W, B>`
            // via the unchecked constructor, exactly mirroring the shape
            // `ConstantFolder::fold_int_bin_op` uses for its (audited,
            // correct) native override -- except here the "audit" is
            // deliberately false: the payload's true IR type never
            // matches `W` when `W` is a 32-bit dyn width.
            Ok(Some(IntValue::<W, B>::from_value_unchecked(
                self.stored.as_value(),
            )))
        }
    }

    /// Locks `accept_folded_int`'s dyn-marker branch (`ir_builder.rs`,
    /// `W::static_bits().is_none()` arm) as the seam that rejects a
    /// wrong-width result from a *native* typed-hook override -- the bug
    /// class external folders are compile-time barred from producing
    /// (see the sibling compile-fail golden
    /// `tests/compile_fail/folder_typed_wrong_width.rs`, which locks the
    /// external-facing half of this contract) but an in-crate folder can
    /// still write by hand via `from_value_unchecked`.
    ///
    /// Trace confirming *this* line rejects, not `narrow_folded_int`:
    /// `build_int_add::<IntDyn, _, _, _>` (this file, `build_int_add`)
    /// calls `self.folder.fold_int_bin_op(BinaryOpcode::Add, lhs, rhs)`.
    /// `HostileTypedFolder`'s override above is a *native* override of
    /// `fold_int_bin_op`, so it runs directly -- it never calls
    /// `fold_bin_op_dyn` or `folder::narrow_folded_int` (those only run
    /// inside the *trait's default* body, which this override replaces).
    /// The native override returns `Ok(Some(wrong_width_value))`
    /// straight back to `build_int_add`, which forwards it to
    /// `self.accept_folded_int(folded, lhs)`. Inside `accept_folded_int`:
    /// `W = IntDyn`, so `W::static_bits().is_none()` is `true`
    /// (`int_width.rs`'s `impl IntWidth for IntDyn`), and
    /// `folded.as_value().ty().id() != like.as_value().ty().id()` is
    /// `true` (the stored value's real type is `i64`, `lhs`'s is the
    /// 32-bit custom-width `IntDyn` type) -- so `accept_folded_int`
    /// returns `Err(IrError::TypeMismatch { .. })`. That is the exact
    /// line under test; `narrow_folded_int` is never reached on this path.
    #[test]
    fn hostile_native_typed_override_wrong_width_rejected_by_accept_folded_int()
    -> Result<(), IrError> {
        Module::with_new("hostile-typed-folder", |m| {
            let i32_dyn_ty = m.custom_width_int_type(32)?;
            let i64_dyn_ty = m.custom_width_int_type(64)?;
            let fn_ty = m.fn_type(m.i32_type(), Vec::<Type>::new(), false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");

            let stored: IntValue<'_, i64, _> =
                IntValue::from_value_unchecked(i64_dyn_ty.const_zero().as_value());
            let folder = HostileTypedFolder { stored };
            let b = IRBuilder::with_folder(&m, folder).position_at_end(entry);

            let lhs = i32_dyn_ty.const_int_checked(1_i32)?;
            let rhs = i32_dyn_ty.const_int_checked(2_i32)?;

            let err = b
                .build_int_add::<IntDyn, _, _, _>(lhs, rhs, "sum")
                .expect_err("wrong-width native-override fold result is rejected");

            assert!(matches!(err, IrError::TypeMismatch { .. }));
            assert_eq!(b.insert_block().instructions().len(), 0);
            Ok(())
        })
    }
}
