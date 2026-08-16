//! IR builder. Mirrors `llvm/include/llvm/IR/IRBuilder.h` and
//! `llvm/lib/IR/IRBuilder.cpp`.
//!
//! ## Type-state
//!
//! The builder carries two state-marker generics:
//!
//! - `S` ([`Unpositioned`] / [`Positioned`]) — distinguishes "I have an
//!   insertion point" from "I do not". The emitter methods are only
//!   available on the [`Positioned`] state.
//! - `R: ReturnMarker` — the parent function's return shape. The
//!   typed `ret` methods are dispatched on `R` so calling
//!   `ret(int_value)` against a `void`-returning builder is a
//!   compile-time error rather than a runtime
//!   [`IrError::ReturnTypeMismatch`].
//!
//! Mirrors the inkwell `Builder<'ctx>` shape but with the additional
//! invariants that an unpositioned builder has no emitter API at
//! all and a `void`-returning builder cannot accidentally emit a
//! value-bearing return.
//!
//! ## What's shipped
//!
//! The builder routes side-effect-free arithmetic, cast, compare, GEP,
//! select, vector, and aggregate construction through
//! [`folder::IrBuilderFolder`] before materialising an instruction.
//! [`constant_folder::ConstantFolder`] is the default strategy, with
//! [`no_folder::NoFolder`] available for callers that want instructions
//! unconditionally.
//!
//! Other emitter methods land as their consumers do; the trait /
//! method names are stable.

pub mod constant_folder;
pub mod folder;
pub mod no_folder;

use crate::Branded;
use core::marker::PhantomData;

use super::align::{Align, MaybeAlign};
use super::array_len::ArrayLen;
use super::atomic_ordering::AtomicOrdering;
use super::atomicrmw_binop::AtomicRmwBinOp;
use super::basic_block::{
    BasicBlock, BasicBlockLabel, BlockCall, IntoBasicBlockLabel, block_parameter_phis,
    require_no_block_parameters,
};
use super::block_params::{BlockParams, BlockParamsDyn};
use super::block_state::{Terminated, Unterminated};
use super::calling_conv::CallingConv;
use super::cmp_predicate::CmpPredicate;
use super::cmp_predicate::{FloatPredicate, IntPredicate};
use super::constant::{Constant, ConstantExprFlags, ConstantExprOpcode};
use super::constant_fold;
use super::constants::ConstantExprOptions;
use super::derived_types::{FloatType, FunctionType, IntType, PointerType, StructType};
use super::element::{ElemDyn, StaticVecElem, VecElem, WrapWitness};
use super::error::{IrError, IrResult, TypeKindLabel};
use super::float_kind::{Bfloat, Fp128, Half, PpcFp128, StaticFloatKind, X86Fp80};
use super::float_kind::{FloatDyn, FloatKind, FloatWiderThan, IntoFloatValue};
use super::fmf::FastMathFlags;
use super::function::{FunctionValue, IntoCallee};
use super::function_signature::token::ValidatedFunctionParams;
use super::function_signature::{
    CallArgs, FunctionParamList, FunctionReturn, FunctionSignature, IntoTypedCallee,
    IntoVarArgsCallee, TypedFunctionValue,
};
use super::gep_no_wrap_flags::GepNoWrapFlags;
use super::inline_asm::InlineAsm;
use super::instr_types::FnegInstData;
use super::instr_types::{
    AddFlags, AllocaFlags, AllocaInstData, AshrFlags, AtomicCmpXchgConfig, AtomicCmpXchgInstData,
    AtomicRmwConfig, AtomicRmwInstData, BranchInstData, BranchKind, CallBrInstData, CallInstData,
    CatchPadInstData, CatchReturnInstData, CatchSwitchInstData, CleanupPadInstData,
    CleanupReturnInstData, CmpInstData, ExtractElementInstData, ExtractValueInstData, FcmpInstData,
    FenceInstData, FreezeInstData, GepInstData, IcmpFlags, IndirectBrInstData,
    InsertElementInstData, InsertValueInstData, IntBinOpFlags, IntCastFlags, InvokeInstData,
    LandingPadInstData, LshrFlags, MulFlags, OrFlags, PhiData, ResumeInstData, SdivFlags,
    SelectInstData, ShlFlags, ShuffleVectorInstData, SubFlags, SwitchInstData, TailCallKind,
    TruncFlags, UdivFlags, UiToFpFlags, UnreachableInstData, VaArgInstData, WriteBinopFlags,
    ZextFlags,
};
use super::instr_types::{
    BinaryOpData, BinaryOpcode, CallAttributeData, CastOpData, CastOpcode, LoadInstData,
    OverflowFlags, ReturnOpData, ShuffleMaskElem, StoreInstData, UnaryOpcode,
};
use super::instruction::{
    Instruction, InstructionKind, InstructionKindData, InstructionView, build_instruction_value,
    state::Attached,
};
use super::instructions::FenceInst;
use super::instructions::{
    CallBrInst, CallInst, CatchPadInst, CatchSwitchInst, CleanupPadInst, IndirectBrInst,
    InvokeInst, LandingPadInst, StoreInst, SwitchInst,
};
use super::int_width::WiderThan;
use super::int_width::{IntDyn, IntWidth, IntoIntValue, StaticIntWidth};
use super::intrinsic_inst::IntrinsicInst;
use super::intrinsics::{BinaryIntrinsic, IntrinsicDescriptor, IntrinsicId};
use super::ir_builder::constant_folder::ConstantFolder;
use super::ir_builder::folder::IrBuilderFolder;
use super::marker::ExpectedRetKind;
use super::marker::{Dyn, Ptr, ReturnMarker};
use super::module::{
    DynBrand, Invariant, Module, ModuleBrand, ModuleCore, ModuleRef, ModuleView, Unverified,
};
use super::struct_body_state::StructBodyDyn;
use super::struct_schema::{FieldOf, IntoIrField, IrField, StructFieldAt, StructSchema};
use super::sync_scope::SyncScope;
use super::term_open_state::{Closed, Open};
use super::r#type::{IrType, MAX_INT_BITS, MIN_INT_BITS, Type, TypeData, TypeSlot};
use super::typed_pointer_value::TypedPointerValue;
use super::value::{
    ArrayValue, FloatValue, IntValue, IntoErasedValue, IntoPointerValue, IsValue, PointerValue,
    Value, ValueKindData, ValueSlot, ValueUse, VectorValue,
};
use super::value_id::{
    AtomicCmpXchgInstId, AtomicRmwInstId, BlockId, CallInstId, FloatValueId, FpPhiInstId,
    FreezeInstId, IntValueId, IntrinsicInstId, OtherPhiInstId, PhiInstId, PointerPhiInstId,
    PointerValueId, TypedCallInstId, VaArgInstId, ValueId, ViewIn,
};
use super::vec_len::{LenDyn, StaticVecLen, VecLen};

/// Pair returned by terminator builders: the terminated insertion block and
/// the emitted terminator instruction.
pub type TerminatedBlockInst<'ctx, R, B> = (
    BasicBlock<'ctx, R, Terminated, B>,
    Instruction<'ctx, Attached, B>,
);

/// Pair returned by [`IrBuilder::append_block_with_params`]: the freshly
/// appended, still-[`Unterminated`] block and one head-phi result [`Value`]
/// per declared block parameter, in declaration order.
pub type BlockWithParams<'ctx, R, B> = (BasicBlock<'ctx, R, Unterminated, B>, Vec<Value<'ctx, B>>);

/// Pair returned by [`IrBuilder::append_block_typed`]: the freshly appended,
/// still-[`Unterminated`] block stamped with its typed parameter schema
/// `Params`, and that schema's typed parameter-handle tuple
/// ([`Params::Values`](FunctionParamList::Values)) sourced from the block's
/// head-phis. Typed sibling of [`BlockWithParams`].
pub type TypedBlockWithParams<'ctx, R, Params, B> = (
    BasicBlock<'ctx, R, Unterminated, B, Params>,
    <Params as FunctionParamList>::Values<'ctx, B>,
);

/// Pair returned by the width-erased [`IrBuilder::switch_dyn`] before
/// the case list is closed. Erased sibling of
/// [`TerminatedBlockSwitchTyped`], the way [`TerminatedBlockInvoke`] is of
/// [`TerminatedBlockTypedInvoke`].
pub type TerminatedBlockSwitch<'ctx, R, B> = (
    BasicBlock<'ctx, R, Terminated, B>,
    SwitchInst<'ctx, Open, B>,
);

/// Pair returned by the TYPED [`IrBuilder::switch`] before the
/// case list is closed: the terminated parent block plus an [`Open`],
/// width-`W` [`SwitchInst`]. Every case added through the returned handle
/// must share the condition's width `W` — a wrong-width case is a compile
/// error. Typed sibling of [`TerminatedBlockSwitch`].
pub type TerminatedBlockSwitchTyped<'ctx, R, W, B> = (
    BasicBlock<'ctx, R, Terminated, B>,
    SwitchInst<'ctx, Open, B, W>,
);

/// Pair returned by the width-erased block-argument switch builder
/// [`IrBuilder::switch_dyn_with_args`]. The case list arrives complete —
/// every case is spelled at the call, with its own block arguments — so the
/// switch comes back already [`Closed`]: there is no `add_case` on it, and
/// therefore no way to bolt on a later case whose target's block parameters
/// nothing seeds. Erased sibling of [`TerminatedBlockSwitchTypedClosed`].
pub type TerminatedBlockSwitchClosed<'ctx, R, B> = (
    BasicBlock<'ctx, R, Terminated, B>,
    SwitchInst<'ctx, Closed, B>,
);

/// Pair returned by the TYPED block-argument switch builder
/// [`IrBuilder::switch_with_args`]: the terminated parent block plus a
/// [`Closed`], width-`W` [`SwitchInst`]. Typed sibling of
/// [`TerminatedBlockSwitchClosed`]; see it for why the case list is closed.
pub type TerminatedBlockSwitchTypedClosed<'ctx, R, W, B> = (
    BasicBlock<'ctx, R, Terminated, B>,
    SwitchInst<'ctx, Closed, B, W>,
);

/// One `switch` case edge after the block-argument switch builders have
/// lowered it: the erased case value, its resolved target label, and the block
/// arguments that edge carries into the target's parameters. Crate-internal —
/// the shared seeding tail's working shape, named so the signature stays
/// readable.
type LoweredSwitchCase<'ctx, 'args, R, B> = (
    Value<'ctx, B>,
    BasicBlockLabel<'ctx, R, B>,
    &'args [Value<'ctx, B>],
);

/// Pair returned by `indirectbr` builders before destination insertion closes.
pub type TerminatedBlockIndirectBr<'ctx, R, B> = (
    BasicBlock<'ctx, R, Terminated, B>,
    IndirectBrInst<'ctx, Open, B>,
);

/// Pair returned by `invoke` builders.
pub type TerminatedBlockInvoke<'ctx, R, Ret, B> =
    (BasicBlock<'ctx, R, Terminated, B>, InvokeInst<'ctx, Ret, B>);

/// Pair returned by the TYPED `invoke` builders
/// ([`IrBuilder::invoke`] / [`IrBuilder::invoke_with_config`]).
/// `R` is the parent function's return marker (drives the terminated
/// block's typestate); `Ret` is the invoke instruction's own schema —
/// the inner [`InvokeInst`] is tagged with `Ret::Marker`, derived from
/// the callee, matching [`TerminatedBlockInvoke`]'s shape one level up.
pub type TerminatedBlockTypedInvoke<'ctx, R, Ret, B> = (
    BasicBlock<'ctx, R, Terminated, B>,
    InvokeInst<'ctx, <Ret as FunctionReturn>::Marker, B>,
);

/// Pair returned by `catchswitch` builders before handler insertion closes.
pub type TerminatedBlockCatchSwitch<'ctx, R, B> = (
    BasicBlock<'ctx, R, Terminated, B>,
    CatchSwitchInst<'ctx, Open, B>,
);

/// Pair returned by `ret void` when the builder's return marker is statically
/// void.
pub type VoidReturnInst<'ctx, B> = TerminatedBlockInst<'ctx, (), B>;

/// Type-state marker: the builder has no insertion point. None of the
/// emitter methods are reachable in this state.
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

/// Sealed marker trait for the [`IrBuilder`] positioning typestate.
/// The two implementors are [`Unpositioned`] and [`Positioned`];
/// external crates cannot invent new states. Public so a caller writing
/// its own state-generic wrapper over an `IrBuilder` field can name the
/// bound. (The in-crate Braun-SSA [`SsaBuilder`](crate::SsaBuilder) used
/// to be such a caller; since 0.0.4 cycle D it is one type whose
/// cursor is data, and always stores a `Positioned` inner builder.)
pub trait BuilderPositionState: state_sealed::Sealed + 'static {}

impl BuilderPositionState for Unpositioned {}
impl BuilderPositionState for Positioned {}

/// Snapshot of an [`IrBuilder`] insertion location. Mirrors
/// `IRBuilderBase::InsertPoint` in `IRBuilder.h`. The `block` is `None`
/// when the builder was unpositioned at save time; `before` is `None`
/// when the saved location was end-of-block.
#[derive(Branded)]
#[branded(Debug)]
pub struct InsertPoint<'ctx, R: ReturnMarker, B: ModuleBrand> {
    pub(super) block_id: Option<ValueSlot>,
    pub(super) before: Option<ValueSlot>,
    /// Variance matches every other handle in the crate (see [`FunctionValue`]):
    /// covariant in `'ctx` and `R`, **invariant** in the brand `B` (next field).
    /// The snapshot stores arena slots only, so shortening the `'ctx` tag is
    /// always sound — and a pass that stashes an insert point across a
    /// higher-ranked `FunctionPass::run` needs exactly that covariance.
    /// [`IrBuilder::save_insert_point`] therefore mints the tag at `'static`:
    /// the snapshot is id-shaped, not a borrow of the module token.
    pub(super) _marker: PhantomData<(&'ctx (), R)>,
    pub(super) _brand: Invariant<B>,
}

#[derive(Debug, Clone)]
pub struct CallSiteConfig {
    name: String,
    calling_conv: CallingConv,
    attrs: CallAttributeData,
    call_site_fn_ty: Option<TypeSlot>,
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

    #[must_use]
    pub fn calling_conv(mut self, calling_conv: CallingConv) -> Self {
        self.calling_conv = calling_conv;
        self
    }

    #[must_use]
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
    pub fn call_site_type<'ctx, B: ModuleBrand + 'ctx>(
        mut self,
        fn_ty: FunctionType<'ctx, B>,
    ) -> Self {
        self.call_site_fn_ty = Some(fn_ty.as_type().id());
        self
    }

    pub(super) fn call_site_fn_ty(&self) -> Option<TypeSlot> {
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
pub struct IrBuilder<'m, 'ctx, B, F, S, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    module: &'ctx ModuleCore,
    _module: PhantomData<&'m Module<B, Unverified>>,
    insert_block: Option<BasicBlock<'ctx, R, Unterminated, B>>,
    /// Optional insertion anchor: when `Some(id)`, new instructions are
    /// inserted *before* the instruction with this id (mirrors upstream
    /// `IRBuilder::SetInsertPoint(Instruction*)`). When `None`, new
    /// instructions append to the end of `insert_block`.
    insert_before: Option<ValueSlot>,
    folder: F,
    fmf: super::fmf::FastMathFlags,
    _state: PhantomData<S>,
}

/// Hand-written rather than derived: a `derive` would bound the folder
/// parameter `F: Debug`, and a folder is a strategy type with no obligation
/// to be printable. The folder is reported by type name instead.
impl<'m, 'ctx, B, F, S, R> core::fmt::Debug for IrBuilder<'m, 'ctx, B, F, S, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IrBuilder")
            .field("module", &self.module.id())
            .field("insert_block", &self.insert_block)
            .field("insert_before", &self.insert_before)
            .field("fast_math_flags", &self.fmf)
            .field("folder", &core::any::type_name::<F>())
            .finish()
    }
}

// --------------------------------------------------------------------------
// Constructors
// --------------------------------------------------------------------------

impl<'m, 'ctx, B> IrBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, Dyn>
where
    B: ModuleBrand + 'ctx,
{
    /// Construct an unpositioned builder using the default
    /// [`ConstantFolder`]. The runtime-checked [`Dyn`] return marker
    /// matches the runtime-equality `ret` path; use
    /// [`IrBuilder::new_for`] when the caller already knows the return
    /// shape statically.
    pub fn new(module: &'ctx Module<B, Unverified>) -> Self {
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
    /// shape; the resulting builder's `ret` is statically
    /// typed.
    ///
    /// ```ignore
    /// let b = IrBuilder::new_for::<i32>(&module);
    /// ```
    pub fn new_for<R>(
        module: &'ctx Module<B, Unverified>,
    ) -> IrBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, R>
    where
        R: ReturnMarker,
    {
        IrBuilder {
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
        module: &'ctx Module<B, Unverified>,
    ) -> IrBuilder<'m, 'ctx, B, ConstantFolder, Unpositioned, <Sig::Ret as FunctionReturn>::Marker>
    where
        Sig: FunctionSignature,
    {
        IrBuilder {
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

impl<'m, 'ctx, B, R> IrBuilder<'m, 'ctx, B, ConstantFolder, Positioned, R>
where
    B: ModuleBrand + 'ctx,
    R: ReturnMarker,
{
    /// Create a builder already positioned at the end of `bb`, inferring the
    /// return marker `R` from the block — the block already pins it, so no
    /// turbofish is needed. Equivalent to
    /// `new_for::<R>(module).position_at_end(bb)` but with `R` derived and the
    /// module taken from `bb` (which carries a `ModuleRef`). Installs the
    /// default [`ConstantFolder`], matching [`new_for`](Self::new_for).
    /// [`new_for`](Self::new_for) remains for the unpositioned case (build a
    /// block first).
    ///
    /// Accepts a block carrying any parameter schema `Params` — mirroring
    /// [`position_at_end`](Self::position_at_end), which erases the parameter
    /// marker at the insertion point.
    pub fn at_end<Params>(
        bb: BasicBlock<'ctx, R, Unterminated, B, Params>,
    ) -> IrBuilder<'m, 'ctx, B, ConstantFolder, Positioned, R>
    where
        Params: BlockParams,
    {
        let module = bb.module_ref().module();
        IrBuilder {
            module,
            _module: PhantomData,
            insert_block: Some(bb.retag_params::<BlockParamsDyn>()),
            insert_before: None,
            folder: ConstantFolder,
            fmf: super::fmf::FastMathFlags::empty(),
            _state: PhantomData,
        }
    }
}

impl<'m, 'ctx, B, F, R> IrBuilder<'m, 'ctx, B, F, Unpositioned, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Construct an unpositioned builder using a caller-supplied
    /// folder.
    pub fn with_folder(module: &'ctx Module<B, Unverified>, folder: F) -> Self {
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
    ///
    /// Accepts a block carrying any parameter schema `Params` — including a
    /// typed block from
    /// [`append_block_typed`](Self::append_block_typed) — so a typed
    /// [`BlockCall`] target can be given a body. The parameter marker is
    /// irrelevant to insertion (it constrains only branch edges), so it is
    /// erased at the insertion point.
    pub fn position_at_end<Params>(
        self,
        bb: BasicBlock<'ctx, R, Unterminated, B, Params>,
    ) -> IrBuilder<'m, 'ctx, B, F, Positioned, R>
    where
        Params: BlockParams,
    {
        IrBuilder {
            module: self.module,
            _module: PhantomData,
            insert_block: Some(bb.retag_params::<BlockParamsDyn>()),
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

impl<'m, 'ctx, B, F, S, R> IrBuilder<'m, 'ctx, B, F, S, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    S: BuilderPositionState,
    R: ReturnMarker,
{
    /// Resolve a storable value id minted in this builder's module back into
    /// its borrowing handle — the builder-side twin of
    /// [`Module::view`](crate::Module::view).
    ///
    /// Builder methods hand back ids (the storable currency); a handle is the
    /// ephemeral *view* you take when you need to read from a value
    /// (`b.view(sum).ty()`). At a build site the owning [`Module`] token is
    /// frequently not in scope while the builder always is, so this is the
    /// canonical read path inside a function body.
    ///
    /// The module-tag check happens exactly as in
    /// [`Module::view`](crate::Module::view): the id's tag is compared against
    /// this builder's module *before* the arena is touched, so an id minted in
    /// a different module can never mis-resolve against an in-range slot here.
    ///
    /// # Panics
    ///
    /// Panics if the id belongs to a different module (foreign tag) or its slot
    /// is absent. Use [`try_view`](Self::try_view) for the fallible form.
    #[inline]
    pub fn view<I>(&self, id: I) -> I::View
    where
        I: ViewIn<'ctx, B>,
    {
        id.resolve_in(ModuleRef::new(self.module))
            .unwrap_or_else(|| {
                panic!(
                    "IrBuilder::view: id does not resolve in this module \
                 (foreign module tag or absent/tombstoned slot)"
                )
            })
    }

    /// Fallible [`view`](Self::view): resolve a storable value id into its
    /// borrowing handle, returning [`None`] when the id belongs to a different
    /// module (foreign tag) or its slot is absent.
    ///
    /// Like [`view`](Self::view), this validates the module tag and arena range
    /// only; a tombstoned-but-in-range slot is not detected (no cheap liveness
    /// flag exists).
    #[inline]
    pub fn try_view<I>(&self, id: I) -> Option<I::View>
    where
        I: ViewIn<'ctx, B>,
    {
        id.resolve_in(ModuleRef::new(self.module))
    }

    /// The builder's [`ModuleView`], which is how it reaches the
    /// user-implementable schema traits ([`IrField::ir_type`],
    /// [`StructSchema::ir_type`], [`FunctionReturn::ir_type`], …).
    ///
    /// Those traits used to be declared against `&Module<Unverified>`, and the
    /// builder — which stores only `&'ctx ModuleCore`, because
    /// [`IrBuilder::at_end`] constructs a builder from a *block* alone with no
    /// module token in scope — had to fabricate an ephemeral borrowed `Module`
    /// to call them, then re-anchor every answer from that local region back to
    /// `'ctx` through its id. Declaring the schema traits against the view
    /// deletes both the fabricated token and the re-anchoring: a view is
    /// already `'ctx`-anchored, so the type it hands back is directly usable.
    #[inline]
    fn schema_view(&self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.module)
    }

    /// [`IrField::ir_type`] for `T`.
    #[inline]
    fn schema_ir_type<T>(&self) -> IrResult<Type<'ctx, B>>
    where
        T: IrField,
    {
        T::ir_type(self.schema_view())
    }

    /// [`StructSchema::ir_type`] for `S`.
    #[inline]
    fn schema_struct_type<Sch>(&self) -> IrResult<StructType<'ctx, StructBodyDyn, B>>
    where
        Sch: StructSchema,
    {
        Ok(Sch::ir_type(self.schema_view())?.as_dyn())
    }

    /// Position the builder at the end of the block named by a storable
    /// [`BlockId`] — the **checked** escape hatch for dynamic or recovered
    /// control-flow graphs, where the linear
    /// [`BasicBlock`] token was consumed long ago (a pass walking
    /// `function.basic_blocks()`, a parser resuming a forward-referenced label,
    /// an analysis re-entering a block it recorded).
    ///
    /// [`position_at_end`](Self::position_at_end) stays the strict form: it
    /// *consumes* an [`Unterminated`] block token, so appending into an already
    /// terminated block is not even representable. An id carries no termination
    /// marker, so this form checks at run time instead and reports:
    ///
    /// - [`IrError::ForeignValueId`] if `block` was minted in another module
    ///   (the tag is compared before the arena is touched) or its slot no longer
    ///   holds a basic block;
    /// - [`IrError::InvalidOperation`] if the block already has a terminator —
    ///   the same rule [`restore_insert_point`](Self::restore_insert_point)
    ///   enforces, so a `_dyn` reposition can never reopen a closed block.
    ///
    /// The `Params` marker is erased at the insertion point, exactly as in
    /// [`position_at_end`](Self::position_at_end): block parameters constrain
    /// branch edges, not insertion.
    pub fn position_at_end_dyn<Params>(
        self,
        block: BlockId<R, B, Params>,
    ) -> IrResult<IrBuilder<'m, 'ctx, B, F, Positioned, R>>
    where
        Params: BlockParams,
    {
        let module_ref = ModuleRef::<B>::new(self.module);
        let label = block
            .resolve_in(module_ref)
            .ok_or(IrError::ForeignValueId)?;
        let insert_block =
            BasicBlock::<R, Unterminated, B>::from_parts(label.slot(), module_ref, label.ty);
        if insert_block
            .terminator()
            .is_some_and(|inst| inst.is_terminator())
        {
            return Err(IrError::InvalidOperation {
                message: "cannot position at end of a terminated block",
            });
        }
        Ok(IrBuilder {
            module: self.module,
            _module: PhantomData,
            insert_block: Some(insert_block),
            insert_before: None,
            folder: self.folder,
            fmf: self.fmf,
            _state: PhantomData,
        })
    }

    /// Re-anchor the builder *before* the given attached instruction.
    /// New instructions land between the prior instruction and `anchor`.
    /// Mirrors `IRBuilder::SetInsertPoint(Instruction *I)` in `IRBuilder.h`,
    /// which sets `BB = I->getParent(); InsertPt = I->getIterator();`.
    pub fn position_before(
        self,
        anchor: &InstructionView<'ctx, B>,
    ) -> IrBuilder<'m, 'ctx, B, F, Positioned, R> {
        let anchor_id = anchor.slot();
        let parent_block_id = anchor.parent().slot();
        let label_ty = self.module.label_type::<B>().as_type().id();
        let bb = BasicBlock::<R, Unterminated, B>::from_parts(
            parent_block_id,
            ModuleRef::<B>::new(self.module),
            label_ty,
        );
        IrBuilder {
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
    ) -> IrBuilder<'m, 'ctx, B, F, Positioned, R> {
        let entry = f.entry_block().unwrap_or_else(|| {
            unreachable!("position_past_allocas requires a function with at least one block")
        });
        // Find the first non-alloca instruction id, mirroring
        // `BasicBlock::getFirstNonPHIOrDbgOrAlloca`. We don't ship phi/dbg
        // filters yet, so the practical filter here is alloca-only.
        let mut anchor: Option<ValueSlot> = None;
        for inst in entry.instructions() {
            match inst.kind() {
                Some(InstructionKind::Alloca(_)) => continue,
                _ => {
                    anchor = Some(inst.slot());
                    break;
                }
            }
        }
        IrBuilder {
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
    /// `IrBuilder::saveIP` (returns `InsertPoint(BB, InsertPt)`).
    ///
    /// The snapshot **borrows nothing** — it is a pair of arena slots plus the
    /// brand — so it is minted at `'static` and shrinks to whatever region the
    /// consumer names. That matters now that a [`Module`] owns its storage: a
    /// pass that stashes an insert point and is then handed to a driver which
    /// *moves* the module token (every typestate transition is a move) would
    /// otherwise be holding a borrow of a token that no longer exists. The
    /// brand `B` remains the cross-module guard, and
    /// [`restore_insert_point`](Self::restore_insert_point) still re-validates
    /// the block against the live module.
    pub fn save_insert_point(&self) -> InsertPoint<'static, R, B> {
        InsertPoint {
            block_id: self.insert_block.as_ref().map(|bb| bb.slot()),
            before: self.insert_before,
            _marker: PhantomData,
            _brand: PhantomData,
        }
    }

    /// Restore a previously-saved insertion point. Mirrors
    /// `IrBuilder::restoreIP(InsertPoint)`, but returns an error instead of
    /// reopening a block that has since grown a terminator.
    pub fn restore_insert_point(
        self,
        ip: InsertPoint<'ctx, R, B>,
    ) -> IrResult<IrBuilder<'m, 'ctx, B, F, Positioned, R>> {
        let Some(block_id) = ip.block_id else {
            return Err(IrError::InvalidOperation {
                message: "cannot restore an empty insert point",
            });
        };
        let label_ty = self.module.label_type::<B>().as_type().id();
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
        Ok(IrBuilder {
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
    /// Errors if `phi_val` does not refer to a phi instruction, if `val`'s
    /// type does not match the phi's result type
    /// ([`IrError::TypeMismatch`]), or if the phi already has an entry for
    /// `block` with a *different* value
    /// ([`IrError::AmbiguousPhiIncoming`]) — the same result-type and
    /// differing-duplicate rules the typed [`PhiInst::add_incoming`]
    /// enforces, applied here so the parser / ssa_builder callers reject at
    /// the call site instead of deferring to
    /// [`Module::verify`](crate::Module::verify). A same-block *same-value*
    /// duplicate stays legal (multi-edges from `switch`). `val` and `block`
    /// already carry the builder brand `B`; remaining predecessor-set
    /// coherence is verified by [`Module::verify`](crate::Module::verify).
    ///
    /// Every parameter speaks the storable-id currency (0.0.4): `phi_val`
    /// and `val` take anything that lifts to an erased value — including the
    /// phi ids the phi builders hand back and any value id — and `block` takes
    /// any [`IntoBasicBlockLabel`], so a [`BlockId`] recovered from a
    /// predecessor walk goes straight in without a view.
    ///
    /// [`PhiInst::add_incoming`]: crate::PhiInst::add_incoming
    /// Internal contract shared with the in-tree `.ll` parser and the SSA
    /// builder (hence `#[doc(hidden)]`); block arguments are the public
    /// phi-authoring surface, so this is not part of the supported API and may
    /// change without notice.
    #[doc(hidden)]
    pub fn phi_add_incoming_from_value<RBb, Phi, Val, Block>(
        &self,
        phi_val: Phi,
        val: Val,
        block: Block,
    ) -> IrResult<()>
    where
        RBb: ReturnMarker,
        Phi: IntoErasedValue<'ctx, B>,
        Val: IntoErasedValue<'ctx, B>,
        Block: IntoBasicBlockLabel<'ctx, RBb, B>,
    {
        let phi_val = phi_val.into_erased_value(ModuleRef::new(self.module))?;
        let val = val.into_erased_value(ModuleRef::new(self.module))?;
        let block = block.into_basic_block_label(ModuleRef::new(self.module))?;
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
        // Result-type check: the same rule the typed `PhiInst::add_incoming`
        // enforces, applied here so the parser / ssa_builder paths reject a
        // mismatched incoming at the call site rather than deferring to
        // `Module::verify`.
        let phi_ty = self.module.context().value_data(phi_val.id).ty;
        if val.ty != phi_ty {
            return Err(IrError::TypeMismatch {
                expected: Type::new(phi_ty, ModuleRef::<B>::new(self.module)).kind_label(),
                got: Type::new(val.ty, ModuleRef::<B>::new(self.module)).kind_label(),
            });
        }
        // Differing-duplicate check: a second entry for the same predecessor
        // block with a different value is meaningless in any CFG (the
        // InstCombine #196954 bug class). A same-block same-value duplicate
        // stays legal (multi-edges from `switch`).
        let block_id = block.slot();
        if phi_payload
            .incoming
            .borrow()
            .iter()
            .any(|(v, b)| *b == block_id && v.get() != val.id)
        {
            return Err(IrError::AmbiguousPhiIncoming {
                block: self.module.context().block_diag_name(block_id),
            });
        }
        phi_payload
            .incoming
            .borrow_mut()
            .push((core::cell::Cell::new(val.id), block_id));
        // Register phi as a user of the incoming value.
        self.module
            .context()
            .value_data(val.id)
            .add_use(ValueUse::Instruction(phi_val.id));
        Ok(())
    }

    /// Build a fresh, operandless phi at the phi head of the block with id
    /// `block_id` and return its result [`Value`]. Independent of the
    /// builder's insertion cursor: the phi always lands at `block_id`'s phi
    /// head (via [`BasicBlock::insert_instruction_at_phi_head`]) regardless
    /// of where -- or whether -- the builder is positioned, so callers can
    /// seed head-phis in a block they are not currently editing. The phi
    /// starts with zero incoming edges.
    ///
    /// Low-level counterpart of [`Self::append_phi_instruction`], which
    /// targets the *insertion* block; this one targets an arbitrary block by
    /// id and hands back the erased [`Value`] rather than a typed phi handle.
    pub(crate) fn make_phi_in_block(
        &self,
        block_id: ValueSlot,
        ty: TypeSlot,
        name: &str,
    ) -> Value<'ctx, B> {
        let payload = PhiData::new();
        let value = build_instruction_value(ty, block_id, InstructionKindData::Phi(payload), None);
        // Snapshot operand ids before the value moves into the arena so the
        // new phi can be registered in each operand's reverse use-list --
        // identical to `append_phi_instruction`. A fresh phi has no operands,
        // so this loop is a no-op until incomings are added later.
        let operand_ids = match &value.kind {
            ValueKindData::Instruction(i) => i.kind.operand_ids(),
            _ => unreachable!("make_phi_in_block built non-instruction value"),
        };
        let id = self.module.context().push_value(value);
        for op in operand_ids {
            self.module
                .context()
                .value_data(op)
                .add_use(ValueUse::Instruction(id));
        }
        let label_ty = self.module.label_type::<B>().as_type().id();
        let bb = BasicBlock::<Dyn, Unterminated, B>::from_parts(
            block_id,
            ModuleRef::<B>::new(self.module),
            label_ty,
        );
        bb.insert_instruction_at_phi_head(id);
        if !name.is_empty()
            && !Type::new(ty, ModuleRef::<B>::new(self.module)).is_void()
            && let Some(parent_fn_id) = bb.parent_id()
        {
            let parent_fn = FunctionValue::<Dyn, B>::from_parts_unchecked(
                parent_fn_id,
                ModuleRef::<B>::new(self.module),
            );
            parent_fn.set_local_value_name(id, Some(name));
        }
        Value::from_parts(id, ModuleRef::<B>::new(self.module), ty)
    }

    /// Append a fresh basic block to `function` whose parameters are
    /// operandless head-phis, in the style of Swift SIL / MLIR block
    /// arguments. One phi is materialised at the new block's head per entry
    /// in `param_types`, in order; the returned `Vec<Value>` holds those phi
    /// results, so `params[i]` is the `Value` for `param_types[i]` and
    /// carries that type.
    ///
    /// Each parameter is a [`Value`] backed by a phi with zero incoming
    /// edges. Supply the incomings later by branching to this block with the
    /// block-argument branch builders, which carry one argument value per
    /// parameter into the matching head-phi.
    ///
    /// The block is returned still [`Unterminated`] and is *not* made the
    /// builder's insertion point, so the caller positions the builder at it
    /// (or elsewhere) and fills its body. Creation is independent of the
    /// builder's current cursor -- the builder need not be positioned.
    ///
    /// Parameter types are passed as a `&[Type<'ctx, B>]` slice; each
    /// parameter phi takes its type directly from the corresponding `Type`.
    pub fn append_block_with_params<Name>(
        &self,
        function: FunctionValue<'ctx, R, B>,
        param_types: &[Type<'ctx, B>],
        name: Name,
    ) -> IrResult<BlockWithParams<'ctx, R, B>>
    where
        Name: Into<String>,
    {
        let bb = function.append_basic_block_unchecked(name);
        let bb_id = bb.slot();
        let mut params = Vec::with_capacity(param_types.len());
        for ty in param_types {
            params.push(self.make_phi_in_block(bb_id, ty.id(), ""));
        }
        bb.set_parameter_count(param_types.len());
        Ok((bb, params))
    }

    /// Like [`Self::append_block_with_params`], but names each parameter phi.
    ///
    /// Each entry is a `(type, name)` pair; the head-phi for that parameter is
    /// emitted with the given name, so the printed IR reads `%name = phi ...`
    /// instead of an anonymous numbered slot. An empty name falls back to the
    /// anonymous form for that parameter. This is the block-argument
    /// counterpart of naming a raw phi — it lets block-argument authoring
    /// reproduce named-phi output byte-for-byte (e.g. the hand-written and
    /// auto-SSA factorial examples print identical IR).
    ///
    /// Ordering and return shape match [`Self::append_block_with_params`]:
    /// `params[i]` is the `Value` for `params[i].0`, in order, and the block is
    /// returned still [`Unterminated`] without becoming the builder's insertion
    /// point.
    pub fn append_block_with_named_params<Params, ParamName, Name>(
        &self,
        function: FunctionValue<'ctx, R, B>,
        params: Params,
        name: Name,
    ) -> IrResult<BlockWithParams<'ctx, R, B>>
    where
        Params: IntoIterator<Item = (Type<'ctx, B>, ParamName)>,
        ParamName: Into<String>,
        Name: Into<String>,
    {
        let bb = function.append_basic_block_unchecked(name);
        let bb_id = bb.slot();
        let mut out = Vec::new();
        for (ty, param_name) in params {
            let param_name = param_name.into();
            out.push(self.make_phi_in_block(bb_id, ty.id(), &param_name));
        }
        bb.set_parameter_count(out.len());
        Ok((bb, out))
    }

    /// Typed sibling of [`Self::append_block_with_params`]: append a fresh
    /// block to `function` whose parameter shape is fixed at the type level by
    /// the schema tuple `Params`, and hand back that block stamped with
    /// `Params` plus a *typed* tuple of its parameter handles.
    ///
    /// One operandless head-phi is materialised per entry in
    /// [`Params::ir_types`](FunctionParamList::ir_types), in declaration
    /// order — the same head-phi path the erased sibling
    /// [`Self::append_block_with_params`] uses — so `Params::Values` position
    /// `i` is the branded handle for the
    /// head-phi of parameter `i` and carries that parameter's IR type. Supply
    /// the incomings later by branching to the block; the returned block is
    /// still [`Unterminated`] and is *not* made the builder's insertion point,
    /// exactly like the erased sibling.
    ///
    /// The reused `Params` schema is the one that also drives typed function
    /// signatures ([`FunctionParamList`]); a block marked `(i32, Ptr)` yields
    /// `(IntValue<'_, i32, B>, PointerValue<'_, B>)`. The parameter types are
    /// built before the block is appended, so a construction failure leaves no
    /// half-built block behind.
    ///
    /// Typed parameter tuples are capped at arity 12: a `Params` tuple with
    /// more than twelve entries does not implement [`BlockParams`] (the standard
    /// library stops deriving `Debug` past arity 12) and so is rejected with a
    /// `BlockParams`-unsatisfied trait-bound error. A block with more than twelve
    /// parameters must use the erased
    /// [`BlockParamsDyn`] form via
    /// [`append_block_with_params`](Self::append_block_with_params).
    pub fn append_block_typed<Params, Name>(
        &self,
        function: FunctionValue<'ctx, R, B>,
        name: Name,
    ) -> IrResult<TypedBlockWithParams<'ctx, R, Params, B>>
    where
        Params: FunctionParamList + BlockParams,
        Name: Into<String>,
    {
        // Build the parameter IR types first, so a failure here appends no
        // block (the erased sibling receives its `&[Type]` pre-built and so
        // cannot fail at this step).
        let param_types = Params::ir_types(self.schema_view())?;
        let bb = function.append_basic_block_unchecked(name);
        let bb_id = bb.slot();
        let mut phi_values = Vec::with_capacity(param_types.len());
        for ty in &param_types {
            phi_values.push(self.make_phi_in_block(bb_id, ty.id(), ""));
        }
        bb.set_parameter_count(param_types.len());
        // One head-phi per `ir_types` entry was built in order, so the
        // per-position `values_from_phi_values` wraps cannot mistype; the
        // capability token is minted here exactly as `TypedFunctionValue`
        // does before reading a function's typed parameters.
        let validated = ValidatedFunctionParams::new();
        let values = Params::values_from_phi_values(&phi_values, &validated);
        Ok((bb.retag_params::<Params>(), values))
    }
}

impl<'m, 'ctx, B, F, R> IrBuilder<'m, 'ctx, B, F, Positioned, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Re-position the builder at the end of `bb`. Accepts any parameter
    /// schema `Params` (including a typed block from
    /// [`append_block_typed`](Self::append_block_typed)); the marker is erased
    /// at the insertion point, which does not constrain block parameters.
    pub fn position_at_end<Params>(self, bb: BasicBlock<'ctx, R, Unterminated, B, Params>) -> Self
    where
        Params: BlockParams,
    {
        Self {
            module: self.module,
            _module: PhantomData,
            insert_block: Some(bb.retag_params::<BlockParamsDyn>()),
            insert_before: None,
            folder: self.folder,
            fmf: self.fmf,
            _state: PhantomData,
        }
    }

    /// Drop the insertion point. Mirrors
    /// `IRBuilder::ClearInsertionPoint`.
    pub fn unposition(self) -> IrBuilder<'m, 'ctx, B, F, Unpositioned, R> {
        IrBuilder {
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
    #[must_use]
    pub fn with_fast_math_flags(self, fmf: FastMathFlags) -> Self {
        Self { fmf, ..self }
    }

    /// Reset the builder's default FMF to empty. Mirrors
    /// `IRBuilderBase::clearFastMathFlags()`.
    #[must_use]
    pub fn clear_fast_math_flags(self) -> Self {
        Self {
            fmf: super::fmf::FastMathFlags::empty(),
            ..self
        }
    }

    // ---- Integer arithmetic ----
    //
    // Every builder in this family hands back a *storable id*
    // (`IntValueId<W, B>`), not a borrowing handle. Ids are the currency you
    // keep; a handle is the ephemeral view you take to read from a value.
    // Chaining costs nothing -- an id satisfies `IntoIntValue` and
    // `IntoCallArg`, so it drops straight into the next builder's operand
    // slot -- while a *read* (`.ty()`, `.as_erased()`, `{}`) goes through
    // `IrBuilder::view` / `Module::view`.

    /// Produce `add lhs, rhs`. Mirrors `IRBuilder::CreateAdd`.
    ///
    /// Operands share width `W` -- enforced at compile time by the
    /// type system. Either side accepts any [`crate::IntoIntValue<'ctx, W, B>`]:
    /// already-typed [`IntValue`]s, [`crate::ConstantIntValue`]s, and
    /// Rust scalar literals (`5_i32`, `true`, ...) all work.
    ///
    /// Returns the storable [`IntValueId<W, B>`](crate::IntValueId) naming the
    /// result -- like every builder in this family. Pass it directly into the
    /// next builder call; take [`view`](Self::view) when you need to *read*
    /// from it:
    ///
    /// ```ignore
    /// let sum = b.int_add::<i32, _, _, _>(x, 1_i32, "sum")?;
    /// let wide = b.int_mul::<i32, _, _, _>(sum, 2_i32, "wide")?; // operand: no view
    /// let ty = b.view(sum).ty();                                        // read: view
    /// ```
    pub fn int_add<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_int_bin_op(BinaryOpcode::Add, lhs, rhs)? {
            return self.accept_folded_int(folded, lhs).map(|v| v.id());
        }
        let payload = BinaryOpData::new(lhs.slot(), rhs.slot());
        Ok(self
            .append_int_like(lhs, InstructionKindData::Add(payload), name)
            .id())
    }

    /// Produce `sub lhs, rhs`. Mirrors `IRBuilder::CreateSub`.
    pub fn int_sub<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_int_bin_op(BinaryOpcode::Sub, lhs, rhs)? {
            return self.accept_folded_int(folded, lhs).map(|v| v.id());
        }
        let payload = BinaryOpData::new(lhs.slot(), rhs.slot());
        Ok(self
            .append_int_like(lhs, InstructionKindData::Sub(payload), name)
            .id())
    }

    /// Produce `mul lhs, rhs`. Mirrors `IRBuilder::CreateMul`.
    pub fn int_mul<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Mul,
            lhs,
            rhs,
            name,
            MulFlags::new(),
            InstructionKindData::Mul,
        )
        .map(|v| v.id())
    }

    /// Produce `udiv lhs, rhs`. Mirrors `IRBuilder::CreateUDiv`.
    pub fn int_udiv<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(
            BinaryOpcode::Udiv,
            lhs,
            rhs,
            name,
            InstructionKindData::Udiv,
        )
        .map(|v| v.id())
    }

    /// Produce `sdiv lhs, rhs`. Mirrors `IRBuilder::CreateSDiv`.
    pub fn int_sdiv<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(
            BinaryOpcode::Sdiv,
            lhs,
            rhs,
            name,
            InstructionKindData::Sdiv,
        )
        .map(|v| v.id())
    }

    /// Produce `urem lhs, rhs`. Mirrors `IRBuilder::CreateURem`.
    pub fn int_urem<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(
            BinaryOpcode::Urem,
            lhs,
            rhs,
            name,
            InstructionKindData::Urem,
        )
        .map(|v| v.id())
    }

    /// Produce `srem lhs, rhs`. Mirrors `IRBuilder::CreateSRem`.
    pub fn int_srem<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(
            BinaryOpcode::Srem,
            lhs,
            rhs,
            name,
            InstructionKindData::Srem,
        )
        .map(|v| v.id())
    }

    /// Produce `shl lhs, rhs`. Mirrors `IRBuilder::CreateShl`.
    pub fn int_shl<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Shl,
            lhs,
            rhs,
            name,
            ShlFlags::new(),
            InstructionKindData::Shl,
        )
        .map(|v| v.id())
    }

    /// Produce `lshr lhs, rhs`. Mirrors `IRBuilder::CreateLShr`.
    pub fn int_lshr<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(
            BinaryOpcode::Lshr,
            lhs,
            rhs,
            name,
            InstructionKindData::Lshr,
        )
        .map(|v| v.id())
    }

    /// Produce `ashr lhs, rhs`. Mirrors `IRBuilder::CreateAShr`.
    pub fn int_ashr<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(
            BinaryOpcode::Ashr,
            lhs,
            rhs,
            name,
            InstructionKindData::Ashr,
        )
        .map(|v| v.id())
    }

    /// Produce `and lhs, rhs`. Mirrors `IRBuilder::CreateAnd`.
    pub fn int_and<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(BinaryOpcode::And, lhs, rhs, name, InstructionKindData::And)
            .map(|v| v.id())
    }

    /// Produce `or lhs, rhs`. Mirrors `IRBuilder::CreateOr`.
    pub fn int_or<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(BinaryOpcode::Or, lhs, rhs, name, InstructionKindData::Or)
            .map(|v| v.id())
    }

    /// Produce `or disjoint lhs, rhs` with explicit [`crate::OrFlags`].
    /// The `disjoint` flag asserts the operands have no bits in common.
    /// Mirrors `IRBuilder::CreateOr` with `IsDisjoint` set.
    pub fn int_or_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: OrFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Or,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Or,
        )
        .map(|v| v.id())
    }

    /// Produce `xor lhs, rhs`. Mirrors `IRBuilder::CreateXor`.
    pub fn int_xor<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop(BinaryOpcode::Xor, lhs, rhs, name, InstructionKindData::Xor)
            .map(|v| v.id())
    }

    /// Produce `add lhs, rhs` with explicit [`crate::AddFlags`]. Mirrors
    /// `IRBuilder::CreateAdd` plus the `nuw`/`nsw` knobs. The flag
    /// set type only exposes flags LLVM accepts on `add`, so
    /// invalid combinations are a compile error.
    pub fn int_add_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: AddFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Add,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Add,
        )
        .map(|v| v.id())
    }

    /// Produce `sub lhs, rhs` with explicit [`crate::SubFlags`].
    pub fn int_sub_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: SubFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Sub,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Sub,
        )
        .map(|v| v.id())
    }

    /// Produce `mul lhs, rhs` with explicit [`crate::MulFlags`].
    pub fn int_mul_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: MulFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Mul,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Mul,
        )
        .map(|v| v.id())
    }

    /// Produce `shl lhs, rhs` with explicit [`crate::ShlFlags`].
    pub fn int_shl_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: ShlFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Shl,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Shl,
        )
        .map(|v| v.id())
    }

    /// Produce `udiv lhs, rhs` with explicit [`crate::UdivFlags`].
    pub fn int_udiv_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: UdivFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Udiv,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Udiv,
        )
        .map(|v| v.id())
    }

    /// Produce `sdiv lhs, rhs` with explicit [`crate::SdivFlags`].
    pub fn int_sdiv_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: SdivFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Sdiv,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Sdiv,
        )
        .map(|v| v.id())
    }

    /// Produce `lshr lhs, rhs` with explicit [`crate::LshrFlags`].
    pub fn int_lshr_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: LshrFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Lshr,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Lshr,
        )
        .map(|v| v.id())
    }

    /// Produce `ashr lhs, rhs` with explicit [`crate::AshrFlags`].
    pub fn int_ashr_with_flags<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        flags: AshrFlags,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_binop_flagged(
            BinaryOpcode::Ashr,
            lhs,
            rhs,
            name,
            flags,
            InstructionKindData::Ashr,
        )
        .map(|v| v.id())
    }

    /// Integer negation: `sub 0, V`. Mirrors `IRBuilder::CreateNeg(V, Name)`,
    /// which expands to `CreateSub(Constant::getNullValue(V->getType()), V, Name)`.
    pub fn int_neg<W, V, Name>(&self, value: V, name: Name) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        V: IntoIntValue<'ctx, W, B>,
    {
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        let zero = W::ir_type(ModuleRef::<B>::new(self.module)).const_zero();
        self.int_sub(zero, v, name)
    }

    /// Integer NSW negation. Mirrors `IRBuilder::CreateNSWNeg(V, Name)` ->
    /// `CreateNeg(V, Name, /*HasNSW=*/true)` -> `CreateSub` with `nsw`.
    pub fn int_neg_nsw<W, V, Name>(&self, value: V, name: Name) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        V: IntoIntValue<'ctx, W, B>,
    {
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        let zero = W::ir_type(ModuleRef::<B>::new(self.module)).const_zero();
        self.int_sub_with_flags(zero, v, super::instr_types::SubFlags::new().nsw(), name)
    }

    /// Bitwise complement: `xor V, -1`. Mirrors `IRBuilder::CreateNot(V, Name)`,
    /// which expands to `CreateXor(V, Constant::getAllOnesValue(V->getType()))`.
    pub fn int_not<W, V, Name>(&self, value: V, name: Name) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        V: IntoIntValue<'ctx, W, B>,
    {
        let v = value.into_int_value(ModuleRef::new(self.module))?;
        let all_ones = W::ir_type(ModuleRef::<B>::new(self.module)).const_all_ones();
        self.int_xor(v, all_ones, name)
    }

    /// Crate-internal helper: emit a flagged binary op. The flag
    /// type's `WriteBinopFlags` impl writes its bits onto the
    /// payload; the kind constructor lifts the payload into the
    /// matching `InstructionKindData` variant.
    fn int_binop_flagged<W, Lhs, Rhs, Flags, Kind, N>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        name: N,
        flags: Flags,
        kind_ctor: Kind,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
        Flags: WriteBinopFlags,
        Kind: FnOnce(BinaryOpData) -> InstructionKindData,
        N: AsRef<str>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        let mut payload = BinaryOpData::new(lhs.slot(), rhs.slot());
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
        Ok(self.append_int_like(lhs, kind_ctor(payload), name))
    }

    /// Crate-internal helper: emit a binary op given a callback that
    /// wraps the payload into an [`InstructionKindData`] variant.
    /// All integer binary opcodes route through the folder before materialising
    /// an instruction.
    fn int_binop<W, Lhs, Rhs, F2, N>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        name: N,
        kind_ctor: F2,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
        N: AsRef<str>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_int_bin_op(opcode, lhs, rhs)? {
            return self.accept_folded_int(folded, lhs);
        }
        let payload = BinaryOpData::new(lhs.slot(), rhs.slot());
        Ok(self.append_int_like(lhs, kind_ctor(payload), name))
    }

    // ---- Type-erased integer binops (scalar OR integer-vector operands) ----
    //
    // The typed `int_*` family routes both operands through
    // `IntoIntValue<W>`, whose `TryFrom<Value>` impls accept only scalar
    // `iN` types and reject integer *vectors* (`<N x iM>`). Element-wise
    // vector arithmetic (`xor <2 x i64> ...`) is legal IR the verifier
    // already accepts (`is_int_or_int_vector`), but there was no builder
    // path to emit it. These `_dyn` wrappers take erased [`Value`] operands
    // and skip the scalar-only `IntoIntValue` conversion, mirroring the
    // untyped cast builder [`bitcast_dyn`]. The result type is the
    // LHS operand's type; the caller is responsible for operand-type
    // agreement (the LLVM verifier rejects ill-formed binops).
    //
    // Because the result may be a *vector*, these return the erased
    // `ValueId<B>` rather than the typed `IntValueId<W, B>` the scalar family
    // mints -- the id analogue of the erased `Value` they used to return.
    // Their operands stay concrete `Value`s, so chaining one `_dyn` result
    // into the next takes a `view`.

    /// Crate-internal: emit an integer binop on erased [`Value`] operands
    /// (scalar `iN` or integer vector `<N x iM>`), the result taking the LHS
    /// operand's type. Skips the scalar-only `IntoIntValue` conversion the
    /// typed `int_*` family performs, so it accepts vector operands.
    fn int_binop_dyn<F2, N>(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        name: N,
        kind_ctor: F2,
    ) -> IrResult<Value<'ctx, B>>
    where
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
        N: AsRef<str>,
    {
        self.int_binop_dyn_with_flags(opcode, lhs, rhs, IntBinOpFlags::new(), name, kind_ctor)
    }

    /// As `int_binop_dyn`, with the flags the opcode accepts.
    ///
    /// Operand types are checked here rather than left to the verifier: a
    /// caller reaching the erased path has a runtime type in hand and no
    /// `IntoIntValue` conversion to bounce off, so without this an `and` on
    /// two floats would build silently and fail only at `verify()`.
    fn int_binop_dyn_with_flags<F2, N>(
        &self,
        opcode: BinaryOpcode,
        lhs: Value<'ctx, B>,
        rhs: Value<'ctx, B>,
        flags: IntBinOpFlags,
        name: N,
        kind_ctor: F2,
    ) -> IrResult<Value<'ctx, B>>
    where
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
        N: AsRef<str>,
    {
        if lhs.ty().id() != rhs.ty().id() {
            return Err(IrError::InvalidOperation {
                message: "integer binop operands must have the same type",
            });
        }
        if !self.is_int_or_int_vector(lhs.ty()) {
            return Err(IrError::InvalidOperation {
                message: "integer binop operand is neither an integer nor an integer vector",
            });
        }
        if let Some(folded) = self.folder.fold_bin_op_dyn(opcode, lhs, rhs)? {
            return self.checked_folded_value(folded, lhs.ty);
        }
        let mut payload = BinaryOpData::new(lhs.id, rhs.id);
        opcode.accepted_flags(flags).apply(&mut payload);
        let inst = self.append_instruction(lhs.ty().id(), kind_ctor(payload), name);
        Ok(inst.to_erased())
    }

    /// `true` for `iN` and for `<N x iM>` / `<vscale x N x iM>`.
    ///
    /// Mirrors the verifier's `is_int_or_int_vector`, which is the rule the
    /// built instruction is checked against.
    fn is_int_or_int_vector(&self, ty: Type<'ctx, B>) -> bool {
        let scalar = match ty.data() {
            TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
                self.module.context().type_data(*elem)
            }
            other => other,
        };
        matches!(scalar, TypeData::Integer { .. })
    }

    /// `<N x i1>` when `operand_ty` is a vector, `i1` when it is a scalar —
    /// the result type of a comparison over `operand_ty`.
    fn cmp_result_type(&self, operand_ty: Type<'ctx, B>) -> Type<'ctx, B> {
        let i1 = ModuleView::<B>::new(self.module).bool_type().as_type();
        let id = match operand_ty.data() {
            TypeData::FixedVector { n, .. } => self.module.context().fixed_vector_type(i1.id(), *n),
            TypeData::ScalableVector { min, .. } => {
                self.module.context().scalable_vector_type(i1.id(), *min)
            }
            _ => return i1,
        };
        Type::new(id, ModuleRef::<B>::new(self.module))
    }

    /// `opcode lhs, rhs` on erased operands (scalar `iN` or integer vector),
    /// with the flags `opcode` accepts.
    ///
    /// The entry point for callers holding a *runtime* opcode — the `.ll`
    /// parser above all. Callers with a statically-known opcode should prefer
    /// the typed `int_*` family, which pins the operand width in the
    /// type system, or the per-opcode `int_*_dyn` wrappers when the
    /// operands are vectors.
    ///
    /// Returns the erased [`ValueId`] rather than a typed id because the
    /// result may be a vector, which no `IntWidth` marker describes.
    pub fn int_binop_erased<Lhs, Rhs, Name>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        flags: IntBinOpFlags,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        let kind_ctor = int_binop_kind_ctor(opcode).ok_or(IrError::InvalidOperation {
            message: "opcode is not an integer binary operator",
        })?;
        self.int_binop_dyn_with_flags(opcode, lhs, rhs, flags, name, kind_ctor)
            .map(|v| v.id())
    }

    /// The number of bits in `ty`'s scalar type — `ty` itself when it is a
    /// scalar, its element when it is a vector.
    ///
    /// Mirrors `Type::getScalarSizeInBits`, which is what
    /// `CastInst::castIsValid` compares for the integer casts.
    fn integer_scalar_bit_width(&self, ty: Type<'ctx, B>) -> Option<u32> {
        let scalar = match ty.data() {
            TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
                self.module.context().type_data(*elem)
            }
            other => other,
        };
        match scalar {
            TypeData::Integer { bits } => Some(*bits),
            _ => None,
        }
    }

    /// The element count and scalability of `ty`, or `None` when it is a
    /// scalar. Two types agree in shape when this answers equal for both.
    fn vector_shape(&self, ty: Type<'ctx, B>) -> Option<(u32, bool)> {
        match ty.data() {
            TypeData::FixedVector { n, .. } => Some((*n, false)),
            TypeData::ScalableVector { min, .. } => Some((*min, true)),
            _ => None,
        }
    }

    /// `trunc` / `zext` / `sext` on an erased operand — a scalar `iN` or an
    /// integer vector — producing `dst_ty`.
    ///
    /// The erased counterpart of the `trunc_dyn` / `zext_dyn` /
    /// `sext_dyn` family, whose `_dyn` means *dynamic width* (`IntDyn`)
    /// rather than *erased value*: those route the source through the
    /// scalar-only `IntoIntValue` and take an `IntType` destination, and a
    /// `<N x iM>` is neither. This is the same split
    /// [`Self::int_binop_erased`] and [`Self::int_cmp_erased`]
    /// already make; upstream needs no such split because
    /// `LLParser::parseCast` hands the operand straight to
    /// `CastInst::Create`.
    ///
    /// Validated against `CastInst::castIsValid`'s integer arm: source and
    /// destination must both be integers or both integer vectors of the same
    /// element count and scalability, and the *scalar* widths must narrow for
    /// `trunc` and widen for `zext` / `sext`.
    ///
    /// Returns the erased [`ValueId`] rather than a typed id because the
    /// result may be a vector, which no `IntWidth` marker describes.
    pub fn int_cast_erased<Src, Name>(
        &self,
        opcode: CastOpcode,
        src: Src,
        dst_ty: Type<'ctx, B>,
        flags: IntCastFlags,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Src: IntoErasedValue<'ctx, B>,
    {
        if !matches!(
            opcode,
            CastOpcode::Trunc | CastOpcode::Zext | CastOpcode::Sext
        ) {
            return Err(IrError::InvalidOperation {
                message: "opcode is not trunc, zext or sext",
            });
        }
        let src = src.into_erased_value(ModuleRef::new(self.module))?;
        let src_ty = src.ty();

        let (Some(src_bits), Some(dst_bits)) = (
            self.integer_scalar_bit_width(src_ty),
            self.integer_scalar_bit_width(dst_ty),
        ) else {
            return Err(IrError::InvalidOperation {
                message: "trunc/zext/sext operand is neither an integer nor an integer vector",
            });
        };
        if self.vector_shape(src_ty) != self.vector_shape(dst_ty) {
            return Err(IrError::InvalidOperation {
                message: "trunc/zext/sext changes the vector element count",
            });
        }
        // `castIsValid` requires a strict change in width for all three; equal
        // widths would be a no-op cast, which upstream spells as no cast.
        let widens = matches!(opcode, CastOpcode::Zext | CastOpcode::Sext);
        if (widens && dst_bits <= src_bits) || (!widens && dst_bits >= src_bits) {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_bits,
                rhs: dst_bits,
            });
        }

        if let Some(folded) = self.folder.fold_cast_dyn(opcode, src, dst_ty)? {
            let folded = self.checked_folded_value(folded, dst_ty.id())?;
            return Ok(folded.id());
        }
        let payload = CastOpData::new(opcode, src.slot());
        payload.nneg.set(flags.nneg);
        payload.nuw.set(flags.nuw);
        payload.nsw.set(flags.nsw);
        let inst = self.append_instruction(dst_ty.id(), InstructionKindData::Cast(payload), name);
        Ok(inst.to_erased().id())
    }

    /// `icmp pred lhs, rhs` on erased operands (scalar `iN` / `ptr`, or a
    /// vector of either), yielding `i1` or `<N x i1>` to match.
    ///
    /// The erased counterpart of [`Self::int_cmp_with_flags_dyn`], whose
    /// `_dyn` means *dynamic width* (`IntDyn`) rather than *erased value*: it
    /// routes operands through the scalar-only `IntoIntValue` and mints an
    /// `IntValueId<bool, B>`, neither of which a vector compare can use.
    pub fn int_cmp_erased<Lhs, Rhs, Name>(
        &self,
        pred: IntPredicate,
        lhs: Lhs,
        rhs: Rhs,
        flags: IcmpFlags,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        if lhs.ty().id() != rhs.ty().id() {
            return Err(IrError::InvalidOperation {
                message: "icmp operands must have the same type",
            });
        }
        if !self.is_int_or_int_vector(lhs.ty()) && !self.is_pointer_or_pointer_vector(lhs.ty()) {
            return Err(IrError::InvalidOperation {
                message: "icmp operand is neither an integer nor a pointer",
            });
        }
        let result_ty = self.cmp_result_type(lhs.ty());
        let mut payload = CmpInstData::new(pred, lhs.id, rhs.id);
        payload.samesign = flags.samesign;
        let inst =
            self.append_instruction(result_ty.id(), InstructionKindData::Icmp(payload), name);
        Ok(inst.to_erased().id())
    }

    /// `select cond, true_arm, false_arm` on erased operands, where `cond` may
    /// be a scalar `i1` *or* a `<N x i1>` selecting lane-wise.
    ///
    /// The erased counterpart of [`Self::select`], which pins the
    /// condition to a scalar through `IntoIntValue<'ctx, bool, B>` and narrows
    /// the result back to the arm's own handle type via `SelectArm`. A
    /// `<N x i1>` is not an `IntValue<bool>` and `<N x iM>` is no `IntWidth`,
    /// so a vector select can use neither half — the same split
    /// [`Self::int_binop_erased`] and [`Self::int_cmp_erased`]
    /// already make. Upstream needs no split because `LLParser::parseSelect`
    /// hands all three operands straight to `SelectInst::Create`.
    ///
    /// Validated against `SelectInst::areInvalidOperands`, arm order included:
    /// the two arms must agree in type and must not be tokens, and only then
    /// is the condition examined — a vector one demanding `i1` elements,
    /// vector arms, and a matching element count.
    ///
    /// Returns the erased [`ValueId`] rather than a typed id because the
    /// result may be a vector.
    pub fn select_erased<Cond, TrueArm, FalseArm, Name>(
        &self,
        cond: Cond,
        true_arm: TrueArm,
        false_arm: FalseArm,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Cond: IntoErasedValue<'ctx, B>,
        TrueArm: IntoErasedValue<'ctx, B>,
        FalseArm: IntoErasedValue<'ctx, B>,
    {
        self.select_erased_with_fmf(cond, true_arm, false_arm, FastMathFlags::empty(), name)
    }

    /// `select cond, true_arm, false_arm` carrying fast-math flags.
    ///
    /// A `select` is an `FPMathOperator` only when its arms are
    /// floating-point (scalar or vector), which is why `LLParser` rejects
    /// flags on any other result type rather than dropping them. Non-empty
    /// flags on a non-FP select are refused here for the same reason.
    ///
    /// Flags are lost if the select folds to a constant — as upstream, where
    /// the folded result is a `Constant` and carries none.
    pub fn select_erased_with_fmf<Cond, TrueArm, FalseArm, Name>(
        &self,
        cond: Cond,
        true_arm: TrueArm,
        false_arm: FalseArm,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Cond: IntoErasedValue<'ctx, B>,
        TrueArm: IntoErasedValue<'ctx, B>,
        FalseArm: IntoErasedValue<'ctx, B>,
    {
        let cond = cond.into_erased_value(ModuleRef::new(self.module))?;
        let true_v = true_arm.into_erased_value(ModuleRef::new(self.module))?;
        let false_v = false_arm.into_erased_value(ModuleRef::new(self.module))?;

        // "both values to select must have same type"
        if true_v.ty().id() != false_v.ty().id() {
            return Err(IrError::TypeMismatch {
                expected: true_v.ty().kind_label(),
                got: false_v.ty().kind_label(),
            });
        }
        // "select values cannot have token type"
        if matches!(true_v.ty().data(), TypeData::Token) {
            return Err(IrError::InvalidOperation {
                message: "select values cannot have token type",
            });
        }

        match self.vector_shape(cond.ty()) {
            // Vector select.
            Some(condition_shape) => {
                if self.integer_scalar_bit_width(cond.ty()) != Some(1) {
                    return Err(IrError::InvalidOperation {
                        message: "vector select condition element type must be i1",
                    });
                }
                // "selected values for vector select must be vectors", and
                // then the element counts must agree.
                if self.vector_shape(true_v.ty()) != Some(condition_shape) {
                    return Err(IrError::InvalidOperation {
                        message: "vector select requires selected vectors to have the same vector \
                                  length as select condition",
                    });
                }
            }
            None => {
                if !matches!(cond.ty().data(), TypeData::Integer { bits: 1 }) {
                    return Err(IrError::InvalidOperation {
                        message: "select condition must be i1 or <n x i1>",
                    });
                }
            }
        }

        if !fmf.is_empty() && !self.is_float_or_float_vector(true_v.ty()) {
            return Err(IrError::InvalidOperation {
                message: "fast-math flags require a floating-point select result",
            });
        }

        let result_ty = true_v.ty().id();
        if let Some(folded) = self.folder.fold_select_dyn(cond, true_v, false_v)? {
            return Ok(self.checked_folded_value(folded, result_ty)?.id());
        }
        let payload = SelectInstData::new(cond.slot(), true_v.id, false_v.id);
        payload.fmf.set(fmf);
        let inst = self.append_instruction(result_ty, InstructionKindData::Select(payload), name);
        Ok(inst.to_erased().id())
    }

    /// `true` for a float type and for a vector of one.
    ///
    /// Mirrors the verifier's `is_fp_or_fp_vector`, which is the rule the built
    /// instruction is checked against.
    fn is_float_or_float_vector(&self, ty: Type<'ctx, B>) -> bool {
        let scalar = match ty.data() {
            TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
                self.module.context().type_data(*elem)
            }
            other => other,
        };
        matches!(
            scalar,
            TypeData::Half
                | TypeData::Bfloat
                | TypeData::Float
                | TypeData::Double
                | TypeData::X86Fp80
                | TypeData::Fp128
                | TypeData::PpcFp128
        )
    }

    /// `opcode lhs, rhs` on erased floating-point operands — a scalar float or
    /// a float vector — carrying `fmf`.
    ///
    /// The erased counterpart of the `fp_*` / `fp_*_fmf` families,
    /// and the floating-point sibling of [`Self::int_binop_erased`]. Both
    /// exist for the same reason: llvmkit's typed float handles carry a
    /// *scalar* `FloatKind`, so `<N x double>` has no typed handle to route
    /// through `IntoFloatValue`. Upstream needs no such split —
    /// `LLParser::parseArithmetic` hands the operands to
    /// `BinaryOperator::Create`.
    ///
    /// The entry point for a caller holding a *runtime* opcode, the `.ll`
    /// parser above all. Callers with a statically-known opcode and scalar
    /// operands should prefer the typed family, which pins the float kind in
    /// the type system.
    ///
    /// Validated against `BinaryOperator::Create`'s assertion that both
    /// operands share a type, plus the verifier's `FloatOpNonFloatOperand`
    /// rule. Returns the erased [`ValueId`] rather than a typed id because the
    /// result may be a vector, which no `FloatKind` marker describes.
    pub fn fp_binop_erased<Lhs, Rhs, Name>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        let kind_ctor = fp_binop_kind_ctor(opcode).ok_or(IrError::InvalidOperation {
            message: "opcode is not a floating-point binary operator",
        })?;
        if lhs.ty().id() != rhs.ty().id() {
            return Err(IrError::InvalidOperation {
                message: "floating-point binop operands must have the same type",
            });
        }
        if !self.is_float_or_float_vector(lhs.ty()) {
            return Err(IrError::InvalidOperation {
                message: "floating-point binop operand is neither a float nor a float vector",
            });
        }
        if let Some(folded) = self.folder.fold_bin_op_fmf_dyn(opcode, lhs, rhs, fmf)? {
            return Ok(self.checked_folded_value(folded, lhs.ty)?.id());
        }
        let mut payload = BinaryOpData::new(lhs.id, rhs.id);
        payload.fmf = fmf;
        let inst = self.append_instruction(lhs.ty().id(), kind_ctor(payload), name);
        Ok(inst.to_erased().id())
    }

    /// `fcmp pred lhs, rhs` on erased floating-point operands, yielding `i1` or
    /// `<N x i1>` to match.
    ///
    /// The erased counterpart of [`Self::fp_cmp`] and its `_fmf` sibling,
    /// which route operands through the scalar-only `IntoFloatValue` and mint a
    /// typed `bool` id — neither of which a vector compare can use. `fcmp` is an
    /// `FPMathOperator` upstream, so it carries fast-math flags like the
    /// arithmetic operators do.
    pub fn fp_cmp_erased<Lhs, Rhs, Name>(
        &self,
        predicate: FloatPredicate,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        if lhs.ty().id() != rhs.ty().id() {
            return Err(IrError::InvalidOperation {
                message: "fcmp operands must have the same type",
            });
        }
        if !self.is_float_or_float_vector(lhs.ty()) {
            return Err(IrError::InvalidOperation {
                message: "fcmp operand is neither a float nor a float vector",
            });
        }
        let result_ty = self.cmp_result_type(lhs.ty());
        let mut payload = FcmpInstData::new(predicate, lhs.id, rhs.id);
        payload.fmf = fmf;
        let inst =
            self.append_instruction(result_ty.id(), InstructionKindData::Fcmp(payload), name);
        Ok(inst.to_erased().id())
    }

    /// `fneg value` on an erased floating-point operand, carrying `fmf`.
    ///
    /// The erased counterpart of [`Self::fp_neg_fmf`], for the
    /// same reason as its binary sibling: a float *vector* has no typed handle.
    pub fn fp_neg_erased<Src, Name>(
        &self,
        value: Src,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Src: IntoErasedValue<'ctx, B>,
    {
        let value = value.into_erased_value(ModuleRef::new(self.module))?;
        if !self.is_float_or_float_vector(value.ty()) {
            return Err(IrError::InvalidOperation {
                message: "fneg operand is neither a float nor a float vector",
            });
        }
        if let Some(folded) = self
            .folder
            .fold_un_op_fmf_dyn(UnaryOpcode::Fneg, value, fmf)?
        {
            return Ok(self.checked_folded_value(folded, value.ty)?.id());
        }
        let payload = FnegInstData::new(value.slot(), fmf);
        let inst =
            self.append_instruction(value.ty().id(), InstructionKindData::Fneg(payload), name);
        Ok(inst.to_erased().id())
    }

    /// `true` for `ptr` and for a vector of `ptr`. Mirrors the verifier's
    /// `is_pointer_or_pointer_vector`.
    fn is_pointer_or_pointer_vector(&self, ty: Type<'ctx, B>) -> bool {
        let scalar = match ty.data() {
            TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
                self.module.context().type_data(*elem)
            }
            other => other,
        };
        matches!(scalar, TypeData::Pointer { .. })
    }

    /// `add lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_add_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(BinaryOpcode::Add, lhs, rhs, name, InstructionKindData::Add)
            .map(|v| v.id())
    }

    /// `sub lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_sub_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(BinaryOpcode::Sub, lhs, rhs, name, InstructionKindData::Sub)
            .map(|v| v.id())
    }

    /// `mul lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_mul_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(BinaryOpcode::Mul, lhs, rhs, name, InstructionKindData::Mul)
            .map(|v| v.id())
    }

    /// `xor lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_xor_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(BinaryOpcode::Xor, lhs, rhs, name, InstructionKindData::Xor)
            .map(|v| v.id())
    }

    /// `and lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_and_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(BinaryOpcode::And, lhs, rhs, name, InstructionKindData::And)
            .map(|v| v.id())
    }

    /// `or lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_or_dyn<Lhs, Rhs, Name>(&self, lhs: Lhs, rhs: Rhs, name: Name) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(BinaryOpcode::Or, lhs, rhs, name, InstructionKindData::Or)
            .map(|v| v.id())
    }

    /// `shl lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_shl_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(BinaryOpcode::Shl, lhs, rhs, name, InstructionKindData::Shl)
            .map(|v| v.id())
    }

    /// `lshr lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_lshr_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(
            BinaryOpcode::Lshr,
            lhs,
            rhs,
            name,
            InstructionKindData::Lshr,
        )
        .map(|v| v.id())
    }

    /// `ashr lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_ashr_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(
            BinaryOpcode::Ashr,
            lhs,
            rhs,
            name,
            InstructionKindData::Ashr,
        )
        .map(|v| v.id())
    }

    /// `udiv lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_udiv_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(
            BinaryOpcode::Udiv,
            lhs,
            rhs,
            name,
            InstructionKindData::Udiv,
        )
        .map(|v| v.id())
    }

    /// `sdiv lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_sdiv_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(
            BinaryOpcode::Sdiv,
            lhs,
            rhs,
            name,
            InstructionKindData::Sdiv,
        )
        .map(|v| v.id())
    }

    /// `urem lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_urem_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(
            BinaryOpcode::Urem,
            lhs,
            rhs,
            name,
            InstructionKindData::Urem,
        )
        .map(|v| v.id())
    }

    /// `srem lhs, rhs` on erased operands (scalar or integer vector).
    /// Uses the shared erased integer-binop validation path.
    pub fn int_srem_dyn<Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        Lhs: IntoErasedValue<'ctx, B>,
        Rhs: IntoErasedValue<'ctx, B>,
    {
        let lhs = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_erased_value(ModuleRef::new(self.module))?;
        self.int_binop_dyn(
            BinaryOpcode::Srem,
            lhs,
            rhs,
            name,
            InstructionKindData::Srem,
        )
        .map(|v| v.id())
    }

    // ---- Floating-point arithmetic ----
    //
    // Like the integer-arithmetic family above, every builder here hands back
    // a *storable id* (`FloatValueId<K, B>`), not a borrowing handle. Feeding
    // the result into the next builder costs nothing -- an id satisfies
    // `IntoFloatValue` / `IntoCallArg` -- while a *read* (`.ty()`,
    // `.as_erased()`, `{}`) goes through `IrBuilder::view` / `Module::view`.
    // The floating-point *comparisons* below return `IntValueId<bool, B>`,
    // because `fcmp` yields `i1`.

    /// Produce `fadd lhs, rhs`. Mirrors `IRBuilder::CreateFAdd`.
    ///
    /// Returns the storable [`FloatValueId<K, B>`](crate::FloatValueId) naming
    /// the result -- like every builder in this family. Pass it straight into
    /// the next builder call; take [`view`](Self::view) when you need to *read*
    /// from it:
    ///
    /// ```ignore
    /// let s = b.fp_add::<f32, _, _, _>(x, y, "s")?;
    /// let p = b.fp_mul::<f32, _, _, _>(s, y, "p")?; // operand: no view
    /// let ty = b.view(s).ty();                             // read: view
    /// ```
    pub fn fp_add<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop(
            BinaryOpcode::Fadd,
            lhs,
            rhs,
            name,
            InstructionKindData::Fadd,
        )
        .map(|v| v.id())
    }

    /// Produce `fsub lhs, rhs`. Mirrors `IRBuilder::CreateFSub`.
    pub fn fp_sub<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop(
            BinaryOpcode::Fsub,
            lhs,
            rhs,
            name,
            InstructionKindData::Fsub,
        )
        .map(|v| v.id())
    }

    /// Produce `fmul lhs, rhs`. Mirrors `IRBuilder::CreateFMul`.
    pub fn fp_mul<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop(
            BinaryOpcode::Fmul,
            lhs,
            rhs,
            name,
            InstructionKindData::Fmul,
        )
        .map(|v| v.id())
    }

    /// Produce `fdiv lhs, rhs`. Mirrors `IRBuilder::CreateFDiv`.
    pub fn fp_div<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop(
            BinaryOpcode::Fdiv,
            lhs,
            rhs,
            name,
            InstructionKindData::Fdiv,
        )
        .map(|v| v.id())
    }

    /// Produce `frem lhs, rhs`. Mirrors `IRBuilder::CreateFRem`.
    pub fn fp_rem<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop(
            BinaryOpcode::Frem,
            lhs,
            rhs,
            name,
            InstructionKindData::Frem,
        )
        .map(|v| v.id())
    }

    /// Crate-internal helper for float binops. Same shape as
    /// `int_binop` but parameterised by `K: FloatKind`.
    fn fp_binop<K, Lhs, Rhs, F2, N>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        name: N,
        kind_ctor: F2,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
        N: AsRef<str>,
    {
        let lhs = lhs.into_float_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_float_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_fp_bin_op(opcode, lhs, rhs, self.fmf)? {
            return self.accept_folded_fp(folded, lhs);
        }
        let mut payload = BinaryOpData::new(lhs.slot(), rhs.slot());
        // Apply the builder-context FMF (parallel to upstream
        // `IRBuilderBase::setFPAttrs` in `IRBuilder.h`, which calls
        // `I->setFastMathFlags(FMF)` on every FP-math instruction).
        payload.fmf = self.fmf;
        Ok(self.append_fp_like(lhs, kind_ctor(payload), name))
    }

    /// Crate-internal helper for float binops with an explicit
    /// [`crate::fmf::FastMathFlags`] parameter rather than the builder-context
    /// FMF. Used by the `fp_*_fmf` family.
    fn fp_binop_with_fmf<K, Lhs, Rhs, F2, N>(
        &self,
        opcode: BinaryOpcode,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: N,
        kind_ctor: F2,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
        N: AsRef<str>,
    {
        let lhs = lhs.into_float_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_float_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_fp_bin_op(opcode, lhs, rhs, fmf)? {
            return self.accept_folded_fp(folded, lhs);
        }
        let mut payload = BinaryOpData::new(lhs.slot(), rhs.slot());
        payload.fmf = fmf;
        Ok(self.append_fp_like(lhs, kind_ctor(payload), name))
    }

    /// `fadd` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    /// Bypasses the builder-context FMF; caller supplies the exact flags.
    pub fn fp_add_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop_with_fmf(
            BinaryOpcode::Fadd,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::Fadd,
        )
        .map(|v| v.id())
    }

    /// `fsub` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    pub fn fp_sub_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop_with_fmf(
            BinaryOpcode::Fsub,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::Fsub,
        )
        .map(|v| v.id())
    }

    /// `fmul` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    pub fn fp_mul_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop_with_fmf(
            BinaryOpcode::Fmul,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::Fmul,
        )
        .map(|v| v.id())
    }

    /// `fdiv` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    pub fn fp_div_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop_with_fmf(
            BinaryOpcode::Fdiv,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::Fdiv,
        )
        .map(|v| v.id())
    }

    /// `frem` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    pub fn fp_rem_fmf<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_binop_with_fmf(
            BinaryOpcode::Frem,
            lhs,
            rhs,
            fmf,
            name,
            InstructionKindData::Frem,
        )
        .map(|v| v.id())
    }

    /// `fcmp` with an explicit [`crate::fmf::FastMathFlags`] parameter.
    /// Bypasses the builder-context FMF. Result is `i1`.
    pub fn fp_cmp_fmf<K, Lhs, Rhs, Name>(
        &self,
        pred: FloatPredicate,
        lhs: Lhs,
        rhs: Rhs,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        let lhs = lhs.into_float_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_float_value(ModuleRef::new(self.module))?;
        let i1 = ModuleView::<B>::new(self.module).bool_type();
        if let Some(folded) = self.folder.fold_fp_cmp(pred, lhs, rhs)? {
            return Ok(folded.id());
        }
        let mut payload = FcmpInstData::new(pred, lhs.slot(), rhs.slot());
        payload.fmf = fmf;
        Ok(self
            .append_int_at(i1, InstructionKindData::Fcmp(payload), name)
            .id())
    }

    /// Produce `fcmp <pred> lhs, rhs`. Mirrors
    /// `IRBuilder::CreateFCmp`. Result is `i1`.
    pub fn fp_cmp<K, Lhs, Rhs, Name>(
        &self,
        pred: FloatPredicate,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        let lhs = lhs.into_float_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_float_value(ModuleRef::new(self.module))?;
        let i1 = ModuleView::<B>::new(self.module).bool_type();
        if let Some(folded) = self.folder.fold_fp_cmp(pred, lhs, rhs)? {
            return Ok(folded.id());
        }
        let mut payload = FcmpInstData::new(pred, lhs.slot(), rhs.slot());
        // Apply builder-context FMF (`fcmp` is an `FPMathOperator` upstream).
        payload.fmf = self.fmf;
        Ok(self
            .append_int_at(i1, InstructionKindData::Fcmp(payload), name)
            .id())
    }

    // ---- Per-predicate fcmp wrappers ----
    //
    // Each method mirrors the matching `IRBuilder::CreateFCmpO<Pred>` /
    // `CreateFCmpU<Pred>` in `IRBuilder.h` (lines 2371-2475). All
    // delegate to `fp_cmp` with the appropriate `FloatPredicate`.

    /// Mirrors `IRBuilder::CreateFCmpOEQ`.
    pub fn fcmp_oeq<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Oeq, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpOGT`.
    pub fn fcmp_ogt<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Ogt, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpOGE`.
    pub fn fcmp_oge<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Oge, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpOLT`.
    pub fn fcmp_olt<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Olt, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpOLE`.
    pub fn fcmp_ole<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Ole, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpONE`.
    pub fn fcmp_one<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::One, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpORD`.
    pub fn fcmp_ord<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Ord, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpUNO`.
    pub fn fcmp_uno<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Uno, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpUEQ`.
    pub fn fcmp_ueq<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Ueq, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpUGT`.
    pub fn fcmp_ugt<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Ugt, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpUGE`.
    pub fn fcmp_uge<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Uge, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpULT`.
    pub fn fcmp_ult<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Ult, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpULE`.
    pub fn fcmp_ule<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Ule, lhs, rhs, name)
    }

    /// Mirrors `IRBuilder::CreateFCmpUNE`.
    pub fn fcmp_une<K, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::FloatKind,
        Lhs: IntoFloatValue<'ctx, K, B>,
        Rhs: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_cmp::<K, Lhs, Rhs, _>(super::cmp_predicate::FloatPredicate::Une, lhs, rhs, name)
    }

    // ---- Unary ops: fneg / freeze / va_arg ----

    /// Produce `fneg <value>`. Mirrors `IRBuilder::CreateFNeg` in
    /// `IRBuilder.h`. The result handle has the same float kind as the
    /// operand (Doctrine D4).
    pub fn fp_neg<K, V, Name>(&self, value: V, name: Name) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        V: IntoFloatValue<'ctx, K, B>,
    {
        self.fp_neg_fmf::<K, V, _>(value, self.fmf, name)
    }

    /// Produce `fneg <fmf> <value>`. Mirrors `IRBuilder::CreateFNegFMF`.
    /// The flags are written verbatim onto the instruction (see
    /// `FPMathOperator::setFastMathFlags`).
    pub fn fp_neg_fmf<K, V, Name>(
        &self,
        value: V,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        V: IntoFloatValue<'ctx, K, B>,
    {
        let v = value.into_float_value(ModuleRef::new(self.module))?;
        if let Some(folded) = self.folder.fold_fp_un_op(UnaryOpcode::Fneg, v, fmf)? {
            return self.accept_folded_fp(folded, v).map(|v| v.id());
        }
        let payload = FnegInstData::new(v.slot(), fmf);
        Ok(self
            .append_fp_like(v, InstructionKindData::Fneg(payload), name)
            .id())
    }

    /// Produce `freeze <value>`. Mirrors `IRBuilder::CreateFreeze`.
    /// Accepts any [`IntoErasedValue`] operand — every value handle plus the
    /// storable ids; the result type matches the operand type. Named by the
    /// storable [`FreezeInstId<B>`](crate::FreezeInstId).
    pub fn freeze<V, Name>(&self, value: V, name: Name) -> IrResult<FreezeInstId<B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
    {
        let v = value.into_erased_value(ModuleRef::new(self.module))?;
        let payload = FreezeInstData::new(v.id);
        let inst = self.append_instruction(v.ty, InstructionKindData::Freeze(payload), name);
        Ok(FreezeInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Produce `va_arg <list>, <ty>`. Mirrors `IRBuilder::CreateVAArg`.
    /// The destination type can be any first-class type; the source
    /// must be a `va_list` pointer. Named by the storable
    /// [`VaArgInstId<B>`](crate::VaArgInstId).
    pub fn va_arg<P, Name>(
        &self,
        list_ptr: P,
        result_ty: Type<'ctx, B>,
        name: Name,
    ) -> IrResult<VaArgInstId<B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let list_ptr = list_ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let v = IsValue::as_erased(list_ptr);
        let payload = VaArgInstData::new(v.id);
        let inst = self.append_instruction(result_ty.id, InstructionKindData::VaArg(payload), name);
        Ok(VaArgInstId::from_raw(self.module.id(), inst.slot()))
    }

    // ---- Aggregate ops: extractvalue / insertvalue ----

    /// Produce `extractvalue <agg-ty> <agg>, idx0, idx1, ...`.
    /// Mirrors `IRBuilder::CreateExtractValue`.
    ///
    /// The index list is a fixed-size array whose length is checked at
    /// compile time (Doctrine D3): `ExtractValueInst::init`
    /// (`lib/IR/Instructions.cpp`) asserts a non-empty index list, and
    /// `N > 0` pulls that assertion forward to monomorphisation instead of
    /// a runtime error. Use [`Self::extract_value_dyn`] for a
    /// slice/`Vec`-driven index list that keeps the runtime check.
    pub fn extract_value<V, const N: usize, Name>(
        &self,
        aggregate: V,
        indices: [u32; N],
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
    {
        const {
            assert!(N > 0, "extractvalue requires at least one index");
        }
        self.extract_value_dyn(aggregate, &indices, name)
    }

    /// Produce `extractvalue <agg-ty> <agg>, idx0, idx1, ...` from a
    /// dynamically-sized index slice. Mirrors `IRBuilder::CreateExtractValue`.
    ///
    /// Ports the empty-index-list rejection in
    /// `ExtractValueInst::init` (`lib/IR/Instructions.cpp`); see
    /// `test/Assembler/extractvalue-no-idx.ll` for the upstream assembler
    /// diagnostic this pulls forward. Prefer
    /// [`Self::extract_value`] when the index count is known at
    /// compile time, which upgrades this runtime check to a compile error.
    pub fn extract_value_dyn<V, Name>(
        &self,
        aggregate: V,
        indices: &[u32],
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
    {
        let agg = aggregate.into_erased_value(ModuleRef::new(self.module))?;
        if indices.is_empty() {
            return Err(IrError::InvalidOperation {
                message: "extractvalue indices must not be empty",
            });
        }
        let leaf_ty = walk_aggregate_for_builder(self.module, agg.ty, indices)?;
        if let Some(folded) = self.folder.fold_extract_value_dyn(agg, indices)? {
            return self.checked_folded_value(folded, leaf_ty).map(|v| v.id());
        }
        let payload = ExtractValueInstData::new(agg.id, indices.to_vec());
        let inst =
            self.append_instruction(leaf_ty, InstructionKindData::ExtractValue(payload), name);
        Ok(inst.to_erased().id())
    }

    /// Produce `insertvalue <agg-ty> <agg>, <elt-ty> <elt>, idx0, ...`.
    /// Mirrors `IRBuilder::CreateInsertValue`.
    ///
    /// The index list is a fixed-size array whose length is checked at
    /// compile time (Doctrine D3): `InsertValueInst::init`
    /// (`lib/IR/Instructions.cpp`) asserts a non-empty index list, and
    /// `N > 0` pulls that assertion forward to monomorphisation instead of
    /// a runtime error. Use [`Self::insert_value_dyn`] for a
    /// slice/`Vec`-driven index list that keeps the runtime check.
    pub fn insert_value<A, V, const N: usize, Name>(
        &self,
        aggregate: A,
        value: V,
        indices: [u32; N],
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        A: IntoErasedValue<'ctx, B>,
        V: IntoErasedValue<'ctx, B>,
    {
        const {
            assert!(N > 0, "insertvalue requires at least one index");
        }
        self.insert_value_dyn(aggregate, value, &indices, name)
    }

    /// Produce `insertvalue <agg-ty> <agg>, <elt-ty> <elt>, idx0, ...` from
    /// a dynamically-sized index slice. Mirrors
    /// `IRBuilder::CreateInsertValue`.
    ///
    /// Ports the empty-index-list rejection in `InsertValueInst::init`
    /// (`lib/IR/Instructions.cpp`); see
    /// `test/Assembler/extractvalue-no-idx.ll` for the upstream assembler
    /// diagnostic this pulls forward (the parser shares one "expected
    /// index" path for both opcodes). Prefer [`Self::insert_value`]
    /// when the index count is known at compile time, which upgrades this
    /// runtime check to a compile error.
    pub fn insert_value_dyn<A, V, Name>(
        &self,
        aggregate: A,
        value: V,
        indices: &[u32],
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        A: IntoErasedValue<'ctx, B>,
        V: IntoErasedValue<'ctx, B>,
    {
        let agg = aggregate.into_erased_value(ModuleRef::new(self.module))?;
        let val = value.into_erased_value(ModuleRef::new(self.module))?;
        if indices.is_empty() {
            return Err(IrError::InvalidOperation {
                message: "insertvalue indices must not be empty",
            });
        }
        let leaf_ty = walk_aggregate_for_builder(self.module, agg.ty, indices)?;
        if val.ty != leaf_ty {
            return Err(IrError::TypeMismatch {
                expected: Type::<B>::new(leaf_ty, self.module).kind_label(),
                got: val.ty().kind_label(),
            });
        }
        if let Some(folded) = self.folder.fold_insert_value_dyn(agg, val, indices)? {
            return self.checked_folded_value(folded, agg.ty).map(|v| v.id());
        }
        let payload = InsertValueInstData::new(agg.id, val.id, indices.to_vec());
        let inst = self.append_instruction(agg.ty, InstructionKindData::InsertValue(payload), name);
        Ok(inst.to_erased().id())
    }

    /// Extract a named-struct schema field and return the field's typed wrapper.
    pub fn extract_field<S, Field, Aggregate, Name>(
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
        let leaf = Type::<B>::new(leaf_ty, self.module);
        if !Field::matches_ir_type(leaf) {
            return Err(IrError::TypeMismatch {
                expected: Field::expected_kind_label(),
                got: leaf.kind_label(),
            });
        }
        let raw = self.view(self.extract_value(aggregate, [index], name)?);
        Field::value_from_ir_value(raw)
    }

    /// Insert a typed field value into a named-struct schema aggregate.
    pub fn insert_field<S, Field, Aggregate, FieldValue, Name>(
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
        let raw = self.view(self.insert_value(aggregate, value, [index], name)?);
        <S as IrField>::value_from_ir_value(raw)
    }

    // ---- Vector ops: extractelement / insertelement / shufflevector ----

    /// Produce `extractelement <vec-ty> <vec>, <idx-ty> <idx>`.
    /// Mirrors `IRBuilder::CreateExtractElement`.
    pub fn extract_element<V, W, I, Name>(
        &self,
        vector: V,
        index: I,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
        W: IntWidth,
        I: IntoIntValue<'ctx, W, B>,
    {
        let vec = vector.into_erased_value(ModuleRef::new(self.module))?;
        let idx_v = index.into_int_value(ModuleRef::new(self.module))?;
        let idx = IsValue::as_erased(idx_v);
        let elem_ty = match self.module.context().type_data(vec.ty).as_vector() {
            Some((e, _, _)) => e,
            None => {
                return Err(IrError::TypeMismatch {
                    expected: TypeKindLabel::FixedVector,
                    got: vec.ty().kind_label(),
                });
            }
        };
        if let Some(folded) = self.folder.fold_extract_element_dyn(vec, idx)? {
            return self.checked_folded_value(folded, elem_ty).map(|v| v.id());
        }
        let payload = ExtractElementInstData::new(vec.id, idx.id);
        let inst =
            self.append_instruction(elem_ty, InstructionKindData::ExtractElement(payload), name);
        Ok(inst.to_erased().id())
    }

    /// Produce `insertelement <vec-ty> <vec>, <elt-ty> <elt>, <idx-ty> <idx>`.
    /// Mirrors `IRBuilder::CreateInsertElement`.
    pub fn insert_element<V, E, W, I, Name>(
        &self,
        vector: V,
        elt: E,
        index: I,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
        E: IntoErasedValue<'ctx, B>,
        W: IntWidth,
        I: IntoIntValue<'ctx, W, B>,
    {
        let vec = vector.into_erased_value(ModuleRef::new(self.module))?;
        let val = elt.into_erased_value(ModuleRef::new(self.module))?;
        let idx_v = index.into_int_value(ModuleRef::new(self.module))?;
        let idx = IsValue::as_erased(idx_v);
        if let Some(folded) = self.folder.fold_insert_element_dyn(vec, val, idx)? {
            return self.checked_folded_value(folded, vec.ty).map(|v| v.id());
        }
        let payload = InsertElementInstData::new(vec.id, val.id, idx.id);
        let inst =
            self.append_instruction(vec.ty, InstructionKindData::InsertElement(payload), name);
        Ok(inst.to_erased().id())
    }

    /// Produce `shufflevector <ty> <v1>, <ty> <v2>, <mask>`. Mirrors
    /// `IRBuilder::CreateShuffleVector`. Each mask element is a
    /// [`ShuffleMaskElem`]: `Lane(n)` selects lane `n` of the two operands
    /// taken as one concatenated vector, and `Poison` is upstream's `-1`.
    pub fn shuffle_vector<L, Rhs2, Name>(
        &self,
        lhs: L,
        rhs: Rhs2,
        mask: &[ShuffleMaskElem],
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        L: IntoErasedValue<'ctx, B>,
        Rhs2: IntoErasedValue<'ctx, B>,
    {
        let l = lhs.into_erased_value(ModuleRef::new(self.module))?;
        let r = rhs.into_erased_value(ModuleRef::new(self.module))?;
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
                    expected: TypeKindLabel::FixedVector,
                    got: l.ty().kind_label(),
                });
            }
        };
        let mask_len = u32::try_from(mask.len()).map_err(|_| IrError::InvalidOperation {
            message: "shufflevector mask too large",
        })?;
        let result_ty_id = self.module.context().fixed_vector_type(elem, mask_len);
        if let Some(folded) = self.folder.fold_shuffle_vector_dyn(l, r, mask)? {
            return self
                .checked_folded_value(folded, result_ty_id)
                .map(|v| v.id());
        }
        let payload = ShuffleVectorInstData::new(l.id, r.id, mask.iter().copied());
        let inst = self.append_instruction(
            result_ty_id,
            InstructionKindData::ShuffleVector(payload),
            name,
        );
        Ok(inst.to_erased().id())
    }

    // ---- Typed vector ops: element/length-checked siblings ----
    //
    // Additive, distinctly-named siblings of the erased vector builders
    // above (`extract_element` / `insert_element` /
    // `vector_splat_dyn`) and the erased integer-vector binop family
    // (`int_add_dyn` & friends). The erased forms take `Value`/`u32`
    // operands and rely on the verifier for shape agreement; these carry the
    // element marker `E` and the lane-count marker `L` in the type system,
    // so an element-width mismatch (`<4 x i32>` vs `<4 x i64>`) or a
    // lane-count mismatch (`Len<4>` vs `Len<2>`) is a compile error at the
    // call site — for free, since both operands name the SAME `E, L`. They
    // lower into the erased builders unchanged; the verifier stays as
    // defense-in-depth.

    /// Crate-internal: shared body for the nine typed integer-vector binops.
    /// Both operands share the SAME `E` (integer element width) and `L`
    /// (static lane count), so element- and length-mismatches are compile
    /// errors. Lowers into the erased [`Self::int_binop_dyn`].
    fn vector_int_binop<E, L, Name, F2>(
        &self,
        opcode: BinaryOpcode,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
        kind_ctor: F2,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
        F2: FnOnce(BinaryOpData) -> InstructionKindData,
    {
        let r = self.int_binop_dyn(opcode, lhs.as_erased(), rhs.as_erased(), name, kind_ctor)?;
        Ok(VectorValue::from_value_unchecked(r))
    }

    /// Typed element-wise `add` on two identically-typed integer vectors.
    /// The element width (`E`) and lane count (`L`) must match at compile
    /// time. Sibling of the erased [`Self::int_add_dyn`].
    pub fn vector_int_add<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(BinaryOpcode::Add, lhs, rhs, name, InstructionKindData::Add)
    }

    /// Typed element-wise `sub`. See [`Self::vector_int_add`].
    pub fn vector_int_sub<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(BinaryOpcode::Sub, lhs, rhs, name, InstructionKindData::Sub)
    }

    /// Typed element-wise `mul`. See [`Self::vector_int_add`].
    pub fn vector_int_mul<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(BinaryOpcode::Mul, lhs, rhs, name, InstructionKindData::Mul)
    }

    /// Typed element-wise `xor`. See [`Self::vector_int_add`].
    pub fn vector_int_xor<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(BinaryOpcode::Xor, lhs, rhs, name, InstructionKindData::Xor)
    }

    /// Typed element-wise `and`. See [`Self::vector_int_add`].
    pub fn vector_int_and<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(BinaryOpcode::And, lhs, rhs, name, InstructionKindData::And)
    }

    /// Typed element-wise `or`. See [`Self::vector_int_add`].
    pub fn vector_int_or<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(BinaryOpcode::Or, lhs, rhs, name, InstructionKindData::Or)
    }

    /// Typed element-wise `shl`. Both operands are the same `<L x E>` vector
    /// type (LLVM vector shifts take a same-typed shift-amount vector). See
    /// [`Self::vector_int_add`].
    pub fn vector_int_shl<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(BinaryOpcode::Shl, lhs, rhs, name, InstructionKindData::Shl)
    }

    /// Typed element-wise `lshr`. See [`Self::vector_int_shl`].
    pub fn vector_int_lshr<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(
            BinaryOpcode::Lshr,
            lhs,
            rhs,
            name,
            InstructionKindData::Lshr,
        )
    }

    /// Typed element-wise `ashr`. See [`Self::vector_int_shl`].
    pub fn vector_int_ashr<E, L, Name>(
        &self,
        lhs: VectorValue<'ctx, E, L, B>,
        rhs: VectorValue<'ctx, E, L, B>,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: VecElem + StaticIntWidth,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        self.vector_int_binop(
            BinaryOpcode::Ashr,
            lhs,
            rhs,
            name,
            InstructionKindData::Ashr,
        )
    }

    /// Typed `extractelement`: read lane `index` out of `vec`, returning the
    /// element as its statically-typed scalar handle (`E::Value` —
    /// `IntValue<iN>` / `FloatValue<fN>`), inferred from `vec`'s element
    /// marker so no annotation is needed. Any lane count is allowed (extract
    /// does not need a static length). Sibling of the erased
    /// [`Self::extract_element`].
    pub fn vector_extract<E, L, W, I, Name>(
        &self,
        vec: VectorValue<'ctx, E, L, B>,
        index: I,
        name: Name,
    ) -> IrResult<E::Value>
    where
        E: StaticVecElem<'ctx, B>,
        L: VecLen,
        W: IntWidth,
        I: IntoIntValue<'ctx, W, B>,
        Name: AsRef<str>,
    {
        let raw = self.view(self.extract_element::<_, W, _, _>(vec, index, name)?);
        Ok(E::wrap_value(raw, WrapWitness::new()))
    }

    /// Typed `insertelement`: write `element` into lane `index` of `vec`,
    /// returning a vector with the same element/length markers. The
    /// `element: E::Value` parameter makes inserting a wrong-typed scalar
    /// (e.g. a `FloatValue<f32>` into a `<4 x i32>`) a compile error.
    /// Sibling of the erased [`Self::insert_element`].
    pub fn vector_insert<E, L, W, I, Name>(
        &self,
        vec: VectorValue<'ctx, E, L, B>,
        element: E::Value,
        index: I,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: StaticVecElem<'ctx, B>,
        L: VecLen,
        W: IntWidth,
        I: IntoIntValue<'ctx, W, B>,
        Name: AsRef<str>,
    {
        let raw = self.view(self.insert_element::<_, _, W, _, _>(vec, element, index, name)?);
        Ok(VectorValue::from_value_unchecked(raw))
    }

    /// Typed vector splat: broadcast `scalar` across a fixed-length vector.
    /// The scalar must be the element's typed handle (`scalar: E::Value`), so
    /// a wrong-typed scalar is a compile error. `E` and `L` are pinned by the
    /// result: annotate it — `let v: VectorValue<i32, Len<4>> =
    /// b.vector_splat(x, "v")?` — or spell the element out with a turbofish
    /// (`vector_splat::<i32, Len<4>, _>(x, "v")`). `E` cannot be left as a
    /// `_` placeholder: Rust does not invert the `E::Value` projection, so it
    /// cannot deduce `E` from the scalar's type alone. Sibling of the erased
    /// [`Self::vector_splat_dyn`], lowering into it with
    /// `count = L::STATIC_LEN`.
    pub fn vector_splat<E, L, Name>(
        &self,
        scalar: E::Value,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, E, L, B>>
    where
        E: StaticVecElem<'ctx, B>,
        L: StaticVecLen,
        Name: AsRef<str>,
    {
        let erased = self.vector_splat_dyn(L::STATIC_LEN, scalar, name)?;
        Ok(VectorValue::from_value_unchecked(erased.as_erased()))
    }

    // ---- Typed array ops: extractvalue / insertvalue ----

    /// Typed single-index `extractvalue` on a statically-typed array: read
    /// element `index` out of `array`, returning it as its statically-typed
    /// scalar handle (`E::Value` — `IntValue<iN>` / `FloatValue<fN>`),
    /// inferred from `array`'s element marker so no annotation is needed.
    /// Sibling of the erased [`Self::extract_value`], lowering into it
    /// with the single-element index list `[index]` (arrays index by a `u32`,
    /// matching the erased aggregate op).
    ///
    /// **Index-in-bounds is NOT checked at compile time.** An out-of-bounds
    /// `index` stays poison per LLVM, exactly as the erased path — the nightly
    /// `generic_const_exprs` `I < N` bound is out of scope here; the win is
    /// element-typing only.
    pub fn array_extract<E, L, Name>(
        &self,
        array: ArrayValue<'ctx, E, L, B>,
        index: u32,
        name: Name,
    ) -> IrResult<E::Value>
    where
        E: StaticVecElem<'ctx, B>,
        L: ArrayLen,
        Name: AsRef<str>,
    {
        let raw = self.view(self.extract_value(array, [index], name)?);
        Ok(E::wrap_value(raw, WrapWitness::new()))
    }

    /// Typed single-index `insertvalue` on a statically-typed array: write
    /// `element` into slot `index` of `array`, returning an array with the
    /// same element/length markers. The `element: E::Value` parameter makes
    /// inserting a wrong-typed element (e.g. a `FloatValue<f32>` into a
    /// `[4 x i32]`) a compile error. Sibling of the erased
    /// [`Self::insert_value`], lowering into it with the single-element
    /// index list `[index]`.
    ///
    /// **Index-in-bounds is NOT checked at compile time** — same
    /// poison-on-out-of-bounds semantics as [`Self::array_extract`].
    pub fn array_insert<E, L, Name>(
        &self,
        array: ArrayValue<'ctx, E, L, B>,
        element: E::Value,
        index: u32,
        name: Name,
    ) -> IrResult<ArrayValue<'ctx, E, L, B>>
    where
        E: StaticVecElem<'ctx, B>,
        L: ArrayLen,
        Name: AsRef<str>,
    {
        let raw = self.view(self.insert_value(array, element, [index], name)?);
        Ok(ArrayValue::from_value_unchecked(raw))
    }

    // ---- Atomic ops: fence / cmpxchg / atomicrmw ----

    /// Produce `fence <ordering>` (or
    /// `fence syncscope("...") <ordering>`). Mirrors
    /// `IRBuilder::CreateFence`.
    pub fn fence<Name>(
        &self,
        ordering: AtomicOrdering,
        sync_scope: SyncScope,
        name: Name,
    ) -> IrResult<FenceInst<'ctx, B>>
    where
        Name: AsRef<str>,
    {
        let payload = FenceInstData::new(ordering, sync_scope);
        let void_ty = self.module.void_type::<B>().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Fence(payload), name);
        Ok(FenceInst::from_raw(inst.slot(), self.module, void_ty))
    }

    /// Produce `cmpxchg [weak] [volatile] <ptr-ty> <ptr>, <cmp-ty> <cmp>,
    /// <new-ty> <new> [syncscope("...")] <success> <failure>, align N`.
    /// Mirrors `IRBuilder::CreateAtomicCmpXchg`.
    ///
    /// Result type is the literal struct `{ <pointee>, i1 }`. Named by the
    /// storable [`AtomicCmpXchgInstId<B>`](crate::AtomicCmpXchgInstId).
    pub fn atomic_cmpxchg<P, C, N, Name>(
        &self,
        ptr: P,
        cmp: C,
        new_val: N,
        config: AtomicCmpXchgConfig,
        name: Name,
    ) -> IrResult<AtomicCmpXchgInstId<B>>
    where
        Name: AsRef<str>,
        P: IntoErasedValue<'ctx, B>,
        C: IntoErasedValue<'ctx, B>,
        N: IntoErasedValue<'ctx, B>,
    {
        let p = ptr.into_erased_value(ModuleRef::new(self.module))?;
        let c = cmp.into_erased_value(ModuleRef::new(self.module))?;
        let n = new_val.into_erased_value(ModuleRef::new(self.module))?;
        if c.ty != n.ty {
            return Err(IrError::TypeMismatch {
                expected: c.ty().kind_label(),
                got: n.ty().kind_label(),
            });
        }
        let module_view = ModuleView::<B>::new(self.module);
        let result_ty = module_view.struct_type([c.ty(), module_view.bool_type().as_type()]);
        let payload = AtomicCmpXchgInstData::new(p.id, c.id, n.id, config);
        let result_id = result_ty.as_type().id();
        let inst =
            self.append_instruction(result_id, InstructionKindData::AtomicCmpXchg(payload), name);
        Ok(AtomicCmpXchgInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Produce `atomicrmw [volatile] <op> <ptr-ty> <ptr>, <val-ty> <val>
    /// [syncscope("...")] <ordering>, align N`. Mirrors
    /// `IRBuilder::CreateAtomicRMW`.
    ///
    /// Result type matches the value-operand type (the "old" value). Named by
    /// the storable [`AtomicRmwInstId<B>`](crate::AtomicRmwInstId).
    pub fn atomicrmw<P, V, Name>(
        &self,
        op: AtomicRmwBinOp,
        ptr: P,
        value: V,
        config: AtomicRmwConfig,
        name: Name,
    ) -> IrResult<AtomicRmwInstId<B>>
    where
        Name: AsRef<str>,
        P: IntoErasedValue<'ctx, B>,
        V: IntoErasedValue<'ctx, B>,
    {
        let p = ptr.into_erased_value(ModuleRef::new(self.module))?;
        let v = value.into_erased_value(ModuleRef::new(self.module))?;
        let payload = AtomicRmwInstData::new(op, p.id, v.id, config);
        let inst = self.append_instruction(v.ty, InstructionKindData::AtomicRmw(payload), name);
        Ok(AtomicRmwInstId::from_raw(self.module.id(), inst.slot()))
    }

    // ---- Casts: trunc / zext / sext ----
    //
    // Every cast builder in this file (integer casts here, the float casts and
    // pointer casts further down) returns a *storable id* rather than a
    // borrowing handle: `IntValueId<W, B>` / `FloatValueId<K, B>` /
    // `PointerValueId<B>` per the result kind, and `ValueId<B>` for the two
    // fully-erased forms (`bitcast_dyn`, `ptr_to_addr_dyn`) whose
    // result may be a vector. The source operand is taken by an `Into*Value`
    // bound, so a returned id feeds straight into the next cast without being
    // rehydrated: `b.sext(t, i64_ty, "e")`.

    /// Produce `trunc <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateTrunc`.
    ///
    /// The `Src: WiderThan<Dst>` bound enforces at compile time that
    /// the destination is strictly narrower than the source. Cross-
    /// width attempts (e.g. `trunc::<i32, i64>`) fail to
    /// compile rather than returning a runtime
    /// [`IrError::OperandWidthMismatch`]. Use
    /// [`Self::trunc_dyn`] when both widths are erased.
    ///
    pub fn trunc<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<IntValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: WiderThan<Dst>,
        Dst: IntWidth,
        V: IntoIntValue<'ctx, Src, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) =
            self.folder
                .fold_cast_to_int(CastOpcode::Trunc, value.as_erased(), dst_ty)?
        {
            return self.accept_folded_cast_int(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(CastOpcode::Trunc, value.slot());
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// `trunc nuw/nsw` with explicit [`crate::TruncFlags`]. Mirrors
    /// `IRBuilder::CreateTrunc` plus `Instruction::setHasNoUnsignedWrap` /
    /// `setHasNoSignedWrap`.
    ///
    /// The `Src: WiderThan<Dst>` bound is the same one [`Self::trunc`]
    /// uses, enforced at compile time. Upstream `IRBuilder::CreateTrunc`
    /// (`IRBuilder.cpp`) returns `V` unchanged -- silently dropping any
    /// requested `nuw`/`nsw` -- when `SrcTy == DestTy`. Because `WiderThan`
    /// requires `Src` strictly wider than `Dst`, that same-type case is
    /// unspellable through this method: the flag-dropping branch cannot
    /// arise here (D10 -- no silent bad-codegen). Use
    /// [`Self::trunc_with_flags_dyn`] when both widths are erased.
    pub fn trunc_with_flags<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, Dst, B>,
        flags: TruncFlags,
        name: Name,
    ) -> IrResult<IntValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: WiderThan<Dst>,
        Dst: IntWidth,
        V: IntoIntValue<'ctx, Src, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) =
            self.folder
                .fold_cast_to_int(CastOpcode::Trunc, value.as_erased(), dst_ty)?
        {
            return self.accept_folded_cast_int(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(CastOpcode::Trunc, value.slot());
        payload.nuw.set(flags.nuw);
        payload.nsw.set(flags.nsw);
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Produce `zext <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateZExt`.
    ///
    /// The `Dst: WiderThan<Src>` bound enforces at compile time that
    /// the destination is strictly wider than the source. Use
    /// [`Self::zext_dyn`] when both widths are erased.
    pub fn zext<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<IntValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: IntWidth,
        Dst: WiderThan<Src>,
        V: IntoIntValue<'ctx, Src, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) =
            self.folder
                .fold_cast_to_int(CastOpcode::Zext, value.as_erased(), dst_ty)?
        {
            return self.accept_folded_cast_int(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(CastOpcode::Zext, value.slot());
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// `zext nneg` with explicit [`crate::ZextFlags`]. Mirrors
    /// `IRBuilder::CreateZExt` plus `Instruction::setNonNeg`.
    ///
    /// The `Dst: WiderThan<Src>` bound is the same one [`Self::zext`]
    /// uses, enforced at compile time. Use [`Self::zext_with_flags_dyn`]
    /// when both widths are erased.
    pub fn zext_with_flags<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, Dst, B>,
        flags: ZextFlags,
        name: Name,
    ) -> IrResult<IntValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: IntWidth,
        Dst: WiderThan<Src>,
        V: IntoIntValue<'ctx, Src, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) =
            self.folder
                .fold_cast_to_int(CastOpcode::Zext, value.as_erased(), dst_ty)?
        {
            return self.accept_folded_cast_int(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(CastOpcode::Zext, value.slot());
        payload.nneg.set(flags.nneg);
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Produce `sext <value> to <dst_ty>`. Mirrors
    /// `IRBuilder::CreateSExt`.
    ///
    /// The `Dst: WiderThan<Src>` bound enforces at compile time that
    /// the destination is strictly wider than the source. Use
    /// [`Self::sext_dyn`] when both widths are erased.
    pub fn sext<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<IntValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: IntWidth,
        Dst: WiderThan<Src>,
        V: IntoIntValue<'ctx, Src, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) =
            self.folder
                .fold_cast_to_int(CastOpcode::Sext, value.as_erased(), dst_ty)?
        {
            return self.accept_folded_cast_int(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(CastOpcode::Sext, value.slot());
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    // ---- Dyn fallbacks (runtime-checked) ----

    /// Runtime-checked `trunc` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly narrower than `value`'s runtime width.
    pub fn trunc_dyn<V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, IntDyn, B>,
        name: Name,
    ) -> IrResult<IntValueId<IntDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        let src_w = value.ty().bit_width();
        let dst_w = dst_ty.bit_width();
        if dst_w >= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(CastOpcode::Trunc, value.as_erased(), dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(IntValue::<IntDyn, B>::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(CastOpcode::Trunc, value.slot());
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// `trunc nuw/nsw` with explicit [`crate::TruncFlags`]. Runtime-checked
    /// like [`Self::trunc_dyn`]; additionally sets `nuw`/`nsw` on the
    /// cast payload.
    pub fn trunc_with_flags_dyn<V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, IntDyn, B>,
        flags: TruncFlags,
        name: Name,
    ) -> IrResult<IntValueId<IntDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        let src_w = value.ty().bit_width();
        let dst_w = dst_ty.bit_width();
        if dst_w >= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(CastOpcode::Trunc, value.as_erased(), dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(IntValue::<IntDyn, B>::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(CastOpcode::Trunc, value.slot());
        payload.nuw.set(flags.nuw);
        payload.nsw.set(flags.nsw);
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Runtime-checked `zext` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly wider than `value`'s runtime width.
    pub fn zext_dyn<V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, IntDyn, B>,
        name: Name,
    ) -> IrResult<IntValueId<IntDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        self.int_extend_dyn(value, dst_ty, name, CastOpcode::Zext)
            .map(|v| v.id())
    }

    /// `zext nneg` with explicit [`crate::ZextFlags`]. Runtime-checked
    /// like [`Self::zext_dyn`]; additionally sets `nneg` on the cast
    /// payload.
    pub fn zext_with_flags_dyn<V, Name>(
        &self,
        src: V,
        dst: IntType<'ctx, IntDyn, B>,
        flags: ZextFlags,
        name: Name,
    ) -> IrResult<IntValueId<IntDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        let src = src.into_int_value(ModuleRef::new(self.module))?;
        let src_w = src.ty().bit_width();
        let dst_w = dst.bit_width();
        if dst_w <= src_w {
            return Err(IrError::OperandWidthMismatch {
                lhs: src_w,
                rhs: dst_w,
            });
        }
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(CastOpcode::Zext, src.as_erased(), dst.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst.as_type().id())?;
            return Ok(IntValue::<IntDyn, B>::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(CastOpcode::Zext, src.slot());
        payload.nneg.set(flags.nneg);
        Ok(self
            .append_int_at(dst, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Runtime-checked `sext` for `IntValue<Dyn>` operands.
    /// Errors with [`IrError::OperandWidthMismatch`] if `dst_ty` is
    /// not strictly wider than `value`'s runtime width.
    pub fn sext_dyn<V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, IntDyn, B>,
        name: Name,
    ) -> IrResult<IntValueId<IntDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        self.int_extend_dyn(value, dst_ty, name, CastOpcode::Sext)
            .map(|v| v.id())
    }

    /// Crate-internal helper for `zext_dyn` / `sext_dyn`.
    fn int_extend_dyn<N>(
        &self,
        value: IntValue<'ctx, IntDyn, B>,
        dst_ty: IntType<'ctx, IntDyn, B>,
        name: N,
        opcode: CastOpcode,
    ) -> IrResult<IntValue<'ctx, IntDyn, B>>
    where
        N: AsRef<str>,
    {
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
                .fold_cast_dyn(opcode, value.as_erased(), dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(IntValue::<IntDyn, B>::from_value_unchecked(folded));
        }
        let payload = CastOpData::new(opcode, value.slot());
        Ok(self.append_int_at(dst_ty, InstructionKindData::Cast(payload), name))
    }

    // ---- Memory: alloca / load / store ----

    /// The DataLayout ABI alignment of a type, materialised so load/store
    /// carry an explicit `align` like upstream (`computeLoadStoreDefaultAlign`
    /// = `getABITypeAlign`).
    fn default_abi_align(&self, ty_id: TypeSlot) -> MaybeAlign {
        let dl = self.module.data_layout();
        MaybeAlign::new(dl.abi_align_of_id(self.module, ty_id))
    }

    /// The DataLayout preferred alignment of a type, materialised so alloca
    /// carries an explicit `align` like upstream (`computeAllocaDefaultAlign`
    /// = `getPrefTypeAlign`).
    fn default_pref_align(&self, ty_id: TypeSlot) -> MaybeAlign {
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
    /// the type's preferred alignment materialised, named by the storable
    /// [`PointerValueId<B>`](crate::PointerValueId).
    pub fn alloca<T, Name>(&self, ty: T, name: Name) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
    {
        self.alloca_inner(
            ty.as_type().id(),
            None,
            MaybeAlign::NONE,
            self.alloca_addr_space(),
            AllocaFlags::none(),
            name,
        )
    }

    /// Produce `alloca <ty>, <size-ty> <num_elements>`. Mirrors
    /// `IRBuilder::CreateAlloca` with an array-size operand.
    pub fn array_alloca<T, N, Name>(
        &self,
        ty: T,
        num_elements: N,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        N: IntoIntValue<'ctx, IntDyn, B>,
    {
        let n = num_elements.into_int_value(ModuleRef::new(self.module))?;
        self.alloca_inner(
            ty.as_type().id(),
            Some(n.slot()),
            MaybeAlign::NONE,
            self.alloca_addr_space(),
            AllocaFlags::none(),
            name,
        )
    }

    /// Produce `alloca <ty>, <size-ty> <num_elements>, align <N>`. The
    /// array-size form of `IRBuilder::CreateAlloca` with an explicit `Align`.
    pub fn array_alloca_with_align<T, N, Name>(
        &self,
        ty: T,
        num_elements: N,
        align: Align,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        N: IntoIntValue<'ctx, IntDyn, B>,
    {
        let n = num_elements.into_int_value(ModuleRef::new(self.module))?;
        self.alloca_inner(
            ty.as_type().id(),
            Some(n.slot()),
            MaybeAlign::new(align),
            self.alloca_addr_space(),
            AllocaFlags::none(),
            name,
        )
    }

    /// Produce `alloca <ty>, align <N>`. Mirrors
    /// `IRBuilder::CreateAlignedAlloca`.
    pub fn alloca_with_align<T, Name>(
        &self,
        ty: T,
        align: Align,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
    {
        self.alloca_inner(
            ty.as_type().id(),
            None,
            MaybeAlign::new(align),
            self.alloca_addr_space(),
            AllocaFlags::none(),
            name,
        )
    }

    fn alloca_inner<N>(
        &self,
        allocated_ty: TypeSlot,
        num_elements: Option<ValueSlot>,
        align: MaybeAlign,
        addr_space: u32,
        flags: AllocaFlags,
        name: N,
    ) -> IrResult<PointerValueId<B>>
    where
        N: AsRef<str>,
    {
        // Materialise the DataLayout preferred alignment when omitted, like
        // upstream — every alloca funnels through here
        // (`computeAllocaDefaultAlign`).
        let align = if align.align().is_none() {
            self.default_pref_align(allocated_ty)
        } else {
            align
        };
        let payload =
            AllocaInstData::new_with_flags(allocated_ty, num_elements, align, addr_space, flags);
        let ptr_ty = ModuleView::<B>::new(self.module).ptr_type(addr_space);
        Ok(self
            .append_ptr(ptr_ty, InstructionKindData::Alloca(payload), name)
            .id())
    }

    /// Builder-pattern `alloca` construction: array size, alignment,
    /// address space, and the `inalloca` / `swifterror` markers are
    /// orthogonal optional knobs, each spelled by its own chainable
    /// setter, emitted by [`AllocaBuilder::build`]. Mirrors
    /// `IRBuilder::CreateAlloca` with every optional slot of
    /// `AllocaInst` reachable; the DataLayout's alloca address space and
    /// preferred alignment are filled in unless overridden, exactly as the
    /// flat [`Self::alloca`] family does. Used by the parser, which
    /// reconstructs any spelled combination.
    ///
    /// ```
    /// use llvmkit_ir::{Align, Dyn, IrBuilder, IrError, Linkage, module_new};
    ///
    /// let m = module_new!("allocas")?;
    /// let i32_ty = m.i32_type();
    /// let fn_ty = m.function_type(m.void_type(), [i32_ty.as_type()]);
    /// let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    /// let entry = m.view(f).append_basic_block(&m, "entry");
    /// let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    /// let n: llvmkit_ir::IntValue<'_, llvmkit_ir::IntDyn, _> = m.view(f).param(0)?.try_into()?;
    ///
    /// let buf = b
    ///     .alloca_builder(i32_ty)
    ///     .array(n)
    ///     .align(Align::new(16)?)
    ///     .name("buf")
    ///     .build()?;
    /// b.ret_void()?;
    ///
    /// assert!(format!("{m}").contains("%buf = alloca i32, i32 %0, align 16\n"));
    /// # let _ = buf;
    /// # Ok::<(), IrError>(())
    /// ```
    pub fn alloca_builder<T>(&self, ty: T) -> AllocaBuilder<'_, 'm, 'ctx, B, F, R>
    where
        T: IrType<'ctx, B>,
    {
        AllocaBuilder {
            parent: self,
            allocated_ty: ty.as_type().id(),
            num_elements: Ok(None),
            align: MaybeAlign::NONE,
            addr_space: self.alloca_addr_space(),
            flags: AllocaFlags::none(),
            name: String::new(),
        }
    }

    /// `alloca` for schema `T`, returning a pointee-typed pointer. The
    /// pointee schema `T` is Rust-side bookkeeping only -- the emitted
    /// IR is identical to [`Self::alloca`] with `T::ir_type`.
    /// Mirrors `IRBuilder::CreateAlloca` + the Rust-side
    /// [`TypedPointerValue`] overlay.
    pub fn typed_alloca<T, Name>(&self, name: Name) -> IrResult<TypedPointerValue<'ctx, T, B>>
    where
        T: IrField,
        Name: AsRef<str>,
    {
        let ty = self.schema_ir_type::<T>()?;
        let ptr = self.view(self.alloca(ty, name)?);
        Ok(ptr.with_pointee::<T>())
    }

    /// Erased load: `load <ty>, ptr <ptr>`. Result type is whatever
    /// `ty` decodes to at runtime; named by the erased storable
    /// [`ValueId<B>`](crate::ValueId), which the caller narrows by viewing
    /// it (`try_view` / `view` + `try_into()`). Mirrors
    /// `IRBuilder::CreateLoad`.
    pub fn load<T, P, Name>(&self, ty: T, ptr: P, name: Name) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty_id = ty.as_type().id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty_id,
            p.slot(),
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.load_inner(payload, name)?;
        Ok(inst.to_erased().id())
    }

    /// `load <ty>, ptr <ptr>, align N`. Non-volatile non-atomic load with explicit
    /// alignment. Mirrors `IRBuilder::CreateLoad` with an explicit `Align` slot.
    pub fn load_with_align<T, P, Name>(
        &self,
        ty: T,
        ptr: P,
        align: Align,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty_id = ty.as_type().id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty_id,
            p.slot(),
            MaybeAlign::new(align),
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        let inst = self.load_inner(payload, name)?;
        Ok(inst.to_erased().id())
    }

    /// Typed integer load: `load iN, ptr <ptr>`. Marker-only form:
    /// the result type comes from `W` via [`crate::StaticIntWidth`].
    /// Mirrors `IRBuilder::CreateLoad` with a fixed integer width.
    pub fn int_load<W, P, Name>(&self, ptr: P, name: Name) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: StaticIntWidth,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = W::ir_type(ModuleRef::<B>::new(self.module));
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            p.slot(),
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        self.append_int_load(ty, payload, name).map(|v| v.id())
    }

    /// Runtime-width integer load. Takes the type explicitly because
    /// the [`crate::IntDyn`] marker carries no static width.
    pub fn int_load_dyn<P, Name>(
        &self,
        ty: IntType<'ctx, IntDyn, B>,
        ptr: P,
        name: Name,
    ) -> IrResult<IntValueId<IntDyn, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            p.slot(),
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        self.append_int_load(ty, payload, name).map(|v| v.id())
    }

    /// Typed float load: `load <fpty>, ptr <ptr>`. Marker-only.
    pub fn fp_load<K, P, Name>(&self, ptr: P, name: Name) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        K: StaticFloatKind,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = K::ir_type(ModuleRef::<B>::new(self.module));
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            p.slot(),
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        self.append_fp_load(ty, payload, name).map(|v| v.id())
    }

    /// Runtime-kind float load. Takes the type explicitly because
    /// [`crate::FloatDyn`] carries no static kind.
    pub fn fp_load_dyn<P, Name>(
        &self,
        ty: FloatType<'ctx, FloatDyn, B>,
        ptr: P,
        name: Name,
    ) -> IrResult<FloatValueId<FloatDyn, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            p.slot(),
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        self.append_fp_load(ty, payload, name).map(|v| v.id())
    }

    /// Pointer-typed load: `load ptr, ptr <ptr>`. Pointer types are
    /// uniform (only address space varies); the loaded ptr is in the
    /// default address space. Use [`Self::load`] erased form for
    /// other address spaces.
    pub fn pointer_load<P, Name>(&self, ptr: P, name: Name) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = ModuleView::<B>::new(self.module).ptr_type(0);
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            p.slot(),
            MaybeAlign::NONE,
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        self.append_ptr_load(ty, payload, name).map(|v| v.id())
    }

    /// Same as [`Self::int_load`] plus an explicit alignment.
    pub fn int_load_with_align<W, P, Name>(
        &self,
        ptr: P,
        align: Align,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: StaticIntWidth,
        P: IntoPointerValue<'ctx, B>,
    {
        let ty = W::ir_type(ModuleRef::<B>::new(self.module));
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let payload = LoadInstData::new(
            ty.as_type().id(),
            p.slot(),
            MaybeAlign::new(align),
            false,
            AtomicOrdering::NotAtomic,
            SyncScope::System,
        );
        self.append_int_load(ty, payload, name).map(|v| v.id())
    }

    /// Typed `load`: the result type is derived from the pointer's
    /// schema `T`. Mirrors `IRBuilder::CreateLoad` + the Rust-side
    /// [`TypedPointerValue`] overlay.
    pub fn typed_load<T, Name>(
        &self,
        ptr: TypedPointerValue<'ctx, T, B>,
        name: Name,
    ) -> IrResult<T::Value<'ctx, B>>
    where
        T: IrField,
        Name: AsRef<str>,
    {
        let ty = self.schema_ir_type::<T>()?;
        let raw = self.view(self.load(ty, ptr.as_pointer_value(), name)?);
        T::value_from_ir_value(raw)
    }

    /// Same as [`Self::typed_load`] plus an explicit alignment.
    pub fn typed_load_with_align<T, Name>(
        &self,
        ptr: TypedPointerValue<'ctx, T, B>,
        align: Align,
        name: Name,
    ) -> IrResult<T::Value<'ctx, B>>
    where
        T: IrField,
        Name: AsRef<str>,
    {
        let ty = self.schema_ir_type::<T>()?;
        let raw = self.view(self.load_with_align(ty, ptr.as_pointer_value(), align, name)?);
        T::value_from_ir_value(raw)
    }

    fn load_inner<N>(
        &self,
        mut payload: LoadInstData,
        name: N,
    ) -> IrResult<Instruction<'ctx, Attached, B>>
    where
        N: AsRef<str>,
    {
        let pointee_ty = payload.pointee_ty;
        // Materialise the DataLayout default like upstream — every load
        // (plain / volatile / atomic) funnels through here, so an omitted
        // alignment is filled once (`computeLoadStoreDefaultAlign`).
        if payload.align.align().is_none() {
            payload.align = self.default_abi_align(pointee_ty);
        }
        Ok(self.append_instruction(pointee_ty, InstructionKindData::Load(payload), name))
    }

    /// Builder-pattern `load` construction: `volatile`, alignment, atomic
    /// ordering, and sync scope are orthogonal optional knobs, each
    /// spelled by its own chainable setter; a typed terminal
    /// ([`LoadBuilder::int`] / [`LoadBuilder::fp`] /
    /// [`LoadBuilder::pointer`] / [`LoadBuilder::typed`] /
    /// [`LoadBuilder::erased`]) picks the result shape and emits the
    /// instruction. Mirrors `IRBuilder::CreateAlignedLoad` and the 5-arg
    /// `LoadInst::LoadInst(Type*, Value*, const Twine&, bool isVolatile,
    /// Align, AtomicOrdering, SyncScope::ID)` constructor
    /// (`lib/IR/Instructions.cpp`) — the single spelling for a volatile
    /// and/or atomic load. The flat [`Self::load`] / [`Self::int_load`]
    /// family remains for the plain non-volatile non-atomic case.
    ///
    /// ```
    /// use llvmkit_ir::{Align, AtomicOrdering, Dyn, IrBuilder, IrError, Linkage, module_new};
    ///
    /// let m = module_new!("loads")?;
    /// let ptr_ty = m.ptr_type(0);
    /// let fn_ty = m.function_type(m.void_type(), [ptr_ty.as_type()]);
    /// let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    /// let entry = m.view(f).append_basic_block(&m, "entry");
    /// let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    /// let p: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    ///
    /// // Every terminal picks the result shape; the knobs are shared.
    /// let n = b
    ///     .load_from(p)
    ///     .volatile()
    ///     .atomic(AtomicOrdering::Acquire)
    ///     .align(Align::new(4)?)
    ///     .int::<i32>("n")?;
    /// let x = b.load_from(p).fp::<f32>("x")?;
    /// let q = b.load_from(p).pointer("q")?;
    /// let s = b.load_from(p).typed::<i32>("s")?;
    /// let e = b.load_from(p).align(Align::new(8)?).erased(m.i64_type(), "e")?;
    /// b.ret_void()?;
    ///
    /// let text = format!("{m}");
    /// assert!(text.contains("%n = load atomic volatile i32, ptr %0 acquire, align 4\n"));
    /// assert!(text.contains("%x = load float, ptr %0, align 4\n"));
    /// assert!(text.contains("%q = load ptr, ptr %0, align 8\n"));
    /// assert!(text.contains("%s = load i32, ptr %0, align 4\n"));
    /// assert!(text.contains("%e = load i64, ptr %0, align 8\n"));
    /// # let _ = (n, x, q, s, e);
    /// # Ok::<(), IrError>(())
    /// ```
    pub fn load_from<P>(&self, ptr: P) -> LoadBuilder<'_, 'm, 'ctx, B, F, R>
    where
        P: IntoPointerValue<'ctx, B>,
    {
        LoadBuilder {
            parent: self,
            ptr: ptr
                .into_pointer_value(ModuleRef::new(self.module))
                .map(|p| p.slot()),
            align: MaybeAlign::NONE,
            volatile: false,
            ordering: AtomicOrdering::NotAtomic,
            sync_scope: SyncScope::System,
        }
    }

    /// Produce `store <value>, ptr <ptr>`. Mirrors
    /// `IRBuilder::CreateStore`.
    pub fn store<V, P>(&self, value: V, ptr: P) -> IrResult<StoreInst<'ctx, B>>
    where
        V: IntoErasedValue<'ctx, B>,
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
        self.store_inner(payload)
    }

    /// Same as `store` plus an explicit alignment slot.
    pub fn store_with_align<V, P>(
        &self,
        value: V,
        ptr: P,
        align: Align,
    ) -> IrResult<StoreInst<'ctx, B>>
    where
        V: IntoErasedValue<'ctx, B>,
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
        self.store_inner(payload)
    }

    /// Typed `store`: the value lifts through the schema's
    /// [`IntoIrField`]. Mirrors `IRBuilder::CreateStore` + the
    /// Rust-side [`TypedPointerValue`] overlay.
    pub fn typed_store<T, V>(
        &self,
        value: V,
        ptr: TypedPointerValue<'ctx, T, B>,
    ) -> IrResult<StoreInst<'ctx, B>>
    where
        T: IrField,
        V: IntoIrField<'ctx, T, B>,
    {
        let v = value.into_ir_field(ModuleRef::new(self.module))?;
        self.store(v, ptr.as_pointer_value())
    }

    /// Same as [`Self::typed_store`] plus an explicit alignment slot.
    pub fn typed_store_with_align<T, V>(
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
        self.store_with_align(v, ptr.as_pointer_value(), align)
    }

    /// Builder-pattern `store` construction: `volatile`, alignment,
    /// atomic ordering, and sync scope are orthogonal optional knobs,
    /// each spelled by its own chainable setter, emitted by
    /// [`StoreBuilder::build`]. Mirrors `IRBuilder::CreateAlignedStore`
    /// and the 6-arg `StoreInst::StoreInst(Value*, Value*, bool
    /// isVolatile, Align, AtomicOrdering, SyncScope::ID)` constructor
    /// (`lib/IR/Instructions.cpp`) — the single spelling for a volatile
    /// and/or atomic store. The flat [`Self::store`] /
    /// [`Self::store_with_align`] pair remains for the plain
    /// non-volatile non-atomic case.
    ///
    /// ```
    /// use llvmkit_ir::{
    ///     Align, AtomicOrdering, Dyn, IrBuilder, IrError, Linkage, SyncScope, module_new,
    /// };
    ///
    /// let m = module_new!("stores")?;
    /// let i32_ty = m.i32_type();
    /// let ptr_ty = m.ptr_type(0);
    /// let fn_ty = m.function_type(m.void_type(), [ptr_ty.as_type()]);
    /// let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    /// let entry = m.view(f).append_basic_block(&m, "entry");
    /// let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    /// let p: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    ///
    /// b.store_to(i32_ty.const_int(7_i32), p)
    ///     .volatile()
    ///     .atomic(AtomicOrdering::Release)
    ///     .sync_scope(SyncScope::SingleThread)
    ///     .align(Align::new(4)?)
    ///     .build()?;
    /// b.ret_void()?;
    ///
    /// assert!(format!("{m}").contains(
    ///     "store atomic volatile i32 7, ptr %0 syncscope(\"singlethread\") release, align 4\n"
    /// ));
    /// # Ok::<(), IrError>(())
    /// ```
    pub fn store_to<V, P>(&self, value: V, ptr: P) -> StoreBuilder<'_, 'm, 'ctx, B, F, R>
    where
        V: IntoErasedValue<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let module = ModuleRef::new(self.module);
        StoreBuilder {
            parent: self,
            operands: value
                .into_erased_value(module)
                .and_then(|v| ptr.into_pointer_value(module).map(|p| (v, p.slot()))),
            align: MaybeAlign::NONE,
            volatile: false,
            ordering: AtomicOrdering::NotAtomic,
            sync_scope: SyncScope::System,
        }
    }

    /// Inner store: caller has already computed the payload and validated
    /// the pointer/value modules. Single-arg helper used by the four
    /// public store builders.
    fn store_inner(&self, payload: StoreInstData) -> IrResult<StoreInst<'ctx, B>> {
        let void_ty = self.module.void_type::<B>().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Store(payload), "");
        Ok(StoreInst::from_raw(
            inst.slot(),
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
        V: IntoErasedValue<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
    {
        let v = value.into_erased_value(ModuleRef::new(self.module))?;
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        Ok(self.store_payload_lifted(v, p.slot(), align, volatile, ordering, sync_scope))
    }

    /// Payload assembly for operands that are already lifted and
    /// module-checked (the [`StoreBuilder`] path, which lifts at
    /// [`Self::store_to`] so the chain can stay infallible).
    fn store_payload_lifted(
        &self,
        value: Value<'ctx, B>,
        ptr: ValueSlot,
        align: MaybeAlign,
        volatile: bool,
        ordering: AtomicOrdering,
        sync_scope: SyncScope,
    ) -> StoreInstData {
        // Materialise the DataLayout default off the stored value's type,
        // like upstream (`computeLoadStoreDefaultAlign` /
        // `getABITypeAlign(Val->getType())`). Every store funnels through
        // here, so an omitted alignment is filled once.
        let align = if align.align().is_none() {
            self.default_abi_align(value.ty().id())
        } else {
            align
        };
        StoreInstData::new(value.id, ptr, align, volatile, ordering, sync_scope)
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
        args: &[ValueSlot],
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
    ///
    /// Returns the storable [`TypedCallInstId<Ret, B>`](crate::TypedCallInstId);
    /// view it to reach [`TypedCallInst::result`](crate::TypedCallInst::result).
    pub fn call<Ret, Params, A, Callee, Name>(
        &self,
        callee: Callee,
        args: A,
        name: Name,
    ) -> IrResult<TypedCallInstId<Ret, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
        Callee: IntoTypedCallee<'ctx, Ret, Params, B>,
        Name: AsRef<str>,
    {
        let f = callee
            .into_typed_callee(ModuleRef::new(self.module))?
            .as_function();
        let arg_ids = args.lower(ModuleRef::new(self.module))?;
        let payload = CallInstData::new(
            f.slot(),
            f.signature().as_type().id(),
            arg_ids,
            f.calling_conv(),
            TailCallKind::None,
        );
        let inst = self.append_instruction(
            f.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(TypedCallInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Typed flat call with explicit call-site configuration
    /// (calling convention / attributes), otherwise identical to
    /// [`Self::call`].
    pub fn call_with_config<Ret, Params, A, Callee>(
        &self,
        callee: Callee,
        args: A,
        config: CallSiteConfig,
    ) -> IrResult<TypedCallInstId<Ret, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
        Callee: IntoTypedCallee<'ctx, Ret, Params, B>,
    {
        let f = callee
            .into_typed_callee(ModuleRef::new(self.module))?
            .as_function();
        let arg_ids = args.lower(ModuleRef::new(self.module))?;
        let (name, calling_conv, attrs) = config.into_parts();
        let payload = CallInstData::new_with_attrs(
            f.slot(),
            f.signature().as_type().id(),
            arg_ids,
            calling_conv,
            TailCallKind::None,
            attrs,
        );
        let inst = self.append_instruction(
            f.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(TypedCallInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Typed chainable call builder: same schema guarantees as
    /// [`Self::call`], with `tail()` / `must_tail()` / `no_tail()`
    /// / `calling_conv(cc)` / `call_attributes(attrs)` / `name(n)`
    /// accumulated before `.build()` emits the call.
    pub fn typed_call_builder<Ret, Params, A, Callee>(
        &self,
        callee: Callee,
        args: A,
    ) -> TypedCallBuilder<'_, 'm, 'ctx, B, F, R, Ret, Params, A>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
        Callee: IntoTypedCallee<'ctx, Ret, Params, B>,
    {
        TypedCallBuilder {
            parent: self,
            callee: callee.into_typed_callee(ModuleRef::new(self.module)),
            args,
            tail_kind: TailCallKind::None,
            calling_conv: None,
            attrs: CallAttributeData::default(),
            name: String::new(),
        }
    }

    /// TYPED varargs call: the fixed-prefix arguments are schema-typed
    /// through `Params` exactly like [`Self::call`]; the trailing
    /// `varargs` are erased [`IntoErasedValue`] operands, matching LLVM's own
    /// variadic-argument contract (the `...` tail carries no static
    /// type checking — only the fixed prefix does). Mirrors
    /// `IRBuilder::CreateCall` against a variadic `FunctionCallee`.
    pub fn varargs_call<Ret, Params, A, I, V, Callee, Name>(
        &self,
        callee: Callee,
        fixed_args: A,
        varargs: I,
        name: Name,
    ) -> IrResult<TypedCallInstId<Ret, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        A: CallArgs<'ctx, Params, B>,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Callee: IntoVarArgsCallee<'ctx, Ret, Params, B>,
        Name: AsRef<str>,
    {
        let f = callee
            .into_varargs_callee(ModuleRef::new(self.module))?
            .as_function();
        let mut arg_ids: Vec<ValueSlot> = fixed_args.lower(ModuleRef::new(self.module))?.into_vec();
        for v in varargs {
            arg_ids.push(v.into_erased_value(ModuleRef::new(self.module))?.slot());
        }
        let payload = CallInstData::new(
            f.slot(),
            f.signature().as_type().id(),
            arg_ids,
            f.calling_conv(),
            TailCallKind::None,
        );
        let inst = self.append_instruction(
            f.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(TypedCallInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Flat call form: pass a [`FunctionValue`] callee, an iterable of
    /// pre-widened arguments (each one already a [`Value<'ctx, B>`]), and
    /// a name. Mirrors the simple shape of `IRBuilder::CreateCall`.
    /// Use [`Self::call_builder`] for mixed-arg-type construction.
    ///
    /// Returns the storable [`CallInstId<R2, B>`](crate::CallInstId); view it
    /// to reach the marker-gated `return_int_value` / `return_float_value` /
    /// `return_pointer_value` accessors.
    pub fn call_dyn<R2, I, V, Callee, Name>(
        &self,
        callee: Callee,
        args: I,
        name: Name,
    ) -> IrResult<CallInstId<R2, B>>
    where
        Name: AsRef<str>,
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Callee: IntoCallee<'ctx, R2, B>,
    {
        let callee = callee.into_callee(ModuleRef::<B>::new(self.module))?;
        let mut builder = self.call_builder(callee).name(name.as_ref());
        for arg in args {
            builder = builder.arg(arg);
        }
        builder.build()
    }

    /// Flat descriptor-backed intrinsic-call form.
    pub fn intrinsic_call<Name>(
        &self,
        descriptor: &IntrinsicDescriptor<'ctx, B>,
        args: &[Value<'ctx, B>],
        name: Name,
    ) -> IrResult<IntrinsicInstId<Dyn, B>>
    where
        Name: AsRef<str>,
    {
        let mut builder = self.intrinsic_call_builder(descriptor)?.name(name.as_ref());
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
    pub fn intrinsic_call_by_id<I, V, IntrinsicName, ResultName>(
        &self,
        id: IntrinsicId,
        intrinsic_name: IntrinsicName,
        args: I,
        result_name: ResultName,
    ) -> IrResult<IntrinsicInstId<Dyn, B>>
    where
        IntrinsicName: AsRef<str>,
        ResultName: AsRef<str>,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
    {
        let mut builder = self
            .intrinsic_call_builder_by_id(id, intrinsic_name)?
            .name(result_name.as_ref());
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
            callee_id: callee.slot(),
            fn_ty: callee.signature().as_type().id(),
            return_ty: callee.return_type().id(),
            args: Vec::new(),
            calling_conv: callee.calling_conv(),
            tail_kind: TailCallKind::None,
            attrs: CallAttributeData::default(),
            name: String::new(),
            intrinsic_descriptor: None,
            arg_error: None,
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
    /// Spell as: `b.indirect_call::<fn(i32) -> i32, _, _, _>(fp, (x,), "r")?`.
    ///
    /// No runtime argument-count/type check is needed: `fn_ty` is
    /// constructed from `Sig::Params` in this same call, and
    /// `A: CallArgs<'ctx, Sig::Params, B>` already proves `args` lowers
    /// to that identical schema — the underlying function pointer's
    /// *actual* pointee type is an indirect-call trust boundary LLVM
    /// itself does not statically check either (mirrors
    /// `IRBuilder::CreateCall`'s own opaque-pointer contract).
    pub fn indirect_call<Sig, A, Callee, Name>(
        &self,
        callee: Callee,
        args: A,
        name: Name,
    ) -> IrResult<TypedCallInstId<Sig::Ret, B>>
    where
        Sig: FunctionSignature,
        A: CallArgs<'ctx, Sig::Params, B>,
        Name: AsRef<str>,
        Callee: IntoPointerValue<'ctx, B>,
    {
        let callee = callee.into_pointer_value(ModuleRef::new(self.module))?;
        let module = self.schema_view();
        let ret = <Sig::Ret as FunctionReturn>::ir_type(module)?;
        let params = <Sig::Params as FunctionParamList>::ir_types(module)?;
        let fn_ty = module.function_type(ret, params);
        let callee_v = IsValue::as_erased(callee);
        let arg_ids = args.lower(ModuleRef::new(self.module))?;
        let payload = CallInstData::new(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            crate::CallingConv::C,
            TailCallKind::None,
        );
        let inst = self.append_instruction(
            fn_ty.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(TypedCallInstId::from_raw(self.module.id(), inst.slot()))
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
    pub fn indirect_call_dyn<R2, I, V, Callee, Name>(
        &self,
        fn_ty: FunctionType<'ctx, B>,
        callee: Callee,
        args: I,
        name: Name,
    ) -> IrResult<CallInstId<R2, B>>
    where
        Name: AsRef<str>,
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Callee: IntoPointerValue<'ctx, B>,
    {
        let callee = callee.into_pointer_value(ModuleRef::new(self.module))?;
        let callee_v = IsValue::as_erased(callee);
        let ret_data = self.module.context().type_data(fn_ty.return_type().id());
        if !crate::function::signature_matches_marker::<R2>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: crate::marker::marker_kind_label::<R2>()
                    .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
                got: fn_ty.return_type().kind_label(),
            });
        }
        let mut arg_ids: Vec<ValueSlot> = Vec::new();
        for arg in args {
            let v = arg.into_erased_value(ModuleRef::new(self.module))?;
            arg_ids.push(v.id);
        }
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let payload = CallInstData::new(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids.into_boxed_slice(),
            crate::CallingConv::C,
            TailCallKind::None,
        );
        let inst = self.append_instruction(
            fn_ty.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(CallInstId::from_raw(self.module.id(), inst.slot()))
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
    pub fn inline_asm_call<R2, I, V, Name>(
        &self,
        asm: InlineAsm<'ctx, B>,
        args: I,
        name: Name,
    ) -> IrResult<CallInstId<R2, B>>
    where
        Name: AsRef<str>,
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
    {
        let asm_v = asm.as_erased();
        let fn_ty = asm.function_type();
        // Reject a return-marker / signature mismatch up front, mirroring
        // the `signature_matches_marker` gate on the typed lookup path
        // (`Module::function`).
        let ret_data = self.module.context().type_data(fn_ty.return_type().id());
        if !crate::function::signature_matches_marker::<R2>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: crate::marker::marker_kind_label::<R2>()
                    .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
                got: fn_ty.return_type().kind_label(),
            });
        }
        let mut arg_ids: Vec<ValueSlot> = Vec::new();
        for arg in args {
            let v = arg.into_erased_value(ModuleRef::new(self.module))?;
            arg_ids.push(v.id);
        }
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let payload = CallInstData::new_with_attrs(
            asm_v.id,
            fn_ty.as_type().id(),
            arg_ids.into_boxed_slice(),
            crate::CallingConv::C,
            TailCallKind::None,
            CallAttributeData::default(),
        );
        let inst = self.append_instruction(
            fn_ty.return_type().id(),
            InstructionKindData::Call(payload),
            name,
        );
        Ok(CallInstId::from_raw(self.module.id(), inst.slot()))
    }

    // ---- GEP ----

    /// Produce `getelementptr <source-ty>, ptr <ptr>, <indices>`.
    /// Mirrors `IRBuilder::CreateGEP`.
    pub fn gep<T, P, I, V, Name>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
        I: IntoIterator<Item = V>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        self.gep_inner(source_ty, ptr, indices, GepNoWrapFlags::empty(), name)
    }

    /// Produce `getelementptr inbounds <source-ty>, ptr <ptr>,
    /// <indices>`. Mirrors `IRBuilder::CreateInBoundsGEP`.
    pub fn inbounds_gep<T, P, I, V, Name>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
        I: IntoIterator<Item = V>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        self.gep_inner(source_ty, ptr, indices, GepNoWrapFlags::inbounds(), name)
    }

    /// Produce `getelementptr inbounds nuw <struct-ty>, ptr <ptr>,
    /// i32 0, i32 <field-idx>`. Mirrors `IRBuilder::CreateStructGEP`
    /// (`IRBuilder.h`), which passes `GEPNoWrapFlags::inBounds() |
    /// GEPNoWrapFlags::noUnsignedWrap()` -- a struct-field offset can
    /// never wrap the pointer's index-width arithmetic, so upstream
    /// asserts `nuw` in addition to `inbounds`.
    pub fn struct_gep<P, Name>(
        &self,
        struct_ty: StructType<'ctx, StructBodyDyn, B>,
        ptr: P,
        idx: u32,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
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
        self.gep_inner(
            struct_ty,
            ptr,
            [zero, idx_val],
            GepNoWrapFlags::inbounds() | GepNoWrapFlags::NUW,
            name,
        )
    }

    /// `getelementptr inbounds %S, ptr %p, i32 0, i32 I` with the field
    /// type projected at compile time from the [`StructSchema`]. An
    /// out-of-range `I` fails to compile (no [`StructFieldAt<I>`] impl).
    /// Mirrors `IRBuilder::CreateStructGEP` + the Rust-side
    /// [`TypedPointerValue`] overlay.
    pub fn field_gep<S, const I: u32, Name>(
        &self,
        ptr: TypedPointerValue<'ctx, S, B>,
        name: Name,
    ) -> IrResult<TypedPointerValue<'ctx, FieldOf<S, I>, B>>
    where
        S: StructSchema,
        S::FieldParams: StructFieldAt<I>,
        Name: AsRef<str>,
    {
        let struct_ty = self.schema_struct_type::<S>()?;
        let raw = self.view(self.struct_gep(struct_ty, ptr.as_pointer_value(), I, name)?);
        Ok(raw.with_pointee::<FieldOf<S, I>>())
    }

    /// `getelementptr T, ptr %p, <idx>` -- element-stride arithmetic;
    /// the pointee schema is preserved. Mirrors the 1-index
    /// `IRBuilder::CreateGEP` + the Rust-side [`TypedPointerValue`]
    /// overlay.
    pub fn element_gep<T, W, Idx, Name>(
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
        let elem_ty = self.schema_ir_type::<T>()?;
        let idx_value = index.into_int_value(ModuleRef::new(self.module))?;
        let raw = self.view(self.gep(
            elem_ty,
            ptr.as_pointer_value(),
            core::iter::once(idx_value.as_dyn()),
            name,
        )?);
        Ok(raw.with_pointee::<T>())
    }

    /// `getelementptr inbounds T, ptr %p, <idx>`. Mirrors the 1-index
    /// `IRBuilder::CreateInBoundsGEP` + the Rust-side
    /// [`TypedPointerValue`] overlay.
    pub fn inbounds_element_gep<T, W, Idx, Name>(
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
        let elem_ty = self.schema_ir_type::<T>()?;
        let idx_value = index.into_int_value(ModuleRef::new(self.module))?;
        let raw = self.view(self.inbounds_gep(
            elem_ty,
            ptr.as_pointer_value(),
            core::iter::once(idx_value.as_dyn()),
            name,
        )?);
        Ok(raw.with_pointee::<T>())
    }

    /// `getelementptr` with explicit [`crate::GepNoWrapFlags`]. Use this
    /// when the parser has decoded `inbounds`, `nuw`, or `nusw` flags directly.
    /// Mirrors `IRBuilder::CreateGEP` with the full flags bitfield.
    pub fn gep_with_flags<T, P, I, V, Name>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        flags: GepNoWrapFlags,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
        I: IntoIterator<Item = V>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        self.gep_inner(source_ty, ptr, indices, flags, name)
    }

    fn gep_inner<T, P, I, V, N>(
        &self,
        source_ty: T,
        ptr: P,
        indices: I,
        flags: GepNoWrapFlags,
        name: N,
    ) -> IrResult<PointerValueId<B>>
    where
        T: IrType<'ctx, B>,
        P: IntoPointerValue<'ctx, B>,
        I: IntoIterator<Item = V>,
        V: IntoIntValue<'ctx, IntDyn, B>,
        N: AsRef<str>,
    {
        let source_ty = source_ty.as_type();
        let source_ty_id = source_ty.id();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let ptr_value = IsValue::as_erased(p);
        let mut idx_ids = Vec::new();
        let mut idx_values = Vec::new();
        for index in indices {
            let iv = index.into_int_value(ModuleRef::new(self.module))?;
            idx_values.push(iv.as_erased());
            idx_ids.push(iv.slot());
        }
        // Reject index sequences that do not index into the source element
        // type (`GetElementPtrInst::getIndexedType`) — the parser's
        // build-time gate mirroring upstream's parse-time rejection (D10).
        if crate::constants::gep_indexed_type(self.module, source_ty_id, &idx_ids).is_none() {
            return Err(IrError::GepInvalidIndices);
        }
        // Mirrors `GetElementPtrInst::getGEPReturnType` (`IR/Instructions.h`):
        // for the scalar (non-vector-of-pointers) case the result type is
        // exactly the base pointer's type, i.e. it lives in the SAME address
        // space as `ptr`, not always address space 0.
        let result_ptr_ty = ModuleView::<B>::new(self.module).ptr_type(p.ty().address_space());
        let result_ty = result_ptr_ty.as_type().id();
        if let Some(folded) = self
            .folder
            .fold_gep_dyn(source_ty, ptr_value, &idx_values, flags)?
        {
            let folded = self.checked_folded_value(folded, result_ty)?;
            return Ok(PointerValue::from_value_unchecked(folded).id());
        }
        let payload = GepInstData::new(
            source_ty_id,
            ptr_value.id,
            idx_ids.into_boxed_slice(),
            flags,
        );
        Ok(self
            .append_ptr(result_ptr_ty, InstructionKindData::Gep(payload), name)
            .id())
    }

    // ---- Floating-point casts ----

    /// Produce `fpext <value> to <dst>`. Compile-time check:
    /// `Dst: FloatWiderThan<Src>`. Mirrors `IRBuilder::CreateFPExt`.
    pub fn fp_ext<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<FloatValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: FloatKind,
        Dst: FloatKind + FloatWiderThan<Src>,
        V: IntoFloatValue<'ctx, Src, B>,
    {
        let value = value.into_float_value(ModuleRef::new(self.module))?;
        self.fp_cast(value, dst_ty, name, CastOpcode::FpExt)
            .map(|v| v.id())
    }

    /// Produce `fptrunc <value> to <dst>`. Compile-time check:
    /// `Src: FloatWiderThan<Dst>`. Mirrors `IRBuilder::CreateFPTrunc`.
    pub fn fp_trunc<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<FloatValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: FloatKind + FloatWiderThan<Dst>,
        Dst: FloatKind,
        V: IntoFloatValue<'ctx, Src, B>,
    {
        let value = value.into_float_value(ModuleRef::new(self.module))?;
        self.fp_cast(value, dst_ty, name, CastOpcode::FpTrunc)
            .map(|v| v.id())
    }

    /// Runtime-kind `fptrunc`. Mirrors [`Self::fp_trunc`] but
    /// accepts dynamically-typed operands so the parser can call it
    /// without static `FloatWiderThan` bounds.
    ///
    /// No compile-time width ordering check is performed; the LLVM
    /// verifier will reject `fptrunc` where `src` is not strictly wider
    /// than `dst`.
    pub fn fp_trunc_dyn<V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, FloatDyn, B>,
        name: Name,
    ) -> IrResult<FloatValueId<FloatDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoFloatValue<'ctx, FloatDyn, B>,
    {
        self.fp_trunc_dyn_with_fmf(value, dst_ty, FastMathFlags::empty(), name)
    }

    /// [`Self::fp_trunc_dyn`] carrying fast-math flags.
    ///
    /// `fptrunc` and `fpext` are the two cast opcodes that are
    /// `FPMathOperator`s, which is why `LLParser::parseInstruction` eats
    /// flags for exactly those two keywords before dispatching to
    /// `parseCast`. No result-type guard is needed here: the destination is
    /// a [`FloatType`] by construction.
    ///
    /// Flags are lost if the cast folds to a constant, as upstream.
    pub fn fp_trunc_dyn_with_fmf<V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, FloatDyn, B>,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValueId<FloatDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoFloatValue<'ctx, FloatDyn, B>,
    {
        let value = value.into_float_value(ModuleRef::new(self.module))?;
        let v = IsValue::as_erased(value);
        if let Some(folded) = self
            .folder
            .fold_cast_dyn(CastOpcode::FpTrunc, v, dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(CastOpcode::FpTrunc, v.id);
        payload.fmf.set(fmf);
        Ok(self
            .append_fp_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Runtime-kind `fpext`. Mirrors [`Self::fp_ext`] but accepts
    /// dynamically-typed operands so the parser can call it without
    /// static `FloatWiderThan` bounds.
    ///
    /// No compile-time width ordering check is performed; the LLVM
    /// verifier will reject `fpext` where `dst` is not strictly wider
    /// than `src`.
    pub fn fp_ext_dyn<V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, FloatDyn, B>,
        name: Name,
    ) -> IrResult<FloatValueId<FloatDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoFloatValue<'ctx, FloatDyn, B>,
    {
        self.fp_ext_dyn_with_fmf(value, dst_ty, FastMathFlags::empty(), name)
    }

    /// [`Self::fp_ext_dyn`] carrying fast-math flags.
    ///
    /// `fptrunc` and `fpext` are the two cast opcodes that are
    /// `FPMathOperator`s, which is why `LLParser::parseInstruction` eats
    /// flags for exactly those two keywords before dispatching to
    /// `parseCast`. No result-type guard is needed here: the destination is
    /// a [`FloatType`] by construction.
    ///
    /// Flags are lost if the cast folds to a constant, as upstream.
    pub fn fp_ext_dyn_with_fmf<V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, FloatDyn, B>,
        fmf: FastMathFlags,
        name: Name,
    ) -> IrResult<FloatValueId<FloatDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoFloatValue<'ctx, FloatDyn, B>,
    {
        let value = value.into_float_value(ModuleRef::new(self.module))?;
        let v = IsValue::as_erased(value);
        if let Some(folded) = self
            .folder
            .fold_cast_dyn(CastOpcode::FpExt, v, dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(CastOpcode::FpExt, v.id);
        payload.fmf.set(fmf);
        Ok(self
            .append_fp_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Crate-internal helper for `fp_ext` / `fp_trunc`.
    fn fp_cast<Src, Dst, N>(
        &self,
        value: FloatValue<'ctx, Src, B>,
        dst_ty: FloatType<'ctx, Dst, B>,
        name: N,
        opcode: CastOpcode,
    ) -> IrResult<FloatValue<'ctx, Dst, B>>
    where
        Src: FloatKind,
        Dst: FloatKind,
        N: AsRef<str>,
    {
        let v = IsValue::as_erased(value);
        if let Some(folded) = self.folder.fold_cast_to_fp(opcode, v, dst_ty)? {
            return self.accept_folded_cast_fp(folded, dst_ty);
        }
        let payload = CastOpData::new(opcode, v.id);
        Ok(self.append_fp_at(dst_ty, InstructionKindData::Cast(payload), name))
    }

    /// Produce `fptoui <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateFPToUI`.
    pub fn fp_to_ui<K, W, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, W, B>,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        W: IntWidth,
        V: IntoFloatValue<'ctx, K, B>,
    {
        let value = value.into_float_value(ModuleRef::new(self.module))?;
        self.fp_to_int(value, dst_ty, name, CastOpcode::FpToUi)
            .map(|v| v.id())
    }

    /// Produce `fptosi <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateFPToSI`.
    pub fn fp_to_si<K, W, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, W, B>,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        K: FloatKind,
        W: IntWidth,
        V: IntoFloatValue<'ctx, K, B>,
    {
        let value = value.into_float_value(ModuleRef::new(self.module))?;
        self.fp_to_int(value, dst_ty, name, CastOpcode::FpToSi)
            .map(|v| v.id())
    }

    fn fp_to_int<K, W, N>(
        &self,
        value: FloatValue<'ctx, K, B>,
        dst_ty: IntType<'ctx, W, B>,
        name: N,
        opcode: CastOpcode,
    ) -> IrResult<IntValue<'ctx, W, B>>
    where
        K: FloatKind,
        W: IntWidth,
        N: AsRef<str>,
    {
        let v = IsValue::as_erased(value);
        if let Some(folded) = self.folder.fold_cast_to_int(opcode, v, dst_ty)? {
            return self.accept_folded_cast_int(folded, dst_ty);
        }
        let payload = CastOpData::new(opcode, v.id);
        Ok(self.append_int_at(dst_ty, InstructionKindData::Cast(payload), name))
    }

    /// Produce `uitofp <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateUIToFP`.
    pub fn ui_to_fp<W, K, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, K, B>,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        K: FloatKind,
        V: IntoIntValue<'ctx, W, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        self.int_to_fp(value, dst_ty, name, CastOpcode::UiToFp)
            .map(|v| v.id())
    }

    /// `uitofp nneg` with explicit [`crate::UiToFpFlags`]. Mirrors
    /// `IRBuilder::CreateUIToFP` plus `Instruction::setNonNeg`. The `nneg`
    /// flag asserts the source value is non-negative.
    pub fn ui_to_fp_with_flags<W, K, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, K, B>,
        flags: UiToFpFlags,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        K: FloatKind,
        V: IntoIntValue<'ctx, W, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        let v = value.as_erased();
        if let Some(folded) = self.folder.fold_cast_to_fp(CastOpcode::UiToFp, v, dst_ty)? {
            return self.accept_folded_cast_fp(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(CastOpcode::UiToFp, v.id);
        payload.nneg.set(flags.nneg);
        Ok(self
            .append_fp_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Produce `sitofp <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateSIToFP`.
    pub fn si_to_fp<W, K, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, K, B>,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        K: FloatKind,
        V: IntoIntValue<'ctx, W, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        self.int_to_fp(value, dst_ty, name, CastOpcode::SiToFp)
            .map(|v| v.id())
    }

    /// `uitofp nneg` with explicit [`crate::UiToFpFlags`]. The `nneg` flag
    /// asserts the source value is non-negative. Both source and destination
    /// types are erased (dyn variants).
    pub fn ui_to_fp_with_flags_dyn<V, Name>(
        &self,
        src: V,
        dst: FloatType<'ctx, FloatDyn, B>,
        flags: UiToFpFlags,
        name: Name,
    ) -> IrResult<FloatValueId<FloatDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoIntValue<'ctx, IntDyn, B>,
    {
        let src = src.into_int_value(ModuleRef::new(self.module))?;
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(CastOpcode::UiToFp, src.as_erased(), dst.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst.as_type().id())?;
            return Ok(FloatValue::<FloatDyn, B>::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(CastOpcode::UiToFp, src.slot());
        payload.nneg.set(flags.nneg);
        Ok(self
            .append_fp_at(dst, InstructionKindData::Cast(payload), name)
            .id())
    }

    fn int_to_fp<W, K, N>(
        &self,
        value: IntValue<'ctx, W, B>,
        dst_ty: FloatType<'ctx, K, B>,
        name: N,
        opcode: CastOpcode,
    ) -> IrResult<FloatValue<'ctx, K, B>>
    where
        W: IntWidth,
        K: FloatKind,
        N: AsRef<str>,
    {
        let v = value.as_erased();
        if let Some(folded) = self.folder.fold_cast_to_fp(opcode, v, dst_ty)? {
            return self.accept_folded_cast_fp(folded, dst_ty);
        }
        let payload = CastOpData::new(opcode, v.id);
        Ok(self.append_fp_at(dst_ty, InstructionKindData::Cast(payload), name))
    }

    // ---- Pointer casts ----

    /// Produce `ptrtoaddr <value> to <address type>`. Mirrors
    /// `IRBuilder::CreatePtrToAddr`, using the module
    /// [`DataLayout`](crate::DataLayout) address type for the pointer
    /// operand's address space.
    pub fn ptr_to_addr<P, Name>(&self, value: P, name: Name) -> IrResult<IntValueId<IntDyn, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ptr = value.into_pointer_value(ModuleRef::new(self.module))?;
        let value = ptr.as_erased();
        let dst_ty = self.ptr_to_addr_result_type(value.ty())?;
        let result = self.ptr_to_addr_dyn(value, dst_ty, name)?;
        Ok(IntValue::<IntDyn, B>::from_value_unchecked(self.view(result)).id())
    }

    /// Runtime-typed `ptrtoaddr`. Accepts either a scalar pointer or a
    /// pointer vector and requires `dst_ty` to be the DataLayout address type
    /// for the source address space (index width, preserving vector shape).
    /// Mirrors `DataLayout::getAddressType(V->getType())`.
    pub fn ptr_to_addr_dyn<V, Name>(
        &self,
        value: V,
        dst_ty: Type<'ctx, B>,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
    {
        let value = value.into_erased_value(ModuleRef::new(self.module))?;
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
            return self
                .checked_folded_value(folded, dst_ty.id())
                .map(|v| v.id());
        }
        let payload = CastOpData::new(CastOpcode::PtrToAddr, value.id);
        let inst = self.append_instruction(dst_ty.id(), InstructionKindData::Cast(payload), name);
        Ok(inst.to_erased().id())
    }

    /// Produce `ptrtoint <value> to <dst>`. Mirrors
    /// `IRBuilder::CreatePtrToInt`.
    pub fn ptr_to_int<W, P, Name>(
        &self,
        value: P,
        dst_ty: IntType<'ctx, W, B>,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        P: IntoPointerValue<'ctx, B>,
    {
        let value = value.into_pointer_value(ModuleRef::new(self.module))?;
        let v = IsValue::as_erased(value);
        if let Some(folded) = self
            .folder
            .fold_cast_to_int(CastOpcode::PtrToInt, v, dst_ty)?
        {
            return self.accept_folded_cast_int(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(CastOpcode::PtrToInt, v.id);
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Produce `inttoptr <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateIntToPtr`.
    pub fn int_to_ptr<W, V, Name>(
        &self,
        value: V,
        dst_ty: PointerType<'ctx, B>,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        V: IntoIntValue<'ctx, W, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        let v = value.as_erased();
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(CastOpcode::IntToPtr, v, dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(PointerValue::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(CastOpcode::IntToPtr, v.id);
        Ok(self
            .append_ptr(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Generic bitcast on values of equal bit width. Mirrors
    /// `IRBuilder::CreateBitCast` (`IRBuilder.h`), which is itself
    /// `CreateCast(Instruction::BitCast, V, DestTy)`. The width
    /// equality is enforced statically through
    /// [`super::int_width::StaticIntWidth::STATIC_BITS`] /
    /// [`super::float_kind::StaticFloatKind::STATIC_BITS`]
    /// `const { assert!(...) }` blocks at monomorphisation; under-spec'd
    /// instantiations are *compile* errors.
    pub fn bitcast_int_to_int<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<IntValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: super::int_width::StaticIntWidth,
        Dst: super::int_width::StaticIntWidth,
        V: IntoIntValue<'ctx, Src, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        const {
            assert!(
                <Src as super::int_width::StaticIntWidth>::STATIC_BITS
                    == <Dst as super::int_width::StaticIntWidth>::STATIC_BITS,
                "bitcast int->int requires Src::STATIC_BITS == Dst::STATIC_BITS",
            );
        }
        let v_value = value.as_erased();
        if let Some(folded) = self.folder.fold_cast_to_int(
            super::instr_types::CastOpcode::BitCast,
            v_value,
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, v_value.id);
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Bitcast an integer value to a same-bit-width float. Mirrors the
    /// `Instruction::BitCast` arm of `CastInst::Create` in
    /// `lib/IR/Instructions.cpp` for the `int -> fp` shape. Width
    /// equality is enforced statically.
    pub fn bitcast_int_to_fp<W, K, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, K, B>,
        name: Name,
    ) -> IrResult<FloatValueId<K, B>>
    where
        Name: AsRef<str>,
        W: super::int_width::StaticIntWidth,
        K: super::float_kind::StaticFloatKind,
        V: IntoIntValue<'ctx, W, B>,
    {
        let value = value.into_int_value(ModuleRef::new(self.module))?;
        const {
            assert!(
                <W as super::int_width::StaticIntWidth>::STATIC_BITS
                    == <K as super::float_kind::StaticFloatKind>::STATIC_BITS,
                "bitcast int->fp requires W::STATIC_BITS == K::STATIC_BITS",
            );
        }
        let v_value = value.as_erased();
        if let Some(folded) =
            self.folder
                .fold_cast_to_fp(super::instr_types::CastOpcode::BitCast, v_value, dst_ty)?
        {
            return self.accept_folded_cast_fp(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, v_value.id);
        Ok(self
            .append_fp_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Bitcast a float to a same-bit-width integer. Mirrors the
    /// `Instruction::BitCast` arm of `CastInst::Create` in
    /// `lib/IR/Instructions.cpp` for the `fp -> int` shape. Width
    /// equality is enforced statically.
    pub fn bitcast_fp_to_int<K, W, V, Name>(
        &self,
        value: V,
        dst_ty: IntType<'ctx, W, B>,
        name: Name,
    ) -> IrResult<IntValueId<W, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::StaticFloatKind,
        W: super::int_width::StaticIntWidth,
        V: IntoFloatValue<'ctx, K, B>,
    {
        let value = value.into_float_value(ModuleRef::new(self.module))?;
        const {
            assert!(
                <K as super::float_kind::StaticFloatKind>::STATIC_BITS
                    == <W as super::int_width::StaticIntWidth>::STATIC_BITS,
                "bitcast fp->int requires K::STATIC_BITS == W::STATIC_BITS",
            );
        }
        let v_value = value.as_erased();
        if let Some(folded) = self.folder.fold_cast_to_int(
            super::instr_types::CastOpcode::BitCast,
            v_value,
            dst_ty,
        )? {
            return self.accept_folded_cast_int(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, v_value.id);
        Ok(self
            .append_int_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Bitcast a float to a same-bit-width float. Used for
    /// `bfloat <-> half` (both 16 bits) and `fp128 <-> ppc_fp128` (both
    /// 128 bits). Mirrors `Instruction::BitCast` in
    /// `lib/IR/Instructions.cpp`.
    pub fn bitcast_fp_to_fp<Src, Dst, V, Name>(
        &self,
        value: V,
        dst_ty: FloatType<'ctx, Dst, B>,
        name: Name,
    ) -> IrResult<FloatValueId<Dst, B>>
    where
        Name: AsRef<str>,
        Src: super::float_kind::StaticFloatKind,
        Dst: super::float_kind::StaticFloatKind,
        V: IntoFloatValue<'ctx, Src, B>,
    {
        let value = value.into_float_value(ModuleRef::new(self.module))?;
        const {
            assert!(
                <Src as super::float_kind::StaticFloatKind>::STATIC_BITS
                    == <Dst as super::float_kind::StaticFloatKind>::STATIC_BITS,
                "bitcast fp->fp requires Src::STATIC_BITS == Dst::STATIC_BITS",
            );
        }
        let v_value = value.as_erased();
        if let Some(folded) =
            self.folder
                .fold_cast_to_fp(super::instr_types::CastOpcode::BitCast, v_value, dst_ty)?
        {
            return self.accept_folded_cast_fp(folded, dst_ty).map(|v| v.id());
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, v_value.id);
        Ok(self
            .append_fp_at(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Runtime-typed bitcast: produce `bitcast <src> to <dst>` with both
    /// types erased to [`Type`]. The caller is responsible for
    /// ensuring `src` and `dst` have the same bit width; the LLVM verifier
    /// will reject ill-formed bitcasts.
    ///
    /// Used by the parser where compile-time static markers are unavailable.
    pub fn bitcast_dyn<V, Name>(
        &self,
        value: V,
        dst_ty: Type<'ctx, B>,
        name: Name,
    ) -> IrResult<ValueId<B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
    {
        let value = value.into_erased_value(ModuleRef::new(self.module))?;
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(super::instr_types::CastOpcode::BitCast, value, dst_ty)?
        {
            return self
                .checked_folded_value(folded, dst_ty.id())
                .map(|v| v.id());
        }
        let payload = CastOpData::new(super::instr_types::CastOpcode::BitCast, value.id);
        let inst = self.append_instruction(dst_ty.id(), InstructionKindData::Cast(payload), name);
        Ok(inst.to_erased().id())
    }

    /// Produce `addrspacecast <value> to <dst>`. Mirrors
    /// `IRBuilder::CreateAddrSpaceCast`.
    pub fn addrspace_cast<P, Name>(
        &self,
        value: P,
        dst_ty: PointerType<'ctx, B>,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let value = value.into_pointer_value(ModuleRef::new(self.module))?;
        let v = IsValue::as_erased(value);
        if let Some(folded) =
            self.folder
                .fold_cast_dyn(CastOpcode::AddrSpaceCast, v, dst_ty.as_type())?
        {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(PointerValue::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(CastOpcode::AddrSpaceCast, v.id);
        Ok(self
            .append_ptr(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// Pointer cast: pick `bitcast` for same-addrspace pointer-to-pointer
    /// (a no-op in opaque-pointer LLVM, but a structurally-distinct `Cast`
    /// instruction) and `addrspacecast` when address spaces differ.
    /// Mirrors `IRBuilder::CreatePointerBitCastOrAddrSpaceCast`
    /// (`IRBuilder.h`), which dispatches the same way.
    pub fn pointer_cast<P, Name>(
        &self,
        value: P,
        dst_ty: PointerType<'ctx, B>,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let value = value.into_pointer_value(ModuleRef::new(self.module))?;
        let v = IsValue::as_erased(value);
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
            return Ok(PointerValue::from_value_unchecked(folded).id());
        }
        if let Some(folded) = self.folder.fold_cast_dyn(opcode, v, dst_ty.as_type())? {
            let folded = self.checked_folded_value(folded, dst_ty.as_type().id())?;
            return Ok(PointerValue::from_value_unchecked(folded).id());
        }
        let payload = CastOpData::new(opcode, v.id);
        Ok(self
            .append_ptr(dst_ty, InstructionKindData::Cast(payload), name)
            .id())
    }

    /// `icmp eq <ptr>, null` -- pointer-null test. Mirrors
    /// `IRBuilder::CreateIsNull(Arg)` ->
    /// `CreateICmpEQ(Arg, Constant::getNullValue(Arg->getType()))`.
    pub fn is_null<P, Name>(&self, ptr: P, name: Name) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ptr = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        self.pointer_cmp(
            super::cmp_predicate::IntPredicate::Eq,
            ptr,
            ptr.ty().const_null(),
            name,
        )
    }

    /// `icmp ne <ptr>, null` -- pointer-non-null test. Mirrors
    /// `IRBuilder::CreateIsNotNull(Arg)` ->
    /// `CreateICmpNE(Arg, Constant::getNullValue(Arg->getType()))`.
    pub fn is_not_null<P, Name>(&self, ptr: P, name: Name) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
    {
        let ptr = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        self.pointer_cmp(
            super::cmp_predicate::IntPredicate::Ne,
            ptr,
            ptr.ty().const_null(),
            name,
        )
    }

    /// Pointer-pointer comparison. Mirrors `IRBuilder::CreateICmp` with
    /// pointer operands; LLVM's `icmp` works on integers OR pointers, but
    /// our typed [`Self::int_cmp`] is integer-only. This helper
    /// covers the pointer arm directly (used by `is_null` /
    /// `is_not_null`).
    pub fn pointer_cmp<L, R2, Name>(
        &self,
        pred: super::cmp_predicate::IntPredicate,
        lhs: L,
        rhs: R2,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        L: IntoPointerValue<'ctx, B>,
        R2: IntoPointerValue<'ctx, B>,
    {
        let lhs = lhs.into_pointer_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_pointer_value(ModuleRef::new(self.module))?;
        let folded = self.folder.fold_cmp_dyn(
            pred.into(),
            IsValue::as_erased(lhs),
            IsValue::as_erased(rhs),
        )?;
        if let Some(folded) = folder::narrow_folded_bool(folded)? {
            return Ok(folded.id());
        }
        let payload = super::instr_types::CmpInstData::new(pred, lhs.slot(), rhs.slot());
        let i1 = ModuleView::<B>::new(self.module).bool_type();
        Ok(self
            .append_int_at(i1, InstructionKindData::Icmp(payload), name)
            .id())
    }

    // ---- Vector splat / ptr arithmetic / aggregate ret convenience ----

    /// Broadcast `scalar` across a fixed-width vector of `count` lanes.
    /// Mirrors `IRBuilderBase::CreateVectorSplat(unsigned NumElts, Value*,
    /// const Twine&)` (`lib/IR/IRBuilder.cpp` line 1141), which expands to
    /// `insertelement <count x T> poison, <T> %v, i64 0` followed by
    /// `shufflevector ..., <count x T> poison, <count x i32> zeroinitializer`.
    /// The result is named `<name>.splat`; the intermediate insertelement
    /// is `<name>.splatinsert`.
    pub fn vector_splat_dyn<V, Name>(
        &self,
        count: u32,
        scalar: V,
        name: Name,
    ) -> IrResult<VectorValue<'ctx, ElemDyn, LenDyn, B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
    {
        if count == 0 {
            return Err(IrError::InvalidOperation {
                message: "vector_splat_dyn requires at least one lane",
            });
        }
        let scalar_value = scalar.into_erased_value(ModuleRef::new(self.module))?;
        let elem_ty = scalar_value.ty();
        let vec_ty = ModuleView::<B>::new(self.module).vector_type(elem_ty, count);
        let poison = vec_ty.as_type().poison();
        let i64_ty = ModuleView::<B>::new(self.module).i64_type();
        let zero_idx = i64_ty.const_int(0_u32);
        let name_ref = name.as_ref();
        let insert_name = if name_ref.is_empty() {
            String::from("splatinsert")
        } else {
            format!("{name_ref}.splatinsert")
        };
        let inserted =
            // Forward the already-erased scalar: `IntoErasedValue` consumes
            // its input (an id is not re-usable after resolution), and the
            // element slot erases it anyway.
            self.insert_element::<_, _, i64, _, _>(
                poison,
                scalar_value,
                zero_idx,
                insert_name,
            )?;
        let n = usize::try_from(count).map_err(|_| IrError::InvalidOperation {
            message: "vector splat lane count exceeds the platform address range",
        })?;
        // A splat selects lane 0 of the inserted vector for every result lane.
        let mask = vec![ShuffleMaskElem::Lane(0); n];
        let splat_name = if name_ref.is_empty() {
            String::from("splat")
        } else {
            format!("{name_ref}.splat")
        };
        let shuf = self.view(self.shuffle_vector(inserted, poison, &mask, splat_name)?);
        Ok(VectorValue::from_value_unchecked(shuf))
    }

    // ---- ptr_add / inbounds_ptr_add ----

    /// `getelementptr i8, ptr <ptr>, <offset>` -- byte-offset pointer
    /// arithmetic. Mirrors `IRBuilder::CreatePtrAdd` in `IRBuilder.h`
    /// (line 2039), which expands to `CreateGEP(getInt8Ty(), Ptr, Offset, ...)`.
    pub fn ptr_add<P, O, W, Name>(
        &self,
        ptr: P,
        offset: O,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
        W: super::int_width::IntWidth,
        O: IntoIntValue<'ctx, W, B>,
    {
        let i8_ty = ModuleView::<B>::new(self.module).i8_type();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let offset_v = offset.into_int_value(ModuleRef::new(self.module))?;
        self.gep(i8_ty, p, core::iter::once(offset_v.as_dyn()), name)
    }

    /// `getelementptr inbounds i8, ptr <ptr>, <offset>`. Mirrors
    /// `IRBuilder::CreateInBoundsPtrAdd` (`IRBuilder.h` line 2044), which
    /// expands to `CreateGEP(getInt8Ty(), Ptr, Offset, Name, GEPNoWrapFlags::inBounds())`.
    pub fn inbounds_ptr_add<P, O, W, Name>(
        &self,
        ptr: P,
        offset: O,
        name: Name,
    ) -> IrResult<PointerValueId<B>>
    where
        Name: AsRef<str>,
        P: IntoPointerValue<'ctx, B>,
        W: super::int_width::IntWidth,
        O: IntoIntValue<'ctx, W, B>,
    {
        let i8_ty = ModuleView::<B>::new(self.module).i8_type();
        let p = ptr.into_pointer_value(ModuleRef::new(self.module))?;
        let offset_v = offset.into_int_value(ModuleRef::new(self.module))?;
        self.inbounds_gep(i8_ty, p, core::iter::once(offset_v.as_dyn()), name)
    }

    // ---- Integer comparison ----
    //
    // `icmp` yields `i1`, so every builder in this family -- and the `fcmp`
    // family above, and `pointer_cmp` / `is_null` /
    // `is_not_null` -- returns the storable `IntValueId<bool, B>`.

    /// Produce `icmp <pred> <ty> <lhs>, <rhs>`. Mirrors
    /// `IRBuilder::CreateICmp`.
    ///
    /// Both operands share width `W` at the type level. The result
    /// type is always `i1`, named by an
    /// [`IntValueId<bool, B>`](crate::IntValueId).
    pub fn int_cmp<W, Lhs, Rhs, Name>(
        &self,
        pred: IntPredicate,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        let i1 = ModuleView::<B>::new(self.module).bool_type();
        if let Some(folded) = self.folder.fold_int_cmp(pred, lhs, rhs)? {
            return Ok(folded.id());
        }
        let payload = CmpInstData::new(pred, lhs.slot(), rhs.slot());
        Ok(self
            .append_int_at(i1, InstructionKindData::Icmp(payload), name)
            .id())
    }

    /// `icmp samesign` with explicit [`crate::IcmpFlags`]. Mirrors
    /// `IRBuilder::CreateICmp` plus `IcmpInst::setSameSign`. The `samesign`
    /// flag asserts both operands carry the same sign (LLVM 20+).
    ///
    /// Upstream sets `samesign` post-hoc via `IcmpInst::setSameSign`
    /// (`Instructions.h`) after construction; llvmkit's construction-time
    /// flag parameter is a deliberate Rust-side improvement -- the flag is
    /// part of the payload from the moment the instruction exists, so there
    /// is no window where an `IcmpInst` is live with a stale `samesign` bit.
    pub fn int_cmp_with_flags<W, Lhs, Rhs, Name>(
        &self,
        predicate: IntPredicate,
        lhs: Lhs,
        rhs: Rhs,
        flags: IcmpFlags,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        let i1 = ModuleView::<B>::new(self.module).bool_type();
        if let Some(folded) = self.folder.fold_int_cmp(predicate, lhs, rhs)? {
            return Ok(folded.id());
        }
        let mut payload = CmpInstData::new(predicate, lhs.slot(), rhs.slot());
        payload.samesign = flags.samesign;
        Ok(self
            .append_int_at(i1, InstructionKindData::Icmp(payload), name)
            .id())
    }

    /// `icmp samesign` with explicit [`crate::IcmpFlags`]. Both operands
    /// must be dynamically-typed (`IntDyn`). The `samesign` flag asserts
    /// both operands carry the same sign (LLVM 20+).
    pub fn int_cmp_with_flags_dyn<Lhs, Rhs, Name>(
        &self,
        pred: IntPredicate,
        lhs: Lhs,
        rhs: Rhs,
        flags: IcmpFlags,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        Lhs: IntoIntValue<'ctx, IntDyn, B>,
        Rhs: IntoIntValue<'ctx, IntDyn, B>,
    {
        let lhs = lhs.into_int_value(ModuleRef::new(self.module))?;
        let rhs = rhs.into_int_value(ModuleRef::new(self.module))?;
        let i1 = ModuleView::<B>::new(self.module).bool_type();
        if let Some(folded) = self.folder.fold_int_cmp(pred, lhs, rhs)? {
            return Ok(folded.id());
        }
        let mut payload = CmpInstData::new(pred, lhs.slot(), rhs.slot());
        payload.samesign = flags.samesign;
        Ok(self
            .append_int_at(i1, InstructionKindData::Icmp(payload), name)
            .id())
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
    pub fn icmp_eq<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Eq, lhs, rhs, name)
    }

    /// `icmp ne` -- not equal. Signedness-irrelevant. Mirrors
    /// `IRBuilder::CreateICmpNE`.
    #[inline]
    pub fn icmp_ne<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Ne, lhs, rhs, name)
    }

    /// `icmp ult` -- unsigned less than. Mirrors
    /// `IRBuilder::CreateICmpULT`.
    #[inline]
    pub fn icmp_ult<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Ult, lhs, rhs, name)
    }

    /// `icmp ule` -- unsigned less than or equal. Mirrors
    /// `IRBuilder::CreateICmpULE`.
    #[inline]
    pub fn icmp_ule<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Ule, lhs, rhs, name)
    }

    /// `icmp ugt` -- unsigned greater than. Mirrors
    /// `IRBuilder::CreateICmpUGT`.
    #[inline]
    pub fn icmp_ugt<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Ugt, lhs, rhs, name)
    }

    /// `icmp uge` -- unsigned greater than or equal. Mirrors
    /// `IRBuilder::CreateICmpUGE`.
    #[inline]
    pub fn icmp_uge<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Uge, lhs, rhs, name)
    }

    /// `icmp slt` -- signed less than. Mirrors
    /// `IRBuilder::CreateICmpSLT`.
    #[inline]
    pub fn icmp_slt<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Slt, lhs, rhs, name)
    }

    /// `icmp sle` -- signed less than or equal. Mirrors
    /// `IRBuilder::CreateICmpSLE`.
    #[inline]
    pub fn icmp_sle<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Sle, lhs, rhs, name)
    }

    /// `icmp sgt` -- signed greater than. Mirrors
    /// `IRBuilder::CreateICmpSGT`.
    #[inline]
    pub fn icmp_sgt<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Sgt, lhs, rhs, name)
    }

    /// `icmp sge` -- signed greater than or equal. Mirrors
    /// `IRBuilder::CreateICmpSGE`.
    #[inline]
    pub fn icmp_sge<W, Lhs, Rhs, Name>(
        &self,
        lhs: Lhs,
        rhs: Rhs,
        name: Name,
    ) -> IrResult<IntValueId<bool, B>>
    where
        Name: AsRef<str>,
        W: IntWidth,
        Lhs: IntoIntValue<'ctx, W, B>,
        Rhs: IntoIntValue<'ctx, W, B>,
    {
        self.int_cmp::<W, Lhs, Rhs, _>(IntPredicate::Sge, lhs, rhs, name)
    }

    // ---- Phi ----

    /// Produce `phi <ty>` with no initial incoming edges. Marker-only
    /// form: the result type comes from the `W` type parameter via
    /// [`crate::StaticIntWidth`], so callers spell it as
    /// `b.int_phi::<i32, _>("acc")?` without first binding
    /// `let i32_ty = m.i32_type();`. Mirrors `IRBuilder::CreatePHI`
    /// followed by zero `PHINode::addIncoming` calls. Returns the storable
    /// [`PhiInstId<W, B>`](crate::PhiInstId); view it
    /// ([`view`](Self::view)) to reach the typed phi surface, and add edges
    /// through [`crate::PhiInst::add_incoming`], which returns `Self` so calls
    /// chain. Inserted at the block's phi head regardless of cursor position,
    /// so phi placement is correct by construction.
    /// Crate-internal since slice 7 — block arguments
    /// (`append_block_with_params`) are the only public phi-authoring surface.
    /// No production caller today (parser/SSA use the `_dyn`/erased paths), so
    /// `dead_code` is allowed in non-test builds; the in-crate raw-phi tests
    /// exercise it.
    #[cfg(test)]
    pub(crate) fn int_phi<W, Name>(&self, name: Name) -> IrResult<PhiInstId<W, B>>
    where
        Name: AsRef<str>,
        W: StaticIntWidth,
    {
        let ty = W::ir_type(ModuleRef::<B>::new(self.module));
        let payload = PhiData::new();
        let inst =
            self.append_phi_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(PhiInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Runtime-width phi for the [`crate::IntDyn`] case. Takes the
    /// type explicitly because the marker carries no static width. Returns the
    /// storable [`PhiInstId<IntDyn, B>`](crate::PhiInstId).
    /// Inserted at the block's phi head regardless of cursor position, so
    /// phi placement is correct by construction.
    /// Internal contract shared with the in-tree `.ll` parser (hence
    /// `#[doc(hidden)]`); block arguments are the public phi-authoring surface.
    #[doc(hidden)]
    pub fn int_phi_dyn<Name>(
        &self,
        ty: IntType<'ctx, IntDyn, B>,
        name: Name,
    ) -> IrResult<PhiInstId<IntDyn, B>>
    where
        Name: AsRef<str>,
    {
        let payload = PhiData::new();
        let inst =
            self.append_phi_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(PhiInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Float-typed phi: `phi <fpty>`. Marker-only form keyed on
    /// `K: StaticFloatKind`. Mirrors `IRBuilder::CreatePHI(Type*, ...)`
    /// applied to a floating-point type. Returns the storable
    /// [`FpPhiInstId<K, B>`](crate::FpPhiInstId). Inserted at the block's phi
    /// head regardless of cursor position, so phi placement is correct by
    /// construction.
    /// Crate-internal since slice 7 — block arguments
    /// (`append_block_with_params`) are the only public phi-authoring surface.
    /// No production caller today (parser/SSA use the `_dyn`/erased paths), so
    /// `dead_code` is allowed in non-test builds; the in-crate raw-phi tests
    /// exercise it.
    #[cfg(test)]
    pub(crate) fn fp_phi<K, Name>(&self, name: Name) -> IrResult<FpPhiInstId<K, B>>
    where
        Name: AsRef<str>,
        K: super::float_kind::StaticFloatKind,
    {
        let ty = K::ir_type(ModuleRef::<B>::new(self.module));
        let payload = super::instr_types::PhiData::new();
        let inst =
            self.append_phi_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(FpPhiInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Runtime-kind float phi: takes the type explicitly because
    /// [`crate::FloatDyn`] carries no static kind. Returns the storable
    /// [`FpPhiInstId<FloatDyn, B>`](crate::FpPhiInstId). Inserted at the
    /// block's phi head regardless of cursor position, so phi placement is
    /// correct by construction.
    /// Internal contract shared with the in-tree `.ll` parser (hence
    /// `#[doc(hidden)]`); block arguments are the public phi-authoring surface.
    #[doc(hidden)]
    pub fn fp_phi_dyn<Name>(
        &self,
        ty: FloatType<'ctx, FloatDyn, B>,
        name: Name,
    ) -> IrResult<FpPhiInstId<FloatDyn, B>>
    where
        Name: AsRef<str>,
    {
        let payload = super::instr_types::PhiData::new();
        let inst =
            self.append_phi_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(FpPhiInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Pointer-typed phi in the default address space (addrspace 0).
    /// Mirrors `IRBuilder::CreatePHI(PointerType::getUnqual(...), ...)`.
    /// Returns the storable [`PointerPhiInstId<B>`](crate::PointerPhiInstId).
    /// Inserted at the block's phi head regardless of cursor position, so
    /// phi placement is correct by construction.
    /// Crate-internal since slice 7 — block arguments
    /// (`append_block_with_params`) are the only public phi-authoring surface.
    /// No production caller today (parser/SSA use the `_dyn`/erased paths), so
    /// `dead_code` is allowed in non-test builds; the in-crate raw-phi tests
    /// exercise it.
    #[cfg(test)]
    pub(crate) fn pointer_phi<Name>(&self, name: Name) -> IrResult<PointerPhiInstId<B>>
    where
        Name: AsRef<str>,
    {
        let ty = self.module.ptr_type::<B>(0);
        let payload = super::instr_types::PhiData::new();
        let inst =
            self.append_phi_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(PointerPhiInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Pointer-typed phi in a caller-specified address space. Mirrors
    /// `IRBuilder::CreatePHI(PointerType::get(Ctx, AS), ...)`. Returns the
    /// storable [`PointerPhiInstId<B>`](crate::PointerPhiInstId). Inserted at
    /// the block's phi head regardless of cursor position, so phi
    /// placement is correct by construction.
    /// Internal contract shared with the in-tree `.ll` parser (hence
    /// `#[doc(hidden)]`); block arguments are the public phi-authoring surface.
    #[doc(hidden)]
    pub fn pointer_phi_in_addrspace<Name>(
        &self,
        ty: PointerType<'ctx, B>,
        name: Name,
    ) -> IrResult<PointerPhiInstId<B>>
    where
        Name: AsRef<str>,
    {
        let payload = super::instr_types::PhiData::new();
        let inst =
            self.append_phi_instruction(ty.as_type().id(), InstructionKindData::Phi(payload), name);
        Ok(PointerPhiInstId::from_raw(self.module.id(), inst.slot()))
    }

    /// Runtime-typed phi for an *arbitrary* first-class result type — the
    /// vector / array / struct cases the int / float / pointer `_dyn`
    /// builders don't cover. Takes the [`Type`] explicitly (the erased
    /// handle carries no static shape) and yields the storable
    /// [`OtherPhiInstId<B>`](crate::OtherPhiInstId), whose view is the erased
    /// [`OtherPhiInst`] — the same classification
    /// [`PhiKind::Other`](crate::PhiKind) surfaces for such phis. Incoming
    /// edges are added through the type-checked
    /// [`phi_add_incoming_from_value`](Self::phi_add_incoming_from_value)
    /// path. Inserted at the block's phi head regardless of cursor position,
    /// so phi placement is correct by construction.
    ///
    /// # Precondition
    ///
    /// Unlike the typed `_dyn` builders, this takes a fully **erased**
    /// [`Type`] with no type-level first-class constraint, so it cannot
    /// reject a nonsense result type statically — the caller carries that
    /// obligation. `ty` **must** be a first-class *data* type: `int`,
    /// `float`, `pointer`, `vector`, `array`, or `struct`. `void`, function,
    /// and opaque-struct types are not valid phi result types; neither are
    /// the other first-class types `label`, `metadata`, and `token` (LLVM
    /// rejects e.g. `phi token`, and note
    /// [`Type::is_first_class`](crate::Type::is_first_class) returns `true`
    /// for those, so it is *not* a sufficient gate). The `.ll` parser
    /// enumerates the acceptable result types explicitly before routing here.
    ///
    /// This is an internal builder shared only with the in-tree `.ll` parser
    /// (hence `#[doc(hidden)]`); it is not part of the supported public
    /// surface and may change without notice.
    #[doc(hidden)]
    pub fn phi_dyn<Name>(&self, ty: Type<'ctx, B>, name: Name) -> IrResult<OtherPhiInstId<B>>
    where
        Name: AsRef<str>,
    {
        let payload = PhiData::new();
        let inst = self.append_phi_instruction(ty.id(), InstructionKindData::Phi(payload), name);
        Ok(OtherPhiInstId::from_raw(self.module.id(), inst.slot()))
    }

    // ---- Branch / Unreachable ----

    /// Resolve a branch target for an edge that carries **no** block
    /// arguments, rejecting a target created with block parameters.
    ///
    /// The plain terminator builders funnel their every successor through
    /// here, so "this edge seeds nothing, therefore its target must need
    /// nothing seeded" is checked once, in one place. Returns the resolved
    /// label so the caller can go straight on to emitting the terminator —
    /// the check runs *before* any instruction is appended, so a rejected
    /// edge leaves no half-formed terminator behind.
    ///
    /// See [`require_no_block_parameters`] for the cost story: an ordinary
    /// (param-less) target costs one `Cell` read on top of the label
    /// resolution the builder had to do anyway.
    fn plain_edge_target<T>(&self, target: T) -> IrResult<BasicBlockLabel<'ctx, R, B>>
    where
        T: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module_ref = ModuleRef::<B>::new(self.module);
        let target = target.into_basic_block_label(module_ref)?;
        require_no_block_parameters(module_ref, target.slot())?;
        Ok(target)
    }

    /// Produce `br label %target`. Mirrors `IRBuilder::CreateBr`.
    ///
    /// Consumes `self`: the builder's insertion block is terminated and
    /// returned alongside the new terminator instruction. The branch
    /// target may be in any termination state -- backward edges (loop
    /// back-edges) target already-terminated blocks.
    ///
    /// A plain `br` carries no block arguments, so `target` must not be a
    /// **parameterised** block — one created by
    /// [`append_block_with_params`](Self::append_block_with_params), its naming
    /// twin, or [`append_block_typed`](Self::append_block_typed). Such an edge
    /// would seed none of the target's parameters and leave an incomplete phi
    /// for a distant [`Module::verify`](crate::Module::verify) to find, so it is
    /// rejected here with [`IrError::PhiArgArityMismatch`] — the same error a
    /// wrong argument *count* gets from
    /// [`br_with_args`](Self::br_with_args), which is what to reach
    /// for instead. Blocks that merely *contain* phis (parsed `.ll`, auto-SSA,
    /// pass-created) are not parameterised and are unaffected: their incomings
    /// arrive through their own checked paths.
    ///
    /// The check runs before the terminator is emitted, so a rejected branch
    /// appends nothing. `self` is consumed either way, exactly as when the
    /// target fails to resolve ([`IrError::ForeignValueId`]) or as when
    /// `br_with_args` rejects an argument: the insertion block is left
    /// unterminated and a fresh builder must be positioned at it to retry.
    pub fn br<T>(self, target: T) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        T: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let target = self.plain_edge_target(target)?;
        self.br_seeded(target)
    }

    /// Emit `br label %target` with the parameterised-target guard already
    /// discharged: either the caller has just seeded `target`'s block
    /// parameters ([`br_with_args`](Self::br_with_args),
    /// [`br_call`](Self::br_call)) or it ran
    /// [`plain_edge_target`](Self::plain_edge_target) first
    /// ([`br`](Self::br)).
    fn br_seeded<T>(self, target: T) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        T: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let target = target.into_basic_block_label(ModuleRef::new(self.module))?;
        let payload = BranchInstData {
            kind: core::cell::RefCell::new(BranchKind::Unconditional(target.slot())),
        };
        let void_ty = self.module.void_type::<B>().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Br(payload), "");
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `br i1 <cond>, label %then, label %else`. Mirrors
    /// `IRBuilder::CreateCondBr`.
    ///
    /// Consumes `self`; both target blocks may be in any termination state.
    ///
    /// Neither arm carries block arguments, so neither target may be a
    /// **parameterised** block — both are checked, before the terminator is
    /// emitted, exactly as [`br`](Self::br) checks its single
    /// target and with the same [`IrError::PhiArgArityMismatch`]. Use
    /// [`cond_br_with_args`](Self::cond_br_with_args) when either
    /// successor has parameters; it takes a per-edge argument list, so one
    /// parameterised and one ordinary arm is spelled with an empty slice for
    /// the ordinary one.
    pub fn cond_br<C, Then, Else>(
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
        let then_bb = self.plain_edge_target(then_bb)?;
        let else_bb = self.plain_edge_target(else_bb)?;
        self.cond_br_seeded(cond, then_bb, else_bb)
    }

    /// Emit `br i1 <cond>, label %then, label %else` with the
    /// parameterised-target guard already discharged on both arms — the
    /// conditional twin of [`br_seeded`](Self::br_seeded).
    fn cond_br_seeded<C, Then, Else>(
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
        let then_bb = then_bb.into_basic_block_label(ModuleRef::new(self.module))?;
        let else_bb = else_bb.into_basic_block_label(ModuleRef::new(self.module))?;
        let cond = cond.into_int_value(ModuleRef::new(self.module))?;
        let payload = BranchInstData {
            kind: core::cell::RefCell::new(BranchKind::Conditional {
                cond: core::cell::Cell::new(cond.slot()),
                then_bb: then_bb.slot(),
                else_bb: else_bb.slot(),
            }),
        };
        let void_ty = self.module.void_type::<B>().as_type().id();
        let inst = self.append_instruction(void_ty, InstructionKindData::Br(payload), "");
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `br label %target` while carrying block arguments into
    /// `target`'s parameters. In the Swift-SIL / MLIR block-argument model a
    /// branch supplies the successor's parameters at the edge: `target`'s
    /// leading head-phis (created by
    /// [`append_block_with_params`](Self::append_block_with_params)) are its
    /// parameters, and `args[i]` becomes the incoming value for the `i`-th
    /// parameter from *this* block.
    ///
    /// The edge and the values move together, so a wrong argument fails here,
    /// not at a distant [`Module::verify`](crate::Module::verify): `args.len()`
    /// must equal `target`'s parameter count ([`IrError::PhiArgArityMismatch`]
    /// otherwise), and each argument's type must match its parameter
    /// ([`IrError::TypeMismatch`] otherwise). Both the arity and every argument
    /// type are checked up front — all-or-nothing for those two — before the
    /// `br` is emitted, so a mis-sized or mistyped argument leaves no
    /// half-formed terminator.
    ///
    /// Consumes `self`; the target may be in any termination state (a
    /// back-edge targets an already-terminated block).
    pub fn br_with_args<T>(
        self,
        target: T,
        args: &[Value<'ctx, B>],
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        T: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let target = target
            .into_basic_block_label(ModuleRef::new(self.module))?
            .id();
        // Capture the predecessor id before the terminator builder consumes
        // `self` — the incoming edges name *this* block as their predecessor.
        let pred = self.insert_block().id();
        self.add_block_args(target, pred, args)?;
        self.br_seeded(target)
    }

    /// Produce `br i1 <cond>, label %then, label %else` while carrying block
    /// arguments down each edge into the matching successor's parameters. Each
    /// edge supplies its own argument list to its own target, following the
    /// same Swift-SIL block-argument model as
    /// [`br_with_args`](Self::br_with_args): `then_args` seed
    /// `then_bb`'s parameter-phis and `else_args` seed `else_bb`'s, each with
    /// *this* block as the predecessor.
    ///
    /// Each edge's arity ([`IrError::PhiArgArityMismatch`]) and argument types
    /// ([`IrError::TypeMismatch`]) are checked up front — all-or-nothing for
    /// those two — before that edge's parameter-phis are seeded, so a mis-sized
    /// or mistyped argument fails here rather than at
    /// [`Module::verify`](crate::Module::verify). The edges are processed in
    /// order and this is *not* one atomic transaction across both: if
    /// `then_bb == else_bb` and the two argument lists differ, the `then`
    /// edge's incomings are already recorded when the `else` edge-add rejects
    /// the differing duplicate for the shared predecessor
    /// ([`IrError::AmbiguousPhiIncoming`]) — the `br` is never emitted and the
    /// consumed builder leaves the block unterminated for `verify()` to catch.
    ///
    /// Consumes `self`; both targets may be in any termination state.
    pub fn cond_br_with_args<C, Then, Else>(
        self,
        cond: C,
        then_bb: Then,
        then_args: &[Value<'ctx, B>],
        else_bb: Else,
        else_args: &[Value<'ctx, B>],
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        C: IntoIntValue<'ctx, bool, B>,
        Then: IntoBasicBlockLabel<'ctx, R, B>,
        Else: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let then_bb = then_bb
            .into_basic_block_label(ModuleRef::new(self.module))?
            .id();
        let else_bb = else_bb
            .into_basic_block_label(ModuleRef::new(self.module))?
            .id();
        let pred = self.insert_block().id();
        self.add_block_args(then_bb, pred, then_args)?;
        self.add_block_args(else_bb, pred, else_args)?;
        self.cond_br_seeded(cond, then_bb, else_bb)
    }

    /// Seed a target block's parameters with the values a branch carries into
    /// it. The parameters are the target's leading head-phis (in order); this
    /// arity-checks `args` against them and type-checks each `args[i]` against
    /// its parameter up front, then records each as an incoming edge from `pred`
    /// into the `i`-th parameter-phi via the type-checked erased path
    /// ([`phi_add_incoming_from_value`](Self::phi_add_incoming_from_value)).
    ///
    /// Arity and per-argument type are all-or-nothing: both are validated before
    /// any incoming is recorded, so a mis-sized or mistyped call leaves the
    /// target parameters untouched. The differing-value-duplicate rejection for
    /// `pred`, however, is *not* part of that up-front pass — it runs per-edge
    /// inside [`phi_add_incoming_from_value`](Self::phi_add_incoming_from_value)
    /// as each incoming is recorded, so it can fire after earlier incomings in
    /// the same call have already been written.
    ///
    /// Shared by [`br_with_args`](Self::br_with_args) and
    /// [`cond_br_with_args`](Self::cond_br_with_args); called
    /// before the terminator is emitted so a rejected argument aborts the
    /// branch cleanly.
    fn add_block_args(
        &self,
        target: BlockId<R, B>,
        pred: BlockId<R, B>,
        args: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        let module_ref = ModuleRef::<B>::new(self.module);
        let label_ty = self.module.label_type::<B>().as_type().id();
        let target = target.into_basic_block_label(module_ref)?;

        // The target block's parameters are its leading head-phis, in order —
        // the shared scan the plain-branch guard also reads, so the arity
        // enforced here and the arity a plain branch is rejected for are one
        // fact.
        let param_phis: Vec<ValueSlot> = block_parameter_phis(module_ref, target.slot());

        // Arity: exactly one argument per parameter. Caught here so a wrong
        // count fails at the branch rather than at a distant `verify()`.
        if param_phis.len() != args.len() {
            return Err(IrError::PhiArgArityMismatch {
                expected: param_phis.len(),
                got: args.len(),
            });
        }

        // Type-check every argument against its parameter-phi up front, before
        // recording any incoming. Recording mutates the phis through interior
        // mutability and cannot be rolled back, so validating arity and type
        // first makes *those two* all-or-nothing: a mis-sized or mistyped
        // argument leaves the target parameters untouched. (The
        // differing-value-duplicate check is not pre-scanned — it runs per-edge
        // in the record loop below, so it can fire after earlier incomings in
        // this call are already written.)
        for (phi_id, arg) in param_phis.iter().zip(args.iter()) {
            let phi_ty = self.module.context().value_data(*phi_id).ty;
            if arg.ty != phi_ty {
                return Err(IrError::TypeMismatch {
                    expected: Type::<B>::new(phi_ty, self.module).kind_label(),
                    got: Type::<B>::new(arg.ty, self.module).kind_label(),
                });
            }
        }

        // Record each argument as an incoming edge from `pred` into the
        // matching parameter-phi (types already validated above; the erased
        // path re-checks and registers the phi in each value's use-list).
        let pred_block =
            BasicBlock::<Dyn, Terminated, B>::from_parts(pred.slot(), module_ref, label_ty);
        for (phi_id, arg) in param_phis.iter().zip(args.iter()) {
            let phi_ty = self.module.context().value_data(*phi_id).ty;
            let phi_val = Value::from_parts(*phi_id, module_ref, phi_ty);
            self.phi_add_incoming_from_value(phi_val, *arg, pred_block.copy_handle())?;
        }
        Ok(())
    }

    /// Rebuild the erased [`Value`] handles a [`BlockCall`] lowered to
    /// value-ids, sourcing each id's IR type from the arena. Shared by the
    /// typed branch builders to feed the erased
    /// [`add_block_args`](Self::add_block_args) phi-seeding path.
    fn block_call_arg_values(&self, arg_ids: &[ValueSlot]) -> Vec<Value<'ctx, B>> {
        let module_ref = ModuleRef::<B>::new(self.module);
        arg_ids
            .iter()
            .map(|&id| {
                let ty = self.module.context().value_data(id).ty;
                Value::from_parts(id, module_ref, ty)
            })
            .collect()
    }

    /// Produce `br label %target` for a **typed** [`BlockCall`] edge, seeding
    /// `target`'s leading head-phis with the edge's block-arguments. The typed
    /// analog of [`br_with_args`](Self::br_with_args): the argument
    /// arity and per-position types were already fixed at *compile* time by the
    /// [`CallArgs<Params>`](crate::CallArgs) bound on
    /// [`BasicBlockLabel::call`](crate::BasicBlockLabel::call) when the
    /// [`BlockCall`] was built, so no mistyped or mis-sized edge can reach here.
    ///
    /// A distinct name from the label-taking [`br`](Self::br): both
    /// are inherent methods on the same builder type, so they cannot share the
    /// `br` name, and a [`BlockCall`] argument does not implement
    /// [`IntoBasicBlockLabel`] (it carries edge arguments the label form does
    /// not), so overloading by argument type is not possible either. The
    /// erased [`br`](Self::br) /
    /// [`br_with_args`](Self::br_with_args) are left unchanged.
    ///
    /// This still returns [`IrResult`]: the [`BlockCall`]'s eager argument
    /// lowering may have failed at a value level (the fallibility
    /// [`CallArgs::lower`](crate::CallArgs) carries), and that deferred error is
    /// surfaced here before the branch is emitted. Consumes `self`; the target
    /// may be in any termination state (a back-edge targets an already-terminated
    /// block).
    pub fn br_call<Params>(
        self,
        target: BlockCall<R, B, Params>,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Params: BlockParams + FunctionParamList,
    {
        let (target, lowered) = target.into_parts();
        let arg_ids = lowered?;
        let args = self.block_call_arg_values(&arg_ids);
        // Capture the predecessor label before the terminator builder consumes
        // `self` — the incoming edges name *this* block as their predecessor.
        let pred = self.insert_block().id();
        self.add_block_args(target, pred, &args)?;
        self.br_seeded(target)
    }

    /// Produce `br i1 <cond>, label %then, label %else` for two **typed**
    /// [`BlockCall`] edges, seeding each successor's leading head-phis with its
    /// own edge's block-arguments. The typed analog of
    /// [`cond_br_with_args`](Self::cond_br_with_args); each edge's
    /// arity and per-position types were fixed at *compile* time by the
    /// [`CallArgs<Params>`](crate::CallArgs) bound when its [`BlockCall`] was
    /// built. The two edges may carry different parameter schemas.
    ///
    /// Named `cond_br_call` for the same reason as
    /// [`br_call`](Self::br_call) — it cannot reuse the
    /// label-taking [`cond_br`](Self::cond_br) name. Any deferred
    /// value-level lowering error from either edge is surfaced here before the
    /// branch is emitted; the `then` edge is lowered and checked first.
    ///
    /// Consumes `self`; both targets may be in any termination state.
    pub fn cond_br_call<C, ThenP, ElseP>(
        self,
        cond: C,
        then_call: BlockCall<R, B, ThenP>,
        else_call: BlockCall<R, B, ElseP>,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        C: IntoIntValue<'ctx, bool, B>,
        ThenP: BlockParams + FunctionParamList,
        ElseP: BlockParams + FunctionParamList,
    {
        let (then_target, then_lowered) = then_call.into_parts();
        let (else_target, else_lowered) = else_call.into_parts();
        let then_ids = then_lowered?;
        let else_ids = else_lowered?;
        let then_args = self.block_call_arg_values(&then_ids);
        let else_args = self.block_call_arg_values(&else_ids);
        let pred = self.insert_block().id();
        self.add_block_args(then_target, pred, &then_args)?;
        self.add_block_args(else_target, pred, &else_args)?;
        self.cond_br_seeded(cond, then_target, else_target)
    }

    /// Produce a TYPED `switch <cond>, label <default> [...]` whose
    /// condition is a width-`W` integer. Mirrors `IRBuilder::CreateSwitch`
    /// with the case-value width statically pinned.
    ///
    /// The width `W` is inferred from a typed integer `cond` (e.g. an
    /// [`IntValue<'ctx, i32, B>`](crate::IntValue)); every case added
    /// through the returned [`Open`]-typestate [`SwitchInst`] must share
    /// that width `W`, so a wrong-width case (`IntValue<i64>` or a bare
    /// `i64` literal on a `W = i32` switch) is a *compile* error — there
    /// is no `IntoIntValue<'ctx, W, B>` impl for the mismatched value —
    /// rather than the runtime [`IrError::TypeMismatch`] the width-erased
    /// [`switch_dyn`](Self::switch_dyn) reports at
    /// [`SwitchInst::add_case`]. The caller still seals the case list with
    /// [`SwitchInst::finish`](SwitchInst::finish).
    ///
    /// Typed member of the pair, so it takes the unsuffixed name and its
    /// erased sibling carries the `_dyn` — as with
    /// [`call`](Self::call) / [`call_dyn`](Self::call_dyn)
    /// and [`invoke`](Self::invoke) /
    /// [`invoke_dyn`](Self::invoke_dyn).
    /// The default target carries no block arguments, so it must not be a
    /// **parameterised** block — the same guard, and the same
    /// [`IrError::PhiArgArityMismatch`], that
    /// [`br`](Self::br) applies. Every case target added through
    /// the returned handle is guarded identically by
    /// [`SwitchInst::add_case`]. Use
    /// [`switch_with_args`](Self::switch_with_args) when the
    /// default or any case reaches a parameterised block: it spells the whole
    /// case list at the call, each edge with its own arguments.
    pub fn switch<W, C, DefaultTarget, Name>(
        self,
        cond: C,
        default_target: DefaultTarget,
        name: Name,
    ) -> IrResult<TerminatedBlockSwitchTyped<'ctx, R, W, B>>
    where
        W: IntWidth,
        C: IntoIntValue<'ctx, W, B>,
        Name: AsRef<str>,
        DefaultTarget: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module_ref = ModuleRef::<B>::new(self.module);
        let cond_id = cond.into_int_value(module_ref)?.slot();
        let default_target = self.plain_edge_target(default_target)?;
        self.switch_seeded::<W, _, _>(cond_id, default_target, name)
    }

    /// Emit `switch <cond>, label <default> [...]` with the
    /// parameterised-target guard already discharged on the default edge:
    /// either the caller has just seeded it
    /// ([`switch_with_args`](Self::switch_with_args) and its erased
    /// twin) or it ran [`plain_edge_target`](Self::plain_edge_target) first.
    ///
    /// `W` is the condition width the returned [`Open`] handle carries; the
    /// erased builders instantiate it at [`IntDyn`], which is what makes this
    /// one body serve both flavours. The condition arrives pre-lowered as a
    /// [`ValueSlot`] because the typed and erased callers reach it through
    /// different bounds ([`IntoIntValue`] vs [`IntoErasedValue`]).
    fn switch_seeded<W, DefaultTarget, Name>(
        self,
        cond_id: ValueSlot,
        default_target: DefaultTarget,
        name: Name,
    ) -> IrResult<TerminatedBlockSwitchTyped<'ctx, R, W, B>>
    where
        W: IntWidth,
        Name: AsRef<str>,
        DefaultTarget: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module_ref = ModuleRef::<B>::new(self.module);
        let default_target = default_target.into_basic_block_label(module_ref)?;
        let void_ty = self.module.void_type::<B>().as_type().id();
        let payload = SwitchInstData::new(cond_id, default_target.slot());
        let inst = self.append_instruction(void_ty, InstructionKindData::Switch(payload), name);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            SwitchInst::<Open, B, W>::from_raw(inst.slot(), module_ref, void_ty),
        ))
    }

    /// Produce a TYPED `switch` whose default edge **and every case edge**
    /// carry block arguments into their target's parameters. The switch
    /// generalisation of
    /// [`cond_br_with_args`](Self::cond_br_with_args): where a
    /// `cond_br` has two edges each with its own argument list, a `switch` has
    /// a default plus N cases.
    ///
    /// **Every edge is one parameter, bundled with the values it carries** —
    /// `default` is a `(target, args)` pair and each entry of `cases` a
    /// `(case_value, target, args)` triple. The case list forces that shape
    /// (an iterator has to yield one item per case), and applying it to the
    /// default too keeps the whole call reading the same way. The frozen
    /// [`br_with_args`](Self::br_with_args) /
    /// [`cond_br_with_args`](Self::cond_br_with_args) keep their
    /// flat `target, args` parameter pairs; their edge count is fixed at one
    /// and two.
    ///
    /// The whole case list is spelled here rather than chained through
    /// [`SwitchInst::add_case`], because an edge and the values it carries have
    /// to move together: the returned [`SwitchInst`] is therefore already
    /// [`Closed`], and there is no way to bolt on a later case whose target's
    /// parameters nothing seeds.
    ///
    /// Each edge's arity ([`IrError::PhiArgArityMismatch`]) and argument types
    /// ([`IrError::TypeMismatch`]) are checked before that edge's
    /// parameter-phis are seeded, and every case value and target is lowered
    /// up front — so a malformed case fails before any incoming is recorded and
    /// before the terminator is emitted. As with
    /// [`cond_br_with_args`](Self::cond_br_with_args) this is *not*
    /// one atomic transaction across edges: two edges into the *same* target
    /// with differing arguments record the first and then reject the second
    /// ([`IrError::AmbiguousPhiIncoming`]), leaving the block unterminated for
    /// [`verify()`](crate::Module::verify) to catch. Two edges into the same
    /// target with the *same* arguments are legal — that is the ordinary
    /// multi-case-to-one-block shape.
    ///
    /// Typed member of the pair, so it takes the unsuffixed name and its erased
    /// sibling is
    /// [`switch_dyn_with_args`](Self::switch_dyn_with_args): the
    /// condition width `W` is inferred from `cond` and every case value shares
    /// it, so a wrong-width case is a *compile* error exactly as in
    /// [`switch`](Self::switch).
    ///
    /// Consumes `self`; every target may be in any termination state.
    pub fn switch_with_args<'args, W, C, DefaultTarget, Cases, CaseValue, CaseTarget, Name>(
        self,
        cond: C,
        default: (DefaultTarget, &'args [Value<'ctx, B>]),
        cases: Cases,
        name: Name,
    ) -> IrResult<TerminatedBlockSwitchTypedClosed<'ctx, R, W, B>>
    where
        'ctx: 'args,
        W: IntWidth,
        C: IntoIntValue<'ctx, W, B>,
        Name: AsRef<str>,
        DefaultTarget: IntoBasicBlockLabel<'ctx, R, B>,
        Cases: IntoIterator<Item = (CaseValue, CaseTarget, &'args [Value<'ctx, B>])>,
        CaseValue: IntoIntValue<'ctx, W, B>,
        CaseTarget: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module_ref = ModuleRef::<B>::new(self.module);
        let cond_id = cond.into_int_value(module_ref)?.slot();
        let lowered = cases
            .into_iter()
            .map(|(case_value, case_target, case_args)| {
                let value = IsValue::as_erased(case_value.into_int_value(module_ref)?);
                let target = case_target.into_basic_block_label(module_ref)?;
                Ok((value, target, case_args))
            })
            .collect::<IrResult<Vec<_>>>()?;
        self.switch_over_seeded_edges(cond_id, default, lowered, name)
    }

    /// Produce a width-ERASED `switch` whose default edge and every case edge
    /// carry block arguments. Erased sibling of
    /// [`switch_with_args`](Self::switch_with_args) — see it for
    /// the edge/argument contract, which is identical.
    ///
    /// `cond` and each case value are bound by [`IntoErasedValue`], so the
    /// condition's width is not pinned and each case value's width is checked
    /// against it at *runtime* ([`IrError::TypeMismatch`]), exactly as
    /// [`switch_dyn`](Self::switch_dyn) + [`SwitchInst::add_case`]
    /// do. Prefer the typed form where the width is statically known.
    pub fn switch_dyn_with_args<'args, C, DefaultTarget, Cases, CaseValue, CaseTarget, Name>(
        self,
        cond: C,
        default: (DefaultTarget, &'args [Value<'ctx, B>]),
        cases: Cases,
        name: Name,
    ) -> IrResult<TerminatedBlockSwitchClosed<'ctx, R, B>>
    where
        'ctx: 'args,
        C: IntoErasedValue<'ctx, B>,
        Name: AsRef<str>,
        DefaultTarget: IntoBasicBlockLabel<'ctx, R, B>,
        Cases: IntoIterator<Item = (CaseValue, CaseTarget, &'args [Value<'ctx, B>])>,
        CaseValue: IntoErasedValue<'ctx, B>,
        CaseTarget: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module_ref = ModuleRef::<B>::new(self.module);
        let cond_id = cond.into_erased_value(module_ref)?.id;
        let lowered = cases
            .into_iter()
            .map(|(case_value, case_target, case_args)| {
                let value = case_value.into_erased_value(module_ref)?;
                let target = case_target.into_basic_block_label(module_ref)?;
                Ok((value, target, case_args))
            })
            .collect::<IrResult<Vec<_>>>()?;
        self.switch_over_seeded_edges::<IntDyn, _, _>(cond_id, default, lowered, name)
    }

    /// Seed every edge of a `switch` and then emit it, cases included.
    ///
    /// Shared tail of [`switch_with_args`](Self::switch_with_args)
    /// and [`switch_dyn_with_args`](Self::switch_dyn_with_args),
    /// which differ only in how they lower the condition and the case values.
    /// The default edge is seeded first, then each case in order; the
    /// terminator is emitted only once every edge's arguments have been
    /// accepted, and the cases go on through the pre-seeded
    /// [`SwitchInst::push_case_seeded`] so the plain-branch guard does not
    /// reject the very targets this call just supplied arguments for.
    fn switch_over_seeded_edges<'args, W, DefaultTarget, Name>(
        self,
        cond_id: ValueSlot,
        default: (DefaultTarget, &'args [Value<'ctx, B>]),
        cases: Vec<LoweredSwitchCase<'ctx, 'args, R, B>>,
        name: Name,
    ) -> IrResult<TerminatedBlockSwitchTypedClosed<'ctx, R, W, B>>
    where
        'ctx: 'args,
        W: IntWidth,
        Name: AsRef<str>,
        DefaultTarget: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module_ref = ModuleRef::<B>::new(self.module);
        let (default_target, default_args) = default;
        let default_target = default_target.into_basic_block_label(module_ref)?;
        // Capture the predecessor id before the terminator builder consumes
        // `self` — every incoming this call records names *this* block.
        let pred = self.insert_block().id();
        self.add_block_args(default_target.id(), pred, default_args)?;
        for (_, target, args) in &cases {
            self.add_block_args(target.id(), pred, args)?;
        }
        let (bb, open) = self.switch_seeded::<W, _, _>(cond_id, default_target, name)?;
        let mut open = open;
        for (value, target, _) in cases {
            open = open.push_case_seeded(value, target)?;
        }
        Ok((bb, open.finish()))
    }

    /// Produce a width-ERASED `switch <cond>, label <default> [...]`.
    /// Mirrors `IRBuilder::CreateSwitch`.
    ///
    /// Returns the terminated parent block plus an [`Open`]-typestate
    /// [`SwitchInst`]. The caller adds
    /// cases via [`SwitchInst::add_case`](SwitchInst::add_case)
    /// (chainable) and seals the case list with
    /// [`SwitchInst::finish`](SwitchInst::finish).
    ///
    /// `cond` is bound by [`IntoErasedValue`], so the condition's width is not
    /// pinned and `add_case` checks each case value at *runtime*
    /// ([`IrError::TypeMismatch`]). Prefer the typed
    /// [`switch`](Self::switch) where the width is statically
    /// known — it makes a wrong-width case a compile error. This form is
    /// what the `.ll` parser and the auto-SSA builder land on, since
    /// neither knows the condition's width until run time.
    ///
    /// The default target is guarded against **parameterised** blocks exactly
    /// as [`switch`](Self::switch)'s is; reach for
    /// [`switch_dyn_with_args`](Self::switch_dyn_with_args) when an
    /// edge needs to carry block arguments.
    pub fn switch_dyn<C, DefaultTarget, Name>(
        self,
        cond: C,
        default_target: DefaultTarget,
        name: Name,
    ) -> IrResult<TerminatedBlockSwitch<'ctx, R, B>>
    where
        Name: AsRef<str>,
        C: IntoErasedValue<'ctx, B>,
        DefaultTarget: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let default_target = self.plain_edge_target(default_target)?;
        let cond_v = cond.into_erased_value(ModuleRef::new(self.module))?;
        self.switch_seeded::<IntDyn, _, _>(cond_v.id, default_target, name)
    }

    /// Produce `indirectbr <addr>, [...]`. Mirrors
    /// `IRBuilder::CreateIndirectBr`.
    ///
    /// The address is bound by [`IntoPointerValue<'ctx, B>`](crate::IntoPointerValue),
    /// so a typed [`PointerValue`] is accepted directly while a typed
    /// non-pointer handle (e.g. [`IntValue<'ctx, i32, B>`](crate::IntValue))
    /// is a *compile* error — there is no `IntoPointerValue` impl for it. An
    /// erased [`Value`]/[`Argument`](crate::Argument)/[`Instruction`] address is still accepted
    /// but is pointer-checked at *build* time (returns [`Err`] if it is not a
    /// pointer) rather than deferring to [`verify()`](crate::Module::verify).
    ///
    /// Returns the terminated parent block plus an [`Open`]-typestate
    /// [`IndirectBrInst`]. The
    /// caller adds destinations via
    /// [`IndirectBrInst::add_destination`](IndirectBrInst::add_destination)
    pub fn indirectbr<A, Name>(
        self,
        address: A,
        name: Name,
    ) -> IrResult<TerminatedBlockIndirectBr<'ctx, R, B>>
    where
        Name: AsRef<str>,
        A: IntoPointerValue<'ctx, B>,
    {
        let addr_v = IsValue::as_erased(address.into_pointer_value(ModuleRef::new(self.module))?);
        let void_ty = self.module.void_type::<B>().as_type().id();
        let payload = IndirectBrInstData::new(addr_v.id);
        let inst = self.append_instruction(void_ty, InstructionKindData::IndirectBr(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            IndirectBrInst::<Open, B>::from_raw(inst.slot(), module_ref, void_ty),
        ))
    }

    /// TYPED `invoke <ret-ty> <callee>(<args>) to label %normal unwind
    /// label %unwind`. Wrong arity / wrong argument types / wrong
    /// result use are compile errors; the invoke's return marker is
    /// derived from the callee. Mirrors `IRBuilder::CreateInvoke` with
    /// the callee schema statically pinned.
    pub fn invoke<Ret, Params, A, Normal, Unwind, Name>(
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
        self.invoke_with_config(
            callee,
            args,
            normal_dest,
            unwind_dest,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Produce a TYPED `invoke` with explicit call-site configuration.
    ///
    /// Neither the normal nor the unwind edge carries block arguments, so
    /// neither destination may be a **parameterised** block — both are checked
    /// before the terminator is emitted, with the same
    /// [`IrError::PhiArgArityMismatch`] [`br`](Self::br) reports.
    /// [`invoke_with_args`](Self::invoke_with_args) is the
    /// argument-carrying form; both `invoke` edges are mandatory, so it takes
    /// an argument list for each.
    pub fn invoke_with_config<Ret, Params, A, Normal, Unwind>(
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
        let normal_dest = self.plain_edge_target(normal_dest)?;
        let unwind_dest = self.plain_edge_target(unwind_dest)?;
        self.invoke_seeded(callee, args, normal_dest, unwind_dest, config)
    }

    /// Produce a TYPED `invoke` whose normal and unwind edges each carry block
    /// arguments into their destination's parameters. The `invoke` member of
    /// the [`br_with_args`](Self::br_with_args) family: an edge and
    /// the values it carries move together, so a destination's parameter-phis
    /// cannot be left one incoming short.
    ///
    /// Both edges are mandatory on an `invoke`, so both are supplied, each as a
    /// `(destination, args)` pair — the same bundled-edge shape
    /// [`switch_with_args`](Self::switch_with_args) uses, and what
    /// keeps `invoke`'s own call arguments and result name from crowding the
    /// signature. Pass an empty slice for a destination that has no parameters.
    ///
    /// `args` is the *call*'s argument list (compile-time-checked against the
    /// callee schema `Params`, as in [`invoke`](Self::invoke));
    /// the slice inside each edge pair holds that edge's *block* arguments,
    /// checked for arity ([`IrError::PhiArgArityMismatch`]) and type
    /// ([`IrError::TypeMismatch`]) against the destination's leading head-phis
    /// before either edge is seeded and before the terminator is emitted.
    ///
    /// The normal edge is seeded first; as with
    /// [`cond_br_with_args`](Self::cond_br_with_args), this is not
    /// one atomic transaction across the two edges — a differing-value
    /// duplicate on a shared destination is rejected
    /// ([`IrError::AmbiguousPhiIncoming`]) after the first edge is recorded,
    /// and the `invoke` is never emitted.
    ///
    /// Consumes `self`; both destinations may be in any termination state.
    pub fn invoke_with_args<Ret, Params, A, Normal, Unwind, Name>(
        self,
        callee: TypedFunctionValue<'ctx, Ret, Params, B>,
        args: A,
        normal: (Normal, &[Value<'ctx, B>]),
        unwind: (Unwind, &[Value<'ctx, B>]),
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
        let module_ref = ModuleRef::<B>::new(self.module);
        let (normal_dest, normal_args) = normal;
        let (unwind_dest, unwind_args) = unwind;
        let normal_dest = normal_dest.into_basic_block_label(module_ref)?;
        let unwind_dest = unwind_dest.into_basic_block_label(module_ref)?;
        // Capture the predecessor id before the terminator builder consumes
        // `self` — the incoming edges name *this* block as their predecessor.
        let pred = self.insert_block().id();
        self.add_block_args(normal_dest.id(), pred, normal_args)?;
        self.add_block_args(unwind_dest.id(), pred, unwind_args)?;
        self.invoke_seeded(
            callee,
            args,
            normal_dest,
            unwind_dest,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Emit a TYPED `invoke` with the parameterised-destination guard already
    /// discharged on both edges: either the caller has just seeded them
    /// ([`invoke_with_args`](Self::invoke_with_args)) or it ran
    /// [`plain_edge_target`](Self::plain_edge_target) on each
    /// ([`invoke_with_config`](Self::invoke_with_config)).
    fn invoke_seeded<Ret, Params, A, Normal, Unwind>(
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
        let normal_dest = normal_dest.into_basic_block_label(ModuleRef::new(self.module))?;
        let unwind_dest = unwind_dest.into_basic_block_label(ModuleRef::new(self.module))?;
        let f = callee.as_function();
        let arg_ids = args.lower(ModuleRef::new(self.module))?;
        let (name, calling_conv, attrs) = config.into_parts();
        let payload = InvokeInstData::new_with_attrs(
            f.slot(),
            f.signature().as_type().id(),
            arg_ids,
            calling_conv,
            normal_dest.slot(),
            unwind_dest.slot(),
            attrs,
        );
        let ret_ty = f.return_type().id();
        let inst = self.append_instruction(ret_ty, InstructionKindData::Invoke(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            InvokeInst::<Dyn, B>::from_raw(inst.slot(), module_ref, ret_ty).retag::<Ret::Marker>(),
        ))
    }

    /// Produce `invoke <ret-ty> <callee>(<args>) to label %normal
    /// unwind label %unwind`. Mirrors `IRBuilder::CreateInvoke`.
    pub fn invoke_dyn<R2, I, V, Normal, Unwind, Name>(
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
        V: IntoErasedValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.invoke_dyn_with_config(
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
    ) -> (FunctionType<'ctx, B>, TypeSlot) {
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
    ///
    /// Both destinations are guarded against **parameterised** blocks exactly
    /// as [`invoke_with_config`](Self::invoke_with_config)'s are;
    /// [`invoke_dyn_with_args`](Self::invoke_dyn_with_args) is the
    /// argument-carrying form.
    pub fn invoke_dyn_with_config<R2, I, V, Normal, Unwind>(
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
        V: IntoErasedValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let normal_dest = self.plain_edge_target(normal_dest)?;
        let unwind_dest = self.plain_edge_target(unwind_dest)?;
        self.invoke_dyn_seeded(callee, args, normal_dest, unwind_dest, config)
    }

    /// Produce an `invoke` whose normal and unwind edges each carry block
    /// arguments into their destination's parameters. Erased sibling of
    /// [`invoke_with_args`](Self::invoke_with_args) — see it for
    /// the edge/argument contract, which is identical; only the callee's
    /// schema is erased (the call arguments are checked against the callee's
    /// declared signature at run time, as in
    /// [`invoke_dyn`](Self::invoke_dyn)).
    pub fn invoke_dyn_with_args<R2, I, V, Normal, Unwind, Name>(
        self,
        callee: FunctionValue<'ctx, R2, B>,
        args: I,
        normal: (Normal, &[Value<'ctx, B>]),
        unwind: (Unwind, &[Value<'ctx, B>]),
        name: Name,
    ) -> IrResult<TerminatedBlockInvoke<'ctx, R, R2, B>>
    where
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Name: AsRef<str>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let module_ref = ModuleRef::<B>::new(self.module);
        let (normal_dest, normal_args) = normal;
        let (unwind_dest, unwind_args) = unwind;
        let normal_dest = normal_dest.into_basic_block_label(module_ref)?;
        let unwind_dest = unwind_dest.into_basic_block_label(module_ref)?;
        let pred = self.insert_block().id();
        self.add_block_args(normal_dest.id(), pred, normal_args)?;
        self.add_block_args(unwind_dest.id(), pred, unwind_args)?;
        self.invoke_dyn_seeded(
            callee,
            args,
            normal_dest,
            unwind_dest,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Emit an erased `invoke` with the parameterised-destination guard
    /// already discharged on both edges — the erased twin of
    /// [`invoke_seeded`](Self::invoke_seeded).
    fn invoke_dyn_seeded<R2, I, V, Normal, Unwind>(
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
        V: IntoErasedValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let normal_dest = normal_dest.into_basic_block_label(ModuleRef::new(self.module))?;
        let unwind_dest = unwind_dest.into_basic_block_label(ModuleRef::new(self.module))?;
        let callee_v = callee.as_erased();
        let (fn_ty, ret_ty) = self.resolve_call_site_type(&callee, &config);
        let (name, calling_conv, attrs) = config.into_parts();
        let arg_ids: Vec<ValueSlot> = args
            .into_iter()
            .map(|a| {
                a.into_erased_value(ModuleRef::new(self.module))
                    .map(|v| v.slot())
            })
            .collect::<IrResult<_>>()?;
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let payload = InvokeInstData::new_with_attrs(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            normal_dest.slot(),
            unwind_dest.slot(),
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::Invoke(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            InvokeInst::<Dyn, B>::from_raw(inst.slot(), module_ref, ret_ty).retag::<R2>(),
        ))
    }

    /// `invoke` through a function-pointer value (D3 dyn form of
    /// [`Self::invoke_dyn_with_config`]): the call-site function type
    /// is supplied explicitly, mirroring `IRBuilder::CreateInvoke(FunctionType*,
    /// Value* Callee, ...)`. Used by the parser for `invoke ... %fp(...)`.
    /// Arguments are validated against the spelled `fn_ty`.
    ///
    /// Both destinations are guarded against **parameterised** blocks like
    /// every other plain terminator edge. There is no argument-carrying twin
    /// for the *indirect*-callee shape (recorded in `docs/future-work.md`), so
    /// a parameterised destination is reachable only through
    /// [`invoke_dyn_with_args`](Self::invoke_dyn_with_args), whose
    /// callee is a named function.
    pub fn indirect_invoke_dyn_with_config<R2, I, V, Normal, Unwind, Callee>(
        self,
        callee: Callee,
        fn_ty: FunctionType<'ctx, B>,
        args: I,
        normal_dest: Normal,
        unwind_dest: Unwind,
        config: CallSiteConfig,
    ) -> IrResult<TerminatedBlockInvoke<'ctx, R, R2, B>>
    where
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
        Callee: IntoPointerValue<'ctx, B>,
    {
        let callee = callee.into_pointer_value(ModuleRef::new(self.module))?;
        let normal_dest = self.plain_edge_target(normal_dest)?;
        let unwind_dest = self.plain_edge_target(unwind_dest)?;
        let callee_v = IsValue::as_erased(callee);
        let ret_ty = fn_ty.return_type().id();
        let (name, calling_conv, attrs) = config.into_parts();
        let arg_ids: Vec<ValueSlot> = args
            .into_iter()
            .map(|a| {
                a.into_erased_value(ModuleRef::new(self.module))
                    .map(|v| v.slot())
            })
            .collect::<IrResult<_>>()?;
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let payload = InvokeInstData::new_with_attrs(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            normal_dest.slot(),
            unwind_dest.slot(),
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::Invoke(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            InvokeInst::<Dyn, B>::from_raw(inst.slot(), module_ref, ret_ty).retag::<R2>(),
        ))
    }

    /// Produce an `invoke` whose callee is an inline-assembly value.
    pub fn inline_asm_invoke<R2, I, V, Normal, Unwind, Name>(
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
        V: IntoErasedValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.inline_asm_invoke_with_config(
            asm,
            args,
            normal_dest,
            unwind_dest,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Produce an inline-assembly `invoke` with explicit call-site configuration.
    ///
    /// Both destinations are guarded against **parameterised** blocks like
    /// every other plain terminator edge; as with the indirect-callee form
    /// there is no argument-carrying twin for an inline-asm callee.
    pub fn inline_asm_invoke_with_config<R2, I, V, Normal, Unwind>(
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
        V: IntoErasedValue<'ctx, B>,
        Normal: IntoBasicBlockLabel<'ctx, R, B>,
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let normal_dest = self.plain_edge_target(normal_dest)?;
        let unwind_dest = self.plain_edge_target(unwind_dest)?;
        let asm_v = asm.as_erased();
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
        let mut arg_ids: Vec<ValueSlot> = Vec::new();
        for arg in args {
            let v = arg.into_erased_value(ModuleRef::new(self.module))?;
            arg_ids.push(v.id);
        }
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let (name, calling_conv, attrs) = config.into_parts();
        let payload = InvokeInstData::new_with_attrs(
            asm_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            normal_dest.slot(),
            unwind_dest.slot(),
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::Invoke(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            InvokeInst::<Dyn, B>::from_raw(inst.slot(), module_ref, ret_ty).retag::<R2>(),
        ))
    }

    /// Produce `callbr <ret-ty> <callee>(<args>) to label %default
    /// [label %indirect1, ...]`. Mirrors `IRBuilder::CreateCallBr`.
    pub fn callbr<R2, I, V, Callee, Default, Indirects, Indirect, Name>(
        self,
        callee: Callee,
        args: I,
        default_dest: Default,
        indirect_dests: Indirects,
        name: Name,
    ) -> IrResult<(BasicBlock<'ctx, R, Terminated, B>, CallBrInst<'ctx, B>)>
    where
        Name: AsRef<str>,
        R2: ReturnMarker,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Callee: IntoCallee<'ctx, R2, B>,
        Default: IntoBasicBlockLabel<'ctx, R, B>,
        Indirects: IntoIterator<Item = Indirect>,
        Indirect: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let callee = callee.into_callee(ModuleRef::<B>::new(self.module))?;
        self.callbr_with_config(
            callee,
            args,
            default_dest,
            indirect_dests,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Produce `callbr` with explicit call-site configuration.
    ///
    /// The default destination and every indirect destination are guarded
    /// against **parameterised** blocks like every other plain terminator
    /// edge. `callbr` has no argument-carrying form — its indirect edges are
    /// taken by inline assembly at run time, so there is nothing to hang a
    /// per-edge argument list on — so a parameterised destination is rejected
    /// outright ([`IrError::PhiArgArityMismatch`]), the same way an
    /// `indirectbr` destination is.
    pub fn callbr_with_config<R2, I, V, Default, Indirects, Indirect>(
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
        V: IntoErasedValue<'ctx, B>,
        Default: IntoBasicBlockLabel<'ctx, R, B>,
        Indirects: IntoIterator<Item = Indirect>,
        Indirect: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let default_dest = self.plain_edge_target(default_dest)?;
        let callee_v = callee.as_erased();
        let (fn_ty, ret_ty) = self.resolve_call_site_type(&callee, &config);
        let (name, calling_conv, attrs) = config.into_parts();
        let arg_ids: Vec<ValueSlot> = args
            .into_iter()
            .map(|a| {
                a.into_erased_value(ModuleRef::new(self.module))
                    .map(|v| v.slot())
            })
            .collect::<IrResult<_>>()?;
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let indirect_ids: Vec<ValueSlot> = indirect_dests
            .into_iter()
            .map(|d| self.plain_edge_target(d).map(|l| l.slot()))
            .collect::<IrResult<_>>()?;
        let payload = CallBrInstData::new_with_attrs(
            callee_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            default_dest.slot(),
            indirect_ids,
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::CallBr(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            CallBrInst::<B>::from_raw(inst.slot(), module_ref, ret_ty),
        ))
    }

    /// Produce a `callbr` whose callee is an inline-assembly value.
    pub fn inline_asm_callbr<R2, I, V, Default, Indirects, Indirect, Name>(
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
        V: IntoErasedValue<'ctx, B>,
        Default: IntoBasicBlockLabel<'ctx, R, B>,
        Indirects: IntoIterator<Item = Indirect>,
        Indirect: IntoBasicBlockLabel<'ctx, R, B>,
    {
        self.inline_asm_callbr_with_config::<R2, _, _, _, _, _>(
            asm,
            args,
            default_dest,
            indirect_dests,
            CallSiteConfig::new(name.as_ref()),
        )
    }

    /// Produce an inline-assembly `callbr` with explicit call-site configuration.
    ///
    /// Every destination is guarded against **parameterised** blocks, exactly
    /// as in [`callbr_with_config`](Self::callbr_with_config).
    pub fn inline_asm_callbr_with_config<R2, I, V, Default, Indirects, Indirect>(
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
        V: IntoErasedValue<'ctx, B>,
        Default: IntoBasicBlockLabel<'ctx, R, B>,
        Indirects: IntoIterator<Item = Indirect>,
        Indirect: IntoBasicBlockLabel<'ctx, R, B>,
    {
        let default_dest = self.plain_edge_target(default_dest)?;
        let asm_v = asm.as_erased();
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
        let mut arg_ids: Vec<ValueSlot> = Vec::new();
        for arg in args {
            let v = arg.into_erased_value(ModuleRef::new(self.module))?;
            arg_ids.push(v.id);
        }
        self.validate_call_site_args(fn_ty, &arg_ids)?;
        let indirect_ids: Vec<ValueSlot> = indirect_dests
            .into_iter()
            .map(|d| self.plain_edge_target(d).map(|l| l.slot()))
            .collect::<IrResult<_>>()?;
        let (name, calling_conv, attrs) = config.into_parts();
        let payload = CallBrInstData::new_with_attrs(
            asm_v.id,
            fn_ty.as_type().id(),
            arg_ids,
            calling_conv,
            default_dest.slot(),
            indirect_ids,
            attrs,
        );
        let inst = self.append_instruction(ret_ty, InstructionKindData::CallBr(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            CallBrInst::<B>::from_raw(inst.slot(), module_ref, ret_ty),
        ))
    }

    /// Produce `unreachable`. Mirrors `IRBuilder::CreateUnreachable`.
    ///
    /// Consumes `self`; infallible (no operands, no brand check).
    /// Produce `landingpad <ty>`. Mirrors `IRBuilder::CreateLandingPad`.
    /// Returns an [`Open`]-typestate
    /// handle; the caller adds clauses with `add_catch_clause` /
    /// `add_filter_clause` and seals the list with `finish`.
    pub fn landingpad<Name>(
        &self,
        result_ty: Type<'ctx, B>,
        cleanup: bool,
        name: Name,
    ) -> IrResult<LandingPadInst<'ctx, Open, B>>
    where
        Name: AsRef<str>,
    {
        let payload = LandingPadInstData::new(cleanup);
        let inst =
            self.append_instruction(result_ty.id, InstructionKindData::LandingPad(payload), name);
        Ok(LandingPadInst::<Open, B>::from_raw(
            inst.slot(),
            ModuleRef::<B>::new(self.module),
            result_ty.id,
        ))
    }

    /// Produce `resume <ty> <value>`. Mirrors `IRBuilder::CreateResume`.
    /// The `value` is typically a previously-built `landingpad` result.
    pub fn resume<V, Name>(self, value: V, name: Name) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Name: AsRef<str>,
        V: IntoErasedValue<'ctx, B>,
    {
        let v = value.into_erased_value(ModuleRef::new(self.module))?;
        let void_ty = self.module.void_type::<B>().as_type().id();
        let payload = ResumeInstData::new(v.id);
        let inst = self.append_instruction(void_ty, InstructionKindData::Resume(payload), name);
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `cleanuppad within <parent> [<args>]`. Mirrors
    /// `IRBuilder::CreateCleanupPad`.
    pub fn cleanup_pad<I, V, Pad, Name>(
        &self,
        parent_pad: Pad,
        args: I,
        name: Name,
    ) -> IrResult<CleanupPadInst<'ctx, B>>
    where
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Name: AsRef<str>,
        Pad: IntoErasedValue<'ctx, B>,
    {
        let parent_pad = parent_pad.into_erased_value(ModuleRef::new(self.module))?;
        self.cleanup_pad_raw(Some(parent_pad.id), args, name)
    }

    /// Produce `cleanuppad within none [<args>]`. Mirrors
    /// `IRBuilder::CreateCleanupPad`.
    pub fn cleanup_pad_within_none<I, V, Name>(
        &self,
        args: I,
        name: Name,
    ) -> IrResult<CleanupPadInst<'ctx, B>>
    where
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Name: AsRef<str>,
    {
        self.cleanup_pad_raw(None, args, name)
    }

    fn cleanup_pad_raw<I, V, Name>(
        &self,
        parent_id: Option<ValueSlot>,
        args: I,
        name: Name,
    ) -> IrResult<CleanupPadInst<'ctx, B>>
    where
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Name: AsRef<str>,
    {
        let arg_ids: Vec<ValueSlot> = args
            .into_iter()
            .map(|a| {
                a.into_erased_value(ModuleRef::new(self.module))
                    .map(|v| v.slot())
            })
            .collect::<IrResult<_>>()?;
        let payload = CleanupPadInstData::new(parent_id, arg_ids);
        let token_ty = self.module.token_type::<B>().as_type().id();
        let inst =
            self.append_instruction(token_ty, InstructionKindData::CleanupPad(payload), name);
        Ok(CleanupPadInst::<B>::from_raw(
            inst.slot(),
            ModuleRef::<B>::new(self.module),
            token_ty,
        ))
    }

    /// Produce `catchpad within <catchswitch> [<args>]`. Mirrors
    /// `IRBuilder::CreateCatchPad`.
    pub fn catch_pad<I, V, Switch, Name>(
        &self,
        catch_switch: Switch,
        args: I,
        name: Name,
    ) -> IrResult<CatchPadInst<'ctx, B>>
    where
        Name: AsRef<str>,
        I: IntoIterator<Item = V>,
        V: IntoErasedValue<'ctx, B>,
        Switch: IntoErasedValue<'ctx, B>,
    {
        let catch_switch = catch_switch.into_erased_value(ModuleRef::new(self.module))?;
        let arg_ids: Vec<ValueSlot> = args
            .into_iter()
            .map(|a| {
                a.into_erased_value(ModuleRef::new(self.module))
                    .map(|v| v.slot())
            })
            .collect::<IrResult<_>>()?;
        let payload = CatchPadInstData::new(Some(catch_switch.id), arg_ids);
        let token_ty = self.module.token_type::<B>().as_type().id();
        let inst = self.append_instruction(token_ty, InstructionKindData::CatchPad(payload), name);
        Ok(CatchPadInst::<B>::from_raw(
            inst.slot(),
            ModuleRef::<B>::new(self.module),
            token_ty,
        ))
    }

    /// Produce `catchret from <catchpad> to label <bb>`. Mirrors
    /// `IRBuilder::CreateCatchRet`.
    pub fn catch_ret<Target, Pad, Name>(
        self,
        catch_pad: Pad,
        target: Target,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Name: AsRef<str>,
        Target: IntoBasicBlockLabel<'ctx, R, B>,
        Pad: IntoErasedValue<'ctx, B>,
    {
        let catch_pad = catch_pad.into_erased_value(ModuleRef::new(self.module))?;
        let target = target.into_basic_block_label(ModuleRef::new(self.module))?;
        let void_ty = self.module.void_type::<B>().as_type().id();
        let payload = CatchReturnInstData::new(catch_pad.id, target.slot());
        let inst =
            self.append_instruction(void_ty, InstructionKindData::CatchReturn(payload), name);
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `cleanupret from <cleanuppad> unwind label <bb>`.
    /// Mirrors `IRBuilder::CreateCleanupRet`.
    pub fn cleanup_ret<Unwind, Pad, Name>(
        self,
        cleanup_pad: Pad,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
        Name: AsRef<str>,
        Pad: IntoErasedValue<'ctx, B>,
    {
        let cleanup_pad = cleanup_pad.into_erased_value(ModuleRef::new(self.module))?;
        let unwind_dest = unwind_dest.into_basic_block_label(ModuleRef::new(self.module))?;
        self.cleanup_ret_raw(cleanup_pad.id, Some(unwind_dest.slot()), name)
    }

    /// Produce `cleanupret from <cleanuppad> unwind to caller`.
    /// Mirrors `IRBuilder::CreateCleanupRet`.
    pub fn cleanup_ret_to_caller<Pad, Name>(
        self,
        cleanup_pad: Pad,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Name: AsRef<str>,
        Pad: IntoErasedValue<'ctx, B>,
    {
        let cleanup_pad = cleanup_pad.into_erased_value(ModuleRef::new(self.module))?;
        self.cleanup_ret_raw(cleanup_pad.id, None, name)
    }

    fn cleanup_ret_raw<Name>(
        self,
        cleanup_pad_id: ValueSlot,
        unwind_id: Option<ValueSlot>,
        name: Name,
    ) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        Name: AsRef<str>,
    {
        let void_ty = self.module.void_type::<B>().as_type().id();
        let payload = CleanupReturnInstData::new(cleanup_pad_id, unwind_id);
        let inst =
            self.append_instruction(void_ty, InstructionKindData::CleanupReturn(payload), name);
        let bb = self.into_insert_block();
        Ok((bb.retag_termination::<Terminated>(), inst))
    }

    /// Produce `catchswitch within <parent> [...] unwind label <bb>`.
    /// Mirrors `IRBuilder::CreateCatchSwitch`.
    pub fn catch_switch<Unwind, Pad, Name>(
        self,
        parent_pad: Pad,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
        Name: AsRef<str>,
        Pad: IntoErasedValue<'ctx, B>,
    {
        let parent_pad = parent_pad.into_erased_value(ModuleRef::new(self.module))?;
        let unwind_dest = unwind_dest.into_basic_block_label(ModuleRef::new(self.module))?;
        self.catch_switch_raw(Some(parent_pad.id), Some(unwind_dest.slot()), name)
    }

    /// Produce `catchswitch within <parent> [...] unwind to caller`.
    /// Mirrors `IRBuilder::CreateCatchSwitch`.
    pub fn catch_switch_to_caller<Pad, Name>(
        self,
        parent_pad: Pad,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Name: AsRef<str>,
        Pad: IntoErasedValue<'ctx, B>,
    {
        let parent_pad = parent_pad.into_erased_value(ModuleRef::new(self.module))?;
        self.catch_switch_raw(Some(parent_pad.id), None, name)
    }

    /// Produce `catchswitch within none [...] unwind label <bb>`.
    /// Mirrors `IRBuilder::CreateCatchSwitch`.
    pub fn catch_switch_within_none<Unwind, Name>(
        self,
        unwind_dest: Unwind,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Unwind: IntoBasicBlockLabel<'ctx, R, B>,
        Name: AsRef<str>,
    {
        let unwind_dest = unwind_dest.into_basic_block_label(ModuleRef::new(self.module))?;
        self.catch_switch_raw(None, Some(unwind_dest.slot()), name)
    }

    /// Produce `catchswitch within none [...] unwind to caller`.
    /// Mirrors `IRBuilder::CreateCatchSwitch`.
    pub fn catch_switch_within_none_to_caller<Name>(
        self,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Name: AsRef<str>,
    {
        self.catch_switch_raw(None, None, name)
    }

    fn catch_switch_raw<Name>(
        self,
        parent_id: Option<ValueSlot>,
        unwind_id: Option<ValueSlot>,
        name: Name,
    ) -> IrResult<TerminatedBlockCatchSwitch<'ctx, R, B>>
    where
        Name: AsRef<str>,
    {
        let token_ty = self.module.token_type::<B>().as_type().id();
        let payload = CatchSwitchInstData::new(parent_id, unwind_id);
        let inst =
            self.append_instruction(token_ty, InstructionKindData::CatchSwitch(payload), name);
        let module_ref = ModuleRef::<B>::new(self.module);
        let bb = self.into_insert_block();
        Ok((
            bb.retag_termination::<Terminated>(),
            CatchSwitchInst::<Open, B>::from_raw(inst.slot(), module_ref, token_ty),
        ))
    }

    pub fn unreachable(
        self,
    ) -> (
        BasicBlock<'ctx, R, Terminated, B>,
        Instruction<'ctx, Attached, B>,
    ) {
        let payload = UnreachableInstData;
        let void_ty = self.module.void_type::<B>().as_type().id();
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
        ty: TypeSlot,
        kind: InstructionKindData,
        name: N,
    ) -> Instruction<'ctx, Attached, B> {
        let name = name.as_ref();
        let bb = self.insert_block();
        let bb_id = bb.slot();
        let value = build_instruction_value(ty, bb_id, kind, None);
        // Snapshot operand ids before the value is moved into the arena;
        // we need them to register the new instruction in each operand's
        // reverse use-list. Mirrors `User::setOperand` in
        // `llvm/lib/IR/User.cpp`, which threads each `Use` into its
        // operand's use-list at construction time.
        let operand_ids = match &value.kind {
            ValueKindData::Instruction(i) => {
                // Block successors are `Use`s upstream (`BranchInst`,
                // `SwitchInst`, `InvokeInst`, …), so they are registered here
                // too — after the value operands, which is the operand-index
                // order every one of those constructors uses.
                let mut ids = i.kind.operand_ids();
                ids.extend(i.kind.block_operand_ids());
                ids
            }
            // append_instruction always builds an Instruction-kind value.
            _ => unreachable!("append_instruction built non-instruction value"),
        };
        let id = self.module.context().push_value(value);
        for op in operand_ids {
            self.module
                .context()
                .value_data(op)
                .add_use(ValueUse::Instruction(id));
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
            && !Type::<B>::new(ty, self.module).is_void()
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

    /// Append `kind` at `like`'s type and wrap the result as width-`W`.
    ///
    /// Sound by construction: the instruction is created AT `like.ty()`, and
    /// `like: IntValue<'ctx, W, B>` is W-typed, so the result is W-typed. This is the
    /// only sanctioned way to attach an `IntValue<W>` marker to a freshly-appended
    /// instruction whose type comes from an operand -- it removes the `from_value_unchecked`
    /// assertion at the call site (see docs/design/unforgeable-markers-design.md, census pattern 1).
    fn append_int_like<W: IntWidth, N: AsRef<str>>(
        &self,
        like: IntValue<'ctx, W, B>,
        kind: InstructionKindData,
        name: N,
    ) -> IntValue<'ctx, W, B> {
        let inst = self.append_instruction(like.ty().as_type().id(), kind, name);
        IntValue::<W, B>::from_value_unchecked(inst.to_erased())
    }

    /// Float analogue of `append_int_like`. Sound by construction: appended at `like.ty()`,
    /// `like: FloatValue<'ctx, K, B>` is K-typed.
    fn append_fp_like<K: FloatKind, N: AsRef<str>>(
        &self,
        like: FloatValue<'ctx, K, B>,
        kind: InstructionKindData,
        name: N,
    ) -> FloatValue<'ctx, K, B> {
        let inst = self.append_instruction(like.ty().as_type().id(), kind, name);
        FloatValue::<K, B>::from_value_unchecked(inst.to_erased())
    }

    /// Append `kind` at `ty` and wrap the result as width-`W`.
    ///
    /// Sound by construction: the instruction is created AT `ty`, and
    /// `ty: IntType<'ctx, W, B>` is W-typed by construction (`W::ir_type()` or a checked
    /// narrow), so the result is W-typed. The sanctioned constructor for results whose type
    /// comes from a typed DESTINATION handle -- casts, comparisons at a fixed `i1`, loads --
    /// removing the `from_value_unchecked` assertion at the call site
    /// (docs/design/unforgeable-markers-design.md, census pattern 2).
    fn append_int_at<W: IntWidth, N: AsRef<str>>(
        &self,
        ty: IntType<'ctx, W, B>,
        kind: InstructionKindData,
        name: N,
    ) -> IntValue<'ctx, W, B> {
        let inst = self.append_instruction(ty.as_type().id(), kind, name);
        IntValue::<W, B>::from_value_unchecked(inst.to_erased())
    }

    /// Float analogue of `append_int_at`. Sound: appended at `ty`, `ty: FloatType<'ctx, K, B>`
    /// is K-typed by construction.
    fn append_fp_at<K: FloatKind, N: AsRef<str>>(
        &self,
        ty: FloatType<'ctx, K, B>,
        kind: InstructionKindData,
        name: N,
    ) -> FloatValue<'ctx, K, B> {
        let inst = self.append_instruction(ty.as_type().id(), kind, name);
        FloatValue::<K, B>::from_value_unchecked(inst.to_erased())
    }

    /// Build and append a `load`, re-stamping the payload's pointee to `ty` and
    /// wrapping the result as width-`W`.
    ///
    /// Takes the whole [`LoadInstData`] (so the caller sets align/volatile/ordering/
    /// scope exactly as before) and overwrites `payload.pointee_ty` with `ty` — the
    /// appended type IS `ty` by construction, so the `W` marker is provably correct.
    /// Routes through [`Self::load_inner`] so the DataLayout default-align fill
    /// is preserved (a raw `append_int_at` would skip it and emit `align 0`). This
    /// removes the `from_value_unchecked` assertion at the load site
    /// (docs/design/unforgeable-markers-design.md, census pattern 2 -- load variant).
    fn append_int_load<W: IntWidth, N: AsRef<str>>(
        &self,
        ty: IntType<'ctx, W, B>,
        mut payload: LoadInstData,
        name: N,
    ) -> IrResult<IntValue<'ctx, W, B>> {
        payload.pointee_ty = ty.as_type().id();
        let inst = self.load_inner(payload, name)?;
        Ok(IntValue::<W, B>::from_value_unchecked(inst.to_erased()))
    }

    /// Float analogue of `append_int_load`. Re-stamps `payload.pointee_ty` with `ty`
    /// so the appended type IS `ty` and the `K` marker is provably correct; routed
    /// through [`Self::load_inner`] so the default-align fill is preserved.
    fn append_fp_load<K: FloatKind, N: AsRef<str>>(
        &self,
        ty: FloatType<'ctx, K, B>,
        mut payload: LoadInstData,
        name: N,
    ) -> IrResult<FloatValue<'ctx, K, B>> {
        payload.pointee_ty = ty.as_type().id();
        let inst = self.load_inner(payload, name)?;
        Ok(FloatValue::<K, B>::from_value_unchecked(inst.to_erased()))
    }

    /// Append `kind` at `ptr_ty` and wrap the result as a `PointerValue`.
    ///
    /// Sound by construction: the instruction is created AT `ptr_ty` (a `PointerType`,
    /// so provably a pointer type), and `PointerValue` asserts only pointer-ness — which
    /// `ptr_ty` supplies. The sanctioned constructor for pointer-result builders
    /// (alloca / GEP / int→ptr / addrspacecast / pointer bitcast); removes the
    /// `from_value_unchecked` assertion (docs/design/unforgeable-markers-design.md, census pattern 6).
    fn append_ptr<N: AsRef<str>>(
        &self,
        ptr_ty: PointerType<'ctx, B>,
        kind: InstructionKindData,
        name: N,
    ) -> PointerValue<'ctx, B> {
        let inst = self.append_instruction(ptr_ty.as_type().id(), kind, name);
        PointerValue::from_value_unchecked(inst.to_erased())
    }

    /// Pointer load: build+append a `load` whose pointee is `ptr_ty`, routed through
    /// [`Self::load_inner`] so the default-align fill is preserved (a raw append would
    /// emit `align 0`). Re-stamps `payload.pointee_ty = ptr_ty` — structural.
    fn append_ptr_load<N: AsRef<str>>(
        &self,
        ptr_ty: PointerType<'ctx, B>,
        mut payload: LoadInstData,
        name: N,
    ) -> IrResult<PointerValue<'ctx, B>> {
        payload.pointee_ty = ptr_ty.as_type().id();
        let inst = self.load_inner(payload, name)?;
        Ok(PointerValue::from_value_unchecked(inst.to_erased()))
    }

    /// Crate-internal: append a freshly-built phi to the insertion block.
    /// Identical to [`Self::append_instruction`] (same operand use-list
    /// registration and value-symbol-table population) except for
    /// placement: the phi is inserted at the block's phi head via
    /// [`BasicBlock::insert_instruction_at_phi_head`], regardless of the
    /// builder's cursor, so phis stay grouped at the top of the block by
    /// construction rather than only by a verifier check.
    fn append_phi_instruction<N: AsRef<str>>(
        &self,
        ty: TypeSlot,
        kind: InstructionKindData,
        name: N,
    ) -> Instruction<'ctx, Attached, B> {
        let name = name.as_ref();
        let bb = self.insert_block();
        let bb_id = bb.slot();
        let value = build_instruction_value(ty, bb_id, kind, None);
        // Snapshot operand ids before the value is moved into the arena so
        // we can register the new instruction in each operand's reverse
        // use-list -- identical to `append_instruction`.
        let operand_ids = match &value.kind {
            ValueKindData::Instruction(i) => i.kind.operand_ids(),
            // append_phi_instruction always builds an Instruction-kind value.
            _ => unreachable!("append_phi_instruction built non-instruction value"),
        };
        let id = self.module.context().push_value(value);
        for op in operand_ids {
            self.module
                .context()
                .value_data(op)
                .add_use(ValueUse::Instruction(id));
        }
        // Unlike `append_instruction`, placement ignores the builder's
        // insert cursor: phis always land at the block's phi head.
        bb.insert_instruction_at_phi_head(id);
        if !name.is_empty()
            && !Type::<B>::new(ty, self.module).is_void()
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
        expected_ty: TypeSlot,
    ) -> IrResult<Value<'ctx, B>> {
        Type::new(expected_ty, ModuleRef::<B>::new(self.module)).require_match(folded.ty())?;
        Ok(folded)
    }

    /// Accept a typed fold result, checking the payload's runtime type
    /// against the operand's for **every** marker — static ones included.
    ///
    /// A static `W` is not self-guaranteeing here. The type system pins `W`
    /// only as tightly as whoever constructed the handle: the crate-internal
    /// [`IntValue::from_value_unchecked`] mints an `IntValue<W>` without
    /// consulting the payload's real type, so an in-crate folder overriding
    /// the typed `fold_int_bin_op<W>` hook can hand back a value whose IR
    /// type contradicts `W`. Keying this check on `W::static_bits().is_none()`
    /// would trust the very claim it exists to verify — the folder's word
    /// that `W` matches the payload. Both markers are checked instead
    /// (`hostile_native_typed_override_wrong_width_rejected_at_static_width`
    /// locks the static half; the `..._by_accept_folded_int` sibling the dyn).
    ///
    /// For the same reason this compares against `like`'s *runtime* type
    /// rather than narrowing to `W`: see [`Type::require_match`], which
    /// carries the error shape and the no-false-rejections argument.
    fn accept_folded_int<W: IntWidth>(
        &self,
        folded: IntValue<'ctx, W, B>,
        like: IntValue<'ctx, W, B>,
    ) -> IrResult<IntValue<'ctx, W, B>> {
        like.as_erased()
            .ty()
            .require_match(folded.as_erased().ty())?;
        Ok(folded)
    }

    /// Mirrors [`Self::accept_folded_int`] for float kinds — including its
    /// reason for checking every marker rather than only the erased
    /// `FloatDyn` one: `FloatValue::from_value_unchecked` forges a static
    /// `K` just as freely as `IntValue`'s does a static `W`.
    fn accept_folded_fp<K: FloatKind>(
        &self,
        folded: FloatValue<'ctx, K, B>,
        like: FloatValue<'ctx, K, B>,
    ) -> IrResult<FloatValue<'ctx, K, B>> {
        crate::value::Typed::ty(like).require_match(crate::value::Typed::ty(folded))?;
        Ok(folded)
    }

    /// Accept a typed cast fold result against the destination int type.
    /// Casts have no same-type operand to compare against (unlike binops),
    /// so this checks against `dst_ty` instead of a `like` operand; otherwise
    /// mirrors [`Self::accept_folded_int`], including its every-marker check
    /// and the reasoning for it.
    fn accept_folded_cast_int<W: IntWidth>(
        &self,
        folded: IntValue<'ctx, W, B>,
        dst_ty: IntType<'ctx, W, B>,
    ) -> IrResult<IntValue<'ctx, W, B>> {
        dst_ty.as_type().require_match(folded.as_erased().ty())?;
        Ok(folded)
    }

    /// Mirrors [`Self::accept_folded_cast_int`] for float destination kinds,
    /// checking every marker for the same reason.
    fn accept_folded_cast_fp<K: FloatKind>(
        &self,
        folded: FloatValue<'ctx, K, B>,
        dst_ty: FloatType<'ctx, K, B>,
    ) -> IrResult<FloatValue<'ctx, K, B>> {
        dst_ty
            .as_type()
            .require_match(crate::value::Typed::ty(folded))?;
        Ok(folded)
    }

    /// Build the `ret` payload and append. Crate-internal: the typed
    /// `ret` methods funnel here after their per-marker
    /// validation. Cannot fail by construction.
    fn append_ret(&self, value: Option<Value<'ctx, B>>) -> Instruction<'ctx, Attached, B> {
        let payload = ReturnOpData::new(value.map(|v| v.id));
        let void_ty = self.module.void_type::<B>().as_type().id();
        self.append_instruction(void_ty, InstructionKindData::Ret(payload), "")
    }
}

// --------------------------------------------------------------------------
// `ret` dispatch via the [`IntoReturnValue`] trait
// --------------------------------------------------------------------------
//
// Rust's coherence checker rejects two blanket impls (`<W: IntWidth>` +
// `<K: FloatKind>`) on `IrBuilder<R>` even when no type implements both
// traits. We dispatch through a single sealed trait that pins the
// return-value lift per concrete marker. Each impl is concrete-typed so
// no overlap arises. Mirrors `IRBuilder::CreateRet` in `IRBuilder.h`.

/// Types that can be passed to [`IrBuilder::ret`] for a function
/// carrying [`ReturnMarker`] `R`. Concrete impls are provided per
/// `(value-shape, R)` pair: for a typed `R` the impls blanket over the
/// now-sealed lift traits ([`IntoIntValue`] / [`IntoFloatValue`] /
/// [`IntoPointerValue`]), so a typed builder accepts every Rust scalar /
/// typed handle that lifts to the correct IR type and an erased handle is
/// rejected; the [`Dyn`] builder blankets over [`IntoErasedValue`] and accepts
/// any value handle or id with a runtime type check. This trait is not itself
/// sealed with a private supertrait — its extension surface is closed
/// transitively by those sealed lift-trait bounds plus the sealed
/// [`IntoErasedValue`].
pub trait IntoReturnValue<'ctx, R: ReturnMarker, B: ModuleBrand>: Sized {
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
                Ok(IsValue::as_erased(self.into_int_value(module)?))
            }
        }
    )+ };
}
impl_into_return_value_int!(bool, i8, i16, i32, i64, i128, IntDyn);

// Float-marker impls. A later revision introduces `IntoFloatValue<'ctx, K, B>`;
// for now the typed `FloatValue<'ctx, K, B>` itself is the only direct
// `IntoReturnValue<'ctx, K>` source. That revision will replace these with
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
                Ok(IsValue::as_erased(self.into_float_value(module)?))
            }
        }
    )+ };
}
impl_into_return_value_float!(f32, f64, Half, Bfloat, Fp128, X86Fp80, PpcFp128, FloatDyn,);

// Pointer-marker impl: `Ptr` accepts any pointer-valued operand source.
impl<'ctx, B: ModuleBrand + 'ctx, V> IntoReturnValue<'ctx, Ptr, B> for V
where
    V: IntoPointerValue<'ctx, B>,
{
    #[inline]
    fn into_return_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(IsValue::as_erased(self.into_pointer_value(module)?))
    }
}

// Top-level erased `Dyn` accepts any erased-operand source: every value
// handle, plus the storable ids (which resolve against `module`).
impl<'ctx, B: ModuleBrand + 'ctx, V> IntoReturnValue<'ctx, Dyn, B> for V
where
    V: IntoErasedValue<'ctx, B>,
{
    #[inline]
    fn into_return_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        self.into_erased_value(module)
    }
}

impl<'m, 'ctx, B, F, R> IrBuilder<'m, 'ctx, B, F, Positioned, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Produce `ret <value>` against the function's declared return
    /// type. The accepted operand types are pinned by `R` through the
    /// [`IntoReturnValue`] trait - a builder for `i32`-returning
    /// function takes any `IntoIntValue<'ctx, i32, B>`, the float / ptr
    /// builders take their corresponding handles, and a [`Dyn`]
    /// builder accepts anything implementing
    /// [`IntoErasedValue`] but runs an extra runtime
    /// type-equality check.
    pub fn ret<V>(self, value: V) -> IrResult<TerminatedBlockInst<'ctx, R, B>>
    where
        V: IntoReturnValue<'ctx, R, B>,
    {
        let v = value.into_return_value(ModuleRef::new(self.module))?;
        // Runtime-check for the fully-erased `Dyn` marker.
        if R::expected_kind() == ExpectedRetKind::Dyn {
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
    /// [`Self::ret`].
    fn parent_function_dyn(&self) -> FunctionValue<'ctx, Dyn, B> {
        let bb = self.insert_block();
        let parent_id = bb.parent_id().unwrap_or_else(|| {
            unreachable!("Positioned builder block always has a parent function")
        });
        FunctionValue::<Dyn, B>::from_parts_unchecked(parent_id, ModuleRef::<B>::new(self.module))
    }
}

impl<'m, 'ctx, B, F> IrBuilder<'m, 'ctx, B, F, Positioned, ()>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
{
    /// Produce `ret void`. Mirrors `IRBuilder::CreateRetVoid`. The
    /// `()` builder does not expose `ret(value)` at all (no
    /// `IntoReturnValue<'ctx, ()>` impls exist), so `ret_void`
    /// is the only return option.
    pub fn ret_void(self) -> VoidReturnInst<'ctx, B> {
        let inst = self.append_ret(None);
        let bb = self.into_insert_block();
        (bb.retag_termination::<Terminated>(), inst)
    }
}

impl<'m, 'ctx, B, F> IrBuilder<'m, 'ctx, B, F, Positioned, Dyn>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
{
    /// Produce `ret void`. Errors with
    /// [`IrError::ReturnTypeMismatch`] if the parent function does
    /// not actually return `void`.
    pub fn ret_void(self) -> IrResult<TerminatedBlockInst<'ctx, Dyn, B>> {
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

/// Builder for [`crate::IrBuilder::call_builder`]. Accumulates
/// per-arg / flag state via chainable methods, then emits the call
/// instruction on `.build()`. Each `.arg(...)` call is statically
/// dispatched against `V: IntoErasedValue<'ctx, B>`; arg types can vary
/// across calls without trait objects.
pub struct CallBuilder<'a, 'm, 'ctx, B, F, RP, RC>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    RC: ReturnMarker,
{
    parent: &'a IrBuilder<'m, 'ctx, B, F, Positioned, RP>,
    callee_id: ValueSlot,
    fn_ty: TypeSlot,
    return_ty: TypeSlot,
    args: Vec<ValueSlot>,
    calling_conv: crate::CallingConv,
    tail_kind: TailCallKind,
    attrs: CallAttributeData,
    name: String,
    intrinsic_descriptor: Option<IntrinsicDescriptor<'ctx, B>>,
    /// First error raised by an [`arg`](CallBuilder::arg) operand, replayed by
    /// [`build`](CallBuilder::build). `arg` returns `Self` to keep the chain
    /// spellable, so a failed operand lift has nowhere to surface until the
    /// terminal call. Only an id from a *foreign* module can set this — every
    /// value handle lifts infallibly — so no pre-existing input can reach it.
    arg_error: Option<IrError>,
    _rp: PhantomData<RP>,
    _rc: PhantomData<RC>,
}

/// Hand-written for the same reason as [`IrBuilder`]'s: a `derive` would
/// bound the folder parameter `F: Debug`. Arguments are summarised by count —
/// the accumulated slots are arena indices, not something a reader can act on.
impl<'a, 'm, 'ctx, B, F, RP, RC> core::fmt::Debug for CallBuilder<'a, 'm, 'ctx, B, F, RP, RC>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    RC: ReturnMarker,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CallBuilder")
            .field("callee", &self.callee_id)
            .field("arguments", &self.args.len())
            .field("calling_conv", &self.calling_conv)
            .field("tail_kind", &self.tail_kind)
            .field("name", &self.name)
            .field("intrinsic_descriptor", &self.intrinsic_descriptor)
            .field("arg_error", &self.arg_error)
            .finish()
    }
}

impl<'a, 'm, 'ctx, B, F, RP, RC> CallBuilder<'a, 'm, 'ctx, B, F, RP, RC>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    RC: ReturnMarker,
{
    /// Add an argument. Statically dispatched per `V: IntoErasedValue` so
    /// mixed-type argument lists work without homogeneity, and a storable id
    /// is accepted alongside a borrowing handle.
    ///
    /// Infallible for every value handle. An id from a foreign module is the
    /// one input that can fail; because the chain returns `Self`, that error
    /// is parked in `arg_error` and reported by [`build`](Self::build).
    #[must_use]
    pub fn arg<V: IntoErasedValue<'ctx, B>>(mut self, value: V) -> Self {
        match value.into_erased_value(ModuleRef::<B>::new(self.parent.module)) {
            Ok(v) => self.args.push(v.id),
            Err(e) => {
                self.arg_error.get_or_insert(e);
            }
        }
        self
    }

    #[must_use]
    pub fn tail(mut self) -> Self {
        self.tail_kind = TailCallKind::Tail;
        self
    }

    #[must_use]
    pub fn must_tail(mut self) -> Self {
        self.tail_kind = TailCallKind::MustTail;
        self
    }

    #[must_use]
    pub fn no_tail(mut self) -> Self {
        self.tail_kind = TailCallKind::NoTail;
        self
    }

    #[must_use]
    pub fn calling_conv(mut self, cc: CallingConv) -> Self {
        self.calling_conv = cc;
        self
    }
    #[must_use]
    pub fn call_attributes(mut self, attrs: CallAttributeData) -> Self {
        self.attrs = attrs;
        self
    }

    #[must_use]
    pub fn name<Name>(mut self, name: Name) -> Self
    where
        Name: Into<String>,
    {
        self.name = name.into();
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

    /// Emit the call instruction, named by the storable
    /// [`CallInstId<RC, B>`](crate::CallInstId).
    pub fn build(self) -> IrResult<CallInstId<RC, B>> {
        let module_id = self.parent.module.id();
        let inst = self.emit()?;
        Ok(CallInstId::from_raw(module_id, inst.slot()))
    }

    /// Validate and append the call, handing back the freshly attached
    /// instruction. Shared by [`build`](Self::build) and the intrinsic
    /// wrapper, which needs the instruction itself to confirm the callee is a
    /// generated intrinsic declaration before minting its own id.
    fn emit(mut self) -> IrResult<Instruction<'ctx, Attached, B>> {
        if let Some(e) = self.arg_error.take() {
            return Err(e);
        }
        self.validate_intrinsic_descriptor_args()?;
        let fn_ty =
            FunctionType::<'ctx, B>::new(self.fn_ty, ModuleRef::<B>::new(self.parent.module));
        self.parent.validate_call_site_args(fn_ty, &self.args)?;
        let payload = CallInstData::new_with_attrs(
            self.callee_id,
            self.fn_ty,
            self.args.into_boxed_slice(),
            self.calling_conv,
            self.tail_kind,
            self.attrs,
        );
        Ok(self.parent.append_instruction(
            self.return_ty,
            InstructionKindData::Call(payload),
            self.name,
        ))
    }
}

impl<'a, 'm, 'ctx, B, F, RP> CallBuilder<'a, 'm, 'ctx, B, F, RP, Dyn>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
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
    #[must_use]
    pub fn call_site_type(mut self, fn_ty: FunctionType<'ctx, B>) -> Self {
        self.return_ty = fn_ty.return_type().id();
        self.fn_ty = fn_ty.as_type().id();
        self
    }
}

// --------------------------------------------------------------------------
// TypedCallBuilder
// --------------------------------------------------------------------------

/// Chainable builder for [`crate::IrBuilder::typed_call_builder`]. Same
/// schema guarantees as [`crate::IrBuilder::call`] — the callee's
/// return marker, parameter schema, and lowered arguments are all
/// pinned by `Ret` / `Params` / `A` — with tail-call kind / calling
/// convention / attributes / result name accumulated via chainable
/// methods before `.build()` emits the call.
pub struct TypedCallBuilder<'a, 'm, 'ctx, B, F, RP, Ret, Params, A>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    Ret: FunctionReturn,
    Params: FunctionParamList,
    A: CallArgs<'ctx, Params, B>,
{
    parent: &'a IrBuilder<'m, 'ctx, B, F, Positioned, RP>,
    /// The resolved callee, or the error its lift raised. The chain
    /// constructor [`IrBuilder::typed_call_builder`](crate::IrBuilder::typed_call_builder)
    /// returns `Self` rather than `IrResult<Self>` to keep the chain
    /// spellable, so a failed callee lift has nowhere to surface until
    /// [`build`](TypedCallBuilder::build), which reads it before emitting
    /// anything. Only a [`TypedFunctionId`](crate::TypedFunctionId) from a
    /// *foreign* module can set it — the borrowing facade lifts infallibly.
    callee: IrResult<TypedFunctionValue<'ctx, Ret, Params, B>>,
    args: A,
    tail_kind: TailCallKind,
    calling_conv: Option<CallingConv>,
    attrs: CallAttributeData,
    name: String,
}

/// Hand-written for the same reason as [`IrBuilder`]'s, plus one more: the
/// lowered argument tuple `A` is a user-supplied schema type with no `Debug`
/// obligation, so it is reported by type name rather than by value.
impl<'a, 'm, 'ctx, B, F, RP, Ret, Params, A> core::fmt::Debug
    for TypedCallBuilder<'a, 'm, 'ctx, B, F, RP, Ret, Params, A>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    Ret: FunctionReturn,
    Params: FunctionParamList,
    A: CallArgs<'ctx, Params, B>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TypedCallBuilder")
            .field(
                "callee",
                &self.callee.as_ref().map(|c| c.as_function().slot()),
            )
            .field("arguments", &core::any::type_name::<A>())
            .field("calling_conv", &self.calling_conv)
            .field("tail_kind", &self.tail_kind)
            .field("name", &self.name)
            .finish()
    }
}

impl<'a, 'm, 'ctx, B, F, RP, Ret, Params, A>
    TypedCallBuilder<'a, 'm, 'ctx, B, F, RP, Ret, Params, A>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
    Ret: FunctionReturn,
    Params: FunctionParamList,
    A: CallArgs<'ctx, Params, B>,
{
    #[must_use]
    pub fn tail(mut self) -> Self {
        self.tail_kind = TailCallKind::Tail;
        self
    }

    #[must_use]
    pub fn must_tail(mut self) -> Self {
        self.tail_kind = TailCallKind::MustTail;
        self
    }

    #[must_use]
    pub fn no_tail(mut self) -> Self {
        self.tail_kind = TailCallKind::NoTail;
        self
    }

    #[must_use]
    pub fn calling_conv(mut self, cc: CallingConv) -> Self {
        self.calling_conv = Some(cc);
        self
    }

    #[must_use]
    pub fn call_attributes(mut self, attrs: CallAttributeData) -> Self {
        self.attrs = attrs;
        self
    }

    #[must_use]
    pub fn name<Name>(mut self, name: Name) -> Self
    where
        Name: Into<String>,
    {
        self.name = name.into();
        self
    }

    /// Emit the call instruction, named by the storable
    /// [`TypedCallInstId<Ret, B>`](crate::TypedCallInstId).
    pub fn build(self) -> IrResult<TypedCallInstId<Ret, B>> {
        let f = self.callee?.as_function();
        let arg_ids = self.args.lower(ModuleRef::new(self.parent.module))?;
        let calling_conv = self.calling_conv.unwrap_or_else(|| f.calling_conv());
        let payload = CallInstData::new_with_attrs(
            f.slot(),
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
        Ok(TypedCallInstId::from_raw(
            self.parent.module.id(),
            inst.slot(),
        ))
    }
}

/// Builder for descriptor-backed intrinsic calls.
pub struct IntrinsicCallBuilder<'a, 'm, 'ctx, B, F, RP>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
{
    inner: CallBuilder<'a, 'm, 'ctx, B, F, RP, Dyn>,
}

/// Hand-written for the same reason as [`CallBuilder`]'s (`F: Debug` would be
/// forced by a `derive`); the wrapped builder carries the whole state, so this
/// forwards to it under this type's own name.
impl<'a, 'm, 'ctx, B, F, RP> core::fmt::Debug for IntrinsicCallBuilder<'a, 'm, 'ctx, B, F, RP>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("IntrinsicCallBuilder")
            .field(&self.inner)
            .finish()
    }
}

impl<'a, 'm, 'ctx, B, F, RP> IntrinsicCallBuilder<'a, 'm, 'ctx, B, F, RP>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    RP: ReturnMarker,
{
    /// Add an argument. Statically dispatched per `V: IntoErasedValue` so
    /// mixed-type argument lists work without homogeneity, and a storable id
    /// is accepted alongside a borrowing handle. A foreign-module id surfaces
    /// from [`build`](Self::build), as on the wrapped [`CallBuilder`].
    #[must_use]
    pub fn arg<V: IntoErasedValue<'ctx, B>>(mut self, value: V) -> Self {
        self.inner = self.inner.arg(value);
        self
    }

    #[must_use]
    pub fn tail(mut self) -> Self {
        self.inner = self.inner.tail();
        self
    }

    #[must_use]
    pub fn must_tail(mut self) -> Self {
        self.inner = self.inner.must_tail();
        self
    }

    #[must_use]
    pub fn no_tail(mut self) -> Self {
        self.inner = self.inner.no_tail();
        self
    }

    #[must_use]
    pub fn calling_conv(mut self, cc: CallingConv) -> Self {
        self.inner = self.inner.calling_conv(cc);
        self
    }

    #[must_use]
    pub fn call_attributes(mut self, attrs: CallAttributeData) -> Self {
        self.inner = self.inner.call_attributes(attrs);
        self
    }

    #[must_use]
    pub fn name<Name>(mut self, name: Name) -> Self
    where
        Name: Into<String>,
    {
        self.inner = self.inner.name(name);
        self
    }

    /// Emit the intrinsic call instruction, named by the storable
    /// [`IntrinsicInstId<Dyn, B>`](crate::IntrinsicInstId).
    pub fn build(self) -> IrResult<IntrinsicInstId<Dyn, B>> {
        let descriptor = self.inner.intrinsic_descriptor.clone();
        let module = ModuleRef::<B>::new(self.inner.parent.module);
        let inst = self.inner.emit()?;
        let call = CallInst::<Dyn, B>::from_raw(inst.slot(), module, inst.ty().id());
        // Reject an ordinary call, exactly as `IntrinsicInst::from_call` does
        // — the id is only minted once the callee is a generated intrinsic
        // declaration, so viewing it can never fail that check afterwards.
        if IntrinsicInst::from_call(call).is_none() {
            return Err(IrError::IntrinsicSignatureMismatch {
                name: descriptor
                    .as_ref()
                    .map(intrinsic_descriptor_error_name)
                    .unwrap_or_else(|| "intrinsic call".to_owned()),
            });
        }
        Ok(IntrinsicInstId::from_raw(module.id(), inst.slot()))
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

// --------------------------------------------------------------------------
// LoadBuilder / StoreBuilder / AllocaBuilder
// --------------------------------------------------------------------------

/// Builder for [`crate::IrBuilder::load_from`]. Accumulates the four
/// orthogonal optional knobs an upstream `LoadInst` carries — `volatile`,
/// `Align`, `AtomicOrdering`, `SyncScope::ID` (`Instructions.h`) — via
/// chainable setters, then emits through a **typed terminal** that picks
/// the result shape: [`int`](Self::int), [`fp`](Self::fp),
/// [`pointer`](Self::pointer), [`typed`](Self::typed), or
/// [`erased`](Self::erased). Mirrors the 5-arg
/// `LoadInst::LoadInst(Type*, Value*, const Twine&, bool isVolatile,
/// Align, AtomicOrdering, SyncScope::ID)` constructor
/// (`lib/IR/Instructions.cpp`) inserted at the builder's insert point.
///
/// The terminal, not a setter, carries the result name — the width /
/// kind / schema marker it takes is the only generic argument, so
/// `.int::<i32>("x")` needs no placeholder turbofish.
#[must_use = "a LoadBuilder emits nothing until a terminal (int / fp / pointer / typed / erased) is called"]
pub struct LoadBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    parent: &'a IrBuilder<'m, 'ctx, B, F, Positioned, R>,
    /// Lifted pointer operand, or the error its lift produced — replayed by
    /// the terminal. [`crate::IrBuilder::load_from`] returns `Self` to keep
    /// the chain spellable, so a failed lift has nowhere to surface until
    /// then. Only an id from a *foreign* module can set it: every pointer
    /// handle lifts infallibly.
    ptr: IrResult<ValueSlot>,
    align: MaybeAlign,
    volatile: bool,
    ordering: AtomicOrdering,
    sync_scope: SyncScope,
}

/// Hand-written for the same reason as [`IrBuilder`]'s: a `derive` would
/// bound the folder parameter `F: Debug`.
impl<'a, 'm, 'ctx, B, F, R> core::fmt::Debug for LoadBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LoadBuilder")
            .field("pointer", &self.ptr)
            .field("align", &self.align)
            .field("volatile", &self.volatile)
            .field("ordering", &self.ordering)
            .field("sync_scope", &self.sync_scope)
            .finish()
    }
}

impl<'a, 'm, 'ctx, B, F, R> LoadBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Mark the load `volatile`. Mirrors `LoadInst::setVolatile(true)`.
    pub fn volatile(mut self) -> Self {
        self.volatile = true;
        self
    }

    /// Set an explicit alignment. Left unset, the DataLayout ABI
    /// alignment of the loaded type is materialised
    /// (`computeLoadStoreDefaultAlign`), exactly as the flat
    /// [`crate::IrBuilder::load`] does.
    pub fn align(mut self, align: Align) -> Self {
        self.align = MaybeAlign::new(align);
        self
    }

    /// Make the load atomic with `ordering`. Mirrors
    /// `LoadInst::setAtomic(AtomicOrdering, SyncScope::ID)`, whose scope
    /// argument defaults to `SyncScope::System`; [`sync_scope`](Self::sync_scope)
    /// overrides it, in either chain order.
    ///
    /// LangRef requires an atomic load to carry a non-zero alignment. An
    /// unset [`align`](Self::align) is filled with the DataLayout ABI
    /// alignment on the way out — never zero — which is what upstream's
    /// own builder path produces for an atomic load
    /// (`computeLoadStoreDefaultAlign` at construction, `setAtomic`
    /// leaving the alignment alone).
    pub fn atomic(mut self, ordering: AtomicOrdering) -> Self {
        self.ordering = ordering;
        self
    }

    /// Set the synchronisation scope. Mirrors `LoadInst::setSyncScopeID`.
    /// Only meaningful on an atomic load — the verifier rejects a
    /// non-default scope on a non-atomic one
    /// (`Verifier::visitLoadInst`).
    pub fn sync_scope(mut self, sync_scope: SyncScope) -> Self {
        self.sync_scope = sync_scope;
        self
    }

    /// Emit with the result typed by the static width marker `W`:
    /// `b.load_from(p).atomic(ordering).align(a).int::<i32>("x")`.
    pub fn int<W>(self, name: &str) -> IrResult<IntValueId<W, B>>
    where
        W: StaticIntWidth,
    {
        let parent = self.parent;
        let ty = W::ir_type(ModuleRef::<B>::new(parent.module));
        let payload = self.payload(ty.as_type().id())?;
        parent.append_int_load(ty, payload, name).map(|v| v.id())
    }

    /// Emit with the result typed by the static float-kind marker `K`.
    pub fn fp<K>(self, name: &str) -> IrResult<FloatValueId<K, B>>
    where
        K: StaticFloatKind,
    {
        let parent = self.parent;
        let ty = K::ir_type(ModuleRef::<B>::new(parent.module));
        let payload = self.payload(ty.as_type().id())?;
        parent.append_fp_load(ty, payload, name).map(|v| v.id())
    }

    /// Emit a `ptr`-typed load in the default address space. Other
    /// address spaces go through [`erased`](Self::erased) with the
    /// matching pointer type, exactly as
    /// [`crate::IrBuilder::pointer_load`] documents.
    pub fn pointer(self, name: &str) -> IrResult<PointerValueId<B>> {
        let parent = self.parent;
        let ty = ModuleView::<B>::new(parent.module).ptr_type(0);
        let payload = self.payload(ty.as_type().id())?;
        parent.append_ptr_load(ty, payload, name).map(|v| v.id())
    }

    /// Emit with the result type derived from the pointee schema `T`,
    /// the [`TypedPointerValue`] route (the flat
    /// [`crate::IrBuilder::typed_load`]'s knob-carrying twin).
    pub fn typed<T>(self, name: &str) -> IrResult<T::Value<'ctx, B>>
    where
        T: IrField,
    {
        let parent = self.parent;
        let ty = parent.schema_ir_type::<T>()?;
        let payload = self.payload(ty.id())?;
        let inst = parent.load_inner(payload, name)?;
        T::value_from_ir_value(inst.to_erased())
    }

    /// Emit with an explicit pointee type; the caller narrows the
    /// returned [`ValueId`] by viewing it.
    pub fn erased<T>(self, ty: T, name: &str) -> IrResult<ValueId<B>>
    where
        T: IrType<'ctx, B>,
    {
        let parent = self.parent;
        let payload = self.payload(ty.as_type().id())?;
        let inst = parent.load_inner(payload, name)?;
        Ok(inst.to_erased().id())
    }

    /// Replay the parked pointer-lift error, then assemble the payload at
    /// `pointee_ty`. The default-alignment fill happens downstream in
    /// `load_inner`, so every terminal inherits it.
    fn payload(self, pointee_ty: TypeSlot) -> IrResult<LoadInstData> {
        Ok(LoadInstData::new(
            pointee_ty,
            self.ptr?,
            self.align,
            self.volatile,
            self.ordering,
            self.sync_scope,
        ))
    }
}

/// Builder for [`crate::IrBuilder::store_to`]. The store counterpart of
/// [`LoadBuilder`]: the same four orthogonal knobs `StoreInst` carries —
/// `volatile`, `Align`, `AtomicOrdering`, `SyncScope::ID`
/// (`Instructions.h`) — set by chainable setters and emitted by
/// [`build`](Self::build). Mirrors the 6-arg
/// `StoreInst::StoreInst(Value*, Value*, bool isVolatile, Align,
/// AtomicOrdering, SyncScope::ID)` constructor
/// (`lib/IR/Instructions.cpp`). A store produces no value, so there is
/// one terminal rather than a typed family.
#[must_use = "a StoreBuilder emits nothing until build() is called"]
pub struct StoreBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    parent: &'a IrBuilder<'m, 'ctx, B, F, Positioned, R>,
    /// Lifted `(value, pointer)` operands, or the first error their lift
    /// produced — replayed by [`build`](Self::build), on the same grounds
    /// as [`LoadBuilder::ptr`].
    operands: IrResult<(Value<'ctx, B>, ValueSlot)>,
    align: MaybeAlign,
    volatile: bool,
    ordering: AtomicOrdering,
    sync_scope: SyncScope,
}

/// Hand-written for the same reason as [`IrBuilder`]'s: a `derive` would
/// bound the folder parameter `F: Debug`.
impl<'a, 'm, 'ctx, B, F, R> core::fmt::Debug for StoreBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StoreBuilder")
            .field("operands", &self.operands)
            .field("align", &self.align)
            .field("volatile", &self.volatile)
            .field("ordering", &self.ordering)
            .field("sync_scope", &self.sync_scope)
            .finish()
    }
}

impl<'a, 'm, 'ctx, B, F, R> StoreBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Mark the store `volatile`. Mirrors `StoreInst::setVolatile(true)`.
    pub fn volatile(mut self) -> Self {
        self.volatile = true;
        self
    }

    /// Set an explicit alignment. Left unset, the DataLayout ABI
    /// alignment of the *stored value's* type is materialised
    /// (`computeLoadStoreDefaultAlign` / `getABITypeAlign(Val->getType())`),
    /// exactly as the flat [`crate::IrBuilder::store`] does.
    pub fn align(mut self, align: Align) -> Self {
        self.align = MaybeAlign::new(align);
        self
    }

    /// Make the store atomic with `ordering`. Mirrors
    /// `StoreInst::setAtomic(AtomicOrdering, SyncScope::ID)`, whose scope
    /// argument defaults to `SyncScope::System`;
    /// [`sync_scope`](Self::sync_scope) overrides it, in either chain
    /// order. The alignment rule is
    /// [`LoadBuilder::atomic`]'s, applied to the stored value's type.
    pub fn atomic(mut self, ordering: AtomicOrdering) -> Self {
        self.ordering = ordering;
        self
    }

    /// Set the synchronisation scope. Mirrors
    /// `StoreInst::setSyncScopeID`. Only meaningful on an atomic store —
    /// the verifier rejects a non-default scope on a non-atomic one
    /// (`Verifier::visitStoreInst`).
    pub fn sync_scope(mut self, sync_scope: SyncScope) -> Self {
        self.sync_scope = sync_scope;
        self
    }

    /// Emit the store.
    pub fn build(self) -> IrResult<StoreInst<'ctx, B>> {
        let parent = self.parent;
        let (value, ptr) = self.operands?;
        let payload = parent.store_payload_lifted(
            value,
            ptr,
            self.align,
            self.volatile,
            self.ordering,
            self.sync_scope,
        );
        parent.store_inner(payload)
    }
}

/// Builder for [`crate::IrBuilder::alloca_builder`]. Accumulates the
/// optional slots of an upstream `AllocaInst` — array size, `Align`,
/// address space, and the `inalloca` / `swifterror` markers
/// (`Instructions.h`) — via chainable setters, emitted by
/// [`build`](Self::build). Mirrors `IRBuilder::CreateAlloca`, whose
/// address space comes from `DataLayout::getAllocaAddrSpace` and whose
/// alignment defaults to `computeAllocaDefaultAlign`
/// (`getPrefTypeAlign`) unless overridden.
#[must_use = "an AllocaBuilder emits nothing until build() is called"]
pub struct AllocaBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    parent: &'a IrBuilder<'m, 'ctx, B, F, Positioned, R>,
    allocated_ty: TypeSlot,
    /// Lifted array-size operand, or the first error a
    /// [`array`](Self::array) lift produced — replayed by
    /// [`build`](Self::build), on the same grounds as
    /// [`LoadBuilder::ptr`].
    num_elements: IrResult<Option<ValueSlot>>,
    align: MaybeAlign,
    addr_space: u32,
    flags: AllocaFlags,
    name: String,
}

/// Hand-written for the same reason as [`IrBuilder`]'s: a `derive` would
/// bound the folder parameter `F: Debug`.
impl<'a, 'm, 'ctx, B, F, R> core::fmt::Debug for AllocaBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AllocaBuilder")
            .field("allocated_type", &self.allocated_ty)
            .field("num_elements", &self.num_elements)
            .field("align", &self.align)
            .field("addr_space", &self.addr_space)
            .field("flags", &self.flags)
            .field("name", &self.name)
            .finish()
    }
}

impl<'a, 'm, 'ctx, B, F, R> AllocaBuilder<'a, 'm, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Allocate `num_elements` of the type instead of one:
    /// `alloca <ty>, <size-ty> <n>`. Mirrors the `ArraySize` operand of
    /// `IRBuilder::CreateAlloca`.
    pub fn array<N>(mut self, num_elements: N) -> Self
    where
        N: IntoIntValue<'ctx, IntDyn, B>,
    {
        let lifted = num_elements.into_int_value(ModuleRef::new(self.parent.module));
        // Keep the FIRST failure, like `CallBuilder::arg`: the chain returns
        // `Self`, so an error has nowhere to surface until `build`.
        if self.num_elements.is_ok() {
            self.num_elements = lifted.map(|n| Some(n.slot()));
        }
        self
    }

    /// Set an explicit alignment. Left unset, the DataLayout preferred
    /// alignment of the allocated type is materialised
    /// (`computeAllocaDefaultAlign`).
    pub fn align(mut self, align: Align) -> Self {
        self.align = MaybeAlign::new(align);
        self
    }

    /// Override the result pointer's address space. Defaults to
    /// `DataLayout::getAllocaAddrSpace`, which is what
    /// `IRBuilder::CreateAlloca` uses.
    pub fn addr_space(mut self, addr_space: u32) -> Self {
        self.addr_space = addr_space;
        self
    }

    /// Mark the allocation `inalloca`. Mirrors
    /// `AllocaInst::setUsedWithInAlloca(true)`.
    pub fn inalloca(mut self) -> Self {
        self.flags = self.flags.with_inalloca();
        self
    }

    /// Mark the allocation `swifterror`. Mirrors
    /// `AllocaInst::setSwiftError(true)`; the verifier constrains it to a
    /// non-array pointer allocation (`Verifier::visitAllocaInst`).
    pub fn swifterror(mut self) -> Self {
        self.flags = self.flags.with_swifterror();
        self
    }

    /// Name the result value.
    pub fn name<Name>(mut self, name: Name) -> Self
    where
        Name: Into<String>,
    {
        self.name = name.into();
        self
    }

    /// Emit the alloca, named by the storable
    /// [`PointerValueId<B>`](crate::PointerValueId).
    pub fn build(self) -> IrResult<PointerValueId<B>> {
        let num_elements = self.num_elements?;
        self.parent.alloca_inner(
            self.allocated_ty,
            num_elements,
            self.align,
            self.addr_space,
            self.flags,
            &self.name,
        )
    }
}

// `require_same_int_width` is no longer needed: the IrBuilder's binary-

// --------------------------------------------------------------------------
// SelectArm + select
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
/// `select`. The associated `Output` pins the result *id*'s shape to the
/// arm category, so `b.select(cond, a, b)` on `IntValue<W>` arms
/// yields an [`IntValueId<W, B>`](crate::IntValueId), on `FloatValue<K>`
/// arms a [`FloatValueId<K, B>`](crate::FloatValueId), and on
/// [`PointerValue`] arms a [`PointerValueId<B>`](crate::PointerValueId).
/// The narrowing itself is unchanged — only the currency the builder hands
/// back. Mirrors LangRef's invariant that the two arms must have identical
/// IR types.
///
/// Each category is implemented for **both** the borrowing handle and the
/// storable id, so chaining two flipped builder results into a select needs no
/// round trip through the module. An id arm resolves like every other operand
/// position — module-checked and fallible, yielding
/// [`IrError::ForeignValueId`] for an id minted elsewhere — which is why
/// [`arm_value`](Self::arm_value) takes a [`ModuleRef`] and returns
/// [`IrResult`], exactly as
/// [`IntoBasicBlockLabel`] does at branch targets.
/// `Output` is unchanged by that: an `IntValueId<W, B>` arm yields the same
/// `IntValueId<W, B>` its handle does, so the two spellings are
/// interchangeable at the call site *and* at the binding.
pub trait SelectArm<'ctx, B: ModuleBrand>: Sized + select_arm_sealed::Sealed {
    type Output;
    #[doc(hidden)]
    fn from_select_value(v: Value<'ctx, B>, narrow: &SelectNarrow<'_>) -> Self::Output;
    #[doc(hidden)]
    fn arm_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>>;
}

mod select_arm_sealed {
    use super::{
        FloatKind, FloatValue, FloatValueId, IntValue, IntValueId, IntWidth, ModuleBrand,
        PointerValue, PointerValueId,
    };

    pub trait Sealed {}

    impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> Sealed for IntValue<'ctx, W, B> {}
    impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> Sealed for FloatValue<'ctx, K, B> {}
    impl<'ctx, B: ModuleBrand + 'ctx> Sealed for PointerValue<'ctx, B> {}

    impl<W: IntWidth, B: ModuleBrand> Sealed for IntValueId<W, B> {}
    impl<K: FloatKind, B: ModuleBrand> Sealed for FloatValueId<K, B> {}
    impl<B: ModuleBrand> Sealed for PointerValueId<B> {}
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for IntValue<'ctx, W, B> {
    type Output = IntValueId<W, B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        IntValue::<W, B>::from_value_unchecked(v).id()
    }
    #[inline]
    fn arm_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(IsValue::as_erased(self))
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for FloatValue<'ctx, K, B> {
    type Output = FloatValueId<K, B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        FloatValue::<K, B>::from_value_unchecked(v).id()
    }
    #[inline]
    fn arm_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(IsValue::as_erased(self))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for PointerValue<'ctx, B> {
    type Output = PointerValueId<B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        PointerValue::from_value_unchecked(v).id()
    }
    #[inline]
    fn arm_value(self, _module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        Ok(IsValue::as_erased(self))
    }
}

impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for IntValueId<W, B> {
    type Output = IntValueId<W, B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        IntValue::<W, B>::from_value_unchecked(v).id()
    }
    #[inline]
    fn arm_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        self.into_erased_value(module)
    }
}

impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for FloatValueId<K, B> {
    type Output = FloatValueId<K, B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        FloatValue::<K, B>::from_value_unchecked(v).id()
    }
    #[inline]
    fn arm_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        self.into_erased_value(module)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> SelectArm<'ctx, B> for PointerValueId<B> {
    type Output = PointerValueId<B>;
    #[inline]
    fn from_select_value(v: Value<'ctx, B>, _narrow: &SelectNarrow<'_>) -> Self::Output {
        PointerValue::from_value_unchecked(v).id()
    }
    #[inline]
    fn arm_value(self, module: ModuleRef<'ctx, B>) -> IrResult<Value<'ctx, B>> {
        self.into_erased_value(module)
    }
}

impl<'m, 'ctx, B, F, R> IrBuilder<'m, 'ctx, B, F, Positioned, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    /// Produce `select i1 <cond>, <ty> <true>, <ty> <false>`.
    /// Mirrors `IRBuilder::CreateSelect`.
    ///
    /// Both arms must share the same Rust type `A`, which pins the
    /// IR-type invariant that LangRef requires. The returned storable id
    /// is `A::Output`, statically tied to the arm category.
    pub fn select<C, A, Name>(
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
        let true_v = true_arm.arm_value(ModuleRef::new(self.module))?;
        let true_ty = true_v.ty().id();
        let false_v = false_arm.arm_value(ModuleRef::new(self.module))?;
        let false_ty = false_v.ty().id();
        if true_ty != false_ty {
            return Err(IrError::TypeMismatch {
                expected: true_v.ty().kind_label(),
                got: false_v.ty().kind_label(),
            });
        }
        if let Some(folded) = self
            .folder
            .fold_select_dyn(c.as_erased(), true_v, false_v)?
        {
            let folded = self.checked_folded_value(folded, true_ty)?;
            return Ok(A::from_select_value(folded, &SelectNarrow::new()));
        }
        let payload = SelectInstData::new(c.slot(), true_v.id, false_v.id);
        let inst = self.append_instruction(true_ty, InstructionKindData::Select(payload), name);
        Ok(A::from_select_value(inst.to_erased(), &SelectNarrow::new()))
    }
}

/// The `InstructionKindData` constructor for an integer binary opcode, or
/// `None` when the opcode is a floating-point one.
///
/// `BinaryOpcode` spans both domains — it is the union LLVM spells as
/// `BinaryOperator`'s opcode range — so a dispatcher over integer binops has
/// to reject the FP half rather than assume it away.
/// The `InstructionKindData` constructor for a floating-point binary opcode,
/// or `None` when the opcode is an integer one.
///
/// The mirror of [`int_binop_kind_ctor`]: `BinaryOpcode` spans both domains, so
/// a dispatcher over one half has to reject the other rather than assume it
/// away.
fn fp_binop_kind_ctor(opcode: BinaryOpcode) -> Option<fn(BinaryOpData) -> InstructionKindData> {
    Some(match opcode {
        BinaryOpcode::Fadd => InstructionKindData::Fadd,
        BinaryOpcode::Fsub => InstructionKindData::Fsub,
        BinaryOpcode::Fmul => InstructionKindData::Fmul,
        BinaryOpcode::Fdiv => InstructionKindData::Fdiv,
        BinaryOpcode::Frem => InstructionKindData::Frem,
        _ => return None,
    })
}

fn int_binop_kind_ctor(opcode: BinaryOpcode) -> Option<fn(BinaryOpData) -> InstructionKindData> {
    Some(match opcode {
        BinaryOpcode::Add => InstructionKindData::Add,
        BinaryOpcode::Sub => InstructionKindData::Sub,
        BinaryOpcode::Mul => InstructionKindData::Mul,
        BinaryOpcode::Udiv => InstructionKindData::Udiv,
        BinaryOpcode::Sdiv => InstructionKindData::Sdiv,
        BinaryOpcode::Urem => InstructionKindData::Urem,
        BinaryOpcode::Srem => InstructionKindData::Srem,
        BinaryOpcode::Shl => InstructionKindData::Shl,
        BinaryOpcode::Lshr => InstructionKindData::Lshr,
        BinaryOpcode::Ashr => InstructionKindData::Ashr,
        BinaryOpcode::And => InstructionKindData::And,
        BinaryOpcode::Or => InstructionKindData::Or,
        BinaryOpcode::Xor => InstructionKindData::Xor,
        BinaryOpcode::Fadd
        | BinaryOpcode::Fsub
        | BinaryOpcode::Fmul
        | BinaryOpcode::Fdiv
        | BinaryOpcode::Frem => return None,
    })
}

// --------------------------------------------------------------------------
// Aggregate path resolution helper
// --------------------------------------------------------------------------

/// Walk the aggregate `root` by `indices` and return the leaf type.
/// Mirrors `ExtractValueInst::getIndexedType` in `Instructions.cpp`, which
/// rejects (rather than clamps) an index at or past the element count.
fn walk_aggregate_for_builder(
    m: &ModuleCore,
    root: TypeSlot,
    indices: &[u32],
) -> IrResult<TypeSlot> {
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
                            expected: TypeKindLabel::Struct,
                            got: Type::<DynBrand>::new(cur, m).kind_label(),
                        });
                    }
                }
            }
            _ => {
                return Err(IrError::TypeMismatch {
                    expected: TypeKindLabel::Struct,
                    got: Type::<DynBrand>::new(cur, m).kind_label(),
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
    /// override -- the class of bug `ConstantFolder`'s own native typed
    /// overrides (see `ir_builder/constant_folder.rs`) are now structurally
    /// incapable of committing, because every one of them re-types its
    /// erased fold result through `W::narrow` / `K::narrow` instead of
    /// rewrapping it unchecked on the authority of a prose invariant audit.
    /// Unlike `tests/constant_folder_builder.rs`'s external
    /// `WideningDynFolder` (which can only override the erased
    /// `fold_bin_op_dyn` hook and so gets caught by
    /// `folder::narrow_folded_int`'s TypeSlot re-check before the builder ever
    /// sees the result), this folder overrides the *typed* hooks directly
    /// and answers with an `IntValue<'ctx, W, B>` built via the
    /// crate-internal `IntValue::from_value_unchecked` escape hatch
    /// (`pub(crate)` in `value.rs`, reachable here because this module lives
    /// at the crate root, same as `value`). That constructor performs no
    /// width check at all, so these overrides can -- and deliberately do --
    /// lie about the width of the `stored` payload: they always answer with
    /// a 64-bit constant, regardless of `W`. Because these are *native*
    /// overrides (not the trait's delegating default bodies at `folder.rs`),
    /// `narrow_folded_int` / `narrow_folded_cast_int` never run on this
    /// path; the only remaining guard is the builder's own
    /// `accept_folded_int` / `accept_folded_cast_int` type check.
    #[derive(Branded)]
    #[branded(Debug, Clone, Copy)]
    struct HostileTypedFolder<'ctx, B: ModuleBrand + 'ctx> {
        /// Always a 64-bit constant, deliberately the wrong width for any
        /// 32-bit `W` the builder calls this with.
        stored: IntValue<'ctx, i64, B>,
    }

    impl<'ctx, B: ModuleBrand + 'ctx> IrBuilderFolder<'ctx, B> for HostileTypedFolder<'ctx, B> {
        fn fold_int_bin_op<W: IntWidth>(
            &self,
            _opcode: BinaryOpcode,
            _lhs: IntValue<'ctx, W, B>,
            _rhs: IntValue<'ctx, W, B>,
        ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
            // Bypasses `narrow_folded_int` entirely: this reuses the
            // already-erased `Value` payload behind `self.stored` (a
            // 64-bit constant) and rewraps it as `IntValue<'ctx, W, B>` via
            // the unchecked constructor. This is the shape
            // `ConstantFolder::fold_int_bin_op` used before `bf57e17`, when
            // a prose "kernel invariant" audit was the only thing standing
            // between it and a mistyped handle -- written out here by hand
            // with the audit deliberately false: the payload's true IR type
            // is `i64` and matches no 32-bit `W`, static or dyn.
            Ok(Some(IntValue::<W, B>::from_value_unchecked(
                self.stored.as_erased(),
            )))
        }

        fn fold_cast_to_int<W: IntWidth>(
            &self,
            _opcode: CastOpcode,
            _value: Value<'ctx, B>,
            _dest_ty: IntType<'ctx, W, B>,
        ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
            // Cast twin of `fold_int_bin_op` above, lying the same way: the
            // requested destination type is ignored and the 64-bit `stored`
            // payload is rewrapped as the destination's `W`. Drives
            // `accept_folded_cast_int`, which checks against `dst_ty`
            // rather than an operand.
            Ok(Some(IntValue::<W, B>::from_value_unchecked(
                self.stored.as_erased(),
            )))
        }
    }

    /// Float twin of [`HostileTypedFolder`], overriding the typed *float*
    /// hooks with the same lie: `FloatValue::from_value_unchecked` forges a
    /// static `K` exactly as freely as `IntValue`'s forges a static `W`, so
    /// both overrides answer with a `double` payload regardless of the `K`
    /// asked for. Drives the two float acceptors
    /// (`accept_folded_fp` / `accept_folded_cast_fp`), which the same
    /// `bf57e17` change made unconditional.
    #[derive(Branded)]
    #[branded(Debug, Clone, Copy)]
    struct HostileTypedFpFolder<'ctx, B: ModuleBrand + 'ctx> {
        /// Always a `double` constant, deliberately the wrong kind for any
        /// `float` `K` the builder calls this with.
        stored: FloatValue<'ctx, f64, B>,
    }

    impl<'ctx, B: ModuleBrand + 'ctx> IrBuilderFolder<'ctx, B> for HostileTypedFpFolder<'ctx, B> {
        fn fold_fp_bin_op<K: FloatKind>(
            &self,
            _opcode: BinaryOpcode,
            _lhs: FloatValue<'ctx, K, B>,
            _rhs: FloatValue<'ctx, K, B>,
            _fmf: FastMathFlags,
        ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
            Ok(Some(FloatValue::<K, B>::from_value_unchecked(
                IsValue::as_erased(self.stored),
            )))
        }

        fn fold_cast_to_fp<K: FloatKind>(
            &self,
            _opcode: CastOpcode,
            _value: Value<'ctx, B>,
            _dest_ty: FloatType<'ctx, K, B>,
        ) -> IrResult<Option<FloatValue<'ctx, K, B>>> {
            Ok(Some(FloatValue::<K, B>::from_value_unchecked(
                IsValue::as_erased(self.stored),
            )))
        }
    }

    /// Locks `accept_folded_int` (`ir_builder.rs`) as the seam that rejects a
    /// wrong-width result from a *native* typed-hook override -- the bug
    /// class external folders are compile-time barred from producing
    /// (see the sibling compile-fail golden
    /// `tests/compile_fail/folder_typed_wrong_width.rs`, which locks the
    /// external-facing half of this contract) but an in-crate folder can
    /// still write by hand via `from_value_unchecked`.
    ///
    /// Trace confirming *this* line rejects, not `narrow_folded_int`:
    /// `int_add::<IntDyn, _, _, _>` (this file, `int_add`)
    /// calls `self.folder.fold_int_bin_op(BinaryOpcode::Add, lhs, rhs)`.
    /// `HostileTypedFolder`'s override above is a *native* override of
    /// `fold_int_bin_op`, so it runs directly -- it never calls
    /// `fold_bin_op_dyn` or `folder::narrow_folded_int` (those only run
    /// inside the *trait's default* body, which this override replaces).
    /// The native override returns `Ok(Some(wrong_width_value))`
    /// straight back to `int_add`, which forwards it to
    /// `self.accept_folded_int(folded, lhs)`. Inside `accept_folded_int`,
    /// `folded.as_erased().ty().id() != like.as_erased().ty().id()` is
    /// `true` (the stored value's real type is `i64`, `lhs`'s is the
    /// 32-bit custom-width `IntDyn` type) -- so `accept_folded_int`
    /// returns `Err(IrError::OperandWidthMismatch { lhs: 32, rhs: 64 })`.
    /// That is the exact line under test; `narrow_folded_int` is never
    /// reached on this path. The comparison is unconditional -- it no
    /// longer keys on `W::static_bits().is_none()` -- so `W = IntDyn` no
    /// longer selects it; the static-`W` sibling test below covers the
    /// other marker class.
    #[test]
    fn hostile_native_typed_override_wrong_width_rejected_by_accept_folded_int()
    -> Result<(), IrError> {
        let m = crate::module_new!("hostile-typed-folder")?;
        let i32_dyn_ty = m.custom_width_int_type(32)?;
        let i64_dyn_ty = m.custom_width_int_type(64)?;
        let fn_ty = m.function_type_no_parameters(m.i32_type());
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");

        let stored: IntValue<'_, i64, _> =
            IntValue::from_value_unchecked(i64_dyn_ty.const_zero().as_erased());
        let folder = HostileTypedFolder { stored };
        let b = IrBuilder::with_folder(&m, folder).position_at_end(entry);

        let lhs = i32_dyn_ty.const_int_checked(1_i32)?;
        let rhs = i32_dyn_ty.const_int_checked(2_i32)?;

        let err = b
            .int_add::<IntDyn, _, _, _>(lhs, rhs, "sum")
            .expect_err("wrong-width native-override fold result is rejected");

        // Both sides are integers, so the acceptor reports the widths
        // rather than a `TypeMismatch { expected: Integer, got: Integer }`
        // that could not say which width was wrong.
        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 32, rhs: 64 });
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    }

    /// Sibling of the `IntDyn` case above, at a *static* width. This is the
    /// half `accept_folded_int`'s old `W::static_bits().is_none() &&`
    /// short-circuit let through: the same `HostileTypedFolder` answers
    /// `int_add::<i32, _, _, _>` with its 64-bit `stored` payload
    /// rewrapped as `IntValue<'ctx, i32, B>`, and because the guard skipped
    /// the TypeSlot compare whenever the marker was static, the builder
    /// accepted it -- handing back an `IntValue<'_, i32>` whose real IR type
    /// is `i64`. A mistyped handle escaping into user code.
    ///
    /// The static marker is not self-guaranteeing: `from_value_unchecked`
    /// mints an `IntValue<W>` without ever consulting the payload's runtime
    /// type, so `W` is only as honest as the in-crate caller that wrote it.
    /// The acceptor now compares TypeIds for every marker, which is exactly
    /// what this test locks.
    #[test]
    fn hostile_native_typed_override_wrong_width_rejected_at_static_width() -> Result<(), IrError> {
        let m = crate::module_new!("hostile-typed-folder-static")?;
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.function_type_no_parameters(m.i32_type());
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");

        // `stored`'s REAL IR type is i64; the folder hands it back as
        // `IntValue<W>` for whatever `W` the builder asks for -- here i32.
        // The handle itself is built through the *checked* path: this is
        // the honest half of the setup (`const_zero()` on an `i64_ty`
        // genuinely is an i64), and in a test whose whole subject is
        // `from_value_unchecked` lying, the lie belongs only where it is
        // under test -- inside the folder's override.
        let stored: IntValue<'_, i64, _> = i64::narrow(i64_ty.const_zero().as_erased())?;
        let folder = HostileTypedFolder { stored };
        let b = IrBuilder::with_folder(&m, folder).position_at_end(entry);

        let lhs = i32_ty.const_int(1_i32);
        let rhs = i32_ty.const_int(2_i32);

        let err = b
            .int_add::<i32, _, _, _>(lhs, rhs, "sum")
            .expect_err("wrong-width fold result must be rejected at a static width too");

        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 32, rhs: 64 });
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    }

    /// Locks `accept_folded_cast_int` at a *static* destination width --
    /// the cast sibling of the two `accept_folded_int` tests above, and one
    /// of the three acceptors `bf57e17` made unconditional without proof.
    ///
    /// Casts have no same-type operand to compare against, so the acceptor
    /// checks the fold result against `dst_ty` instead of a `like` operand.
    /// `trunc::<i64, i32>` asks the folder to narrow an i64 to i32;
    /// `HostileTypedFolder::fold_cast_to_int` ignores the destination and
    /// answers with its 64-bit `stored` payload rewrapped as
    /// `IntValue<'ctx, i32, B>`. Being a native override it bypasses
    /// `folder::narrow_folded_cast_int`, so `accept_folded_cast_int`'s
    /// TypeSlot compare against `dst_ty` is the only thing between that lie
    /// and a mistyped handle -- and before `bf57e17` it was skipped outright
    /// for a static `Dst`.
    #[test]
    fn hostile_native_typed_override_wrong_width_rejected_by_accept_folded_cast_int()
    -> Result<(), IrError> {
        let m = crate::module_new!("hostile-typed-folder-cast-int")?;
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.function_type_no_parameters(m.i32_type());
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");

        let stored: IntValue<'_, i64, _> = i64::narrow(i64_ty.const_zero().as_erased())?;
        let folder = HostileTypedFolder { stored };
        let b = IrBuilder::with_folder(&m, folder).position_at_end(entry);

        let src: IntValue<'_, i64, _> = i64::narrow(i64_ty.const_int(1_i64).as_erased())?;
        let err = b
            .trunc::<i64, i32, _, _>(src, i32_ty, "narrowed")
            .expect_err("wrong-width cast fold result must be rejected at a static width");

        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 32, rhs: 64 });
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    }

    /// Float twin of `..._rejected_at_static_width`, locking
    /// `accept_folded_fp` at a *static* kind.
    ///
    /// `HostileTypedFpFolder::fold_fp_bin_op` answers `fp_add::<f32>`
    /// with its `double` `stored` payload rewrapped as
    /// `FloatValue<'ctx, f32, B>`. Unlike the int side, the error names both
    /// kinds directly: `TypeKindLabel` has a distinct variant per float kind,
    /// so `TypeMismatch { expected: Float, got: Double }` is already precise
    /// and needs no width-carrying variant.
    #[test]
    fn hostile_native_typed_override_wrong_kind_rejected_by_accept_folded_fp() -> Result<(), IrError>
    {
        let m = crate::module_new!("hostile-typed-fp-folder")?;
        let f32_ty = m.f32_type();
        let f64_ty = m.f64_type();
        let fn_ty = m.function_type_no_parameters(m.i32_type());
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");

        let stored: FloatValue<'_, f64, _> = f64::narrow(f64_ty.const_double(0.0).as_erased())?;
        let folder = HostileTypedFpFolder { stored };
        let b = IrBuilder::with_folder(&m, folder).position_at_end(entry);

        let lhs = f32_ty.const_float(1.0_f32);
        let rhs = f32_ty.const_float(2.0_f32);

        let err = b
            .fp_add::<f32, _, _, _>(lhs, rhs, "sum")
            .expect_err("wrong-kind fold result must be rejected at a static kind");

        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Float,
                got: TypeKindLabel::Double,
            }
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    }

    /// Locks `accept_folded_cast_fp` at a *static* destination kind -- the
    /// last of the four acceptors, and the float twin of
    /// `..._rejected_by_accept_folded_cast_int`.
    ///
    /// `fp_trunc::<f64, f32>` asks the folder to narrow a `double` to
    /// a `float`; `HostileTypedFpFolder::fold_cast_to_fp` ignores the
    /// destination and answers with the `double` `stored` payload rewrapped
    /// as `FloatValue<'ctx, f32, B>`, leaving the acceptor's compare against
    /// `dst_ty` as the only guard.
    #[test]
    fn hostile_native_typed_override_wrong_kind_rejected_by_accept_folded_cast_fp()
    -> Result<(), IrError> {
        let m = crate::module_new!("hostile-typed-fp-folder-cast")?;
        let f32_ty = m.f32_type();
        let f64_ty = m.f64_type();
        let fn_ty = m.function_type_no_parameters(m.i32_type());
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");

        let stored: FloatValue<'_, f64, _> = f64::narrow(f64_ty.const_double(0.0).as_erased())?;
        let folder = HostileTypedFpFolder { stored };
        let b = IrBuilder::with_folder(&m, folder).position_at_end(entry);

        let src: FloatValue<'_, f64, _> = f64::narrow(f64_ty.const_double(1.0).as_erased())?;
        let err = b
            .fp_trunc::<f64, f32, _, _>(src, f32_ty, "narrowed")
            .expect_err("wrong-kind cast fold result must be rejected at a static kind");

        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Float,
                got: TypeKindLabel::Double,
            }
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        Ok(())
    }
}
